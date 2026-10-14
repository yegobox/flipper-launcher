//! The app catalog (`apps.json`) the launcher installs from.
//!
//! Written by `scripts/ci/update_launcher_manifest.py` in yegobox/flipper and
//! published on that repo's fixed `launcher-manifest` release. Each platform
//! entry carries its own version: a CI run that built only Windows leaves the
//! last macOS and Linux releases in place.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 1;
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/yegobox/flipper/releases/download/launcher-manifest/apps.json";

/// `LAUNCHER_MANIFEST_URL` points a dev build at a local manifest.
pub fn manifest_url() -> String {
    std::env::var("LAUNCHER_MANIFEST_URL").unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    #[serde(default)]
    pub apps: Vec<AppEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// One line under the name, e.g. "Point of sale, stock and EBM receipts".
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    /// Card accent colour, `#rrggbb`.
    #[serde(default)]
    pub accent: Option<String>,
    /// Android package of an app that ships through Google Play (e.g.
    /// `rw.flipper`). Android Atlas shows it as a Play link and never
    /// installs or updates it itself; desktop ignores it.
    #[serde(default)]
    pub play_package: Option<String>,
    #[serde(default)]
    pub channels: BTreeMap<String, Channel>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Channel {
    #[serde(default)]
    pub platforms: BTreeMap<String, Release>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub pub_date: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "zero_version")]
    pub min_launcher: String,
    pub url: String,
    pub sha256: String,
    pub signature: String,
    #[serde(default)]
    pub size: u64,
    /// Desktop: the executable inside the archive. Android: the package name.
    pub entry: String,
    /// Android only: the APK's applicationId (must equal `entry`).
    #[serde(default)]
    pub package: Option<String>,
    /// Android only: the APK's versionCode. Android orders installs by this,
    /// not by `version`, so it decides whether an update is newer.
    #[serde(default)]
    pub version_code: Option<u64>,
}

