//! Rescans the playlist's folder in the background, so videos added to it
//! show up without reopening it.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use eframe::egui;

use crate::library;

const INTERVAL: Duration = Duration::from_secs(10);

/// What to watch: a folder, and whether its subfolders count.
pub type Target = Option<(PathBuf, bool)>;

pub struct FolderWatch {
    targets: Sender<Target>,
    /// The videos found in the folder, sent whenever they change.
    pub scans: Receiver<(PathBuf, Vec<PathBuf>)>,
}

impl FolderWatch {
    pub fn new(ctx: egui::Context, target: Target) -> Self {
        let (targets, target_rx) = mpsc::channel();
        let (scan_tx, scans) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("folder-watch".into())
            .spawn(move || watch(target, &target_rx, &scan_tx, &ctx));
        if let Err(e) = spawned {
            eprintln!("could not start watching the folder: {e}");
        }
        Self { targets, scans }
    }

    pub fn set_target(&self, target: Target) {
        let _ = self.targets.send(target);
    }
}

fn watch(mut target: Target, targets: &Receiver<Target>, scans: &Sender<(PathBuf, Vec<PathBuf>)>, ctx: &egui::Context) {
    let mut last: Option<Vec<PathBuf>> = None;
    loop {
        match targets.recv_timeout(INTERVAL) {
            Ok(new) => {
                target = new;
                last = None;
                // The app already scanned it; just start watching from here.
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Some((folder, include_subfolders)) = &target else { continue };
        // An unreachable folder (e.g. an unplugged drive) is skipped, not empty.
        let Ok(found) = library::scan_folder(folder, *include_subfolders) else { continue };
        if last.as_ref() != Some(&found) {
            last = Some(found.clone());
            if scans.send((folder.clone(), found)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}
