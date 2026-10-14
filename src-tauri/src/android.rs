//! Android: installs and updates apps that ship outside Google Play (the SMS
//! Bridge) through Android's PackageInstaller, shows Play apps as links, and
//! keeps itself current. The catalog, downloads and signature checks are the
//! same code desktop uses; everything Android-specific goes through the
//! `tauri-plugin-atlas-android` Kotlin bridge.
//!
//! Background updates: a WorkManager job (Kotlin) calls
//! `AtlasNative.backgroundCheck` every few hours, which lands in
//! [`background_check`] below and returns verified APKs for Kotlin to install.

use crate::installer;
use crate::manifest::{self, is_newer, valid_id, valid_package, Manifest, Release};
use crate::store::{self, Settings};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_atlas_android::{AtlasAndroidExt, Installed, Paths};

const PLATFORM: &str = "android";
const CHECK_HOURS: u32 = 6;
const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Atlas's own applicationId (tauri.conf.json `identifier`).
const ATLAS_PACKAGE: &str = "com.yegobox.launcher";
/// Atlas's own update feed; its `platforms.android` entry is the Atlas APK.
const ATLAS_FEED: &str =
    "https://github.com/yegobox/flipper-launcher/releases/latest/download/latest.json";
const CONF: &str = include_str!("../tauri.conf.json");

/// The catalog/updater public key, read from tauri.conf.json at build time so
/// the background worker (no Tauri app running) verifies with the same key.
fn pubkey() -> String {
    serde_json::from_str::<Value>(CONF)
        .ok()
        .and_then(|c| c["plugins"]["updater"]["pubkey"].as_str().map(String::from))
        .unwrap_or_default()
}

/// reqwest's rustls default verifies through the platform, which on Android
/// needs extra Kotlin wiring; Mozilla's root store needs none.
fn http_client() -> Result<reqwest::Client, String> {
    let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|c| reqwest::Certificate::from_der(c.as_ref()).ok());
    reqwest::Client::builder()
        .user_agent(concat!("atlas-android/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .tls_certs_only(roots)
        .build()
        .map_err(|e| e.to_string())
}

fn config_dir(files_dir: &str) -> PathBuf {
    Path::new(files_dir).join("atlas")
}

/// The android release of an app, if it names a valid package (entry == package).
fn android_release<'a>(app: &'a manifest::AppEntry, channel: &str) -> Option<&'a Release> {
    app.release_for(channel, PLATFORM).filter(|r| {
        r.version_code.is_some()
            && r.package.as_deref().is_some_and(valid_package)
            && r.package.as_deref() == Some(r.entry.as_str())
    })
}

fn play_package(app: &manifest::AppEntry) -> Option<&str> {
    app.play_package.as_deref().filter(|p| valid_package(p))
}

/// Download `release` to the cache and check its sha256 + signature.
async fn fetch_verified(
    client: &reqwest::Client,
    release: &Release,
    pubkey: &str,
    cache_dir: &str,
    id: &str,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    let file = Path::new(cache_dir)
        .join("downloads")
        .join(format!("{id}-{}.apk", release.version_code.unwrap_or(0)));
    installer::download(client, &release.url, &file, &mut on_progress).await?;
    if let Err(e) = installer::verify_archive(&file, release, pubkey) {
        let _ = std::fs::remove_file(&file);
        return Err(e);
    }
    Ok(file)
}

#[derive(Deserialize)]
struct AtlasApk {
    url: String,
    signature: String,
    sha256: String,
    version_code: u64,
    #[serde(default)]
    size: u64,
}

/// Atlas's own Android release from its update feed, as a catalog Release.
async fn atlas_release(client: &reqwest::Client) -> Result<Option<Release>, String> {
    let feed: Value = client
        .get(ATLAS_FEED)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("could not reach the Atlas update feed: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let Ok(apk) = serde_json::from_value::<AtlasApk>(feed["platforms"]["android"].clone()) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version: feed["version"].as_str().unwrap_or_default().to_string(),
        pub_date: String::new(),
        notes: String::new(),
        min_launcher: "0.0.0".into(),
        url: apk.url,
        sha256: apk.sha256,
        signature: apk.signature,
        size: apk.size,
        entry: ATLAS_PACKAGE.into(),
        package: Some(ATLAS_PACKAGE.into()),
        version_code: Some(apk.version_code),
    }))
}