impl Release {
    /// Android: is this release newer than the installed versionCode (or is nothing installed)?
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn newer_than_code(&self, installed: Option<u64>) -> bool {
        match (self.version_code, installed) {
            (Some(c), Some(i)) => c > i,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

fn zero_version() -> String {
    "0.0.0".into()
}

/// The manifest key for this build, named as Tauri names targets.
pub fn platform_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        // One universal APK serves every Android ABI.
        ("android", _) => Some("android"),
        ("windows", "x86_64") => Some("windows-x86_64"),
        ("macos", "aarch64") => Some("darwin-aarch64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

pub fn parse_version(v: &str) -> Option<Version> {
    Version::parse(v.trim()).ok()
}

/// True when `candidate` is a valid version newer than `installed` (or nothing is installed).
pub fn is_newer(candidate: &str, installed: Option<&str>) -> bool {
    let Some(c) = parse_version(candidate) else {
        return false;
    };
    match installed.and_then(parse_version) {
        Some(i) => c > i,
        None => true,
    }
}

/// Android package name: two or more `[A-Za-z][A-Za-z0-9_]*` segments joined by dots.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn valid_package(pkg: &str) -> bool {
    let parts: Vec<&str> = pkg.split('.').collect();
    parts.len() >= 2
        && pkg.len() <= 255
        && parts.iter().all(|p| {
            p.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// App ids become folder names, so keep them to `[a-z0-9-]`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Manifest {
    pub fn app(&self, id: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|a| a.id == id)
    }
}

impl AppEntry {
    /// The release to install on `platform` for a user on `channel`.
    /// Beta users get whichever of beta and stable is newer, so a stable
    /// hotfix is never held back behind an older beta.
    pub fn release_for(&self, channel: &str, platform: &str) -> Option<&Release> {
        let pick = |c: &str| {
            self.channels
                .get(c)
                .and_then(|ch| ch.platforms.get(platform))
                .filter(|r| parse_version(&r.version).is_some())
        };
        let stable = pick("stable");
        if channel != "beta" {
            return stable;
        }
        match (pick("beta"), stable) {
            (Some(b), Some(s)) => Some(if is_newer(&b.version, Some(&s.version)) {
                b
            } else {
                s
            }),
            (b, s) => b.or(s),
        }
    }
}

pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<Manifest, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("could not reach the app catalog: {e}"))?;
    // No catalog published yet (the first desktop release creates it): an
    // empty list, not an error.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Manifest {
            schema: SCHEMA,
            apps: Vec::new(),
        });
    }
    let manifest: Manifest = resp
        .error_for_status()
        .map_err(|e| format!("could not reach the app catalog: {e}"))?
        .json()
        .await
        .map_err(|e| format!("app catalog is not valid JSON: {e}"))?;
    if manifest.schema != SCHEMA {
        return Err(format!(
            "app catalog schema {} is newer than this launcher understands; update the launcher",
            manifest.schema
        ));
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "schema": 1,
      "apps": [{
        "id": "flipper", "name": "Flipper POS",
        "channels": {
          "stable": { "platforms": {
            "windows-x86_64": { "version": "1.505.0", "url": "u", "sha256": "s", "signature": "g", "entry": "flipper.exe" },
            "linux-x86_64":   { "version": "1.504.0", "url": "u", "sha256": "s", "signature": "g", "entry": "flipper" }
          }},
          "beta": { "platforms": {
            "windows-x86_64": { "version": "1.506.0-beta.3", "url": "u", "sha256": "s", "signature": "g", "entry": "flipper.exe" },
            "linux-x86_64":   { "version": "1.504.0-beta.1", "url": "u", "sha256": "s", "signature": "g", "entry": "flipper" }
          }}
        }
      }]
    }"#;

    fn sample() -> Manifest {
        serde_json::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn parses_and_defaults_optional_fields() {
        let m = sample();
        let r = m
            .app("flipper")
            .unwrap()
            .release_for("stable", "windows-x86_64")
            .unwrap();
        assert_eq!(r.version, "1.505.0");
        assert_eq!(r.min_launcher, "0.0.0");
        assert_eq!(r.size, 0);
    }

    #[test]
    fn stable_users_never_see_beta() {
        let app = sample().apps.remove(0);
        assert_eq!(
            app.release_for("stable", "windows-x86_64").unwrap().version,
            "1.505.0"
        );
    }

    #[test]
    fn beta_users_get_the_newer_of_beta_and_stable() {
        let app = sample().apps.remove(0);
        assert_eq!(
            app.release_for("beta", "windows-x86_64").unwrap().version,
            "1.506.0-beta.3"
        );
        // 1.504.0-beta.1 < 1.504.0, so the stable release wins.
        assert_eq!(
            app.release_for("beta", "linux-x86_64").unwrap().version,
            "1.504.0"
        );
    }

    #[test]
    fn unknown_platform_has_no_release() {
        let app = sample().apps.remove(0);
        assert!(app.release_for("stable", "darwin-aarch64").is_none());
    }

    #[test]
    fn semver_ordering() {
        assert!(is_newer("1.505.0", Some("1.504.9")));
        assert!(is_newer("1.505.0", Some("1.505.0-beta.9")));
        assert!(!is_newer("1.505.0-beta.9", Some("1.505.0")));
        assert!(!is_newer("1.505.0", Some("1.505.0")));
        assert!(is_newer("1.0.0", None));
        assert!(!is_newer("garbage", None));
    }

    /// Serves one HTTP response on a local port and returns its URL.
    fn serve_once(status: &'static str, body: &'static str) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let _ = write!(
                sock,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        format!("http://127.0.0.1:{port}/apps.json")
    }

    #[tokio::test]
    async fn unpublished_catalog_is_empty_not_an_error() {
        let url = serve_once("404 Not Found", "");
        let m = fetch(&reqwest::Client::new(), &url).await.unwrap();
        assert!(m.apps.is_empty());
    }

    #[tokio::test]
    async fn server_errors_are_still_errors() {
        let url = serve_once("500 Internal Server Error", "");
        assert!(fetch(&reqwest::Client::new(), &url)
            .await
            .unwrap_err()
            .contains("500"));
    }

    #[test]
    fn android_entries_and_play_links_parse() {
        let m: Manifest = serde_json::from_str(r#"{
          "schema": 1,
          "apps": [
            { "id": "sms-bridge", "name": "SMS Bridge", "channels": { "stable": { "platforms": {
                "android": { "version": "1.2.0", "version_code": 42, "package": "rw.flipper.smsbridge",
                             "url": "u", "sha256": "s", "signature": "g", "entry": "rw.flipper.smsbridge" } } } } },
            { "id": "flipper", "name": "Flipper POS", "play_package": "rw.flipper", "channels": {} }
          ]
        }"#)
        .unwrap();
        let r = m
            .app("sms-bridge")
            .unwrap()
            .release_for("stable", "android")
            .unwrap();
        assert_eq!(r.version_code, Some(42));
        assert_eq!(r.package.as_deref(), Some("rw.flipper.smsbridge"));
        assert!(r.newer_than_code(None));
        assert!(r.newer_than_code(Some(41)));
        assert!(!r.newer_than_code(Some(42)));
        assert_eq!(
            m.app("flipper").unwrap().play_package.as_deref(),
            Some("rw.flipper")
        );
        // Desktop-only catalogs still parse: the new fields default to None.
        assert_eq!(
            serde_json::from_str::<Manifest>(SAMPLE).unwrap().apps[0].play_package,
            None
        );
    }

    #[test]
    fn android_package_names() {
        assert!(valid_package("rw.flipper.smsbridge"));
        assert!(valid_package("com.yegobox.launcher"));
        assert!(!valid_package("flipper"));
        assert!(!valid_package("rw..flipper"));
        assert!(!valid_package("rw.1flipper"));
        assert!(!valid_package("rw.flipper/../x"));
    }

    #[test]
    fn ids_are_folder_safe() {
        assert!(valid_id("flipper"));
        assert!(valid_id("flipper-hr"));
        assert!(!valid_id("../etc"));
        assert!(!valid_id("Flipper"));
        assert!(!valid_id(""));
    }
}
