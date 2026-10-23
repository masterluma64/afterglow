//! A shortcut to Afterglow on the desktop.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IPersistFile,
};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::core::{HSTRING, Interface};

/// Where the shortcut goes. The desktop isn't always in the user's folder;
/// OneDrive, for one, moves it.
fn path() -> Option<PathBuf> {
    unsafe {
        let dir = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(OsString::from_wide(dir.as_wide()));
        CoTaskMemFree(Some(dir.0 as _));
        Some(path.join(format!("{}.lnk", crate::APP_NAME)))
    }
}

pub fn exists() -> bool {
    path().is_some_and(|path| path.exists())
}

/// Puts a shortcut to this executable on the desktop, replacing an older one.
pub fn create() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let link = path().ok_or("The desktop folder wasn't found")?;
    unsafe {
        // The UI thread has COM already; this only balances the count.
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let saved = save(&exe, &link);
        if com {
            CoUninitialize();
        }
        saved.map_err(|e| e.message())
    }
}

unsafe fn save(exe: &Path, link: &Path) -> windows::core::Result<()> {
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        shell_link.SetPath(&HSTRING::from(exe))?;
        if let Some(dir) = exe.parent() {
            shell_link.SetWorkingDirectory(&HSTRING::from(dir))?;
        }
        shell_link.cast::<IPersistFile>()?.Save(&HSTRING::from(link), true)
    }
}

pub fn remove() -> Result<(), String> {
    let Some(link) = path() else { return Ok(()) };
    match std::fs::remove_file(link) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_a_shortcut() {
        let dir = std::env::temp_dir().join(format!("afterglow-shortcut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("test.lnk");
        let exe = std::env::current_exe().unwrap();
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            save(&exe, &link).unwrap();
            if com {
                CoUninitialize();
            }
        }
        let bytes = std::fs::read(&link).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // Every shell link starts with its header size, 0x4C.
        assert_eq!(bytes[..4], [0x4C, 0, 0, 0]);
    }
}
