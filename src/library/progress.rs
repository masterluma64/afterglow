//! Persistent state: watch progress per video and the playlist, stored as
//! JSON in the user's config folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use super::stats::{Stats, Tally};

/// How many recently opened folders are remembered.
const RECENT_FOLDERS: usize = 10;
/// How many videos opened by themselves are remembered.
const RECENT_VIDEOS: usize = 10;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Entry {
    pub position: f64,
    pub duration: f64,
    pub watched: bool,
}

impl Entry {
    /// Where playback should resume, if anywhere, given the fraction of a
    /// video after which it counts as watched.
    fn resume_position(&self, watched_fraction: f64) -> Option<f64> {
        (self.position > 5.0 && self.position < self.duration * watched_fraction).then_some(self.position)
    }

    pub fn fraction(&self) -> f32 {
        if self.duration > 0.0 { (self.position / self.duration).clamp(0.0, 1.0) as f32 } else { 0.0 }
    }
}

/// Preferences from the settings dialog.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Seconds the arrow keys seek, and with Shift.
    pub seek_short: f64,
    pub seek_long: f64,
    /// How far into a video (in percent) it counts as watched.
    pub watched_percent: f64,
    /// What dropping videos onto the window does; `None` asks (the first time).
    pub dropped_videos: Option<DroppedVideos>,
    /// Which side of the video the playlist is on.
    pub sidebar_side: Side,
    pub theme: Theme,
    /// Accent color (selection, seek bar, progress); `None` for the default blue.
    pub accent: Option<[u8; 3]>,
    /// Preferred languages, as mpv's comma-separated `slang` / `alang` lists.
    pub sub_languages: String,
    pub audio_languages: String,
    pub hardware_decoding: bool,
    /// Evens out loud and quiet parts of the sound ("night mode").
    pub level_volume: bool,
    /// Whether screenshots are also copied to the clipboard (they are always saved).
    pub copy_screenshots: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            seek_short: 5.0,
            seek_long: 30.0,
            watched_percent: 90.0,
            dropped_videos: None,
            sidebar_side: Side::Left,
            theme: Theme::System,
            accent: None,
            sub_languages: "en,eng".into(),
            audio_languages: String::new(),
            hardware_decoding: true,
            level_volume: false,
            copy_screenshots: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DroppedVideos {
    /// Add them to the end of the playlist.
    Add,
    /// Make them the playlist.
    Replace,
}

/// The window's place and size, in points, to open it the same way next time.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct WindowPlacement {
    /// Top-left corner of the window, including its frame.
    pub x: f32,
    pub y: f32,
    /// Size of the area inside the frame, when not maximized.
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    /// Pixels per point when saved, to find the position on screen.
    pub scale: f32,
}

#[derive(Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Side {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

/// Settings for the videos in one folder. `None` follows the global
/// setting.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderSettings {
    /// Also used for the folders directly inside this one (seasons of a
    /// show, say) that have no settings of their own.
    pub include_subfolders: bool,
    /// Where the opening ends, in seconds from the start of each video: it
    /// can be skipped until then.
    pub opening_end: Option<f64>,
    /// Where the ending starts, in seconds back from the end of each video
    /// (so it fits videos of different lengths): from then on it can be
    /// skipped, which goes to the next video.
    pub ending_start: Option<f64>,
    pub sub_languages: Option<String>,
    pub audio_languages: Option<String>,
    /// Where the subtitle delay and timing start for a video in the folder,
    /// until it is changed for that video.
    pub sub_delay: Option<f64>,
    pub sub_speed: Option<f64>,
    pub watched_percent: Option<f64>,
    pub autoplay_next: Option<bool>,
    pub level_volume: Option<bool>,
    pub speed: Option<f64>,
}

impl FolderSettings {
    /// Where the opening and ending are in a video of `duration` seconds, in
    /// seconds from its start, if the folder has them and they fit.
    pub fn skips(&self, duration: f64) -> Vec<(Skip, f64, f64)> {
        let opening = self.opening_end.map(|end| (Skip::Opening, 0.0, end));
        let ending = self.ending_start.map(|start| (Skip::Ending, duration - start, duration));
        [opening, ending]
            .into_iter()
            .flatten()
            .filter(|&(_, start, end)| duration > 0.0 && start >= 0.0 && end <= duration && end > start)
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skip {
    Opening,
    Ending,
}

/// Track choices made for one video, restored when it is opened again.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackPrefs {
    /// mpv's `aid` / `sid` values ("no" for off). `None` keeps mpv's choice.
    pub audio: Option<String>,
    pub subtitle: Option<String>,
    /// Seconds subtitles are shifted by.
    pub sub_delay: f64,
    /// Factor subtitle timing is stretched by, if not 1.
    pub sub_speed: Option<f64>,
    /// Subtitle files loaded by hand, in the order they were added.
    pub sub_files: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub entries: HashMap<String, Entry>,
    /// Track choices per video.
    pub tracks: HashMap<String, TrackPrefs>,
    /// The title stored in each video's file, once read: "" when it has none
    /// worth showing (see `set_title`).
    pub titles: HashMap<String, String>,
    /// The playlist, in play order.
    pub playlist: Vec<PathBuf>,
    /// The playlist entry that was playing last.
    pub current: Option<PathBuf>,
    /// The folder the playlist was opened from, if any.
    pub folder: Option<PathBuf>,
    /// Folders opened recently, most recent first.
    pub recent_folders: Vec<PathBuf>,
    /// Videos opened by themselves recently (see `is_movie`), most recent first.
    pub recent_videos: Vec<PathBuf>,
    /// Whether opening a folder also includes videos in its subfolders.
    pub include_subfolders: bool,
    pub volume: f64,
    pub settings: Settings,
    /// Whether the playlist is docked beside the video, or slides out over it.
    pub sidebar_pinned: bool,
    /// Whether the next video starts by itself when one finishes.
    pub autoplay_next: bool,
    /// Where the window was, to open it there again.
    pub window: Option<WindowPlacement>,
    /// Watching stats, which outlast the progress of deleted videos.
    pub stats: Stats,
    /// Settings per folder of videos.
    pub folders: HashMap<String, FolderSettings>,
    /// Settings per video opened by itself (see `is_movie`), which it uses in
    /// place of its folder's: a folder of movies isn't a show.
    pub videos: HashMap<String, FolderSettings>,
    #[serde(skip)]
    dirty: bool,
    /// Set when the saved file exists but couldn't be read: saving would
    /// replace it with this empty state, so nothing is saved.
    #[serde(skip)]
    read_failed: bool,
    /// Whether the last save failed, so a failure is reported only once.
    #[serde(skip)]
    save_failing: bool,
    #[serde(skip)]
    saver: Option<Saver>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            tracks: HashMap::new(),
            titles: HashMap::new(),
            playlist: Vec::new(),
            current: None,
            folder: None,
            recent_folders: Vec::new(),
            recent_videos: Vec::new(),
            include_subfolders: false,
            volume: 100.0,
            autoplay_next: true,
            sidebar_pinned: true,
            window: None,
            stats: Stats::default(),
            folders: HashMap::new(),
            videos: HashMap::new(),
            settings: Settings::default(),
            dirty: false,
            read_failed: false,
            save_failing: false,
            saver: None,
        }
    }
}

