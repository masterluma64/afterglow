//! Installing and uninstalling, run from the command line rather than the
//! window:
//!
//! - `afterglow.exe --register [--desktop] [--no-start-menu]`: the setup
//!   (`setup/`) runs this once it has put the files in place. It adds the
//!   Start menu and desktop shortcuts asked for (removing the others), "Open
//!   with" for videos, and the entry in Settings › Apps.
//! - `afterglow.exe --uninstall [--quiet]`: that entry's Uninstall button.
//!   It runs again from a copy in the temp folder (an exe can't delete itself
//!   while it runs), which removes all of the above and the program's files,
//!   but keeps the progress and settings.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::installed::{self, Key};
use super::shortcut::{self, Place};
use crate::library::VIDEO_EXTENSIONS;

/// The file type "Open with" lists Afterglow under.
const PROG_ID: &str = "Afterglow.Video";
const CLASSES: &str = r"Software\Classes";
/// Where Windows looks for an app's name and the types it opens.
const APPLICATION_KEY: &str = r"Software\Classes\Applications\afterglow.exe";

/// Runs the command the arguments ask for, if any, returning the exit code;
/// `None` means start the player as usual.
pub fn run_command() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let quiet = has("--quiet");
    let result = match args.first()?.to_str()? {
        "--register" => register(has("--desktop"), !has("--no-start-menu")),
        "--uninstall" => uninstall(quiet),
        "--uninstall-from" => {
            let dir = PathBuf::from(args.get(1)?);
            let parent = args.iter().position(|a| a == "--parent").and_then(|i| args.get(i + 1)?.to_str()?.parse().ok());
            finish_uninstall(&dir, parent, quiet)
        }
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(Cancelled) => 1,
        Err(Failed(message)) => {
            if !quiet {
                message_box(&message, MB_ICONERROR | MB_OK);
            }
            eprintln!("{message}");
            2
        }
    })
}

enum Stop {
    Cancelled,
    Failed(String),
}
use Stop::{Cancelled, Failed};

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Failed(message)
    }
}

fn this_exe() -> Result<PathBuf, Stop> {
    Ok(std::env::current_exe().map_err(|e| format!("Afterglow couldn't find its own program file: {e}"))?)
}

/// After an update (see `crate::update`): the installed copy registers again,
/// so Settings › Apps shows the new version. Quick when there's nothing to do.
pub fn refresh_entry() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(installed) = installed::find() else { return };
    if installed.version != env!("CARGO_PKG_VERSION") && exe.parent().is_some_and(|dir| installed::same_path(dir, &installed.dir)) {
        let _ = register(installed.desktop, installed.start_menu);
    }
}

