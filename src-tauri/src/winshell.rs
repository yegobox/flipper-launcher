//! Windows shortcuts and taskbar identity for the apps Atlas launches.
//!
//! Pinning a running app's taskbar button records that window's exe,
//! `apps/<id>/versions/<ver>/flipper.exe`. After one update the pin opens the
//! old version behind Atlas's back; after the next, `prune` deletes the folder
//! and the pin is dead. So each app gets one AppUserModelID, shared by its
//! shortcuts and its window, and the launch tells the window how to relaunch
//! it (through Atlas). Pinning the window then pins Atlas's command, which
//! always opens the current version. The app's Windows runner reads the
//! `YEGOBOX_*` variables from [`launch_env`].

use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use windows::core::{Interface, Result, HSTRING, PWSTR};
use windows::Win32::Foundation::E_OUTOFMEMORY;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_RoamingAppData, IShellLinkW,
    SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
};

/// One per app, never per version: the taskbar groups and pins by it.
pub fn app_user_model_id(app_id: &str) -> String {
    format!("Yegobox.Atlas.{app_id}")
}

/// What the app's runner puts on its window (see the module docs).
pub fn launch_env(
    launcher: &Path,
    app_id: &str,
    name: &str,
    icon: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("YEGOBOX_AUMID", app_user_model_id(app_id)),
        (
            "YEGOBOX_RELAUNCH_COMMAND",
            format!(
                "\"{}\" {}",
                launcher.display(),
                super::launch::launch_args(app_id)
            ),
        ),
        ("YEGOBOX_RELAUNCH_NAME", name.to_string()),
    ];
    if let Some(icon) = icon {
        env.push(("YEGOBOX_RELAUNCH_ICON", format!("{},0", icon.display())));
    }
    env
}

/// Start menu, desktop and taskbar pins: where users keep shortcuts.
pub fn shortcut_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = known_folder(&FOLDERID_Programs) {
        dirs.push(p);
    }
    // The known folder, not %USERPROFILE%\Desktop: OneDrive moves the desktop.
    if let Some(p) = known_folder(&FOLDERID_Desktop) {
        dirs.push(p);
    }
    if let Some(p) = known_folder(&FOLDERID_RoamingAppData) {
        dirs.push(p.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"));
    }
    dirs
}

/// Where new shortcuts go: the start menu and the desktop.
pub fn new_shortcut_dirs() -> Vec<PathBuf> {
    [&FOLDERID_Programs, &FOLDERID_Desktop]
        .into_iter()
        .filter_map(known_folder)
        .collect()
}

/// A `.lnk` as read back from disk.
pub struct Link {
    pub target: String,
    pub args: String,
    pub app_user_model_id: Option<String>,
}

/// Write `path` as a shortcut that opens `app_id` through the launcher.
pub fn write_link(
    path: &Path,
    launcher: &Path,
    app_id: &str,
    name: &str,
    icon: Option<&Path>,
) -> Result<()> {
    let _com = Com::init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(launcher))?;
        link.SetArguments(&HSTRING::from(super::launch::launch_args(app_id)))?;
        link.SetDescription(&HSTRING::from(name))?;
        link.SetIconLocation(&HSTRING::from(icon.unwrap_or(launcher)), 0)?;
        let store: IPropertyStore = link.cast()?;
        let id = lpwstr(&app_user_model_id(app_id))?;
        store.SetValue(&PKEY_AppUserModel_ID, &id)?;
        store.Commit()?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(path), true)?;
    }
    Ok(())
}

pub fn read_link(path: &Path) -> Result<Link> {
    let _com = Com::init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.cast::<IPersistFile>()?
            .Load(&HSTRING::from(path), STGM_READ)?;
        let mut target = [0u16; 1024];
        // S_FALSE (no path, e.g. a shell-namespace link) leaves it empty.
        link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        let mut args = [0u16; 1024];
        link.GetArguments(&mut args)?;
        let store: IPropertyStore = link.cast()?;
        let id = store.GetValue(&PKEY_AppUserModel_ID).ok().and_then(|v| {
            let inner = &v.Anonymous.Anonymous;
            (inner.vt == VT_LPWSTR && !inner.Anonymous.pwszVal.is_null())
                .then(|| inner.Anonymous.pwszVal.to_string().ok())
                .flatten()
        });
        Ok(Link {
            target: wide_to_string(&target),
            args: wide_to_string(&args),
            app_user_model_id: id,
        })
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as *const _));
        path
    }
}

/// Shell links want the AUMID as VT_LPWSTR; `PROPVARIANT::from(&str)` makes a
/// BSTR, which the link's property store rejects. Freed by PROPVARIANT's Drop
/// (PropVariantClear → CoTaskMemFree).
fn lpwstr(value: &str) -> Result<PROPVARIANT> {
    let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let ptr = CoTaskMemAlloc(wide.len() * 2) as *mut u16;
        if ptr.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        Ok(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_LPWSTR,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        pwszVal: PWSTR(ptr),
                    },
                }),
            },
        })
    }
}

/// COM for the current thread, released only if this call initialized it.
/// A thread already in the MTA (RPC_E_CHANGED_MODE) can still use ShellLink.
struct Com(bool);

impl Com {
    fn init() -> Self {
        Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
