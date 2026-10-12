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
    let versions = versions.canonicalize().unwrap_or(versions);
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

/// Start `entry` detached so it outlives the launcher.
pub fn spawn(entry: &Path) -> io::Result<()> {
    let dir = entry.parent().map(Path::to_path_buf).unwrap_or_default();

    #[cfg(target_os = "macos")]
    {
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
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map(|_| ())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::process::Stdio;
        let mut child = Command::new(entry)
            .current_dir(dir)
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

/// Start-menu + desktop shortcut on Windows, a .desktop entry on Linux.
/// macOS users open apps from the launcher (or keep it in the Dock).
#[allow(unused_variables)]
pub fn install_shortcut(app_id: &str, name: &str) -> io::Result<()> {
    let launcher = launcher_command()?;

    #[cfg(windows)]
    {
        let mut targets = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            targets.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            targets.push(PathBuf::from(profile).join("Desktop"));
        }
        for dir in targets.into_iter().filter(|d| d.is_dir()) {
            let mut link = mslnk::ShellLink::new(&launcher).map_err(io::Error::other)?;
            link.set_arguments(Some(format!("--launch {app_id}")));
            link.set_name(Some(name.to_string()));
            link.set_icon_location(Some(launcher.to_string_lossy().into_owned()));
            link.create_lnk(dir.join(format!("{name}.lnk")))
                .map_err(io::Error::other)?;
        }
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let Some(home) = std::env::var_os("HOME") else {
            return Ok(());
        };
        let dir = PathBuf::from(home).join(".local/share/applications");
        std::fs::create_dir_all(&dir)?;
        let exec = launcher.to_string_lossy().replace('"', "\\\"");
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec=\"{exec}\" --launch {app_id}\n\
             Icon=yegobox-launcher\nTerminal=false\nCategories=Office;Finance;\n"
        );
        std::fs::write(dir.join(format!("yegobox-{app_id}.desktop")), desktop)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = launcher;
        Ok(())
    }
}
