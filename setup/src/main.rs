//! The Afterglow setup. It puts afterglow.exe (built into this exe), its
//! license and notices in `%LOCALAPPDATA%\Programs\afterglow`, or where an
//! earlier install is, then has the new afterglow.exe add its shortcuts and
//! registry entries (`afterglow.exe --register`, see
//! `src/platform/install.rs`). No administrator rights are needed.
//!
//! `--quiet` installs without asking anything, keeping the desktop shortcut
//! as it was.

#![windows_subsystem = "windows"]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_ERROR_ICON, TD_WARNING_ICON,
    TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_SIZE_TO_CONTENT, TaskDialogIndirect,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

#[path = "../../src/platform/installed.rs"]
mod installed;

const APP: &[u8] = include_bytes!(env!("AFTERGLOW_PAYLOAD"));
const LICENSE: &[u8] = include_bytes!("../../LICENSE");
const NOTICES: &[u8] = include_bytes!("../../THIRD-PARTY-NOTICES.txt");
const VERSION: &str = env!("AFTERGLOW_VERSION");

const INSTALL: i32 = 100;
const START: i32 = 101;
const RETRY: i32 = 102;

enum Stop {
    Cancelled,
    Failed(String),
    /// Failed, and afterglow.exe already said why.
    Told,
}

fn main() {
    let quiet = std::env::args().any(|a| a == "--quiet");
    let code = match run(quiet) {
        Ok(()) => 0,
        Err(Stop::Cancelled) => 1,
        Err(Stop::Told) => 2,
        Err(Stop::Failed(message)) => {
            if !quiet {
                Dialog::new("Afterglow couldn't be installed", &message).icon(Icon::Error).show();
            }
            2
        }
    };
    std::process::exit(code);
}

fn run(quiet: bool) -> Result<(), Stop> {
    let earlier = installed::find();
    let dir = match &earlier {
        Some(earlier) => earlier.dir.clone(),
        None => installed::default_dir().ok_or_else(|| Stop::Failed("The folder for programs (%LOCALAPPDATA%) wasn't found.".into()))?,
    };
    let exe = dir.join(installed::EXE);

    let mut desktop = earlier.as_ref().is_some_and(|e| e.desktop);
    if !quiet {
        let (title, button, content) = match &earlier {
            None => (
                format!("Install Afterglow {VERSION}"),
                "Install",
                format!(
                    "Afterglow goes in {}, just for you. It gets a Start menu shortcut, and you can open videos with it from \"Open with\".\n\nAfterglow is free software, under the GNU General Public License 3.0 or later.",
                    dir.display()
                ),
            ),
            Some(earlier) => (
                if earlier.version == VERSION { format!("Reinstall Afterglow {VERSION}") } else { format!("Update Afterglow {} to {VERSION}", earlier.version) },
                if earlier.version == VERSION { "Reinstall" } else { "Update" },
                format!("Afterglow is in {}. Your progress and settings are kept.\n\nIf Afterglow is open, it closes first.", dir.display()),
            ),
        };
        let (pressed, checked) =
            Dialog::new(&title, &content).button(INSTALL, button).common(TDCBF_CANCEL_BUTTON).checkbox("Add a shortcut to the desktop", desktop).show();
        if pressed != INSTALL {
            return Err(Stop::Cancelled);
        }
        desktop = checked;
    }

    while !installed::close_running(&exe, Duration::from_secs(10)) {
        if quiet {
            return Err(Stop::Failed("Afterglow is open and didn't close.".into()));
        }
        let (pressed, _) = Dialog::new("Close Afterglow to continue", "Afterglow is still open. Close it, then try again.")
            .icon(Icon::Warning)
            .button(RETRY, "Try again")
            .common(TDCBF_CANCEL_BUTTON)
            .show();
        if pressed != RETRY {
            return Err(Stop::Cancelled);
        }
    }

    put_files(&dir).map_err(|e| Stop::Failed(format!("Writing to {} failed: {e}", dir.display())))?;

    let mut register = Command::new(&exe);
    register.arg("--register").current_dir(&dir);
    if desktop {
        register.arg("--desktop");
    }
    if quiet {
        register.arg("--quiet");
    }
    let status = register.status().map_err(|e| Stop::Failed(format!("{} couldn't be started: {e}", exe.display())))?;
    if !status.success() {
        return Err(if quiet { Stop::Failed("afterglow.exe --register failed".into()) } else { Stop::Told });
    }

    if !quiet {
        let (pressed, _) = Dialog::new(
            &format!("Afterglow {VERSION} is installed"),
            "Find it in the Start menu, or open a video with it from \"Open with\". To uninstall it, go to Settings › Apps.",
        )
        .button(START, "Start Afterglow")
        .common(TDCBF_CLOSE_BUTTON)
        .show();
        if pressed == START {
            let _ = Command::new(&exe).current_dir(&dir).spawn();
        }
    }
    Ok(())
}