/// Which of these remembered videos (or folders) are gone: they no longer
/// exist, though the drive or network share they were on is there. Ones on a
/// drive that isn't connected right now are not counted. Checking can be slow on
/// network drives, so this is meant for a background thread.
pub fn gone(keys: Vec<String>) -> Vec<String> {
    let mut reachable: HashMap<PathBuf, bool> = HashMap::new();
    keys.into_iter()
        .filter(|k| {
            let path = Path::new(k);
            // The drive (`D:\`) or share (`\\server\share\`) it is on.
            let root: PathBuf = path.components().take(2).collect();
            path.is_absolute()
                && *reachable.entry(root.clone()).or_insert_with(|| root.exists())
                && !path.exists()
        })
        .collect()
}

/// The UTF-8 byte order mark.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Where the app keeps its data: a `data` folder next to the executable, so
/// the app is portable, or Afterglow's folder in the local AppData. Whichever
/// holds the progress file wins; the first start is portable.
/// `AFTERGLOW_DATA_DIR` overrides both, e.g. for testing.
pub fn data_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("AFTERGLOW_DATA_DIR") {
            return Some(PathBuf::from(dir));
        }
        let portable = data_dir_for(true);
        let has_progress = |dir: &Option<PathBuf>| dir.as_ref().is_some_and(|dir| dir.join(FILE_NAME).is_file());
        match data_dir_for(false) {
            appdata if !has_progress(&portable) && has_progress(&appdata) => appdata,
            _ => portable,
        }
    })
    .clone()
}

/// The data folder when portable, or when not.
pub fn data_dir_for(portable: bool) -> Option<PathBuf> {
    if portable {
        Some(std::env::current_exe().ok()?.parent()?.join("data"))
    } else {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("afterglow"))
    }
}

/// Whether the data folder is chosen with `AFTERGLOW_DATA_DIR`, so it can't
/// be moved.
pub fn data_dir_fixed() -> bool {
    std::env::var_os("AFTERGLOW_DATA_DIR").is_some()
}

const FILE_NAME: &str = "progress.json";

fn file_path() -> Option<PathBuf> {
    Some(data_dir()?.join(FILE_NAME))
}

