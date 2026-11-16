//! What an installed Afterglow leaves on the PC: where it is, its registry
//! entries, and closing it before its files are replaced or removed.
//!
//! The setup (`setup/`) includes this file as well, so it uses nothing from
//! the rest of the app.

// Some of this is only used by the setup, some only by the app.
#![allow(dead_code)]

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The program's own files in the install folder: uninstalling removes these,
/// and the folder only when that leaves it empty.
pub const FILES: [&str; 3] = ["afterglow.exe", "LICENSE.txt", "THIRD-PARTY-NOTICES.txt"];
pub const EXE: &str = "afterglow.exe";

/// Its entry in Settings › Apps, under HKEY_CURRENT_USER.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\afterglow";

/// Where a fresh install goes: `%LOCALAPPDATA%\Programs\afterglow`, the
/// folder Windows uses for programs installed for one user.
pub fn default_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(local).join("Programs").join("afterglow"))
}

/// An installed Afterglow, as its Settings › Apps entry has it.
pub struct Installed {
    pub dir: PathBuf,
    pub version: String,
    /// Whether it was given a desktop shortcut.
    pub desktop: bool,
    /// Whether it was given a Start menu shortcut (always, before this was
    /// a choice).
    pub start_menu: bool,
}

pub fn find() -> Option<Installed> {
    let key = Key::open(UNINSTALL_KEY)?;
    let dir = PathBuf::from(key.string("InstallLocation")?);
    let version = key.string("DisplayVersion").unwrap_or_default();
    let desktop = key.dword("DesktopShortcut") == Some(1);
    let start_menu = key.dword("StartMenuShortcut") != Some(0);
    Some(Installed { dir, version, desktop, start_menu })
}

/// Whether two paths name the same file, as far as their text tells.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let clean = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    clean(a) == clean(b)
}

// Registry

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const KEY_READ: u32 = 0x20019;
const KEY_WRITE: u32 = 0x20006;
const REG_SZ: u32 = 1;
const REG_NONE: u32 = 0;
const REG_DWORD: u32 = 4;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: isize,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        desired: u32,
        security: *const c_void,
        result: *mut isize,
        disposition: *mut u32,
    ) -> i32;
    fn RegOpenKeyExW(key: isize, sub_key: *const u16, options: u32, desired: u32, result: *mut isize) -> i32;
    fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, kind: u32, data: *const u8, size: u32) -> i32;
    fn RegQueryValueExW(key: isize, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, size: *mut u32) -> i32;
    fn RegDeleteTreeW(key: isize, sub_key: *const u16) -> i32;
    fn RegDeleteKeyValueW(key: isize, sub_key: *const u16, name: *const u16) -> i32;
    fn RegCloseKey(key: isize) -> i32;
}

const ERROR_FILE_NOT_FOUND: i32 = 2;

pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn error(code: i32) -> String {
    std::io::Error::from_raw_os_error(code).to_string()
}

/// A registry key under HKEY_CURRENT_USER, closed when dropped.
pub struct Key(isize);

impl Key {
    /// Opens the key, making it (and those above it) when it isn't there.
    pub fn create(path: &str) -> Result<Key, String> {
        let mut key = 0;
        let code = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, std::ptr::null(), 0, KEY_WRITE | KEY_READ, std::ptr::null(), &mut key, std::ptr::null_mut())
        };
        if code == 0 { Ok(Key(key)) } else { Err(format!("{path}: {}", error(code))) }
    }

    pub fn open(path: &str) -> Option<Key> {
        let mut key = 0;
        (unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, KEY_READ, &mut key) } == 0).then_some(Key(key))
    }

    fn set(&self, name: &str, kind: u32, data: &[u8]) -> Result<(), String> {
        let code = unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
        if code == 0 { Ok(()) } else { Err(format!("{name}: {}", error(code))) }
    }

    /// Sets a text value; an empty `name` is the key's default value.
    pub fn set_string(&self, name: &str, value: impl AsRef<std::ffi::OsStr>) -> Result<(), String> {
        let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        self.set(name, REG_SZ, &data)
    }

    pub fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }

    /// A value with no data, as `OpenWithProgids` lists take.
    pub fn set_empty(&self, name: &str) -> Result<(), String> {
        self.set(name, REG_NONE, &[])
    }

    pub fn dword(&self, name: &str) -> Option<u32> {
        let (mut kind, mut value, mut size) = (0, 0u32, 4);
        let code = unsafe { RegQueryValueExW(self.0, wide(name).as_ptr(), std::ptr::null_mut(), &mut kind, (&mut value as *mut u32).cast(), &mut size) };
        (code == 0 && kind == REG_DWORD).then_some(value)
    }

    pub fn string(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let (mut kind, mut size) = (0, 0);
        if unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut size) } != 0 || kind != REG_SZ {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        if unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, buffer.as_mut_ptr().cast(), &mut size) } != 0 {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(OsString::from_wide(&buffer[..len]).to_string_lossy().into_owned())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// Removes a key under HKEY_CURRENT_USER with everything in it; one that
/// isn't there is fine.
pub fn delete_key(path: &str) -> Result<(), String> {
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr()) } {
        0 | ERROR_FILE_NOT_FOUND => Ok(()),
        code => Err(format!("{path}: {}", error(code))),
    }
}