/// Writes the files, the exe last and through a temporary name, so a
/// failure doesn't leave half an exe behind.
fn put_files(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("LICENSE.txt"), LICENSE)?;
    std::fs::write(dir.join("THIRD-PARTY-NOTICES.txt"), NOTICES)?;
    let new = dir.join("afterglow.exe.new");
    std::fs::write(&new, APP)?;
    // The old exe was only just closed, so it may take a moment to be free.
    let mut tries = 0;
    loop {
        match std::fs::rename(&new, dir.join(installed::EXE)) {
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&new);
                return Err(e);
            }
            Ok(()) => return Ok(()),
        }
    }
}

enum Icon {
    App,
    Warning,
    Error,
}

/// A Windows task dialog: a title, text, buttons and maybe a checkbox.
struct Dialog {
    title: HSTRING,
    content: HSTRING,
    icon: Icon,
    buttons: Vec<(i32, HSTRING)>,
    common: TASKDIALOG_COMMON_BUTTON_FLAGS,
    checkbox: Option<(HSTRING, bool)>,
}

impl Dialog {
    fn new(title: &str, content: &str) -> Self {
        Dialog {
            title: title.into(),
            content: content.into(),
            icon: Icon::App,
            buttons: Vec::new(),
            common: TDCBF_CLOSE_BUTTON,
            checkbox: None,
        }
    }

    fn icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }

    fn button(mut self, id: i32, text: &str) -> Self {
        self.buttons.push((id, text.into()));
        self
    }

    fn common(mut self, buttons: TASKDIALOG_COMMON_BUTTON_FLAGS) -> Self {
        self.common = buttons;
        self
    }

    fn checkbox(mut self, text: &str, checked: bool) -> Self {
        self.checkbox = Some((text.into(), checked));
        self
    }

    /// Shows it, returning the button pressed and whether the box is ticked.
    fn show(self) -> (i32, bool) {
        let window_title = HSTRING::from("Afterglow setup");
        let buttons: Vec<TASKDIALOG_BUTTON> =
            self.buttons.iter().map(|(id, text)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: PCWSTR(text.as_ptr()) }).collect();
        let main_icon = match self.icon {
            // The icon built into this exe (see build.rs), by its number.
            Icon::App => PCWSTR(std::ptr::without_provenance(1)),
            Icon::Warning => TD_WARNING_ICON,
            Icon::Error => TD_ERROR_ICON,
        };
        let mut flags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT;
        if self.checkbox.as_ref().is_some_and(|(_, checked)| *checked) {
            flags |= windows::Win32::UI::Controls::TDF_VERIFICATION_FLAG_CHECKED;
        }
        let config = TASKDIALOGCONFIG {
            cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
            hwndParent: HWND::default(),
            hInstance: unsafe { GetModuleHandleW(None) }.map(Into::into).unwrap_or_default(),
            dwFlags: flags,
            dwCommonButtons: self.common,
            pszWindowTitle: PCWSTR(window_title.as_ptr()),
            Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: main_icon },
            pszMainInstruction: PCWSTR(self.title.as_ptr()),
            pszContent: PCWSTR(self.content.as_ptr()),
            cButtons: buttons.len() as u32,
            pButtons: if buttons.is_empty() { std::ptr::null() } else { buttons.as_ptr() },
            nDefaultButton: self.buttons.first().map_or(0, |(id, _)| *id),
            pszVerificationText: self.checkbox.as_ref().map_or(PCWSTR::null(), |(text, _)| PCWSTR(text.as_ptr())),
            ..Default::default()
        };
        let (mut pressed, mut checked) = (0, BOOL(0));
        if unsafe { TaskDialogIndirect(&config, Some(&mut pressed), None, Some(&mut checked)) }.is_err() {
            return (0, false);
        }
        (pressed, checked.as_bool())
    }
}
