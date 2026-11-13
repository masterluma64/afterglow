//! Updating afterglow from the owner's update server (the Worker in
//! `worker/`, see docs/updates.md): it asks for the newest release, downloads
//! it in the background, checks it was signed with the owner's key, and puts
//! the new exe in place of this one when told to.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;
use serde::Deserialize;

/// The public half of the key releases are signed with (see
/// examples/sign_release.rs). An update signed with another key isn't
/// installed.
const PUBLIC_KEY: &str = "a34ddcca28d3d5f373a15ce82e5b9ffa4f73004ecdcf9b898feae8f18b892f8c";
/// This copy's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Passed to the new exe, with this process's id, so it waits for this one
/// to have quit before it starts.
pub const AFTER_UPDATE_ARG: &str = "--after-update";

/// A release, as the update server describes it.
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub version: String,
    /// The release notes: its body on GitHub.
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    size: u64,
}

/// Where updating stands.
#[derive(Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer release is downloading; `done` is how much of it, 0 to 1.
    Downloading { release: Release, done: f32 },
    /// It is downloaded, checked and unpacked: `exe` replaces this one on a
    /// restart.
    Ready { release: Release, exe: PathBuf },
    Failed(String),
}

enum Message {
    UpToDate,
    Found(Release),
    Progress(f32),
    Ready(PathBuf),
    Failed(String),
}

pub struct Updater {
    pub state: State,
    messages: Option<Receiver<Message>>,
    ctx: egui::Context,
}

impl Updater {
    pub fn new(ctx: egui::Context) -> Self {
        Self { state: State::Idle, messages: None, ctx }
    }

    /// Asks `server` for the newest release, with `key`, and downloads it if
    /// it is newer than this copy, all in the background. Nothing happens
    /// while a check is under way or an update is ready.
    pub fn check(&mut self, server: &str, key: &str, data_dir: Option<PathBuf>) {
        if matches!(self.state, State::Checking | State::Downloading { .. } | State::Ready { .. }) {
            return;
        }
        let Some(dir) = data_dir.map(|d| d.join("update")) else {
            self.state = State::Failed("There is no data folder to download updates into".into());
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.messages = Some(rx);
        self.state = State::Checking;
        let (server, key, ctx) = (server.trim_end_matches('/').to_string(), key.trim().to_string(), self.ctx.clone());
        let spawned = std::thread::Builder::new().name("update".into()).spawn(move || {
            let send = |message| {
                let _ = tx.send(message);
                ctx.request_repaint();
            };
            match fetch(&server, &key, &dir, &send) {
                Ok(Some(exe)) => send(Message::Ready(exe)),
                Ok(None) => send(Message::UpToDate),
                Err(e) => send(Message::Failed(e)),
            }
        });
        if let Err(e) = spawned {
            self.state = State::Failed(e.to_string());
        }
    }

    /// Takes in what the background work found; call once per frame.
    /// Returns a release once it is ready to install.
    pub fn poll(&mut self) -> Option<Release> {
        let messages: Vec<Message> = self.messages.as_ref()?.try_iter().collect();
        let mut ready = None;
        for message in messages {
            self.state = match (message, std::mem::take(&mut self.state)) {
                (Message::UpToDate, _) => State::UpToDate,
                (Message::Found(release), _) => State::Downloading { release, done: 0.0 },
                (Message::Progress(done), State::Downloading { release, .. }) => State::Downloading { release, done },
                (Message::Ready(exe), State::Downloading { release, .. }) => {
                    ready = Some(release.clone());
                    State::Ready { release, exe }
                }
                (Message::Failed(e), _) => State::Failed(e),
                (_, state) => state,
            };
        }
        ready
    }
}

/// Whether version `a` ("1.2.3") is newer than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split('.').map(|n| n.parse().unwrap_or(0)).collect() };
    parse(a) > parse(b)
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(format!("afterglow/{VERSION}"))
        .build()
        .into()
}

/// Checks for a newer release and downloads, verifies and unpacks it into
/// `dir`, returning the new exe; `None` when this copy is the newest.
fn fetch(server: &str, key: &str, dir: &Path, send: &dyn Fn(Message)) -> Result<Option<PathBuf>, String> {
    if server.is_empty() {
        return Err("No update server is set".into());
    }
    let agent = agent();
    let auth = format!("Bearer {key}");
    let release: Release = agent
        .get(format!("{server}/latest"))
        .header("Authorization", &auth)
        .call()
        .map_err(|e| describe(&e))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("The update server's answer couldn't be read ({e})"))?;
    if !newer(&release.version, VERSION) {
        return Ok(None);
    }
    let name = format!("afterglow-{}-windows-x64.7z", release.version);
    let size = release.assets.iter().find(|a| a.name == name).map(|a| a.size).ok_or(format!("Release {} has no {name}", release.version))?;
    send(Message::Found(release.clone()));

    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let download = |asset: &str| agent.get(format!("{server}/download/v{}/{asset}", release.version)).header("Authorization", &auth).call().map_err(|e| describe(&e));

    // The signature first: it is small, and without it the rest is no use.
    let signature = download(&format!("{name}.sig"))?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let archive = dir.join(&name);
    let mut response = download(&name)?;
    let mut reader = response.body_mut().with_config().limit(size + 1).reader();
    let mut file = std::fs::File::create(&archive).map_err(|e| format!("Could not save the update: {e}"))?;
    let (mut buffer, mut done, mut reported) = (vec![0u8; 256 * 1024], 0u64, 0.0f32);
    loop {
        let n = reader.read(&mut buffer).map_err(|e| format!("The download broke off ({e})"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).map_err(|e| format!("Could not save the update: {e}"))?;
        done += n as u64;
        let fraction = done as f32 / size.max(1) as f32;
        if fraction - reported >= 0.01 {
            reported = fraction;
            send(Message::Progress(fraction.min(1.0)));
        }
    }
    drop(file);
    if done != size {
        return Err(format!("The download is incomplete ({done} of {size} bytes)"));
    }
    let bytes = std::fs::read(&archive).map_err(|e| e.to_string())?;
    verify(&bytes, &signature)?;
    drop(bytes);

    let unpacked = dir.join("unpacked");
    sevenz_rust2::decompress_file(&archive, &unpacked).map_err(|e| format!("Could not unpack the update ({e})"))?;
    let exe = unpacked.join(format!("afterglow-{}-windows-x64.exe", release.version));
    if !exe.is_file() {
        return Err(format!("The update has no {}", exe.file_name().unwrap_or_default().to_string_lossy()));
    }
    Ok(Some(exe))
}

