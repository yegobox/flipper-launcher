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
    pub entry: String,
}

fn zero_version() -> String {
    "0.0.0".into()
}

/// The manifest key for this build, named as Tauri names targets.
pub fn platform_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
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
    let manifest: Manifest = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
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

    #[test]
    fn ids_are_folder_safe() {
        assert!(valid_id("flipper"));
        assert!(valid_id("flipper-hr"));
        assert!(!valid_id("../etc"));
        assert!(!valid_id("Flipper"));
        assert!(!valid_id(""));
    }
}