impl Store {
    /// Loads the saved state. A damaged file is set aside as
    /// `progress.bad.json` rather than overwritten, and a warning is returned.
    /// A file that can't be read at all (e.g. locked by another program) is
    /// left alone, and nothing is saved until the next start.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = file_path() else { return (Self::default(), None) };
        let (mut store, warning) = Self::load_from(&path);
        if !store.read_failed {
            store.seed_stats();
        }
        (store, warning)
    }

    fn load_from(path: &Path) -> (Self, Option<String>) {
        // Read as bytes, so text that isn't UTF-8 counts as damaged below.
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                let store = Self { read_failed: true, ..Self::default() };
                let warning = format!(
                    "Could not read saved progress ({e}). It is left as it is, and nothing will be saved until Afterglow is restarted."
                );
                return (store, Some(warning));
            }
        };
        // Editors such as Notepad may save the file with a byte order mark.
        let json = bytes.strip_prefix(BOM).unwrap_or(&bytes);
        match serde_json::from_slice(json) {
            Ok(store) => (store, None),
            Err(e) => {
                let backup = path.with_extension("bad.json");
                let kept = std::fs::rename(path, &backup).is_ok();
                let mut warning = format!("Saved progress was damaged and could not be loaded ({e}).");
                if kept {
                    warning.push_str(&format!(" A copy was kept as {}.", backup.display()));
                }
                (Self::default(), Some(warning))
            }
        }
    }

    /// Saves if anything changed. The file is written by a background thread,
    /// so a slow disk never holds up the UI.
    pub fn save(&mut self) {
        if !self.dirty || self.read_failed {
            return;
        }
        let Some(path) = file_path() else { return };
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                self.saver.get_or_insert_with(|| Saver::new(path)).write(bytes);
                self.dirty = false;
            }
            Err(e) => eprintln!("failed to save progress: {e}"),
        }
    }

    /// Says what went wrong once saving starts failing (e.g. Afterglow's
    /// folder can't be written to). After that it stays quiet until a save
    /// works again, so a lasting problem is reported once.
    pub fn save_problem(&mut self) -> Option<String> {
        let saver = self.saver.as_ref()?;
        let mut problem = None;
        for result in saver.results.try_iter() {
            match result {
                Err(e) if !self.save_failing => {
                    self.save_failing = true;
                    problem = Some(e);
                }
                Err(_) => {}
                Ok(()) => self.save_failing = false,
            }
        }
        let folder = saver.path.parent().unwrap_or(&saver.path).display();
        problem.map(|e| format!("Progress can't be saved in {folder} ({e}). Afterglow needs a folder it can write to."))
    }

    /// The folder progress is saved in.
    pub fn folder(&self) -> Option<PathBuf> {
        match &self.saver {
            Some(saver) => saver.path.parent().map(Path::to_path_buf),
            None => data_dir(),
        }
    }

    /// Saves progress in `dir` from now on, and removes the progress file
    /// from the folder it was in. One already in `dir` is kept as
    /// `progress.bak.json`.
    pub fn move_to(&mut self, dir: &Path) -> Result<(), String> {
        if self.read_failed {
            return Err("the progress file couldn't be read, so it stays where it is".into());
        }
        let old = self.folder().map(|folder| folder.join(FILE_NAME));
        let new = dir.join(FILE_NAME);
        if old.as_ref() == Some(&new) {
            return Ok(());
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if new.is_file() {
            std::fs::rename(&new, dir.join("progress.bak.json")).map_err(|e| e.to_string())?;
        }
        write_atomically(&new, &bytes).map_err(|e| e.to_string())?;
        // Replacing the saver waits for writes still on their way to the old file.
        self.saver = Some(Saver::new(new));
        self.dirty = false;
        if let Some(old) = old {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }

    #[cfg(test)]
    fn save_to(&mut self, path: &Path) {
        let bytes = serde_json::to_vec_pretty(self).unwrap();
        write_atomically(path, &bytes).unwrap();
        self.dirty = false;
    }

    pub fn get(&self, path: &Path) -> Entry {
        self.entries.get(&key(path)).copied().unwrap_or_default()
    }

    /// The title stored in a video's file, if it has one worth showing.
    pub fn title(&self, path: &Path) -> Option<&str> {
        self.titles.get(&key(path)).map(String::as_str).filter(|t| !t.is_empty())
    }

    /// Whether a video's file was read for its title yet.
    pub fn title_known(&self, path: &Path) -> bool {
        self.titles.contains_key(&key(path))
    }

    /// Records the title read from a video's file. One that looks like a web
    /// address (left by whoever shared the file) counts as none.
    pub fn set_title(&mut self, path: &Path, title: Option<String>) {
        let title = title.map(|t| t.trim().to_string()).unwrap_or_default();
        let lower = title.to_lowercase();
        let junk = ["www.", "http", ".com", ".net", ".org"].iter().any(|w| lower.contains(w));
        self.titles.insert(key(path), if junk { String::new() } else { title });
        self.dirty = true;
    }

    pub fn tracks(&self, path: &Path) -> Option<&TrackPrefs> {
        self.tracks.get(&key(path))
    }

    pub fn tracks_mut(&mut self, path: &Path) -> &mut TrackPrefs {
        self.dirty = true;
        self.tracks.entry(key(path)).or_default()
    }

    /// The settings of the folder a video is in, or else those of the folder
    /// above it when they include subfolders, with the folder they belong to.
    /// A video opened by itself has its own instead, which belong to it.
    pub fn folder_settings<'v>(&self, video: &'v Path) -> Option<(&'v Path, &FolderSettings)> {
        if self.is_movie(video) {
            return self.videos.get(&key(video)).map(|settings| (video, settings));
        }
        let own = video.parent()?;
        if let Some(settings) = self.folders.get(&key(own)) {
            return Some((own, settings));
        }
        let parent = own.parent()?;
        self.folders.get(&key(parent)).filter(|s| s.include_subfolders).map(|s| (parent, s))
    }

    /// Saves the settings for a video's folder: for its own folder, or with
    /// `include_subfolders` for the folder above it (in place of its own).
    /// For a video opened by itself, they are its own.
    /// Turning `include_subfolders` off moves them back to its own folder.
    pub fn set_folder_settings(&mut self, video: &Path, settings: FolderSettings) {
        if self.is_movie(video) {
            if settings == FolderSettings::default() {
                self.videos.remove(&key(video));
            } else {
                self.videos.insert(key(video), FolderSettings { include_subfolders: false, ..settings });
            }
            self.dirty = true;
            return;
        }
        let Some(own) = video.parent() else { return };
        let parent = own.parent().filter(|_| settings.include_subfolders);
        let target = parent.unwrap_or(own);
        // Wherever they were saved before, they are moved.
        if let Some((from, _)) = self.folder_settings(video) {
            let from = key(from);
            self.folders.remove(&from);
        }
        if settings != FolderSettings::default() {
            let include_subfolders = parent.is_some();
            self.folders.insert(key(target), FolderSettings { include_subfolders, ..settings });
        }
        self.dirty = true;
    }

    /// The global settings, with those of a video's folder in their place.
    pub fn settings_for(&self, video: Option<&Path>) -> Settings {
        let mut settings = self.settings.clone();
        let Some((_, folder)) = video.and_then(|v| self.folder_settings(v)) else { return settings };
        if let Some(languages) = &folder.sub_languages {
            settings.sub_languages = languages.clone();
        }
        if let Some(languages) = &folder.audio_languages {
            settings.audio_languages = languages.clone();
        }
        settings.watched_percent = folder.watched_percent.unwrap_or(settings.watched_percent);
        settings.level_volume = folder.level_volume.unwrap_or(settings.level_volume);
        settings
    }

    /// Whether the next video starts by itself after this one.
    pub fn autoplay_after(&self, video: &Path) -> bool {
        self.folder_settings(video).and_then(|(_, f)| f.autoplay_next).unwrap_or(self.autoplay_next)
    }

    /// Fraction of a video after which it counts as watched.
    pub fn watched_fraction(&self, video: &Path) -> f64 {
        let folder = self.folder_settings(video).and_then(|(_, f)| f.watched_percent);
        folder.unwrap_or(self.settings.watched_percent).clamp(50.0, 100.0) / 100.0
    }

    /// Where a video should resume, if anywhere.
    pub fn resume_position(&self, path: &Path) -> Option<f64> {
        self.get(path).resume_position(self.watched_fraction(path))
    }

    pub fn update_position(&mut self, path: &Path, position: f64, duration: f64) {
        let watched_fraction = self.watched_fraction(path);
        let entry = self.entries.entry(key(path)).or_default();
        entry.position = position;
        entry.duration = duration;
        let finished = !entry.watched && duration > 0.0 && position >= duration * watched_fraction;
        if finished {
            entry.watched = true;
            self.count_finished(path);
        }
        self.dirty = true;
    }

    /// Records the position without it counting as watched, for a video
    /// marked unwatched while it plays (it may already be past the point
    /// where it would count).
    pub fn update_position_unwatched(&mut self, path: &Path, position: f64, duration: f64) {
        let entry = self.entries.entry(key(path)).or_default();
        entry.position = position;
        entry.duration = duration;
        self.dirty = true;
    }

    /// Marks a video watched because it played to its end, counting it as
    /// finished in the stats unless it already counted as watched.
    pub fn finish(&mut self, path: &Path) {
        if !self.get(path).watched {
            self.count_finished(path);
        }
        self.set_watched(path, true);
    }

    /// Adds seconds of a video played to the stats.
    pub fn record_watching(&mut self, path: &Path, seconds: f64) {
        let today = crate::clock::today();
        if self.is_movie(path) {
            let movie = self.movie_name(path);
            self.stats.add_movie_time(&movie, seconds, today);
        } else {
            let series = self.series_of(path);
            self.stats.add_time(&series, seconds, today);
        }
        self.dirty = true;
    }

    fn count_finished(&mut self, path: &Path) {
        let today = crate::clock::today();
        if self.is_movie(path) {
            let movie = self.movie_name(path);
            self.stats.add_movie_finished(&movie, today);
        } else {
            let series = self.series_of(path);
            self.stats.add_finished(&series, today);
        }
    }

    /// The name a movie counts under in the stats: its title, or else its file name.
    fn movie_name(&self, path: &Path) -> String {
        match self.title(path) {
            Some(title) => title.to_string(),
            None => path.file_stem().map_or_else(|| key(path), |s| s.to_string_lossy().into_owned()),
        }
    }

    /// The series a video counts towards in the stats: the opened folder it
    /// is in (so seasons in subfolders count as one), or else its own folder.
    fn series_of(&self, path: &Path) -> String {
        let folder = self.folder.as_deref().filter(|f| path.starts_with(f)).or_else(|| path.parent());
        folder.and_then(Path::file_name).map_or_else(|| "Other".into(), |name| name.to_string_lossy().into_owned())
    }

    /// The first time stats are kept, estimates what was watched before from
    /// the saved progress.
    fn seed_stats(&mut self) {
        if self.stats.seeded {
            return;
        }
        let videos: Vec<(String, Tally)> = self
            .entries
            .iter()
            .map(|(k, e)| {
                let seconds = if e.watched { e.duration } else { e.position };
                (self.series_of(Path::new(k)), Tally { seconds: seconds.max(0.0), finished: u32::from(e.watched) })
            })
            .collect();
        self.stats.seed(videos);
        self.dirty = true;
    }

    /// Whether a video was opened by itself, like a movie: it is the whole
    /// playlist, which didn't come from a folder.
    pub fn is_movie(&self, video: &Path) -> bool {
        self.folder.is_none() && self.playlist.len() == 1 && self.playlist[0] == video
    }

    pub fn set_watched(&mut self, path: &Path, watched: bool) {
        let entry = self.entries.entry(key(path)).or_default();
        entry.watched = watched;
        entry.position = if watched { entry.duration } else { 0.0 };
        self.dirty = true;
    }

    /// Every video and folder anything is remembered about, to look for ones
    /// that are gone (see [`gone`]).
    pub fn remembered_paths(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .entries
            .keys()
            .chain(self.tracks.keys())
            .chain(self.titles.keys())
            .chain(self.videos.keys())
            .chain(self.folders.keys())
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Forgets everything saved about these videos and folders: progress,
    /// length, title, track choices and settings. Returns how many there were.
    pub fn forget(&mut self, keys: &[String]) -> usize {
        let mut forgotten = 0;
        for k in keys {
            let entry = self.entries.remove(k).is_some();
            let tracks = self.tracks.remove(k).is_some();
            let title = self.titles.remove(k).is_some();
            let settings = self.videos.remove(k).is_some() | self.folders.remove(k).is_some();
            forgotten += usize::from(entry || tracks || title || settings);
        }
        if forgotten > 0 {
            self.dirty = true;
        }
        forgotten
    }

    /// Forgets the position and watched state of every video, returning how
    /// many had any. Known lengths are kept, since they describe the files.
    /// Clears every stat. The estimate from saved progress isn't made again,
    /// so they start from nothing.
    pub fn clear_stats(&mut self) {
        self.stats = Stats { seeded: true, ..Stats::default() };
        self.dirty = true;
    }

    pub fn reset_progress(&mut self) -> usize {
        let mut reset = 0;
        for entry in self.entries.values_mut() {
            if entry.watched || entry.position > 0.0 {
                reset += 1;
            }
            entry.position = 0.0;
            entry.watched = false;
        }
        self.dirty = true;
        reset
    }

    /// Records a video's length, found without playing it.
    pub fn set_duration(&mut self, path: &Path, duration: f64) {
        self.entries.entry(key(path)).or_default().duration = duration;
        self.dirty = true;
    }

    pub fn set_playlist(&mut self, playlist: &[PathBuf]) {
        self.playlist = playlist.to_vec();
        self.dirty = true;
    }

    pub fn set_current(&mut self, current: Option<&Path>) {
        self.current = current.map(Path::to_path_buf);
        self.dirty = true;
    }

    pub fn set_folder(&mut self, folder: Option<&Path>) {
        self.folder = folder.map(Path::to_path_buf);
        if let Some(folder) = folder {
            self.recent_folders.retain(|f| f != folder);
            self.recent_folders.insert(0, folder.to_path_buf());
            self.recent_folders.truncate(RECENT_FOLDERS);
        }
        self.dirty = true;
    }

    pub fn clear_recent_folders(&mut self) {
        self.recent_folders.clear();
        self.dirty = true;
    }

    /// Puts a video opened by itself first among the recent videos.
    pub fn add_recent_video(&mut self, video: &Path) {
        if self.recent_videos.first().map(PathBuf::as_path) != Some(video) {
            self.recent_videos.retain(|v| v != video);
            self.recent_videos.insert(0, video.to_path_buf());
            self.recent_videos.truncate(RECENT_VIDEOS);
            self.dirty = true;
        }
    }

    pub fn clear_recent_videos(&mut self) {
        self.recent_videos.clear();
        self.dirty = true;
    }

    /// Drops recent folders and videos that no longer exist.
    pub fn forget_recent(&mut self, gone: &[PathBuf]) {
        let before = self.recent_folders.len() + self.recent_videos.len();
        self.recent_folders.retain(|f| !gone.contains(f));
        self.recent_videos.retain(|v| !gone.contains(v));
        self.dirty |= self.recent_folders.len() + self.recent_videos.len() != before;
    }

    pub fn set_include_subfolders(&mut self, include: bool) {
        self.include_subfolders = include;
        self.dirty = true;
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume;
        self.dirty = true;
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.dirty = true;
    }

    pub fn set_sidebar_pinned(&mut self, pinned: bool) {
        self.sidebar_pinned = pinned;
        self.dirty = true;
    }

    pub fn set_window(&mut self, window: WindowPlacement) {
        if self.window != Some(window) {
            self.window = Some(window);
            self.dirty = true;
        }
    }

    pub fn set_autoplay_next(&mut self, autoplay: bool) {
        self.autoplay_next = autoplay;
        self.dirty = true;
    }
}

/// Writes to a temporary file that replaces the old one only once it is
/// fully on disk, so a crash mid-save never leaves a half-written file.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)
}

