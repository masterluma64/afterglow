//! Windows integration: one instance at a time, media keys, staying awake,
//! monitors, the date and the desktop shortcut.

pub(crate) mod clock;
pub(crate) mod instance;
pub(crate) mod media_keys;
pub(crate) mod power;
pub(crate) mod screen;
pub(crate) mod shortcut;

use std::path::Path;

/// Whether `exe` is this copy of Afterglow.
pub fn is_this_exe(exe: &Path) -> bool {
    std::env::current_exe().is_ok_and(|this| this.to_string_lossy().eq_ignore_ascii_case(&exe.to_string_lossy()))
}