/// Adds the shortcuts, "Open with" and the Settings › Apps entry for this exe.
fn register(desktop: bool, start_menu: bool) -> Result<(), Stop> {
    let exe = this_exe()?;
    let dir = exe.parent().ok_or_else(|| "Afterglow's folder wasn't found".to_string())?;
    let failed = |what: &str, e: String| format!("Afterglow couldn't add {what}: {e}");

    for (place, wanted, what) in [(Place::StartMenu, start_menu, "its Start menu shortcut"), (Place::Desktop, desktop, "its desktop shortcut")] {
        if wanted {
            shortcut::create(place).map_err(|e| failed(what, e))?;
        } else if shortcut::target(place).is_some_and(|t| installed::same_path(&t, &exe)) {
            shortcut::remove(place).map_err(|e| failed(what, e))?;
        }
    }

    let exe_text = exe.display().to_string();
    let open = format!("\"{exe_text}\" \"%1\"");
    (|| -> Result<(), String> {
        let prog_id = format!(r"{CLASSES}\{PROG_ID}");
        Key::create(&prog_id)?.set_string("", "Video")?;
        Key::create(&format!(r"{prog_id}\DefaultIcon"))?.set_string("", format!("\"{exe_text}\",0"))?;
        Key::create(&format!(r"{prog_id}\shell\open\command"))?.set_string("", &open)?;

        let app = Key::create(APPLICATION_KEY)?;
        app.set_string("FriendlyAppName", crate::APP_NAME)?;
        Key::create(&format!(r"{APPLICATION_KEY}\shell\open\command"))?.set_string("", &open)?;
        let supported = Key::create(&format!(r"{APPLICATION_KEY}\SupportedTypes"))?;
        for ext in VIDEO_EXTENSIONS {
            supported.set_string(&format!(".{ext}"), "")?;
            Key::create(&format!(r"{CLASSES}\.{ext}\OpenWithProgids"))?.set_empty(PROG_ID)?;
        }
        Ok(())
    })()
    .map_err(|e| failed("itself to \"Open with\"", e))?;

    (|| -> Result<(), String> {
        let entry = Key::create(installed::UNINSTALL_KEY)?;
        entry.set_string("DisplayName", crate::APP_NAME)?;
        entry.set_string("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
        entry.set_string("DisplayIcon", format!("\"{exe_text}\",0"))?;
        entry.set_string("InstallLocation", dir)?;
        entry.set_string("UninstallString", format!("\"{exe_text}\" --uninstall"))?;
        entry.set_string("QuietUninstallString", format!("\"{exe_text}\" --uninstall --quiet"))?;
        entry.set_string("URLInfoAbout", env!("CARGO_PKG_REPOSITORY"))?;
        entry.set_dword("NoModify", 1)?;
        entry.set_dword("NoRepair", 1)?;
        // For the setup, when it updates this.
        entry.set_dword("DesktopShortcut", desktop as u32)?;
        entry.set_dword("StartMenuShortcut", start_menu as u32)?;
        let size: u64 = installed::FILES.iter().filter_map(|f| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len()).sum();
        entry.set_dword("EstimatedSize", (size / 1024) as u32)?;
        Ok(())
    })()
    .map_err(|e| failed("its entry in Settings › Apps", e))?;

    associations_changed();
    Ok(())
}

/// The first half of uninstalling, from the installed exe: asks, then starts
/// a copy in the temp folder to do the rest once this one has quit.
fn uninstall(quiet: bool) -> Result<(), Stop> {
    let exe = this_exe()?;
    let dir = exe.parent().ok_or_else(|| "Afterglow's folder wasn't found".to_string())?.to_path_buf();
    // Only the copy Settings › Apps knows of uninstalls, so that one in, say,
    // Downloads can't remove the files beside it.
    if !installed::find().is_some_and(|i| installed::same_path(&i.dir, &dir)) {
        return Err(Failed(format!("This copy of Afterglow ({}) isn't the installed one, so there is nothing to uninstall.", exe.display())));
    }
    if !quiet {
        let question = format!(
            "Uninstall Afterglow?\n\nYour progress and settings stay in {}, in case you install it again. Delete that folder to remove them too.",
            data_dir_text()
        );
        if message_box(&question, MB_ICONQUESTION | MB_YESNO) != IDYES {
            return Err(Cancelled);
        }
    }
    let copy = std::env::temp_dir().join(format!("afterglow-uninstall-{}.exe", std::process::id()));
    std::fs::copy(&exe, &copy).map_err(|e| format!("Afterglow couldn't be uninstalled: copying it to {} failed: {e}", copy.display()))?;
    let mut command = std::process::Command::new(&copy);
    command.arg("--uninstall-from").arg(&dir).arg("--parent").arg(std::process::id().to_string());
    if quiet {
        command.arg("--quiet");
    }
    // Run from the temp folder, so the program's folder isn't in use.
    command.current_dir(std::env::temp_dir());
    command.spawn().map_err(|e| format!("Afterglow couldn't be uninstalled: {e}"))?;
    Ok(())
}

/// The second half, from the copy in the temp folder.
fn finish_uninstall(dir: &Path, parent: Option<u32>, quiet: bool) -> Result<(), Stop> {
    if let Some(parent) = parent {
        installed::wait_for_exit(parent, Duration::from_secs(10));
    }
    let exe = dir.join(installed::EXE);
    while !installed::close_running(&exe, Duration::from_secs(10)) {
        if quiet || message_box("Close Afterglow to uninstall it.", MB_ICONWARNING | MB_RETRYCANCEL) != IDRETRY {
            remove_self_later();
            return Err(Cancelled);
        }
    }

    let mut problems = Vec::new();
    for place in Place::ALL {
        if shortcut::target(place).is_some_and(|t| installed::same_path(&t, &exe))
            && let Err(e) = shortcut::remove(place)
        {
            problems.push(format!("the {} shortcut: {e}", place.name().to_lowercase()));
        }
    }
    let mut registry = vec![
        installed::delete_key(&format!(r"{CLASSES}\{PROG_ID}")),
        installed::delete_key(APPLICATION_KEY),
        installed::delete_key(installed::UNINSTALL_KEY),
    ];
    registry.extend(VIDEO_EXTENSIONS.iter().map(|ext| installed::delete_value(&format!(r"{CLASSES}\.{ext}\OpenWithProgids"), PROG_ID)));
    problems.extend(registry.into_iter().filter_map(Result::err));
    associations_changed();

    // An update leaves the exe it replaced until the next start.
    for file in installed::FILES.into_iter().chain(["afterglow.old.exe"]) {
        if let Err(e) = remove_file_patiently(&dir.join(file)) {
            problems.push(format!("{file}: {e}"));
        }
    }
    // Only when nothing else was put there.
    let _ = std::fs::remove_dir(dir);

    remove_self_later();
    if !problems.is_empty() {
        return Err(Failed(format!("Afterglow is uninstalled, but these couldn't be removed:\n\n{}", problems.join("\n"))));
    }
    if !quiet {
        message_box(&format!("Afterglow is uninstalled.\n\nYour progress and settings are still in {}.", data_dir_text()), MB_ICONINFORMATION | MB_OK);
    }
    Ok(())
}

/// A file that was just in use (by an Afterglow that is closing) can take a
/// moment to be free.
fn remove_file_patiently(path: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            result => return result,
        }
    }
}