/// Writes saved state on a background thread. When saves pile up only the
/// newest is written. Dropping it waits for the last write to finish.
struct Saver {
    path: PathBuf,
    queue: Option<Sender<Vec<u8>>>,
    /// How each write went.
    results: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
}

impl Saver {
    fn new(path: PathBuf) -> Self {
        let (queue, saves) = mpsc::channel::<Vec<u8>>();
        let (result_tx, results) = mpsc::channel();
        let thread_path = path.clone();
        let thread = std::thread::Builder::new()
            .name("save".into())
            .spawn(move || {
                while let Ok(mut bytes) = saves.recv() {
                    while let Ok(newer) = saves.try_recv() {
                        bytes = newer;
                    }
                    let result = write_atomically(&thread_path, &bytes);
                    if let Err(e) = &result {
                        eprintln!("failed to save progress: {e}");
                    }
                    let _ = result_tx.send(result.map_err(|e| e.to_string()));
                }
            })
            .map_err(|e| eprintln!("could not start saving in the background: {e}"))
            .ok();
        Self { path, queue: Some(queue), results, thread }
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(queue) = &self.queue {
            let _ = queue.send(bytes);
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        // Closing the queue ends the thread once it has written what's left.
        self.queue = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgets_gone_videos_but_not_ones_on_missing_drives() {
        let dir = std::env::temp_dir().join(format!("afterglow-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let there = dir.join("there.mkv");
        std::fs::write(&there, b"x").unwrap();
        let deleted = dir.join("deleted.mkv");
        // A drive letter with nothing connected, like an unplugged USB drive.
        let unplugged = ('D'..='Z')
            .map(|d| PathBuf::from(format!("{d}:\\")))
            .find(|root| !root.exists())
            .expect("a free drive letter")
            .join("Shows")
            .join("pilot.mkv");

        let mut store = Store::default();
        for path in [&there, &deleted, &unplugged] {
            store.set_watched(path, true);
        }
        store.tracks_mut(&deleted).subtitle = Some("2".into());
        store.tracks_mut(&dir.join("only tracks.mkv")).audio = Some("1".into());
        // Settings of a folder that is there, of one that was deleted, and of a movie that was.
        let speed = FolderSettings { speed: Some(1.5), ..FolderSettings::default() };
        store.folders.insert(key(&dir), speed.clone());
        store.folders.insert(key(&dir.join("Deleted show")), speed.clone());
        store.videos.insert(key(&dir.join("deleted movie.mkv")), speed);

        let mut found = gone(store.remembered_paths());
        found.sort();
        let mut expected = vec![
            key(&deleted),
            key(&dir.join("only tracks.mkv")),
            key(&dir.join("Deleted show")),
            key(&dir.join("deleted movie.mkv")),
        ];
        expected.sort();
        assert_eq!(found, expected, "neither a video that exists nor one on an unplugged drive is gone");
        assert_eq!(store.forget(&found), 4);
        assert!(store.tracks(&deleted).is_none());
        assert!(store.videos.is_empty());
        assert_eq!(store.folders.keys().collect::<Vec<_>>(), [&key(&dir)], "the folder that is there keeps its settings");
        assert!(store.get(&there).watched && store.get(&unplugged).watched);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_progress_keeps_lengths_and_playlist() {
        let (a, b, c) = (Path::new("a.mkv"), Path::new("b.mkv"), Path::new("c.mkv"));
        let mut store = Store::default();
        store.set_playlist(&[a.to_path_buf(), b.to_path_buf(), c.to_path_buf()]);
        store.update_position(a, 600.0, 1200.0);
        store.update_position(b, 0.0, 900.0);
        store.set_watched(b, true);
        store.set_duration(c, 300.0);

        assert_eq!(store.reset_progress(), 2, "only a and b had progress");
        for (path, length) in [(a, 1200.0), (b, 900.0), (c, 300.0)] {
            let entry = store.get(path);
            assert_eq!((entry.position, entry.watched), (0.0, false));
            assert_eq!(entry.duration, length, "lengths are kept");
        }
        assert_eq!(store.playlist.len(), 3);
    }

    #[test]
    fn saves_in_the_background_and_finishes_on_drop() {
        let dir = std::env::temp_dir().join(format!("afterglow-saver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("progress.json");
        let saver = Saver::new(path.clone());
        for i in 0..50 {
            saver.write(format!("{{\"n\": {i}}}").into_bytes());
        }
        drop(saver);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"n\": 49}", "the newest save is on disk");
        assert!(!dir.join("progress.json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reports_a_failing_save_once() {
        let dir = std::env::temp_dir().join(format!("afterglow-unwritable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the data folder should be: nothing can be saved in it.
        let blocker = dir.join("data");
        std::fs::write(&blocker, b"").unwrap();
        let mut store = Store { saver: Some(Saver::new(blocker.join("progress.json"))), ..Store::default() };
        let write = |store: &Store| store.saver.as_ref().unwrap().write(b"{}".to_vec());
        // Saves happen in the background: collect what they report until `done`.
        let collect = |store: &mut Store, done: &dyn Fn(&Store) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut problems = Vec::new();
            while !done(store) && std::time::Instant::now() < deadline {
                problems.extend(store.save_problem());
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            problems
        };
        write(&store);
        let problems = collect(&mut store, &|s| s.save_failing);
        assert_eq!(problems.len(), 1, "the failure is reported");
        assert!(problems[0].contains(&blocker.display().to_string()), "{}", problems[0]);

        // More failures, then a save that works: results come in order, so
        // once it's seen, the failures before it have been too.
        write(&store);
        write(&store);
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::remove_file(&blocker).unwrap();
        write(&store);
        assert!(collect(&mut store, &|s| !s.save_failing).is_empty(), "a lasting problem is reported once");
        assert!(!store.save_failing, "saving works again");

        // After a save that worked, the next failure is reported again.
        std::fs::remove_dir_all(&blocker).unwrap();
        std::fs::write(&blocker, b"").unwrap();
        write(&store);
        assert_eq!(collect(&mut store, &|s| s.save_failing).len(), 1);
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uses_the_watched_threshold_setting() {
        let mut store = Store::default();
        let video = Path::new("a.mkv");
        store.update_position(video, 60.0, 100.0);
        assert!(!store.get(video).watched);
        assert_eq!(store.resume_position(video), Some(60.0));
        store.settings.watched_percent = 50.0;
        assert_eq!(store.resume_position(video), None, "past the threshold: nothing to resume");
        store.update_position(video, 61.0, 100.0);
        assert!(store.get(video).watched);
    }

    #[test]
    fn keeps_stats_apart_from_progress() {
        let mut store = Store::default();
        let (a, b) = (Path::new(r"D:\Shows\Kirby\1.mkv"), Path::new(r"D:\Shows\Kirby\2.mkv"));
        store.update_position(a, 1000.0, 1200.0);
        store.seed_stats();
        assert_eq!(store.stats.before, Tally { seconds: 1000.0, finished: 0 }, "estimated from saved progress");

        store.record_watching(a, 150.0);
        store.update_position(a, 1150.0, 1200.0);
        store.update_position(a, 1190.0, 1200.0);
        store.finish(a);
        assert_eq!(store.stats.total.finished, 1, "counted once, when it crossed the threshold");
        store.set_watched(b, true);
        assert_eq!(store.stats.total.finished, 1, "marking by hand doesn't count");
        store.finish(b);
        assert_eq!(store.stats.total.finished, 1);

        store.forget(&[key(a), key(b)]);
        assert_eq!(store.stats.total, Tally { seconds: 1150.0, finished: 1 }, "forgetting deleted videos keeps their stats");
        assert_eq!(store.stats.series["Kirby"].seconds, 1150.0);

        store.update_position(a, 600.0, 1200.0);
        store.clear_stats();
        store.seed_stats();
        assert_eq!(store.stats.total, Tally::default(), "cleared stats aren't estimated again from saved progress");
        assert!(store.stats.series.is_empty() && store.stats.days.is_empty());
    }

    #[test]
    fn keeps_titles_worth_showing() {
        let mut store = Store::default();
        let (a, b, c) = (Path::new("a.mkv"), Path::new("b.mkv"), Path::new("c.mkv"));
        assert!(!store.title_known(a));
        store.set_title(a, Some(" Spirited Away ".into()));
        store.set_title(b, Some("Encoded by www.example.org".into()));
        store.set_title(c, None);
        assert_eq!(store.title(a), Some("Spirited Away"));
        assert_eq!((store.title(b), store.title(c)), (None, None), "a web address or nothing counts as no title");
        assert!(store.title_known(b) && store.title_known(c), "but they were read");
        assert_eq!(store.forget(&[key(c)]), 1);
        assert!(!store.title_known(c));
    }

    #[test]
    fn tells_movies_from_videos_in_a_folder() {
        let mut store = Store::default();
        let (movie, other) = (Path::new(r"D:\Movies\Up.mkv"), Path::new(r"D:\Movies\Cars.mkv"));
        store.set_playlist(&[movie.to_path_buf()]);
        assert!(store.is_movie(movie) && !store.is_movie(other));
        store.set_folder(Some(Path::new(r"D:\Movies")));
        assert!(!store.is_movie(movie), "opened from a folder");
        store.set_folder(None);
        store.set_playlist(&[movie.to_path_buf(), other.to_path_buf()]);
        assert!(!store.is_movie(movie), "one of several");
    }

    #[test]
    fn counts_movies_apart_from_series() {
        let mut store = Store { stats: Stats { seeded: true, ..Stats::default() }, ..Store::default() };
        let (movie, episode) = (Path::new(r"D:\Movies\Up.mkv"), Path::new(r"D:\Shows\Kirby\1.mkv"));
        store.set_title(movie, Some("Up (2009)".into()));
        store.set_playlist(&[movie.to_path_buf()]);
        store.record_watching(movie, 5000.0);
        store.finish(movie);
        store.set_playlist(&[episode.to_path_buf()]);
        store.set_folder(Some(Path::new(r"D:\Shows\Kirby")));
        store.record_watching(episode, 1200.0);
        store.finish(episode);

        let s = store.stats.summary(crate::clock::today());
        assert_eq!(s.total, Tally { seconds: 6200.0, finished: 2 }, "the totals count both");
        assert_eq!((s.videos_finished, s.movies_watched, s.movie_seconds), (1, 1, 5000.0));
        assert_eq!(s.top_series.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["Kirby"], "movies aren't a series");
        assert_eq!(s.recent_movies[0].0, "Up (2009)", "by its title");
    }

    #[test]
    fn a_movie_has_settings_of_its_own() {
        let mut store = Store::default();
        let (movie, other) = (Path::new(r"D:\Movies\Up.mkv"), Path::new(r"D:\Movies\Cars.mkv"));
        let folder = FolderSettings { speed: Some(1.5), ..FolderSettings::default() };
        store.set_folder_settings(other, folder.clone());
        store.set_playlist(&[movie.to_path_buf()]);
        assert!(store.folder_settings(movie).is_none(), "not its folder's");
        let own = FolderSettings { opening_end: Some(30.0), include_subfolders: true, ..FolderSettings::default() };
        store.set_folder_settings(movie, own);
        let (owner, settings) = store.folder_settings(movie).unwrap();
        assert_eq!((owner, settings.opening_end, settings.include_subfolders), (movie, Some(30.0), false));
        assert_eq!(store.folders[r"D:\Movies"], folder, "the folder's are left alone");
        // Opened with its folder, it goes by the folder's again.
        store.set_folder(Some(Path::new(r"D:\Movies")));
        assert_eq!(store.folder_settings(movie).map(|(_, s)| s.speed), Some(Some(1.5)));
        store.set_folder(None);
        store.set_folder_settings(movie, FolderSettings::default());
        assert!(store.videos.is_empty(), "reset: nothing kept");
    }

    #[test]
    fn remembers_recent_folders() {
        let mut store = Store::default();
        for i in 0..12 {
            store.set_folder(Some(Path::new(&format!("folder {i}"))));
        }
        store.set_folder(Some(Path::new("folder 5")));
        store.set_folder(None);
        let names: Vec<_> = store.recent_folders.iter().map(|f| f.to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), RECENT_FOLDERS);
        assert_eq!(names[..3], ["folder 5", "folder 11", "folder 10"], "most recent first, without duplicates");
        assert!(!names.contains(&"folder 0".to_string()), "the oldest are forgotten");

        for i in 0..12 {
            store.add_recent_video(Path::new(&format!("video {i}.mkv")));
        }
        store.add_recent_video(Path::new("video 3.mkv"));
        assert_eq!(store.recent_videos.len(), RECENT_VIDEOS);
        assert_eq!(store.recent_videos[..2], [PathBuf::from("video 3.mkv"), PathBuf::from("video 11.mkv")]);
    }

    #[test]
    fn saves_and_sets_aside_damaged_files() {
        let dir = std::env::temp_dir().join(format!("afterglow-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("progress.json");

        let mut store = Store::default();
        store.update_position(Path::new("a.mkv"), 60.0, 600.0);
        let prefs = store.tracks_mut(Path::new("a.mkv"));
        prefs.subtitle = Some("3".into());
        prefs.sub_delay = -1.5;
        prefs.sub_files.push(PathBuf::from("a.en.srt"));
        store.save_to(&path);
        let (loaded, warning) = Store::load_from(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.get(Path::new("a.mkv")).position, 60.0);
        let prefs = loaded.tracks(Path::new("a.mkv")).expect("track choices are saved");
        assert_eq!((prefs.subtitle.as_deref(), prefs.audio.as_deref(), prefs.sub_delay), (Some("3"), None, -1.5));
        assert_eq!(prefs.sub_files, [PathBuf::from("a.en.srt")]);
        assert!(!dir.join("progress.json.tmp").exists());

        let mut with_bom = BOM.to_vec();
        with_bom.extend(std::fs::read(&path).unwrap());
        std::fs::write(&path, with_bom).unwrap();
        let (loaded, warning) = Store::load_from(&path);
        assert!(warning.is_none(), "a byte order mark is fine");
        assert_eq!(loaded.get(Path::new("a.mkv")).position, 60.0);

        std::fs::write(&path, "{ not json").unwrap();
        let (loaded, warning) = Store::load_from(&path);
        assert!(warning.is_some_and(|w| w.contains("progress.bad.json")));
        assert!(loaded.entries.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("progress.bad.json")).unwrap(), "{ not json");
        assert!(!path.exists());

        std::fs::write(&path, b"{\"volume\": \xff}").unwrap();
        let (_, warning) = Store::load_from(&path);
        assert!(warning.is_some_and(|w| w.contains("damaged")), "text that isn't UTF-8 is damaged too");
        assert!(!path.exists());

        let (_, warning) = Store::load_from(&dir.join("missing.json"));
        assert!(warning.is_none(), "a missing file is a fresh start, not an error");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn never_saves_over_a_file_it_could_not_read() {
        // A folder where the file should be can't be read as one.
        let dir = std::env::temp_dir().join(format!("afterglow-unreadable-{}", std::process::id()));
        let path = dir.join("progress.json");
        std::fs::create_dir_all(&path).unwrap();
        let (mut store, warning) = Store::load_from(&path);
        assert!(warning.is_some_and(|w| w.contains("nothing will be saved")));
        assert!(path.is_dir(), "left as it is, not set aside");
        store.update_position(Path::new("a.mkv"), 60.0, 600.0);
        store.save();
        assert!(store.dirty && store.saver.is_none(), "nothing was saved");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_settings_apply_to_their_folder_and_opted_in_subfolders() {
        let mut store = Store::default();
        let s1 = Path::new("D:\\Shows\\Kirby\\Season 1\\01.mkv");
        let s2 = Path::new("D:\\Shows\\Kirby\\Season 2\\01.mkv");
        let opening_end = Some(100.0);
        store.set_folder_settings(s1, FolderSettings { opening_end, ..FolderSettings::default() });
        assert_eq!(store.folder_settings(s1).map(|(f, s)| (f, s.opening_end)), Some((Path::new("D:\\Shows\\Kirby\\Season 1"), opening_end)));
        assert!(store.folder_settings(s2).is_none(), "not used for other folders by itself");

        // Also used for other folders in Kirby: moved up a level.
        store.set_folder_settings(s1, FolderSettings { include_subfolders: true, opening_end, ..FolderSettings::default() });
        assert_eq!(store.folders.keys().collect::<Vec<_>>(), ["D:\\Shows\\Kirby"]);
        assert_eq!(store.folder_settings(s2).map(|(_, s)| s.opening_end), Some(opening_end));
        assert_eq!(store.folder_settings(Path::new("D:\\Shows\\Kirby\\OVA.mkv")).map(|(_, s)| s.opening_end), Some(opening_end));
        assert!(store.folder_settings(Path::new("D:\\Shows\\Kirby\\Season 1\\Extras\\a.mkv")).is_none(), "one level only");

        // A folder with settings of its own uses only those.
        let s3 = Path::new("D:\\Shows\\Kirby\\Season 3\\01.mkv");
        store.folders.insert("D:\\Shows\\Kirby\\Season 3".into(), FolderSettings { speed: Some(1.5), ..FolderSettings::default() });
        assert_eq!(store.folder_settings(s3).map(|(_, s)| s.opening_end), Some(None));

        // Turned off again: back to Season 2 only. Reset: nothing saved.
        store.set_folder_settings(s2, FolderSettings { opening_end, ..FolderSettings::default() });
        assert!(store.folder_settings(s1).is_none());
        assert_eq!(store.folder_settings(s2).map(|(_, s)| s.opening_end), Some(opening_end));
        store.set_folder_settings(s2, FolderSettings::default());
        assert!(store.folder_settings(s2).is_none() && !store.folders.contains_key("D:\\Shows\\Kirby\\Season 2"));
    }

    #[test]
    fn folder_settings_take_the_place_of_global_ones() {
        let mut store = Store::default();
        let video = Path::new("D:\\Shows\\Kirby\\01.mkv");
        let other = Path::new("D:\\Movies\\film.mkv");
        let folder = FolderSettings {
            sub_languages: Some("ja".into()),
            watched_percent: Some(60.0),
            autoplay_next: Some(false),
            ..FolderSettings::default()
        };
        store.set_folder_settings(video, folder);
        assert_eq!(store.settings_for(Some(video)).sub_languages, "ja");
        assert_eq!(store.settings_for(Some(other)).sub_languages, "en,eng");
        assert_eq!((store.watched_fraction(video), store.watched_fraction(other)), (0.6, 0.9));
        assert!(!store.autoplay_after(video) && store.autoplay_after(other));
        store.update_position(video, 61.0, 100.0);
        assert!(store.get(video).watched, "watched at the folder's 60%");
    }

    #[test]
    fn finds_the_opening_and_ending_in_a_video() {
        let folder = FolderSettings {
            opening_end: Some(120.0),
            ending_start: Some(100.0),
            ..FolderSettings::default()
        };
        assert_eq!(folder.skips(1400.0), [(Skip::Opening, 0.0, 120.0), (Skip::Ending, 1300.0, 1400.0)]);
        // A video too short for them gets neither.
        assert_eq!(folder.skips(60.0), []);
        assert_eq!(FolderSettings::default().skips(1400.0), []);
    }

    #[test]
    fn loads_files_saved_before_folder_settings() {
        let store: Store = serde_json::from_slice(br#"{"volume": 80.0, "playlist": []}"#).unwrap();
        assert!(store.folders.is_empty());
    }

    #[test]
    fn moves_the_progress_file() {
        let root = std::env::temp_dir().join(format!("afterglow-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (from, to) = (root.join("from"), root.join("to"));
        let mut store = Store { volume: 42.0, saver: Some(Saver::new(from.join(FILE_NAME))), ..Store::default() };
        store.dirty = true;
        store.save();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join(FILE_NAME), b"older").unwrap();
        store.move_to(&to).unwrap();
        assert_eq!(store.folder().as_deref(), Some(to.as_path()));
        assert!(!from.join(FILE_NAME).exists(), "gone from where it was");
        assert_eq!(std::fs::read(to.join("progress.bak.json")).unwrap(), b"older");
        let (moved, _) = Store::load_from(&to.join(FILE_NAME));
        assert_eq!(moved.volume, 42.0);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
