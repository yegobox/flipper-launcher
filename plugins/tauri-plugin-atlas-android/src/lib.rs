//! Atlas's Android bridge. The Kotlin side (`android/`) does what only
//! Android APIs can: read installed versions, install APKs through
//! PackageInstaller, open apps and Google Play, and schedule the background
//! update worker. Atlas calls it from Rust; it exposes nothing to JS.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{Manager, Runtime};

/// An installed (or missing) package.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub installed: bool,
    pub version_code: Option<u64>,
    pub version_name: Option<String>,
}

/// Where Atlas keeps settings and downloads on this device.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paths {
    pub files_dir: String,
    pub cache_dir: String,
    pub package_name: String,
}

#[derive(Deserialize)]
struct Flag {
    value: bool,
}

#[derive(Serialize)]
struct Pkg<'a> {
    pkg: &'a str,
}

pub struct AtlasAndroid<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> AtlasAndroid<R> {
    async fn call<T: DeserializeOwned>(&self, cmd: &str, payload: impl Serialize) -> Result<T, String> {
        self.0
            .run_mobile_plugin_async(cmd, payload)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn installed(&self, pkg: &str) -> Result<Installed, String> {
        self.call("installedVersion", Pkg { pkg }).await
    }

    /// Resolves once Android has installed the APK. Updates of apps Atlas
    /// installed are silent on Android 12+; otherwise the system dialog shows.
    pub async fn install_apk(&self, path: &str, label: &str) -> Result<(), String> {
        #[derive(Serialize)]
        struct Args<'a> {
            path: &'a str,
            label: &'a str,
        }
        self.call::<serde::de::IgnoredAny>("installApk", Args { path, label }).await.map(|_| ())
    }

    pub async fn launch(&self, pkg: &str) -> Result<(), String> {
        self.call::<serde::de::IgnoredAny>("launch", Pkg { pkg }).await.map(|_| ())
    }

    pub async fn open_play(&self, pkg: &str) -> Result<(), String> {
        self.call::<serde::de::IgnoredAny>("openPlay", Pkg { pkg }).await.map(|_| ())
    }

    /// Has the user allowed Atlas to install apps ("Install unknown apps")?
    pub async fn can_install(&self) -> Result<bool, String> {
        self.call::<Flag>("canInstall", ()).await.map(|f| f.value)
    }

    pub async fn open_install_settings(&self) -> Result<(), String> {
        self.call::<serde::de::IgnoredAny>("openInstallSettings", ()).await.map(|_| ())
    }

    pub async fn paths(&self) -> Result<Paths, String> {
        self.call("paths", ()).await
    }

    /// Run the background update check every `hours`; 0 cancels it.
    pub async fn schedule_updates(&self, hours: u32) -> Result<(), String> {
        #[derive(Serialize)]
        struct Args {
            hours: u32,
        }
        self.call::<serde::de::IgnoredAny>("scheduleUpdates", Args { hours }).await.map(|_| ())
    }

    /// Ask for the notification permission (Android 13+), used to say an
    /// update needs a tap when it can't install silently.
    pub async fn request_notifications(&self) -> Result<(), String> {
        #[derive(Serialize)]
        struct Args {
            permissions: [&'static str; 1],
        }
        self.call::<serde::de::IgnoredAny>("requestPermissions", Args { permissions: ["notifications"] })
            .await
            .map(|_| ())
    }
}

pub trait AtlasAndroidExt<R: Runtime> {
    fn atlas_android(&self) -> &AtlasAndroid<R>;
}

impl<R: Runtime, T: Manager<R>> AtlasAndroidExt<R> for T {
    fn atlas_android(&self) -> &AtlasAndroid<R> {
        self.state::<AtlasAndroid<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("atlas-android")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.yegobox.atlas", "AtlasPlugin")?;
            app.manage(AtlasAndroid(handle));
            Ok(())
        })
        .build()
}
