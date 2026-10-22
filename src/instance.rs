//! One Afterglow per data folder. Two at once would each save their own
//! state over the other's, so a second start hands the files it was given
//! to the first one (which opens them and comes to the front) and exits.
//!
//! The first start owns a named pipe; later starts write to it. A message is
//! the paths as UTF-16, each followed by a 0 (which no path contains).

use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;

const PIPE_ACCESS_INBOUND: u32 = 0x0000_0001;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;
const ERROR_NO_DATA: i32 = 232;
const ERROR_PIPE_CONNECTED: i32 = 535;
const ASFW_ANY: u32 = u32::MAX;
const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer_size: u32,
        in_buffer_size: u32,
        default_timeout: u32,
        security_attributes: *mut c_void,
    ) -> *mut c_void;
    fn ConnectNamedPipe(pipe: *mut c_void, overlapped: *mut c_void) -> i32;
    fn DisconnectNamedPipe(pipe: *mut c_void) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn AllowSetForegroundWindow(process_id: u32) -> i32;
}

/// What starting found.
pub enum Claim {
    /// This is the first Afterglow: it listens for later starts.
    First(Listener),
    /// Another Afterglow is running; hand it the files with [`forward`].
    Running,
    /// It couldn't be told (e.g. the pipe couldn't be made): just run.
    Unknown,
}

/// The pipe of the first Afterglow, not yet listening.
pub struct Listener {
    pipe: *mut c_void,
}

// The handle is only used by whichever thread owns the listener.
unsafe impl Send for Listener {}

impl Drop for Listener {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.pipe) };
    }
}

/// The pipe for a data folder. Its name is derived from the folder, so
/// copies of Afterglow with their own data folders don't get in each
/// other's way.
pub fn pipe_name(data_dir: &Path) -> String {
    // FNV-1a, which (unlike std's hasher) is the same in every build, so
    // another version of Afterglow finds the pipe too.
    let folder = data_dir.to_string_lossy().to_lowercase();
    let hash = folder.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    format!(r"\\.\pipe\afterglow-{hash:016x}")
}

/// Becomes the first Afterglow for `pipe`, unless one is already running.
pub fn claim(pipe: &str) -> Claim {
    let name: Vec<u16> = pipe.encode_utf16().chain([0]).collect();
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            64 * 1024,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle != INVALID_HANDLE_VALUE {
        return Claim::First(Listener { pipe: handle });
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY) => Claim::Running,
        _ => Claim::Unknown,
    }
}

impl Listener {
    /// Listens on a background thread. Each later start sends the paths it
    /// was given (none if it was started without any).
    pub fn listen(self, ctx: egui::Context) -> Receiver<Vec<PathBuf>> {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("other-starts".into()).spawn(move || {
            // The whole listener moves to the thread, which then owns the pipe.
            let listener = self;
            loop {
                // A start that connected before this (or already wrote and
                // left) still has its message waiting in the pipe.
                let connected = unsafe { ConnectNamedPipe(listener.pipe, std::ptr::null_mut()) } != 0
                    || matches!(std::io::Error::last_os_error().raw_os_error(), Some(ERROR_PIPE_CONNECTED | ERROR_NO_DATA));
                if !connected {
                    eprintln!("stopped listening for other starts: {}", std::io::Error::last_os_error());
                    return;
                }
                // Borrowed as a file to read it; the listener closes the handle.
                let pipe = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(listener.pipe) });
                let mut bytes = Vec::new();
                let read = (&*pipe).read_to_end(&mut bytes);
                unsafe { DisconnectNamedPipe(listener.pipe) };
                if read.is_ok() {
                    if tx.send(decode(&bytes)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not listen for other starts: {e}");
        }
        rx
    }
}

/// Hands `paths` to the Afterglow that is running, and lets it come to the front.
pub fn forward(pipe: &str, paths: &[PathBuf]) -> std::io::Result<()> {
    // Only the program the user just started may bring a window to the front;
    // this passes that on.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    // The running one takes one start at a time; wait for it a moment.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut file = loop {
        match std::fs::OpenOptions::new().write(true).open(pipe) {
            Ok(file) => break file,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    };
    file.write_all(&encode(paths))
}

fn encode(paths: &[PathBuf]) -> Vec<u8> {
    paths
        .iter()
        .flat_map(|p| p.as_os_str().encode_wide().chain([0]))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn decode(bytes: &[u8]) -> Vec<PathBuf> {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&b| u16::from_le_bytes(b)).collect();
    units
        .split(|&u| u == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(std::ffi::OsString::from_wide(p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_files_to_the_first_start() {
        let pipe = pipe_name(&std::env::temp_dir().join(format!("afterglow-instance-{}", std::process::id())));
        let Claim::First(listener) = claim(&pipe) else { panic!("the first start claims the pipe") };
        assert!(matches!(claim(&pipe), Claim::Running), "a second start finds it taken");

        let starts = listener.listen(egui::Context::default());
        let recv = || starts.recv_timeout(Duration::from_secs(5)).expect("the paths arrive");
        let paths = [PathBuf::from(r"D:\Shows\Foo"), PathBuf::from(r"C:\Vidéos\Ünïcode 映画.mkv")];
        forward(&pipe, &paths).unwrap();
        assert_eq!(recv(), paths);
        forward(&pipe, &[]).unwrap();
        assert_eq!(recv(), Vec::<PathBuf>::new(), "a start without files just asks for the window");
        // Several starts in a row all get through.
        for i in 0..5 {
            forward(&pipe, &[PathBuf::from(format!(r"C:\{i}.mkv"))]).unwrap();
        }
        for i in 0..5 {
            assert_eq!(recv(), [PathBuf::from(format!(r"C:\{i}.mkv"))]);
        }
    }

    #[test]
    fn names_the_pipe_after_the_data_folder() {
        let name = |dir: &str| pipe_name(Path::new(dir));
        assert_eq!(name(r"C:\Afterglow\data"), name(r"c:\afterglow\DATA"), "case doesn't matter on Windows");
        assert_ne!(name(r"C:\Afterglow\data"), name(r"D:\Afterglow\data"));
        assert!(name(r"C:\Afterglow\data").starts_with(r"\\.\pipe\afterglow-"));
    }
}