// ── the window ──────────────────────────────────────────────────────────────

struct Shared {
    client: reqwest::Client,
    pubkey: String,
    paths: Mutex<Option<Paths>>,
    manifest: Mutex<Option<Manifest>>,
    busy: Mutex<HashSet<String>>,
    checked_at: Mutex<Option<u64>>,
    /// A newer Atlas APK, found by the last check.
    atlas_update: Mutex<Option<Release>>,
}

/// Same shape as the desktop AppView, so the UI is shared.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppView {
    id: String,
    name: String,
    icon: Option<String>,
    description: Option<String>,
    website: Option<String>,
    accent: Option<String>,
    pub_date: Option<String>,
    installed: Option<String>,
    staged: Option<String>,
    previous: Option<String>,
    available: Option<String>,
    notes: String,
    size: u64,
    /// unsupported | not_installed | installing | needs_launcher_update |
    /// update_available | up_to_date | on_play
    status: &'static str,
    running: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    id: String,
    done: u64,
    total: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherInfo {
    version: String,
    platform: Option<&'static str>,
    update_ready: Option<String>,
    checked_at: Option<u64>,
}

async fn paths(app: &AppHandle) -> Result<Paths, String> {
    if let Some(p) = app.state::<Shared>().paths.lock().unwrap().clone() {
        return Ok(p);
    }
    let p = app.atlas_android().paths().await?;
    *app.state::<Shared>().paths.lock().unwrap() = Some(p.clone());
    Ok(p)
}

async fn settings(app: &AppHandle) -> Settings {
    match paths(app).await {
        Ok(p) => store::load_settings(&config_dir(&p.files_dir)),
        Err(_) => Settings::default(),
    }
}

async fn refresh(app: &AppHandle) -> Result<(), String> {
    let shared = app.state::<Shared>();
    let manifest = manifest::fetch(&shared.client, &manifest::manifest_url()).await?;
    *shared.manifest.lock().unwrap() = Some(manifest);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    *shared.checked_at.lock().unwrap() = Some(now);
    // Atlas's own update: a failure here must not hide the app list.
    if let Ok(Some(r)) = atlas_release(&shared.client).await {
        let own = app.atlas_android().installed(ATLAS_PACKAGE).await.ok();
        let newer = r.newer_than_code(own.and_then(|o| o.version_code));
        *shared.atlas_update.lock().unwrap() = newer.then_some(r);
    }
    Ok(())
}

async fn views(app: &AppHandle) -> Vec<AppView> {
    let shared = app.state::<Shared>();
    let Some(manifest) = shared.manifest.lock().unwrap().clone() else {
        return Vec::new();
    };
    let channel = settings(app).await.channel;
    let busy = shared.busy.lock().unwrap().clone();
    let mut out = Vec::new();
    for a in manifest.apps.iter().filter(|a| valid_id(&a.id)) {
        let release = android_release(a, &channel);
        let pkg = release
            .and_then(|r| r.package.as_deref())
            .or_else(|| play_package(a));
        let inst = match pkg {
            Some(p) => app.atlas_android().installed(p).await.unwrap_or_default(),
            None => Installed::default(),
        };
        let newer = release.is_some_and(|r| r.newer_than_code(inst.version_code));
        let status = match release {
            Some(_) if busy.contains(&a.id) => "installing",
            Some(r) if newer && is_newer(&r.min_launcher, Some(LAUNCHER_VERSION)) => {
                "needs_launcher_update"
            }
            Some(_) if !inst.installed => "not_installed",
            Some(_) if newer => "update_available",
            Some(_) => "up_to_date",
            None if pkg.is_some() && inst.installed => "up_to_date",
            None if pkg.is_some() => "on_play",
            None => "unsupported",
        };
        out.push(AppView {
            id: a.id.clone(),
            name: a.name.clone(),
            icon: a.icon.clone(),
            description: a.description.clone(),
            website: a.website.clone().filter(|w| w.starts_with("https://")),
            accent: a.accent.clone().filter(|c| {
                c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
            }),
            pub_date: release
                .map(|r| r.pub_date.clone())
                .filter(|d| !d.is_empty()),
            installed: inst
                .installed
                .then(|| inst.version_name.clone().unwrap_or_default()),
            staged: None,
            previous: None,
            available: release.map(|r| r.version.clone()),
            notes: release.map(|r| r.notes.clone()).unwrap_or_default(),
            size: release.map_or(0, |r| r.size),
            status,
            running: false,
        });
    }
    out
}