pub fn delete_value(path: &str, name: &str) -> Result<(), String> {
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(path).as_ptr(), wide(name).as_ptr()) } {
        0 | ERROR_FILE_NOT_FOUND => Ok(()),
        code => Err(format!("{path}\\{name}: {}", error(code))),
    }
}

// Running copies

const WM_CLOSE: u32 = 0x0010;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(isize, isize) -> i32, param: isize) -> i32;
    fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
    fn IsWindowVisible(window: isize) -> i32;
    fn GetWindowTextLengthW(window: isize) -> i32;
    fn PostMessageW(window: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
    fn QueryFullProcessImageNameW(process: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetCurrentProcessId() -> u32;
}

/// The executable a process runs.
fn image(process_id: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size) != 0;
        CloseHandle(process);
        ok.then(|| PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
    }
}

/// The windows of processes running `exe` (other than this one), with their
/// process ids: the visible ones with a title, as winit also has a visible
/// but untitled window of its own, which must be left alone (closing it stops
/// the app from handling anything, closing included).
fn windows_of(exe: &Path) -> Vec<(isize, u32)> {
    unsafe extern "system" fn each(window: isize, found: isize) -> i32 {
        let found = unsafe { &mut *(found as *mut Vec<(isize, u32)>) };
        if unsafe { IsWindowVisible(window) != 0 && GetWindowTextLengthW(window) > 0 } {
            let mut process_id = 0;
            unsafe { GetWindowThreadProcessId(window, &mut process_id) };
            found.push((window, process_id));
        }
        1
    }
    let mut all: Vec<(isize, u32)> = Vec::new();
    unsafe { EnumWindows(each, &mut all as *mut _ as isize) };
    let this = unsafe { GetCurrentProcessId() };
    let mut images = std::collections::HashMap::new();
    all.into_iter()
        .filter(|&(_, id)| id != this && *images.entry(id).or_insert_with(|| image(id).is_some_and(|p| same_path(&p, exe))))
        .collect()
}

/// Asks every copy of `exe` that is running to close, as clicking its close
/// button would (so it saves first), and waits up to `wait` for them to go.
/// Whether none is left running.
pub fn close_running(exe: &Path, wait: Duration) -> bool {
    let found = windows_of(exe);
    let mut ids: Vec<u32> = found.iter().map(|&(_, id)| id).collect();
    ids.dedup();
    for &(window, _) in &found {
        unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
    }
    let deadline = Instant::now() + wait;
    for id in ids {
        unsafe {
            let process = OpenProcess(SYNCHRONIZE, 0, id);
            if process.is_null() {
                continue;
            }
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
            let gone = WaitForSingleObject(process, left) == WAIT_OBJECT_0;
            CloseHandle(process);
            if !gone {
                return false;
            }
        }
    }
    windows_of(exe).is_empty()
}

/// Waits up to `wait` for a process to end.
pub fn wait_for_exit(process_id: u32, wait: Duration) {
    unsafe {
        let process = OpenProcess(SYNCHRONIZE, 0, process_id);
        if !process.is_null() {
            WaitForSingleObject(process, wait.as_millis() as u32);
            CloseHandle(process);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_registry_values() {
        let path = format!(r"Software\afterglow-test-{}", std::process::id());
        {
            let key = Key::create(&format!(r"{path}\inner")).unwrap();
            key.set_string("Name", "Afterglow ✓").unwrap();
            key.set_string("", "default").unwrap();
            key.set_dword("Number", 7).unwrap();
            key.set_empty("Empty").unwrap();
        }
        let key = Key::open(&format!(r"{path}\inner")).unwrap();
        assert_eq!(key.string("Name").as_deref(), Some("Afterglow ✓"));
        assert_eq!(key.string("").as_deref(), Some("default"));
        assert_eq!(key.string("Number"), None);
        assert_eq!(key.dword("Number"), Some(7));
        assert_eq!(key.dword("Name"), None);
        drop(key);
        delete_value(&format!(r"{path}\inner"), "Name").unwrap();
        assert_eq!(Key::open(&format!(r"{path}\inner")).unwrap().string("Name"), None);
        delete_key(&path).unwrap();
        assert!(Key::open(&path).is_none());
        // Removing what isn't there is fine.
        delete_key(&path).unwrap();
        delete_value(&path, "Name").unwrap();
    }

    #[test]
    fn compares_paths_loosely() {
        assert!(same_path(Path::new(r"C:\Users\A\Programs\afterglow\"), Path::new(r"c:\users\a\programs\AFTERGLOW")));
        assert!(!same_path(Path::new(r"C:\a\afterglow.exe"), Path::new(r"C:\b\afterglow.exe")));
    }
}
