//! Desktop (Windows, macOS, Linux): installs apps into versioned folders,
//! swaps them when they're not running, rolls back, updates itself with
//! tauri-plugin-updater, and sits in the tray.

use crate::manifest::{is_newer, platform_key, valid_id, Manifest};
use crate::store::{AppPaths, Settings};
use crate::{installer, launch, manifest, store};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_updater::UpdaterExt;

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(30 * 60);
const CRASH_WINDOW: Duration = Duration::from_secs(10);

struct Shared {
    client: reqwest::Client,
    apps_root: PathBuf,
    config_dir: PathBuf,
    /// Same key Tauri's updater checks the launcher's own updates with.
    pubkey: String,
    manifest: Mutex<Option<Manifest>>,
    busy: Mutex<HashSet<String>>,
    notice: Mutex<Option<Notice>>,
    launcher_update: Mutex<Option<String>>,
    /// Unix ms of the last successful catalog fetch (footer: "checked 2 min ago").
    checked_at: Mutex<Option<u64>>,
}

impl Shared {
    fn paths(&self, id: &str) -> AppPaths {
        AppPaths::new(&self.apps_root, id)
    }

    fn settings(&self) -> Settings {
        store::load_settings(&self.config_dir)
    }
}

/// Shown once in the window, e.g. after an update crashed on start.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Notice {
    app_id: String,
    name: String,
    version: String,
    previous: String,
}

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
    /// update_available | update_staged | up_to_date
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

fn launcher_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

fn views(app: &AppHandle, shared: &Shared) -> Vec<AppView> {
    let Some(manifest) = shared.manifest.lock().unwrap().clone() else {
        return Vec::new();
    };
    let channel = shared.settings().channel;
    let busy = shared.busy.lock().unwrap().clone();
    let launcher = launcher_version(app);
    manifest
        .apps
        .iter()
        .filter(|a| valid_id(&a.id))
        .map(|a| {
            let paths = shared.paths(&a.id);
            let state = paths.load();
            let release = platform_key().and_then(|k| a.release_for(&channel, k));
            let newer = release.is_some_and(|r| {
                is_newer(&r.version, state.current.as_deref())
                    && state.skipped.as_deref() != Some(r.version.as_str())
                    && state.staged.as_deref() != Some(r.version.as_str())
            });
            let status = match release {
                None => "unsupported",
                Some(_) if busy.contains(&a.id) => "installing",
                Some(r) if newer && is_newer(&r.min_launcher, Some(&launcher)) => {
                    "needs_launcher_update"
                }
                Some(_) if state.current.is_none() => "not_installed",
                Some(_) if newer => "update_available",
                Some(_) if state.staged.is_some() => "update_staged",
                Some(_) => "up_to_date",
            };
            AppView {
                id: a.id.clone(),
                name: a.name.clone(),
                icon: a.icon.clone(),
                description: a.description.clone(),
                website: a.website.clone().filter(|w| w.starts_with("https://")),
                accent: a.accent.clone().filter(|c| {
                    c.len() == 7
                        && c.starts_with('#')
                        && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
                }),
                pub_date: release
                    .map(|r| r.pub_date.clone())
                    .filter(|d| !d.is_empty()),
                running: state.current.is_some() && launch::is_running(&paths),
                installed: state.current,
                staged: state.staged,
                previous: state.previous,
                available: release.map(|r| r.version.clone()),
                notes: release.map(|r| r.notes.clone()).unwrap_or_default(),
                size: release.map_or(0, |r| r.size),
                status,
            }
        })
        .collect()
}

async fn refresh(shared: &Shared) -> Result<(), String> {
    let manifest = manifest::fetch(&shared.client, &manifest::manifest_url()).await?;
    *shared.manifest.lock().unwrap() = Some(manifest);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    *shared.checked_at.lock().unwrap() = Some(now);
    Ok(())
}

/// Clears the busy flag however install() returns.
struct BusyGuard<'a>(&'a Shared, String);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.busy.lock().unwrap().remove(&self.1);
    }
}

/// Install `id`, or update it to the newest release for the user's channel.
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
    let platform = platform_key().ok_or("this computer's platform is not supported")?;
    let release = entry
        .release_for(&shared.settings().channel, platform)
        .cloned()
        .ok_or_else(|| format!("{} has no build for this computer yet", entry.name))?;
    if is_newer(&release.min_launcher, Some(&launcher_version(app))) {
        return Err(format!(
            "{} {} needs a newer launcher; restart to update it",
            entry.name, release.version
        ));
    }
    if !shared.busy.lock().unwrap().insert(id.to_string()) {
        return Err(format!("{} is already updating", entry.name));
    }
    let _guard = BusyGuard(&shared, id.to_string());
    let _ = app.emit("apps-changed", ());

    let paths = shared.paths(id);
    let file_name = release
        .url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("download");
    let archive = paths.downloads().join(file_name);
    let mut last = 0u64;
    installer::download(&shared.client, &release.url, &archive, |done, total| {
        // ~1 MB steps keep the IPC channel quiet.
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
    })
    .await?;

    let first_install = paths.load().current.is_none();
    let running = launch::is_running(&paths);
    let pubkey = shared.pubkey.clone();
    let apps_root = shared.apps_root.clone();
    let id_owned = id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        installer::stage(
            &AppPaths::new(&apps_root, &id_owned),
            &release,
            &archive,
            &pubkey,
            running,
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    if first_install {
        if let Err(e) = launch::install_shortcut(id, &entry.name) {
            eprintln!("shortcut for {id}: {e}");
        }
    }
    let _ = app.emit("apps-changed", ());
    Ok(())
}

