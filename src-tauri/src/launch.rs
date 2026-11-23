//! Start an installed app, tell whether it is running, and put a shortcut
//! where users look for it.

use crate::store::AppPaths;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// True when any process runs an executable from this app's version folders.
/// Flipper's Ditto store allows one process per data dir, so the launcher must
/// never swap a version (or start a second copy) underneath a running one.
pub fn is_running(paths: &AppPaths) -> bool {
    let versions = paths.versions();
    // dunce, not std: on Windows std returns `\\?\C:\…`, which never prefixes
    // the `C:\…` paths processes report, so a running app looked closed.
    let versions = dunce::canonicalize(&versions).unwrap_or(versions);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    sys.processes()
        .values()
        .any(|p| p.exe().is_some_and(|exe| exe.starts_with(&versions)))
}

/// Start `entry` detached so it outlives the launcher, with `env` added
/// (macOS `open` drops it; nothing there needs it).
pub fn spawn(entry: &Path, env: &[(&'static str, String)]) -> io::Result<()> {
    let dir = entry.parent().map(Path::to_path_buf).unwrap_or_default();

    #[cfg(target_os = "macos")]
    {
        let _ = env;
        // LaunchServices start: Dock icon, focus, and a second `open` just
        // brings the running copy forward instead of starting another.
        let _ = dir;
        let status = Command::new("/usr/bin/open").arg(entry).status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "open {} failed: {status}",
                entry.display()
            )));
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        Command::new(entry)
            .current_dir(dir)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map(|_| ())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::process::Stdio;
        let mut child = Command::new(entry)
            .current_dir(dir)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reap it if the launcher is still around when it exits.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// After starting a freshly updated version: did it die inside `window`?
/// `None` means it never appeared at all.
pub async fn exited_early(paths: &AppPaths, window: Duration) -> bool {
    let start = Instant::now();
    let mut seen = false;
    while start.elapsed() < window {
        let running = is_running(paths);
        if running {
            seen = true;
        } else if seen || start.elapsed() > Duration::from_secs(5) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    !seen
}

/// The command a shortcut should run to open `app_id` through the launcher.
fn launcher_command() -> io::Result<PathBuf> {
    // Inside an AppImage, current_exe() is the temporary mount; APPIMAGE is the file.
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Ok(PathBuf::from(appimage));
    }
    std::env::current_exe()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_entry(app_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(format!(
        ".local/share/applications/yegobox-{app_id}.desktop"
    )))
}

/// What a shortcut passes the launcher to open `app_id`.
pub fn launch_args(app_id: &str) -> String {
    format!("--launch {app_id}")
}

/// Does a shortcut open `app_id`: through this launcher, or straight at one of
/// its version folders (a pin made from the running window before the app had
/// a taskbar identity)? Windows paths compare case-insensitively.
#[cfg_attr(not(windows), allow(dead_code))]
fn opens_app(target: &str, args: &str, launcher: &str, versions: &str, app_id: &str) -> bool {
    let target = target.to_lowercase();
    if target.is_empty() {
        return false;
    }
    let versions = versions.trim_end_matches(['\\', '/']).to_lowercase();
    (target == launcher.to_lowercase() && args.trim() == launch_args(app_id))
        || target.starts_with(&format!("{versions}\\"))
        || target.starts_with(&format!("{versions}/"))
}

/// What the app's window needs to pin as "open through the launcher"
/// (Windows; see `winshell`). Empty elsewhere.
#[allow(unused_variables)]
pub fn taskbar_env(paths: &AppPaths, app_id: &str, name: &str) -> Vec<(&'static str, String)> {
    #[cfg(windows)]
    if let Ok(launcher) = launcher_command() {
        let icon = paths.shortcut_icon();
        return crate::winshell::launch_env(
            &launcher,
            app_id,
            name,
            icon.exists().then_some(icon.as_path()),
        );
    }
    Vec::new()
}

/// Shortcuts for `app_id`. With `create` (first install) it adds a start-menu
/// and desktop shortcut on Windows, a .desktop entry on Linux. On Windows it
/// also rewrites every existing shortcut and taskbar pin that opens the app,
/// so each one carries the app's AppUserModelID and goes through the
/// launcher, never a version folder that `prune` will delete.
/// macOS users open apps from the launcher (or keep it in the Dock).
#[allow(unused_variables)]
pub fn sync_shortcuts(paths: &AppPaths, app_id: &str, create: Option<&str>) -> io::Result<()> {
    let launcher = launcher_command()?;

    #[cfg(windows)]
    {
        use crate::winshell;
        // Stable icon: a copy of the current entry, outside versions/.
        let state = paths.load();
        let icon = paths.shortcut_icon();
        if let Some(entry) = state
            .current
            .as_deref()
            .and_then(|v| paths.entry_path(&state, v))
        {
            if std::fs::read(&entry).ok() != std::fs::read(&icon).ok() {
                // Explorer may hold the old copy open; keep it and retry next start.
                let _ = std::fs::copy(&entry, &icon);
            }
        }
        let icon = icon.exists().then_some(icon.as_path());
        let aumid = winshell::app_user_model_id(app_id);
        let mut result = Ok(());

        if let Some(name) = create {
            for dir in winshell::new_shortcut_dirs()
                .into_iter()
                .filter(|d| d.is_dir())
            {
                let path = dir.join(format!("{name}.lnk"));
                if let Err(e) = winshell::write_link(&path, &launcher, app_id, name, icon) {
                    result = Err(io::Error::other(format!("{}: {e}", path.display())));
                }
            }
        }

        for (path, link) in app_links(&launcher, paths, app_id) {
            let current = link
                .target
                .eq_ignore_ascii_case(&launcher.to_string_lossy())
                && link.app_user_model_id.as_deref() == Some(aumid.as_str());
            if current {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| app_id.to_string());
            if let Err(e) = winshell::write_link(&path, &launcher, app_id, &name, icon) {
                result = Err(io::Error::other(format!("{}: {e}", path.display())));
            }
        }
        result
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let Some(name) = create else {
            return Ok(());
        };
        let Some(file) = desktop_entry(app_id) else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let exec = launcher.to_string_lossy().replace('"', "\\\"");
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec=\"{exec}\" --launch {app_id}\n\
             Icon=yegobox-launcher\nTerminal=false\nCategories=Office;Finance;\n"
        );
        std::fs::write(file, desktop)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = launcher;
        Ok(())
    }
}

