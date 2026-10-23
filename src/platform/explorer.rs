//! Afterglow in Explorer: "Open in Afterglow" on folders, in "Open with" for
//! videos, and as their default app. All of it is registered for the current
//! user only, so it needs no admin rights, and turning a part off removes its
//! keys again.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_NONE, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW,
    RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::HSTRING;

use crate::library::VIDEO_EXTENSIONS;

/// The file type Afterglow registers videos as.
const PROG_ID: &str = "afterglow.video";
/// The name of the folder menu entry's key, and of Afterglow in the list of
/// apps Windows can make the default.
const NAME: &str = "afterglow";
/// Where it all goes: `Software` of the current user, or elsewhere for tests.
const BASE: &str = "Software";

/// Explorer's "Open in Afterglow" on folders and inside them: the executable
/// it opens, if it's there.
pub fn folder_menu() -> Option<PathBuf> {
    command_exe(&format!(r"{BASE}\Classes\Directory\Background\shell\{NAME}\command"))
}

pub fn set_folder_menu(on: bool) -> Result<(), String> {
    set_folder_menu_in(BASE, on, &exe()?)
}

/// Afterglow in the "Open with" menu of videos: the executable it opens, if
/// it's there.
pub fn open_with() -> Option<PathBuf> {
    command_exe(&format!(r"{BASE}\Classes\{PROG_ID}\shell\open\command"))
}

/// Turning it off also stops Afterglow being the default app.
pub fn set_open_with(on: bool) -> Result<(), String> {
    let exe = exe()?;
    if !on {
        set_default_in(BASE, false)?;
    }
    set_open_with_in(BASE, on, &exe)?;
    changed();
    Ok(())
}

/// Whether Afterglow is registered as a default app for videos.
pub fn default() -> bool {
    get(HKEY_CURRENT_USER, &format!(r"{BASE}\RegisteredApplications"), NAME).is_some()
}

/// Making Afterglow the default also puts it in "Open with".
pub fn set_default(on: bool) -> Result<(), String> {
    let exe = exe()?;
    if on {
        set_open_with_in(BASE, true, &exe)?;
    }
    set_default_in(BASE, on)?;
    changed();
    Ok(())
}

/// The video types Windows opens with another app. Windows guards the choice
/// the user made with "Always", so only its own settings can change that.
pub fn opened_elsewhere() -> Vec<&'static str> {
    VIDEO_EXTENSIONS.iter().copied().filter(|ext| !opens_with_afterglow(BASE, ext)).collect()
}