/// Open `id`. Returns true when a fresh update died on start and the user
/// should be offered a rollback (see `Notice`).
async fn open(app: &AppHandle, id: &str) -> Result<bool, String> {
    if !valid_id(id) {
        return Err(format!("unknown app {id}"));
    }
    let shared = app.state::<Shared>();
    let paths = shared.paths(id);
    if launch::is_running(&paths) {
        // Never start a second copy (Ditto holds a lock on its data dir).
        // On macOS `open` only brings the running copy to the front.
        #[cfg(target_os = "macos")]
        {
            let state = paths.load();
            if let Some(entry) = state
                .current
                .as_deref()
                .and_then(|v| paths.entry_path(&state, v))
            {
                let _ = launch::spawn(&entry);
            }
        }
        return Ok(false);
    }
    installer::apply_staged(&paths)?;
    let state = paths.load();
    let current = state.current.clone().ok_or("not installed")?;
    let entry = paths
        .entry_path(&state, &current)
        .ok_or("installation is damaged; install it again")?;
    launch::spawn(&entry).map_err(|e| format!("could not start: {e}"))?;
    let _ = app.emit("apps-changed", ());

    if state.last_good.as_deref() == Some(current.as_str()) {
        return Ok(false);
    }
    let crashed = launch::exited_early(&paths, CRASH_WINDOW).await;
    let mut state = paths.load();
    if !crashed {
        state.last_good = Some(current);
        paths.save(&state).map_err(|e| e.to_string())?;
        return Ok(false);
    }
    let Some(previous) = state.previous.clone() else {
        return Ok(false);
    };
    let name = shared
        .manifest
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.app(id).map(|a| a.name.clone()))
        .unwrap_or_else(|| id.to_string());
    *shared.notice.lock().unwrap() = Some(Notice {
        app_id: id.to_string(),
        name,
        version: current,
        previous,
    });
    let _ = app.emit("apps-changed", ());
    Ok(true)
}

async fn check_launcher_update(app: &AppHandle) {
    let Ok(updater) = app.updater() else { return };
    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
                *app.state::<Shared>().launcher_update.lock().unwrap() = Some(version.clone());
                let _ = app.emit("launcher-update-ready", version);
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("launcher update check: {e}"),
    }
}

/// Refresh the catalog, update installed apps, update the launcher, repeat.
async fn update_once(app: &AppHandle) -> Result<(), String> {
    let shared = app.state::<Shared>();
    refresh(&shared).await?;
    if shared.settings().auto_update {
        let pending: Vec<String> = views(app, &shared)
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
    check_launcher_update(app).await;
    let _ = app.emit("apps-changed", ());
    Ok(())
}

fn start_updater(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let wait = match update_once(&app).await {
                Ok(()) => CHECK_EVERY,
                Err(e) => {
                    let _ = app.emit("error", e);
                    RETRY_AFTER_ERROR
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// A shortcut click: open the app; if that fails or the update crashed,
/// show the window so the user can see why (and roll back).
fn launch_from_shortcut(app: &AppHandle, id: String, exit_when_done: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = refresh(&app.state::<Shared>()).await;
        match open(&app, &id).await {
            Ok(false) if exit_when_done => app.exit(0),
            Ok(false) => {}
            Ok(true) => show_main(&app),
            Err(e) => {
                let _ = app.emit("error", e);
                show_main(&app);
                if exit_when_done {
                    start_updater(&app);
                }
            }
        }
    });
}

/// `--install <id>`: no window; the exit code says whether it worked.
fn install_headless(app: &AppHandle, id: String, exit_when_done: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = async {
            refresh(&app.state::<Shared>()).await?;
            install(&app, &id).await
        }
        .await;
        if let Err(e) = &result {
            eprintln!("install {id}: {e}");
            let _ = app.emit("error", e.clone());
        }
        if exit_when_done {
            app.exit(if result.is_ok() { 0 } else { 1 });
        }
    });
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let autolaunch = app.autolaunch();
    let result = match (enabled, autolaunch.is_enabled().unwrap_or(false)) {
        (true, false) => autolaunch.enable(),
        (false, true) => autolaunch.disable(),
        _ => Ok(()),
    };
    if let Err(e) = result {
        eprintln!("autostart: {e}");
    }
}