/// Every Windows shortcut and taskbar pin that opens `app_id` (see `opens_app`).
#[cfg(windows)]
fn app_links(
    launcher: &Path,
    paths: &AppPaths,
    app_id: &str,
) -> Vec<(PathBuf, crate::winshell::Link)> {
    let launcher = launcher.to_string_lossy();
    let versions = paths.versions();
    let versions = versions.to_string_lossy();
    let mut found = Vec::new();
    for dir in crate::winshell::shortcut_dirs() {
        let Ok(items) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in items.flatten().map(|i| i.path()) {
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
            {
                continue;
            }
            let Ok(link) = crate::winshell::read_link(&path) else {
                continue;
            };
            if opens_app(&link.target, &link.args, &launcher, &versions, app_id) {
                found.push((path, link));
            }
        }
    }
    found
}

/// Uninstall: remove what `sync_shortcuts` made, and any pin that opens the
/// app. Shortcuts the user already deleted are fine.
#[allow(unused_variables)]
pub fn remove_shortcut(paths: &AppPaths, app_id: &str) -> io::Result<()> {
    #[allow(unused_mut)]
    let mut files: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    files.extend(
        app_links(&launcher_command()?, paths, app_id)
            .into_iter()
            .map(|(path, _)| path),
    );
    #[cfg(all(unix, not(target_os = "macos")))]
    files.extend(desktop_entry(app_id));
    for file in files {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::opens_app;

    const LAUNCHER: &str = r"C:\Users\a\AppData\Local\Atlas\atlas.exe";
    const VERSIONS: &str = r"C:\Users\a\AppData\Local\com.yegobox.launcher\apps\flipper\versions";

    #[test]
    fn pin_to_a_version_folder_opens_the_app() {
        let pinned = r"c:\users\a\appdata\local\com.yegobox.launcher\apps\flipper\versions\1.587.65\flipper.exe";
        assert!(opens_app(pinned, "", LAUNCHER, VERSIONS, "flipper"));
    }

    #[test]
    fn launcher_shortcut_opens_only_its_app() {
        assert!(opens_app(
            LAUNCHER,
            "--launch flipper",
            LAUNCHER,
            VERSIONS,
            "flipper"
        ));
        assert!(!opens_app(
            LAUNCHER,
            "--launch eduai",
            LAUNCHER,
            VERSIONS,
            "flipper"
        ));
        assert!(!opens_app(LAUNCHER, "", LAUNCHER, VERSIONS, "flipper"));
    }

    #[test]
    fn other_shortcuts_are_left_alone() {
        let other =
            r"C:\Users\a\AppData\Local\com.yegobox.launcher\apps\flipper\versions-old\x.exe";
        assert!(!opens_app(other, "", LAUNCHER, VERSIONS, "flipper"));
        assert!(!opens_app(
            r"C:\Windows\notepad.exe",
            "",
            LAUNCHER,
            VERSIONS,
            "flipper"
        ));
        assert!(!opens_app("", "", LAUNCHER, VERSIONS, "flipper"));
    }
}
