//! Windows integration: one instance at a time, media keys, staying awake,
//! monitors, the date, the desktop shortcut and the Pictures folder.

pub(crate) mod clock;
pub(crate) mod instance;
pub(crate) mod media_keys;
pub(crate) mod power;
pub(crate) mod screen;
pub(crate) mod shortcut;

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::GUID;

/// Whether `exe` is this copy of Afterglow.
pub fn is_this_exe(exe: &Path) -> bool {
    std::env::current_exe().is_ok_and(|this| this.to_string_lossy().eq_ignore_ascii_case(&exe.to_string_lossy()))
}

/// A folder of the user's, such as the desktop, wherever it really is
/// (OneDrive, for one, moves them).
fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let dir = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(OsString::from_wide(dir.as_wide()));
        CoTaskMemFree(Some(dir.0 as _));
        Some(path)
    }
}

/// The user's Pictures folder.
pub fn pictures() -> Option<PathBuf> {
    known_folder(&FOLDERID_Pictures)
}