#[tauri::command]
async fn list_apps(app: AppHandle, shared: State<'_, Shared>) -> Result<Vec<AppView>, String> {
    if shared.manifest.lock().unwrap().is_none() {
        refresh(&shared).await?;
    }
    Ok(views(&app, &shared))
}

#[tauri::command]
async fn refresh_apps(app: AppHandle, shared: State<'_, Shared>) -> Result<Vec<AppView>, String> {
    refresh(&shared).await?;
    Ok(views(&app, &shared))
}

#[tauri::command]
async fn install_app(app: AppHandle, id: String) -> Result<(), String> {
    install(&app, &id).await
}

#[tauri::command]
async fn open_app(app: AppHandle, id: String) -> Result<bool, String> {
    open(&app, &id).await
}

#[tauri::command]
async fn rollback_app(
    app: AppHandle,
    shared: State<'_, Shared>,
    id: String,
) -> Result<String, String> {
    if !valid_id(&id) {
        return Err(format!("unknown app {id}"));
    }
    let paths = shared.paths(&id);
    if launch::is_running(&paths) {
        return Err("close the app before rolling back".into());
    }
    let version = installer::rollback(&paths)?;
    shared.notice.lock().unwrap().take();
    let _ = app.emit("apps-changed", ());
    Ok(version)
}

#[tauri::command]
fn take_notice(shared: State<'_, Shared>) -> Option<Notice> {
    shared.notice.lock().unwrap().take()
}

#[tauri::command]
fn get_settings(shared: State<'_, Shared>) -> Settings {
    shared.settings()
}

#[tauri::command]
fn set_settings(
    app: AppHandle,
    shared: State<'_, Shared>,
    settings: Settings,
) -> Result<(), String> {
    if settings.channel != "stable" && settings.channel != "beta" {
        return Err(format!("unknown channel {}", settings.channel));
    }
    store::save_settings(&shared.config_dir, &settings).map_err(|e| e.to_string())?;
    apply_autostart(&app, settings.launch_at_login);
    let _ = app.emit("apps-changed", ());
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherInfo {
    version: String,
    platform: Option<&'static str>,
    update_ready: Option<String>,
    checked_at: Option<u64>,
}

#[tauri::command]
fn launcher_info(app: AppHandle, shared: State<'_, Shared>) -> LauncherInfo {
    LauncherInfo {
        version: launcher_version(&app),
        platform: platform_key(),
        update_ready: shared.launcher_update.lock().unwrap().clone(),
        checked_at: *shared.checked_at.lock().unwrap(),
    }
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<(), String> {
    update_once(&app).await
}

#[tauri::command]
fn restart_launcher(app: AppHandle) {
    app.restart();
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Atlas", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "Check for updates", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &check, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Atlas")
        .menu(&menu);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "open" => show_main(app),
        "check" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = update_once(&app).await {
                    let _ = app.emit("error", e);
                }
            });
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .build(app)?;
    Ok(())
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let launch_id = arg_value(&args, "--launch");
    let install_id = arg_value(&args, "--install");
    let background = args.iter().any(|a| a == "--background");

    tauri::Builder::default()
        // Must be first: a second start (e.g. a shortcut while the launcher
        // sits in the tray) hands its arguments to the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(id) = arg_value(&argv, "--install") {
                install_headless(app, id, false);
            } else if let Some(id) = arg_value(&argv, "--launch") {
                launch_from_shortcut(app, id, false);
            } else {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .setup(move |app| {
            let pubkey = app
                .config()
                .plugins
                .0
                .get("updater")
                .and_then(|u| u.get("pubkey"))
                .and_then(|k| k.as_str())
                .unwrap_or_default()
                .to_string();
            // Debug builds only: test against a catalog signed with a throwaway key.
            #[cfg(debug_assertions)]
            let pubkey = std::env::var("LAUNCHER_PUBKEY").unwrap_or(pubkey);
            let client = reqwest::Client::builder()
                .user_agent(concat!("yegobox-launcher/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(20))
                .build()?;
            app.manage(Shared {
                client,
                apps_root: app.path().app_local_data_dir()?.join("apps"),
                config_dir: app.path().app_config_dir()?,
                pubkey,
                manifest: Mutex::new(None),
                busy: Mutex::new(HashSet::new()),
                notice: Mutex::new(None),
                launcher_update: Mutex::new(None),
                checked_at: Mutex::new(None),
            });
            let handle = app.handle().clone();
            if let Some(id) = install_id.clone() {
                install_headless(&handle, id, true);
                return Ok(());
            }
            apply_autostart(&handle, app.state::<Shared>().settings().launch_at_login);
            build_tray(app)?;
            match launch_id.clone() {
                Some(id) => launch_from_shortcut(&handle, id, true),
                None => {
                    if !background {
                        show_main(&handle);
                    }
                    start_updater(&handle);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the launcher in the tray so updates keep arriving.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building Atlas")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_main(_app);
            }
        });
}