struct BusyGuard<'a>(&'a Shared, String);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.busy.lock().unwrap().remove(&self.1);
    }
}

async fn install(app: &AppHandle, id: &str) -> Result<(), String> {
    let shared = app.state::<Shared>();
    let entry = shared
        .manifest
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.app(id).cloned())
        .filter(|a| valid_id(&a.id))
        .ok_or_else(|| format!("{id} is not in the app catalog"))?;
    let channel = settings(app).await.channel;
    let release = android_release(&entry, &channel)
        .cloned()
        .ok_or_else(|| format!("{} has no Android build", entry.name))?;
    if is_newer(&release.min_launcher, Some(LAUNCHER_VERSION)) {
        return Err(format!(
            "{} {} needs a newer Atlas; update Atlas first",
            entry.name, release.version
        ));
    }
    if !app.atlas_android().can_install().await.unwrap_or(false) {
        return Err("Allow Atlas to install apps first (Settings → Install unknown apps).".into());
    }
    if !shared.busy.lock().unwrap().insert(id.to_string()) {
        return Err(format!("{} is already installing", entry.name));
    }
    let _guard = BusyGuard(&shared, id.to_string());
    let _ = app.emit("apps-changed", ());

    let p = paths(app).await?;
    let mut last = 0u64;
    let file = fetch_verified(
        &shared.client,
        &release,
        &shared.pubkey,
        &p.cache_dir,
        id,
        |done, total| {
            if done - last >= 1 << 20 || Some(done) == total {
                last = done;
                let _ = app.emit(
                    "progress",
                    Progress {
                        id: id.to_string(),
                        done,
                        total,
                    },
                );
            }
        },
    )
    .await?;
    let result = app
        .atlas_android()
        .install_apk(&file.to_string_lossy(), &entry.name)
        .await;
    let _ = std::fs::remove_file(&file);
    let _ = app.emit("apps-changed", ());
    result
}

async fn open(app: &AppHandle, id: &str) -> Result<(), String> {
    let entry = app
        .state::<Shared>()
        .manifest
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.app(id).cloned())
        .ok_or_else(|| format!("{id} is not in the app catalog"))?;
    let channel = settings(app).await.channel;
    let release_pkg = android_release(&entry, &channel).and_then(|r| r.package.clone());
    let pkg = release_pkg
        .clone()
        .or_else(|| play_package(&entry).map(String::from))
        .ok_or_else(|| format!("{} is not available on Android", entry.name))?;
    let installed = app.atlas_android().installed(&pkg).await?.installed;
    match (installed, play_package(&entry)) {
        (true, _) => app.atlas_android().launch(&pkg).await,
        (false, Some(play)) if release_pkg.is_none() => app.atlas_android().open_play(play).await,
        (false, _) => Err(format!("{} is not installed yet", entry.name)),
    }
}