/// What went wrong talking to the update server, in words.
fn describe(error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(401) => "The update server didn't accept the key".into(),
        ureq::Error::StatusCode(code) => format!("The update server answered {code}"),
        e => format!("The update server couldn't be reached ({e})"),
    }
}

/// Checks that `bytes` were signed with the owner's key; `signature` is in hex.
fn verify(bytes: &[u8], signature: &str) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let unsigned = || "The update isn't signed with Afterglow's key, so it wasn't installed".to_string();
    let key: [u8; 32] = unhex(PUBLIC_KEY).and_then(|k| k.try_into().ok()).ok_or_else(unsigned)?;
    let signature: [u8; 64] = unhex(signature).and_then(|s| s.try_into().ok()).ok_or_else(unsigned)?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| unsigned())?;
    key.verify(bytes, &Signature::from_bytes(&signature)).map_err(|_| unsigned())
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// Puts `exe` in place of the running one and starts it; the caller then
/// quits. Windows lets a running exe be renamed but not replaced, so this one
/// becomes `afterglow.old.exe`, removed by the next start (see `tidy_up`).
pub fn install(exe: &Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    swap(&current, exe)?;
    std::process::Command::new(&current)
        .arg(AFTER_UPDATE_ARG)
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|e| format!("Updated, but couldn't start the new version ({e}); start Afterglow again"))?;
    Ok(())
}

/// Moves `current` aside as `<name>.old.exe` and puts `new` in its place,
/// putting `current` back if that fails.
fn swap(current: &Path, new: &Path) -> Result<(), String> {
    let old = current.with_extension("old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(current, &old).map_err(|e| format!("Could not move {} aside: {e}", current.display()))?;
    // A rename is instant, but the data folder may be on another drive.
    let placed = std::fs::rename(new, current).or_else(|_| std::fs::copy(new, current).map(|_| ()));
    if let Err(e) = placed {
        let _ = std::fs::rename(&old, current);
        return Err(format!("Could not put the new version in place: {e}"));
    }
    Ok(())
}

/// At startup: waits for the copy that was just updated to have quit (when
/// started by `install`), then removes what the update left behind. Returns
/// the command line without the `install` arguments.
pub fn tidy_up(mut args: Vec<std::ffi::OsString>, data_dir: Option<&Path>) -> Vec<std::ffi::OsString> {
    if args.first().is_some_and(|a| a == AFTER_UPDATE_ARG) {
        let pid = args.get(1).and_then(|p| p.to_str()?.parse().ok());
        args.drain(..args.len().min(2));
        if let Some(pid) = pid {
            wait_for_exit(pid, Duration::from_secs(20));
        }
    }
    if let Ok(current) = std::env::current_exe() {
        let _ = std::fs::remove_file(current.with_extension("old.exe"));
    }
    if let Some(dir) = data_dir {
        let _ = std::fs::remove_dir_all(dir.join("update"));
    }
    args
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

fn wait_for_exit(pid: u32, timeout: Duration) {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if !handle.is_null() {
        unsafe {
            WaitForSingleObject(handle, timeout.as_millis() as u32);
            CloseHandle(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.6.0", "0.5.2"));
        assert!(newer("0.10.0", "0.9.9"), "by number, not as text");
        assert!(newer("v1.0.0", "0.99.0"));
        assert!(!newer("0.5.2", "0.5.2"));
        assert!(!newer("0.5.1", "0.5.2"));
    }

    #[test]
    fn only_trusts_the_owners_signature() {
        use ed25519_dalek::{Signer, SigningKey};
        let stranger = SigningKey::from_bytes(&[7; 32]);
        let signature: String = stranger.sign(b"update").to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert!(verify(b"update", &signature).is_err(), "signed with another key");
        assert!(verify(b"update", "not hex").is_err());
        assert!(unhex(PUBLIC_KEY).is_some_and(|k| k.len() == 32));
    }

    #[test]
    fn swaps_the_exe_and_puts_it_back_on_failure() {
        let dir = std::env::temp_dir().join(format!("afterglow-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (current, new) = (dir.join("afterglow.exe"), dir.join("new.exe"));
        std::fs::write(&current, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap(&current, &new).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join("afterglow.old.exe")).unwrap(), b"old", "kept aside until the next start");
        // Nothing to put in place: the running one stays.
        assert!(swap(&current, &dir.join("missing.exe")).is_err());
        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn takes_the_update_arguments_off() {
        let args = ["--after-update", "999999", "D:\\Shows"].map(std::ffi::OsString::from).to_vec();
        assert_eq!(tidy_up(args, None), [std::ffi::OsString::from("D:\\Shows")]);
    }
}
