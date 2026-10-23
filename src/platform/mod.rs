//! Windows integration: one instance at a time, media keys, staying awake,
//! monitors, the date, the desktop shortcut and Explorer.

pub(crate) mod clock;
pub(crate) mod explorer;
pub(crate) mod instance;
pub(crate) mod media_keys;
pub(crate) mod power;
pub(crate) mod screen;
pub(crate) mod shortcut;

use std::path::{Path, PathBuf};

/// How Afterglow is set up in Windows, as shown in Settings.
#[derive(Default)]
pub struct Integration {
    /// The executables the desktop shortcut, the folder menu and "Open with"
    /// start, where they're there: another copy of Afterglow can have set them up.
    pub shortcut: Option<PathBuf>,
    pub folder_menu: Option<PathBuf>,
    pub open_with: Option<PathBuf>,
    /// Registered as an app that can be the default for videos.
    pub default: bool,
    /// The video types Windows opens with another app.
    pub elsewhere: Vec<&'static str>,
}

impl Integration {
    pub fn read() -> Self {
        Integration {
            shortcut: shortcut::target(),
            folder_menu: explorer::folder_menu(),
            open_with: explorer::open_with(),
            default: explorer::default(),
            elsewhere: explorer::opened_elsewhere(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    DesktopShortcut,
    FolderMenu,
    OpenWith,
    Default,
}

pub fn set(part: Part, on: bool) -> Result<(), String> {
    match (part, on) {
        (Part::DesktopShortcut, true) => shortcut::create(),
        (Part::DesktopShortcut, false) => shortcut::remove(),
        (Part::FolderMenu, on) => explorer::set_folder_menu(on),
        (Part::OpenWith, on) => explorer::set_open_with(on),
        (Part::Default, on) => explorer::set_default(on),
    }
}

/// Whether `exe` is this copy of Afterglow.
pub fn is_this_exe(exe: &Path) -> bool {
    std::env::current_exe().is_ok_and(|this| this.to_string_lossy().eq_ignore_ascii_case(&exe.to_string_lossy()))
}