async fn update_all(app: &AppHandle) -> Result<(), String> {
    refresh(app).await?;
    if settings(app).await.auto_update {
        let pending: Vec<String> = views(app)
            .await
            .into_iter()
            .filter(|v| v.status == "update_available")
            .map(|v| v.id)
            .collect();
        for id in pending {
            if let Err(e) = install(app, &id).await {
                let _ = app.emit("error", format!("{id}: {e}"));
            }
        }
    }
    let _ = app.emit("apps-changed", ());
    Ok(())
}

#[tauri::command]
async fn list_apps(app: AppHandle, shared: State<'_, Shared>) -> Result<Vec<AppView>, String> {
    if shared.manifest.lock().unwrap().is_none() {
        refresh(&app).await?;
    }
    Ok(views(&app).await)
}

#[tauri::command]
async fn refresh_apps(app: AppHandle) -> Result<Vec<AppView>, String> {
    refresh(&app).await?;
    Ok(views(&app).await)
}

#[tauri::command]
async fn install_app(app: AppHandle, id: String) -> Result<(), String> {
    install(&app, &id).await
}

#[tauri::command]
async fn open_app(app: AppHandle, id: String) -> Result<bool, String> {
    open(&app, &id).await.map(|_| false)
}

#[tauri::command]
fn rollback_app() -> Result<String, String> {
    Err("Android can't roll back without uninstalling the app and losing its data".into())
}

#[tauri::command]
fn take_notice() -> Option<()> {
    None
}

#[tauri::command]
async fn get_settings(app: AppHandle) -> Settings {
    settings(&app).await
}

#[tauri::command]
async fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    if settings.channel != "stable" && settings.channel != "beta" {
        return Err(format!("unknown channel {}", settings.channel));
    }
    let p = paths(&app).await?;
    store::save_settings(&config_dir(&p.files_dir), &settings).map_err(|e| e.to_string())?;
    app.atlas_android()
        .schedule_updates(if settings.auto_update { CHECK_HOURS } else { 0 })
        .await?;
    let _ = app.emit("apps-changed", ());
    Ok(())
}

#[tauri::command]
fn launcher_info(shared: State<'_, Shared>) -> LauncherInfo {
    LauncherInfo {
        version: LAUNCHER_VERSION.to_string(),
        platform: Some(PLATFORM),
        update_ready: shared
            .atlas_update
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r.version.clone()),
        checked_at: *shared.checked_at.lock().unwrap(),
    }
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<(), String> {
    update_all(&app).await
}

/// Android "Update and restart": install the newer Atlas APK. Android stops
/// this process and reopens the app once it's installed.
#[tauri::command]
async fn restart_launcher(app: AppHandle) -> Result<(), String> {
    let shared = app.state::<Shared>();
    let Some(release) = shared.atlas_update.lock().unwrap().clone() else {
        return Ok(());
    };
    let p = paths(&app).await?;
    let file = fetch_verified(
        &shared.client,
        &release,
        &shared.pubkey,
        &p.cache_dir,
        "atlas",
        |_, _| {},
    )
    .await?;
    app.atlas_android()
        .install_apk(&file.to_string_lossy(), "Atlas")
        .await
}

#[tauri::command]
async fn android_can_install(app: AppHandle) -> Result<bool, String> {
    app.atlas_android().can_install().await
}

