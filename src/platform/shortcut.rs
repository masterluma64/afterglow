//! Shortcuts to Afterglow on the desktop and in the Start menu.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};

/// Where a shortcut can go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Desktop,
    /// The user's own Start menu, so no admin rights are needed.
    StartMenu,
}

impl Place {
    pub const ALL: [Place; 2] = [Place::Desktop, Place::StartMenu];

    /// Its name in the UI.
    pub fn name(self) -> &'static str {
        match self {
            Place::Desktop => "Desktop",
            Place::StartMenu => "Start menu",
        }
    }
}

/// Where the shortcut goes. The desktop isn't always in the user's folder;
/// OneDrive, for one, moves it.
fn path(place: Place) -> Option<PathBuf> {
    let folder = match place {
        Place::Desktop => &FOLDERID_Desktop,
        Place::StartMenu => &FOLDERID_Programs,
    };
    Some(super::known_folder(folder)?.join(format!("{}.lnk", crate::APP_NAME)))
}

/// The executable the shortcut in `place` starts, if there is one.
pub fn target(place: Place) -> Option<PathBuf> {
    let link = path(place)?;
    if !link.exists() {
        return None;
    }
    // A shortcut that can't be read still counts, so it can be replaced.
    Some(with_com(|| unsafe { load(&link) }).unwrap_or_default())
}

/// Puts a shortcut to this executable in `place`, replacing an older one.
pub fn create(place: Place) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let link = path(place).ok_or("The folder wasn't found")?;
    with_com(|| unsafe { save(&exe, &link) }).map_err(|e| e.message())
}

fn with_com<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        // The UI thread has COM already; this only balances the count.
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = f();
        if com {
            CoUninitialize();
        }
        result
    }
}

unsafe fn load(link: &Path) -> windows::core::Result<PathBuf> {
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        shell_link.cast::<IPersistFile>()?.Load(&HSTRING::from(link), STGM_READ)?;
        let mut buffer = [0u16; 1024];
        shell_link.GetPath(&mut buffer, std::ptr::null_mut(), 0)?;
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Ok(PathBuf::from(OsString::from_wide(&buffer[..len])))
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

pub fn remove(place: Place) -> Result<(), String> {
    let Some(link) = path(place) else { return Ok(()) };
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
        with_com(|| unsafe { save(&exe, &link) }).unwrap();
        assert_eq!(with_com(|| unsafe { load(&link) }).unwrap(), exe);
        let bytes = std::fs::read(&link).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // Every shell link starts with its header size, 0x4C.
        assert_eq!(bytes[..4], [0x4C, 0, 0, 0]);
    }
}
