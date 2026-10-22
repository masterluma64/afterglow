//! Disk work for the playlist, done on a background thread. On a network
//! drive that is slow or gone, reading a folder (or even checking that a
//! file exists) can take many seconds, and the window must not freeze
//! meanwhile. Jobs run one at a time, in the order they were given, and
//! each comes back as a [`Done`] for the UI to take in.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;

use crate::library;

pub(crate) enum Job {
    /// Folders and videos to open: which are folders, and their videos.
    Open {
        paths: Vec<PathBuf>,
        include_subfolders: bool,
        dropped: bool,
        /// A video to play once it's in the playlist.
        play: Option<PathBuf>,
    },
    /// The playlist's folder, read again.
    Rescan {
        folder: PathBuf,
        include_subfolders: bool,
        why: Rescan,
        /// The playlist when asked: what's new is what isn't in it, and on
        /// a refresh, its entries are checked to be still there.
        playlist: Vec<PathBuf>,
    },
    /// Videos the folder watch found new in the playlist's folder, taken in
    /// once the jobs before them are done.
    Arrived(Vec<PathBuf>),
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
    },
    Rescanned {
        folder: PathBuf,
        include_subfolders: bool,
        why: Rescan,
        found: std::io::Result<Vec<PathBuf>>,
        /// Playlist entries whose files are gone (on a refresh).
        missing: HashSet<PathBuf>,
    },
    Arrived(Vec<PathBuf>),
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
        Job::Open { paths, include_subfolders, dropped, play } => {
            let (folders, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.is_dir());
            let (videos, others): (Vec<_>, Vec<_>) = files.into_iter().partition(|p| library::is_video(p));
            let folders: Vec<_> = folders.into_iter().map(|f| {
                let found = library::scan_folder(&f, include_subfolders);
                (f, found)
            }).collect();
            Done::Opened { folders, videos, others, dropped, play }
        }
        Job::Rescan { folder, include_subfolders, why, playlist } => {
            let found = library::scan_folder(&folder, include_subfolders);
            let missing = match why {
                Rescan::Refresh if found.is_ok() => playlist.iter().filter(|p| !p.is_file()).cloned().collect(),
                _ => HashSet::new(),
            };
            Done::Rescanned { folder, include_subfolders, why, found, missing }
        }
        Job::Arrived(videos) => Done::Arrived(videos),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_rescans() {
        let dir = std::env::temp_dir().join(format!("afterglow-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, size: usize| {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; size]).unwrap();
            path
        };
        let (a, b, notes) = (file("a.mkv", 5), file("b.mkv", 10), file("notes.txt", 1));

        let job = Job::Open { paths: vec![dir.clone(), notes.clone()], include_subfolders: false, dropped: false, play: None };
        let Done::Opened { folders, videos, others, .. } = run(job) else { panic!("opened") };
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].1.as_ref().unwrap(), &[a.clone(), b.clone()]);
        assert!(videos.is_empty());
        assert_eq!(others, [notes]);

        let playlist = vec![a.clone(), dir.join("deleted.mkv")];
        let job = Job::Rescan { folder: dir.clone(), include_subfolders: false, why: Rescan::Refresh, playlist };
        let Done::Rescanned { found, missing, .. } = run(job) else { panic!("rescanned") };
        assert_eq!(found.unwrap(), [a.clone(), b.clone()]);
        assert_eq!(missing, HashSet::from([dir.join("deleted.mkv")]));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
