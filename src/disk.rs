//! Disk work for the playlist, done on a background thread. On a network
//! drive that is slow or gone, reading a folder (or even checking that a
//! file exists) can take many seconds, and the window must not freeze
//! meanwhile. Jobs run one at a time, in the order they were given, and
//! each comes back as a [`Done`] for the UI to take in.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::SystemTime;

use eframe::egui;

use crate::{library, progress};

pub(crate) enum Job {
    /// Folders and videos to open: which are folders, and their videos.
    Open {
        paths: Vec<PathBuf>,
        include_subfolders: bool,
        dropped: bool,
        /// A video to play once it's in the playlist.
        play: Option<PathBuf>,
        remembered: progress::Remembered,
    },
    /// The playlist's folder, read again.
    Rescan {
        folder: PathBuf,
        include_subfolders: bool,
        why: Rescan,
        /// The playlist when asked: what's new is what isn't in it, and on
        /// a refresh, its entries are checked to be still there.
        playlist: Vec<PathBuf>,
        remembered: progress::Remembered,
    },
    /// Videos the folder watch found new in the playlist's folder.
    Arrived { videos: Vec<PathBuf>, remembered: progress::Remembered },
    /// When each video was last changed, to sort by it.
    Modified(Vec<PathBuf>),
}

#[derive(Clone, Copy)]
pub(crate) enum Rescan {
    /// Refresh: add new videos and drop missing ones.
    Refresh,
    /// The subfolder setting changed.
    Subfolders,
}

pub(crate) enum Done {
    Opened {
        /// Each folder, with its videos or why it couldn't be read.
        folders: Vec<(PathBuf, std::io::Result<Vec<PathBuf>>)>,
        videos: Vec<PathBuf>,
        /// Paths that are neither folders nor videos.
        others: Vec<PathBuf>,
        dropped: bool,
        play: Option<PathBuf>,
        moves: progress::Moves,
    },
    Rescanned {
        folder: PathBuf,
        include_subfolders: bool,
        why: Rescan,
        found: std::io::Result<Vec<PathBuf>>,
        /// Playlist entries whose files are gone (on a refresh).
        missing: HashSet<PathBuf>,
        moves: progress::Moves,
    },
    Arrived { videos: Vec<PathBuf>, moves: progress::Moves },
    Modified(HashMap<PathBuf, Option<SystemTime>>),
}

pub(crate) struct Disk {
    jobs: Sender<Job>,
    pub(crate) done: Receiver<Done>,
}

impl Disk {
    pub(crate) fn new(ctx: egui::Context) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("disk".into()).spawn(move || {
            for job in job_rx {
                if done_tx.send(run(job)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not start the disk thread: {e}");
        }
        Self { jobs, done }
    }

    /// Queues `job`; `false` if the disk thread isn't running.
    pub(crate) fn start(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }
}

/// Carries out a job, synchronously (normally on the disk thread).
pub(crate) fn run(job: Job) -> Done {
    match job {
        Job::Open { paths, include_subfolders, dropped, play, remembered } => {
            let (folders, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.is_dir());
            let (videos, others): (Vec<_>, Vec<_>) = files.into_iter().partition(|p| library::is_video(p));
            let folders: Vec<_> = folders.into_iter().map(|f| {
                let found = library::scan_folder(&f, include_subfolders);
                (f, found)
            }).collect();
            let all: Vec<PathBuf> =
                folders.iter().filter_map(|(_, found)| found.as_ref().ok()).flatten().chain(&videos).cloned().collect();
            let moves = progress::find_moves(&remembered, &all);
            Done::Opened { folders, videos, others, dropped, play, moves }
        }
        Job::Rescan { folder, include_subfolders, why, playlist, remembered } => {
            let found = library::scan_folder(&folder, include_subfolders);
            let (missing, moves) = match (why, &found) {
                (Rescan::Refresh, Ok(found)) => {
                    let listed: HashSet<&PathBuf> = playlist.iter().collect();
                    let new: Vec<PathBuf> = found.iter().filter(|p| !listed.contains(p)).cloned().collect();
                    let missing = playlist.iter().filter(|p| !p.is_file()).cloned().collect();
                    (missing, progress::find_moves(&remembered, &new))
                }
                _ => (HashSet::new(), progress::Moves::default()),
            };
            Done::Rescanned { folder, include_subfolders, why, found, missing, moves }
        }
        Job::Arrived { videos, remembered } => {
            let moves = progress::find_moves(&remembered, &videos);
            Done::Arrived { videos, moves }
        }
        Job::Modified(paths) => Done::Modified(
            paths.into_iter().map(|p| {
                let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, modified)
            }).collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn opens_rescans_and_recognizes_moved_videos() {
        let dir = std::env::temp_dir().join(format!("afterglow-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, size: usize| {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; size]).unwrap();
            path
        };
        let (a, renamed, notes) = (file("a.mkv", 5), file("renamed.mkv", 10), file("notes.txt", 1));
        // Watched as "old name.mkv", which is gone now.
        let mut store = progress::Store::default();
        let gone = dir.join("old name.mkv");
        store.update_position(&gone, 60.0, 600.0);
        store.set_size(&gone, 10);

        let job = Job::Open { paths: vec![dir.clone(), notes.clone()], include_subfolders: false, dropped: false, play: None, remembered: store.remembered() };
        let Done::Opened { folders, videos, others, moves, .. } = run(job) else { panic!("opened") };
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].1.as_ref().unwrap(), &[a.clone(), renamed.clone()]);
        assert!(videos.is_empty());
        assert_eq!(others, [notes]);
        assert_eq!(store.apply_moves(&moves), [(gone.clone(), renamed.clone())], "found off the UI thread");
        assert_eq!(store.get(&renamed).position, 60.0);

        let playlist = vec![a.clone(), dir.join("deleted.mkv")];
        let job = Job::Rescan { folder: dir.clone(), include_subfolders: false, why: Rescan::Refresh, playlist, remembered: store.remembered() };
        let Done::Rescanned { found, missing, .. } = run(job) else { panic!("rescanned") };
        assert_eq!(found.unwrap(), [a.clone(), renamed.clone()]);
        assert_eq!(missing, HashSet::from([dir.join("deleted.mkv")]));

        let Done::Modified(times) = run(Job::Modified(vec![a.clone(), dir.join("deleted.mkv")])) else { panic!("modified") };
        assert!(times[&a].is_some() && times[Path::new(&dir.join("deleted.mkv"))].is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
