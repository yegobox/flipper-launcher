//! On-disk state: one folder per app under `<app_local_data>/apps/<id>/`.
//!
//! ```text
//! apps/flipper/
//!   state.json          InstallState (current / previous / staged versions)
//!   versions/<ver>/     one extracted archive per version
//!   downloads/          archives while they download; removed once installed
//! ```
//! App data (Ditto, the local DB, preferences) never lives here: the apps keep
//! it in their own OS data dirs, so swapping a version folder leaves it alone.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallState {
    /// Version the shortcut opens.
    pub current: Option<String>,
    /// Version before `current`, kept on disk for rollback.
    pub previous: Option<String>,
    /// Installed while the app was running; becomes `current` at next launch.
    pub staged: Option<String>,
    /// Last version that stayed open past the crash window.
    pub last_good: Option<String>,
    /// Rolled back from; auto-update skips it until something newer ships.
    pub skipped: Option<String>,
    /// Executable path inside each version folder, as the manifest gave it.
    pub entries: BTreeMap<String, String>,
}

pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// `id` must already have passed `manifest::valid_id`.
    pub fn new(apps_root: &Path, id: &str) -> Self {
        Self {
            root: apps_root.join(id),
        }
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// `version` must already have parsed as semver (no path separators).
    pub fn version(&self, version: &str) -> PathBuf {
        self.versions().join(version)
    }

    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn load(&self) -> InstallState {
        fs::read(self.state_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state: &InstallState) -> io::Result<()> {
        write_json(&self.state_file(), state)
    }

    /// Absolute path of the executable for `version`, if it is installed.
    pub fn entry_path(&self, state: &InstallState, version: &str) -> Option<PathBuf> {
        let entry = state.entries.get(version)?;
        let path = self.version(version).join(entry);
        path.exists().then_some(path)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "stable" or "beta". Beta also receives stable when it is newer.
    pub channel: String,
    pub auto_update: bool,
    pub launch_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: "stable".into(),
            auto_update: true,
            launch_at_login: true,
        }
    }
}

pub fn load_settings(config_dir: &Path) -> Settings {
    fs::read(config_dir.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_settings(config_dir: &Path, settings: &Settings) -> io::Result<()> {
    write_json(&config_dir.join("settings.json"), settings)
}

/// Write via a temp file + rename so a crash never leaves half a JSON file.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path(), "flipper");
        assert_eq!(paths.load(), InstallState::default());

        let mut state = InstallState {
            current: Some("1.0.0".into()),
            ..Default::default()
        };
        state.entries.insert("1.0.0".into(), "flipper.exe".into());
        paths.save(&state).unwrap();
        assert_eq!(paths.load(), state);
    }

    #[test]
    fn settings_fill_missing_fields_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), r#"{"channel":"beta"}"#).unwrap();
        let s = load_settings(dir.path());
        assert_eq!(s.channel, "beta");
        assert!(s.auto_update);
    }
}