/// Windows' default apps settings, at Afterglow's page.
pub fn open_default_apps() {
    let page = format!("ms-settings:defaultapps?registeredAppUser={NAME}");
    let _ = std::process::Command::new("explorer").arg(page).spawn();
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// Lets Explorer know file types changed, so menus and icons update.
fn changed() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn set_folder_menu_in(base: &str, on: bool, exe: &Path) -> Result<(), String> {
    for (key, arg) in [("Directory", "%1"), (r"Directory\Background", "%V")] {
        let key = format!(r"{base}\Classes\{key}\shell\{NAME}");
        if on {
            set(&key, "", "Open in Afterglow")?;
            set(&key, "Icon", &format!("{},0", exe.display()))?;
            // One folder at a time: each would replace the playlist.
            set(&key, "MultiSelectModel", "Single")?;
            set(&format!(r"{key}\command"), "", &command(exe, arg))?;
        } else {
            delete_tree(&key)?;
        }
    }
    Ok(())
}

fn set_open_with_in(base: &str, on: bool, exe: &Path) -> Result<(), String> {
    let classes = format!(r"{base}\Classes");
    let prog = format!(r"{classes}\{PROG_ID}");
    // Named after the executable, which may have been renamed since.
    if let Some(old) = command_exe(&format!(r"{prog}\shell\open\command")) {
        delete_tree(&format!(r"{classes}\Applications\{}", exe_name(&old)))?;
    }
    let app = format!(r"{classes}\Applications\{}", exe_name(exe));
    if !on {
        for ext in VIDEO_EXTENSIONS {
            delete_value(&format!(r"{classes}\.{ext}\OpenWithProgids"), PROG_ID)?;
        }
        delete_tree(&app)?;
        return delete_tree(&prog);
    }
    set(&prog, "", "Video")?;
    set(&format!(r"{prog}\DefaultIcon"), "", &format!("{},0", exe.display()))?;
    set(&format!(r"{prog}\shell\open\command"), "", &command(exe, "%1"))?;
    // How "Open with" names Afterglow, and which files it offers it for.
    set(&app, "FriendlyAppName", crate::APP_NAME)?;
    set(&format!(r"{app}\shell\open\command"), "", &command(exe, "%1"))?;
    for ext in VIDEO_EXTENSIONS {
        set(&format!(r"{app}\SupportedTypes"), &format!(".{ext}"), "")?;
        set_empty(&format!(r"{classes}\.{ext}\OpenWithProgids"), PROG_ID)?;
    }
    Ok(())
}

/// Registers Afterglow with Windows as an app that can be the default for
/// videos, and makes it the default for the types nobody chose an app for.
fn set_default_in(base: &str, on: bool) -> Result<(), String> {
    let capabilities = format!(r"{base}\{NAME}\Capabilities");
    let registered = format!(r"{base}\RegisteredApplications");
    for ext in VIDEO_EXTENSIONS {
        let class = format!(r"{base}\Classes\.{ext}");
        if on {
            if get(HKEY_CURRENT_USER, &class, "").is_none() {
                set(&class, "", PROG_ID)?;
            }
        } else if get(HKEY_CURRENT_USER, &class, "").as_deref() == Some(PROG_ID) {
            delete_value(&class, "")?;
        }
    }
    if !on {
        delete_value(&registered, NAME)?;
        return delete_tree(&format!(r"{base}\{NAME}"));
    }
    set(&capabilities, "ApplicationName", crate::APP_NAME)?;
    set(&capabilities, "ApplicationDescription", "Plays videos and keeps track of what you've watched")?;
    for ext in VIDEO_EXTENSIONS {
        set(&format!(r"{capabilities}\FileAssociations"), &format!(".{ext}"), PROG_ID)?;
    }
    set(&registered, NAME, &capabilities)
}

/// Whether Windows opens files with this extension in Afterglow: the user's
/// choice if they made one, otherwise the extension's file type.
fn opens_with_afterglow(base: &str, ext: &str) -> bool {
    let choices = format!(r"{base}\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.{ext}");
    // Newer Windows keeps the choice in UserChoiceLatest.
    let chosen = get(HKEY_CURRENT_USER, &format!(r"{choices}\UserChoiceLatest\ProgId"), "ProgId")
        .or_else(|| get(HKEY_CURRENT_USER, &format!(r"{choices}\UserChoice"), "ProgId"))
        .or_else(|| get(HKEY_CURRENT_USER, &format!(r"{base}\Classes\.{ext}"), ""))
        .or_else(|| get(HKEY_LOCAL_MACHINE, &format!(r"Software\Classes\.{ext}"), ""));
    chosen.is_some_and(|id| {
        id.eq_ignore_ascii_case(PROG_ID) || id.to_ascii_lowercase().starts_with(r"applications\afterglow")
    })
}

fn exe_name(exe: &Path) -> String {
    exe.file_name().map_or_else(|| "afterglow.exe".into(), |name| name.to_string_lossy().into_owned())
}

fn command(exe: &Path, arg: &str) -> String {
    format!("\"{}\" \"{arg}\"", exe.display())
}

/// The executable a command starts, as written by [`command`].
fn command_exe(key: &str) -> Option<PathBuf> {
    let command = get(HKEY_CURRENT_USER, key, "")?;
    let exe = command.strip_prefix('"')?.split('"').next()?;
    Some(PathBuf::from(exe))
}

fn check(error: WIN32_ERROR, what: &str) -> Result<(), String> {
    match error {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        error => Err(format!("{what}: {}", windows::core::Error::from(error.to_hresult()).message())),
    }
}

/// Sets a text value in a key of the current user, creating the key.
fn set(key: &str, name: &str, value: &str) -> Result<(), String> {
    let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let error = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    check(error, key)
}

/// Sets a value that only matters by being there.
fn set_empty(key: &str, name: &str) -> Result<(), String> {
    let error = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), REG_NONE.0, None, 0) };
    check(error, key)
}

fn get(root: HKEY, key: &str, name: &str) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let error = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    let len = (size as usize / 2).saturating_sub(1);
    (error == ERROR_SUCCESS && len > 0).then(|| String::from_utf16_lossy(&buffer[..len]))
}

fn delete_value(key: &str, name: &str) -> Result<(), String> {
    check(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) }, key)
}

fn delete_tree(key: &str) -> Result<(), String> {
    check(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) }, key)?;
    // RegDeleteTreeW only empties the key; this removes the key itself.
    let error = unsafe { windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    check(error, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers everything in a key of its own, never where Explorer looks.
    #[test]
    fn registers_and_removes_everything() {
        let base = format!(r"Software\afterglow-test-{}", std::process::id());
        let exe = Path::new(r"C:\Some Folder\afterglow.exe");
        set_folder_menu_in(&base, true, exe).unwrap();
        set_open_with_in(&base, true, exe).unwrap();
        set_default_in(&base, true).unwrap();
        let menu = format!(r"{base}\Classes\Directory\Background\shell\{NAME}\command");
        assert_eq!(command_exe(&menu).as_deref(), Some(exe));
        assert_eq!(get(HKEY_CURRENT_USER, &menu, "").unwrap(), r#""C:\Some Folder\afterglow.exe" "%V""#);
        assert_eq!(command_exe(&format!(r"{base}\Classes\{PROG_ID}\shell\open\command")).as_deref(), Some(exe));
        assert!(get(HKEY_CURRENT_USER, &format!(r"{base}\RegisteredApplications"), NAME).is_some());
        assert!(opens_with_afterglow(&base, "mkv"), "nobody chose an app, so Afterglow it is");

        set_default_in(&base, false).unwrap();
        set_open_with_in(&base, false, exe).unwrap();
        set_folder_menu_in(&base, false, exe).unwrap();
        assert_eq!(command_exe(&menu), None);
        assert_eq!(get(HKEY_CURRENT_USER, &format!(r"{base}\Classes\.mkv"), ""), None);
        assert!(!opens_with_afterglow(&base, "mkv"));
        // Removing what isn't there is fine.
        set_folder_menu_in(&base, false, exe).unwrap();
        delete_tree(&base).unwrap();
    }
}