#[tauri::command]
async fn android_open_install_settings(app: AppHandle) -> Result<(), String> {
    app.atlas_android().open_install_settings().await
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_atlas_android::init())
        .setup(|app| {
            app.manage(Shared {
                client: http_client()?,
                pubkey: pubkey(),
                paths: Mutex::new(None),
                manifest: Mutex::new(None),
                busy: Mutex::new(HashSet::new()),
                checked_at: Mutex::new(None),
                atlas_update: Mutex::new(None),
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let auto = settings(&handle).await.auto_update;
                let _ = handle
                    .atlas_android()
                    .schedule_updates(if auto { CHECK_HOURS } else { 0 })
                    .await;
                let _ = handle.atlas_android().request_notifications().await;
                if let Err(e) = refresh(&handle).await {
                    let _ = handle.emit("error", e);
                }
                let _ = handle.emit("apps-changed", ());
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_apps,
            refresh_apps,
            install_app,
            open_app,
            rollback_app,
            take_notice,
            get_settings,
            set_settings,
            launcher_info,
            check_for_updates,
            restart_launcher,
            android_can_install,
            android_open_install_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Atlas");
}

// ── the background worker ───────────────────────────────────────────────────

/// Called by `AtlasUpdateWorker` (Kotlin) with every installed package's
/// versionCode. Downloads and verifies updates for apps that are already
/// installed (it never installs new apps), Atlas's own APK last, and returns
/// `{"apks":[{"path","pkg","label"}]}` plus `"error"` if anything failed.
fn background_check(files_dir: &str, cache_dir: &str, installed_json: &str) -> String {
    let installed: HashMap<String, u64> = serde_json::from_str(installed_json).unwrap_or_default();
    let settings = store::load_settings(&config_dir(files_dir));
    if !settings.auto_update {
        return json!({ "apks": [] }).to_string();
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return json!({ "apks": [], "error": e.to_string() }).to_string(),
    };
    rt.block_on(async {
        let mut apks = Vec::new();
        let mut errors = Vec::new();
        let client = match http_client() {
            Ok(c) => c,
            Err(e) => return json!({ "apks": [], "error": e }).to_string(),
        };
        let pubkey = pubkey();
        match manifest::fetch(&client, &manifest::manifest_url()).await {
            Ok(m) => {
                for a in m.apps.iter().filter(|a| valid_id(&a.id)) {
                    let Some(r) = android_release(a, &settings.channel) else {
                        continue;
                    };
                    let pkg = r.package.clone().unwrap_or_default();
                    let Some(&code) = installed.get(&pkg) else {
                        continue;
                    };
                    if !r.newer_than_code(Some(code))
                        || is_newer(&r.min_launcher, Some(LAUNCHER_VERSION))
                    {
                        continue;
                    }
                    match fetch_verified(&client, r, &pubkey, cache_dir, &a.id, |_, _| {}).await {
                        Ok(path) => apks.push(json!({ "path": path, "pkg": pkg, "label": a.name })),
                        Err(e) => errors.push(format!("{}: {e}", a.id)),
                    }
                }
            }
            Err(e) => errors.push(e),
        }
        // Atlas last: installing it ends the worker's process.
        if let Some(&own) = installed.get(ATLAS_PACKAGE) {
            match atlas_release(&client).await {
                Ok(Some(r)) if r.newer_than_code(Some(own)) => {
                    match fetch_verified(&client, &r, &pubkey, cache_dir, "atlas", |_, _| {}).await
                    {
                        Ok(path) => apks
                            .push(json!({ "path": path, "pkg": ATLAS_PACKAGE, "label": "Atlas" })),
                        Err(e) => errors.push(format!("atlas: {e}")),
                    }
                }
                Ok(_) => {}
                Err(e) => errors.push(e),
            }
        }
        let mut out = serde_json::Map::new();
        out.insert("apks".into(), Value::Array(apks));
        if !errors.is_empty() {
            out.insert("error".into(), Value::String(errors.join("; ")));
        }
        Value::Object(out).to_string()
    })
}

/// JNI entry for `com.yegobox.atlas.AtlasNative.backgroundCheck`.
#[no_mangle]
pub extern "system" fn Java_com_yegobox_atlas_AtlasNative_backgroundCheck<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    files_dir: jni::objects::JString<'local>,
    cache_dir: jni::objects::JString<'local>,
    installed: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let mut read = |s: &jni::objects::JString<'local>| -> String {
        env.get_string(s).map(String::from).unwrap_or_default()
    };
    let (files_dir, cache_dir, installed) = (read(&files_dir), read(&cache_dir), read(&installed));
    let out = std::panic::catch_unwind(|| background_check(&files_dir, &cache_dir, &installed))
        .unwrap_or_else(|_| {
            json!({ "apks": [], "error": "background check panicked" }).to_string()
        });
    env.new_string(out)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
