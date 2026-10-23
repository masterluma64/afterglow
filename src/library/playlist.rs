//! Opening folders and videos, and changing the playlist.

use std::collections::HashMap;

use crate::*;

/// Finds the length of videos that haven't been played yet, on a background
/// thread with its own headless mpv.
pub(crate) struct DurationProbe {
    pub(crate) requests: Sender<PathBuf>,
    pub(crate) results: Receiver<(PathBuf, Option<f64>)>,
    pub(crate) pending: HashSet<PathBuf>,
}

impl DurationProbe {
    pub(crate) fn new(ctx: egui::Context) -> Self {
        let (requests, request_rx) = mpsc::channel::<PathBuf>();
        let (result_tx, results) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("duration-probe".into()).spawn(move || {
            let Ok(prober) = mpv::Prober::new() else { return };
            for path in request_rx {
                let duration = prober.duration(&path, PROBE_TIMEOUT);
                if result_tx.send((path, duration)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not start the duration probe: {e}");
        }
        Self { requests, results, pending: HashSet::new() }
    }

    pub(crate) fn request(&mut self, path: &Path) {
        // If the probe thread isn't running (e.g. mpv failed to start), the
        // send fails and the video simply stays without a length.
        if self.pending.insert(path.to_path_buf()) && self.requests.send(path.to_path_buf()).is_err() {
            self.pending.remove(path);
        }
    }
}

/// Disk work under way for the playlist (see `App::start_disk`).
pub(crate) struct DiskBusy {
    since: Instant,
    /// What is being read, for the notice.
    what: String,
    /// Whether the notice is up.
    announced: bool,
}

impl DiskBusy {
    fn notice(&self) -> String {
        format!("Reading {}…", self.what)
    }
}

/// Whether the recent folders are there, checked in the background: on an
/// unplugged or offline drive, checking takes long enough to freeze the
/// window, and the menus showing them are drawn every frame.
#[derive(Default)]
pub(crate) struct RecentFolders {
    found: HashMap<PathBuf, bool>,
    checking: Option<Receiver<HashMap<PathBuf, bool>>>,
    checked_at: Option<Instant>,
}

impl RecentFolders {
    /// Whether `folder` was there when last checked; not checked yet counts as there.
    pub(crate) fn found(&self, folder: &Path) -> bool {
        self.found.get(folder).copied().unwrap_or(true)
    }

    /// Checks `folders` again, unless that is under way or was just done.
    pub(crate) fn check(&mut self, folders: &[PathBuf], ctx: &egui::Context) {
        self.receive();
        let recent = self.checked_at.is_some_and(|at| at.elapsed() < Duration::from_secs(5));
        if self.checking.is_some() || recent {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (folders, ctx) = (folders.to_vec(), ctx.clone());
        let spawned = std::thread::Builder::new().name("check-folders".into()).spawn(move || {
            let found = folders.into_iter().map(|f| {
                let there = f.is_dir();
                (f, there)
            }).collect();
            if tx.send(found).is_ok() {
                ctx.request_repaint();
            }
        });
        if spawned.is_ok() {
            self.checking = Some(rx);
        }
    }

    pub(crate) fn receive(&mut self) {
        if let Some(found) = self.checking.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.found = found;
            self.checking = None;
            self.checked_at = Some(Instant::now());
        }
    }
}

/// Moves the item at `from` so it lands before the item now at `to`, or at
/// the end when `to` is the length.
pub(crate) fn move_item<T>(list: &mut Vec<T>, from: usize, to: usize) {
    if from >= list.len() || to > list.len() {
        return;
    }
    let item = list.remove(from);
    list.insert(if to > from { to - 1 } else { to }, item);
}

impl App {
    /// Queues every playlist video whose length isn't known yet.
    pub(crate) fn request_durations(&mut self) {
        for path in &self.playlist {
            if self.store.get(path).duration <= 0.0 {
                self.durations.request(path);
            }
        }
    }

    pub(crate) fn receive_durations(&mut self) {
        while let Ok((path, duration)) = self.durations.results.try_recv() {
            self.durations.pending.remove(&path);
            if let Some(d) = duration {
                self.store.set_duration(&path, d);
            }
        }
    }

    pub(crate) fn save_playlist(&mut self) {
        // Indices may have shifted.
        self.up_next = None;
        self.names.clear();
        self.summary = None;
        self.request_durations();
        self.store.set_playlist(&self.playlist);
        let current = self.current_path().map(Path::to_path_buf);
        if current.is_some() {
            self.store.set_current(current.as_deref());
        }
        self.store.save();
    }

    /// Starts disk work for the playlist (see `disk`); `what` names it in the
    /// notice shown if it takes a while. One job runs at a time: actions that
    /// need another wait for it (see `Action::needs_disk`).
    pub(crate) fn start_disk(&mut self, job: disk::Job, what: String) {
        if self.disk.start(job) {
            self.disk_busy = Some(DiskBusy { since: Instant::now(), what, announced: false });
        }
    }

    /// Takes in finished disk work, and says what's being read if it takes a while.
    pub(crate) fn receive_disk(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.disk.done.try_recv() {
            if let Some(busy) = self.disk_busy.take() {
                // Done reading: the notice saying so goes.
                if self.notice.as_ref().is_some_and(|n| n.text == busy.notice()) {
                    self.notice = None;
                }
            }
            match done {
                disk::Done::Opened { folders, videos, others, dropped, play } => {
                    self.opened(folders, videos, others, dropped, ctx);
                    if let Some(index) = play.and_then(|v| self.playlist.iter().position(|p| *p == v))
                        && self.current != Some(index)
                    {
                        self.play(index, ctx);
                    }
                }
                disk::Done::Rescanned { folder, include_subfolders, why, found, missing } => {
                    // The playlist may come from another folder by now.
                    if self.store.folder.as_ref() != Some(&folder) {
                        continue;
                    }
                    let found = match found {
                        Ok(found) => found,
                        Err(e) => {
                            self.warn(format!("Could not open {}: {e}", display_name(&folder)));
                            continue;
                        }
                    };
                    match why {
                        disk::Rescan::Subfolders => self.rescanned_subfolders(&folder, include_subfolders, found, ctx),
                        disk::Rescan::Refresh => self.refreshed(found, &missing),
                    }
                }
                disk::Done::Arrived(videos) => {
                    self.append_new(videos);
                }
            }
        }
        let Some(busy) = &mut self.disk_busy else { return };
        let wait = Duration::from_millis(400);
        if !busy.announced && busy.since.elapsed() >= wait {
            busy.announced = true;
            // Shown until the reading is done.
            let text = busy.notice();
            self.notice = Some(Notice { text, warning: false, until: Instant::now() + Duration::from_secs(600) });
        } else if !busy.announced {
            ctx.request_repaint_after(wait.saturating_sub(busy.since.elapsed()));
        }
    }

    /// Opens dropped or passed paths. Folders replace the playlist with their
    /// videos (paused where you left off, or playing if set so). Videos are
    /// added to the end, unless they were dropped: then the drop setting
    /// decides, asking the first time.
    /// With `play`, that video plays once it's in the playlist.
    pub(crate) fn open(&mut self, paths: Vec<PathBuf>, dropped: bool, play: Option<PathBuf>) {
        let what = match &paths[..] {
            [path] => display_name(path),
            _ => format!("{} items", paths.len()),
        };
        let include_subfolders = self.store.include_subfolders;
        let job = disk::Job::Open { paths, include_subfolders, dropped, play };
        self.start_disk(job, what);
    }

    /// Takes in what `open` found on disk.
    fn opened(
        &mut self,
        folders: Vec<(PathBuf, std::io::Result<Vec<PathBuf>>)>,
        videos: Vec<PathBuf>,
        others: Vec<PathBuf>,
        dropped: bool,
        ctx: &egui::Context,
    ) {
        if let Some(other) = others.first() {
            self.warn(format!("Not a video: {}", display_name(other)));
        }
        if folders.is_empty() {
            match self.store.settings.dropped_videos {
                _ if !dropped || videos.is_empty() || self.playlist.is_empty() => self.add_videos(videos, ctx),
                Some(behavior) => self.drop_videos(videos, behavior, ctx),
                None => self.pending_drop = Some(videos),
            }
            return;
        }
        let mut list = Vec::new();
        for (folder, found) in &folders {
            match found {
                Ok(found) => list.extend(found.iter().cloned()),
                Err(e) => self.warn(format!("Could not open {}: {e}", display_name(folder))),
            }
        }
        list.extend(videos);
        if list.is_empty() {
            let hint = if self.store.include_subfolders { "" } else { " (subfolders are not included)" };
            self.warn(format!("No videos found in {}{hint}", display_name(&folders[0].0)));
            return;
        }
        // Remember a single opened folder, so the subfolder setting can rescan it.
        self.store.set_folder((folders.len() == 1).then(|| folders[0].0.as_path()));
        self.sync_watch();
        let play = self.store.settings.play_opened_folders;
        self.replace_playlist(list, play, ctx);
    }

    /// Adds dropped videos to the playlist, or makes them the playlist.
    pub(crate) fn drop_videos(&mut self, videos: Vec<PathBuf>, behavior: progress::DroppedVideos, ctx: &egui::Context) {
        match behavior {
            progress::DroppedVideos::Add => self.add_videos(videos, ctx),
            progress::DroppedVideos::Replace => {
                // The playlist no longer comes from a folder.
                self.store.set_folder(None);
                self.sync_watch();
                self.replace_playlist(videos, true, ctx);
            }
        }
    }

    /// Changes whether folders include their subfolders, and rescans the folder
    /// the playlist came from. Videos added by hand stay, and so does the
    /// playing video if it is still part of the list.
    pub(crate) fn set_include_subfolders(&mut self, include: bool) {
        self.store.set_include_subfolders(include);
        self.store.save();
        self.sync_watch();
        let Some(folder) = self.store.folder.clone() else { return };
        let what = display_name(&folder);
        let why = disk::Rescan::Subfolders;
        let job = disk::Job::Rescan { folder, include_subfolders: include, why, playlist: Vec::new() };
        self.start_disk(job, what);
    }

    /// Takes in the rescan `set_include_subfolders` asked for.
    fn rescanned_subfolders(&mut self, folder: &Path, include: bool, scanned: Vec<PathBuf>, ctx: &egui::Context) {
        let added_by_hand = self.playlist.iter().filter(|p| !p.starts_with(folder)).cloned();
        let list: Vec<PathBuf> = scanned.into_iter().chain(added_by_hand).collect();
        let playing = self.current_path().map(Path::to_path_buf);
        if playing.as_ref().is_some_and(|p| !list.contains(p)) {
            self.stop(ctx);
        }
        self.current = playing.and_then(|p| list.iter().position(|q| *q == p));
        self.playlist = list;
        self.save_playlist();
        let what = if include { "with subfolders" } else { "without subfolders" };
        self.notify(format!("{}: {} videos {what}", display_name(folder), self.playlist.len()));
    }

    /// Makes `list` the playlist and opens where you left off in it: playing
    /// with `play`, otherwise paused, so nothing plays before you're ready.
    pub(crate) fn replace_playlist(&mut self, list: Vec<PathBuf>, play: bool, ctx: &egui::Context) {
        self.stop(ctx);
        self.playlist = list;
        self.show_sidebar = true;
        self.save_playlist();
        match self.continue_target() {
            Some(index) => self.load(index, !play, true, ctx),
            None => self.notify(format!("{} videos, all watched", self.playlist.len())),
        }
    }

    /// Appends videos (skipping ones already listed). Starts the first of them
    /// when nothing is playing.
    pub(crate) fn add_videos(&mut self, videos: Vec<PathBuf>, ctx: &egui::Context) {
        if videos.is_empty() {
            return;
        }
        let mut first = None;
        let mut added = 0;
        for video in videos {
            let index = match self.playlist.iter().position(|p| *p == video) {
                Some(i) => i,
                None => {
                    self.playlist.push(video);
                    added += 1;
                    self.playlist.len() - 1
                }
            };
            first.get_or_insert(index);
        }
        self.save_playlist();
        match first {
            Some(index) if self.current.is_none() => self.play(index, ctx),
            _ if added > 0 => self.notify(format!("Added {added} video{} to the playlist", if added == 1 { "" } else { "s" })),
            _ => self.notify("Already in the playlist"),
        }
    }

    pub(crate) fn remove(&mut self, index: usize, ctx: &egui::Context) {
        if index >= self.playlist.len() {
            return;
        }
        match self.current {
            Some(c) if c == index => self.stop(ctx),
            Some(c) if c > index => self.current = Some(c - 1),
            _ => {}
        }
        self.playlist.remove(index);
        self.save_playlist();
    }

    /// Points the folder watch at the folder the playlist came from.
    pub(crate) fn sync_watch(&self) {
        self.watch.set_target(self.store.folder.clone().map(|f| (f, self.store.include_subfolders)));
    }

    /// Starts looking, in the background, for remembered videos whose files
    /// are gone; [`Self::receive_gone`] forgets them.
    pub(crate) fn find_gone(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (videos, ctx) = (self.store.remembered_videos(), ctx.clone());
        let spawned = std::thread::Builder::new().name("find-gone".into()).spawn(move || {
            if tx.send(progress::gone(videos)).is_ok() {
                ctx.request_repaint();
            }
        });
        self.gone_scan = spawned.is_ok().then_some(rx);
    }

    /// Forgets what was saved about the videos `find_gone` found gone.
    pub(crate) fn receive_gone(&mut self) {
        let Some(gone) = self.gone_scan.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.gone_scan = None;
        self.forget_gone(gone);
    }

    /// Forgets the progress, length and track choices of these videos, whose
    /// files are gone. Whatever is playing right now stays, even if its file
    /// just went.
    fn forget_gone(&mut self, mut gone: Vec<String>) {
        if let Some(playing) = self.current_path() {
            let playing = playing.to_string_lossy();
            gone.retain(|k| *k != playing);
        }
        if self.store.forget(&gone) > 0 {
            self.summary = None;
            self.store.save();
        }
    }

    /// Drops the playlist entries whose files were found missing at startup.
    pub(crate) fn receive_missing_files(&mut self) {
        let Some(missing) = self.missing_files.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.missing_files = None;
        if !missing.is_empty() {
            let playing = self.current_path().map(Path::to_path_buf);
            self.reorder(|list| list.retain(|p| !missing.contains(p) || Some(p) == playing.as_ref()));
        }
    }

    /// Takes in background rescans of the playlist's folder.
    pub(crate) fn receive_scans(&mut self) {
        while let Ok((folder, found)) = self.watch.scans.try_recv() {
            if self.store.folder.as_ref() == Some(&folder) {
                let new: Vec<PathBuf> = found.into_iter().filter(|p| !self.playlist.contains(p)).collect();
                if !new.is_empty() {
                    self.actions.push(Action::Arrived(new));
                }
            }
        }
    }

    /// Adds the videos that aren't in the playlist yet to its end, returning
    /// how many there were.
    pub(crate) fn append_new(&mut self, found: Vec<PathBuf>) -> usize {
        let new: Vec<PathBuf> = found.into_iter().filter(|p| !self.playlist.contains(p)).collect();
        if new.is_empty() {
            return 0;
        }
        self.names.clear();
        self.summary = None;
        let count = new.len();
        // Appending shifts no indices, so a countdown to the next video stays.
        self.playlist.extend(new);
        self.store.set_playlist(&self.playlist);
        self.request_durations();
        self.store.save();
        if count > 0 {
            self.notify(format!("{count} new video{} in the folder", if count == 1 { "" } else { "s" }));
        }
        count
    }

    pub(crate) fn refresh_folder(&mut self) {
        let Some(folder) = self.store.folder.clone() else {
            return self.notify("The playlist wasn't opened from a folder");
        };
        let what = display_name(&folder);
        let job = disk::Job::Rescan {
            folder,
            include_subfolders: self.store.include_subfolders,
            why: disk::Rescan::Refresh,
            playlist: self.playlist.clone(),
        };
        self.start_disk(job, what);
    }

    /// Takes in the rescan `refresh_folder` asked for.
    fn refreshed(&mut self, found: Vec<PathBuf>, missing: &HashSet<PathBuf>) {
        let added = self.append_new(found);
        let before = self.playlist.len();
        let playing = self.current_path().map(Path::to_path_buf);
        self.reorder(|list| list.retain(|p| !missing.contains(p) || Some(p) == playing.as_ref()));
        let removed = before - self.playlist.len();
        self.forget_gone(missing.iter().map(|p| p.to_string_lossy().into_owned()).collect());
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        self.notify(match (added, removed) {
            (0, 0) => "No changes in the folder".to_string(),
            (a, 0) => format!("{a} new video{}", plural(a)),
            (0, r) => format!("Removed {r} missing video{}", plural(r)),
            (a, r) => format!("{a} new video{}, removed {r} missing", plural(a)),
        });
    }

    /// Rearranges the playlist, keeping track of the playing video.
    pub(crate) fn reorder(&mut self, rearrange: impl FnOnce(&mut Vec<PathBuf>)) {
        let playing = self.current_path().map(Path::to_path_buf);
        rearrange(&mut self.playlist);
        self.current = playing.and_then(|p| self.playlist.iter().position(|q| *q == p));
        self.save_playlist();
    }

    pub(crate) fn clear_playlist(&mut self, ctx: &egui::Context) {
        self.stop(ctx);
        self.playlist.clear();
        self.store.set_current(None);
        self.store.set_folder(None);
        self.save_playlist();
        self.sync_watch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_items() {
        let moved = |from, to| {
            let mut list = vec!['a', 'b', 'c', 'd'];
            move_item(&mut list, from, to);
            list.into_iter().collect::<String>()
        };
        assert_eq!(moved(0, 2), "bacd", "before c");
        assert_eq!(moved(3, 1), "adbc", "before b");
        assert_eq!(moved(1, 4), "acdb", "to the end");
        assert_eq!(moved(2, 0), "cabd", "to the start");
        assert_eq!(moved(1, 1), "abcd");
        assert_eq!(moved(1, 2), "abcd", "before the next one is where it already is");
        assert_eq!(moved(9, 0), "abcd", "out of range is ignored");
    }
}
