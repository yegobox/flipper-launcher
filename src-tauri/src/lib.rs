//! Atlas (the Yegobox launcher): installs Yegobox apps from a signed catalog and
//! keeps them, and itself, up to date. Desktop logic lives in `desktop.rs`,
//! Android in `android.rs`; the catalog, downloads and signature checks
//! (`manifest.rs`, `installer.rs`) are shared.
//!
//! Desktop entry modes:
//!   (none)             open the window, run the background updater
//!   --background       autostart at login: tray only, run the updater
//!   --launch <app-id>  what app shortcuts run: apply a staged update, open
//!                      the app, then exit (or show the window on trouble)
//!   --install <app-id> install or update without the window, exit 0/1
//!                      (IT pre-installs on tills, and end-to-end tests)

mod installer;
mod manifest;
mod store;

#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
mod launch;

#[cfg(target_os = "android")]
mod android;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(desktop)]
    desktop::run();
    #[cfg(target_os = "android")]
    android::run();
}