/// Deletes the temp copy doing the uninstalling once it has quit.
fn remove_self_later() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(exe) = std::env::current_exe() else { return };
    // `ping` waits a couple of seconds, by which time this has quit.
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"", exe.display()))
        .current_dir(std::env::temp_dir())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Where the progress is, to say it's kept.
fn data_dir_text() -> String {
    match std::env::var_os("LOCALAPPDATA") {
        Some(local) => PathBuf::from(local).join("afterglow").display().to_string(),
        None => r"%LOCALAPPDATA%\afterglow".to_string(),
    }
}

// Win32

const MB_OK: u32 = 0x0;
const MB_YESNO: u32 = 0x4;
const MB_RETRYCANCEL: u32 = 0x5;
const MB_ICONERROR: u32 = 0x10;
const MB_ICONQUESTION: u32 = 0x20;
const MB_ICONWARNING: u32 = 0x30;
const MB_ICONINFORMATION: u32 = 0x40;
const MB_SETFOREGROUND: u32 = 0x1_0000;
const IDRETRY: i32 = 4;
const IDYES: i32 = 6;
const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;

#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(window: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHChangeNotify(event: i32, flags: u32, item1: *const std::ffi::c_void, item2: *const std::ffi::c_void);
}

fn message_box(text: &str, kind: u32) -> i32 {
    let (text, caption) = (installed::wide(text), installed::wide(crate::APP_NAME));
    unsafe { MessageBoxW(0, text.as_ptr(), caption.as_ptr(), kind | MB_SETFOREGROUND) }
}

/// Tells Explorer that "Open with" changed.
fn associations_changed() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, 0, std::ptr::null(), std::ptr::null()) };
}
