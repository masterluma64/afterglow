//! Persistent state: watch progress per video and the playlist, stored as
//! JSON in the user's config folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

/// How many recently opened folders are remembered.
const RECENT_FOLDERS: usize = 10;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Entry {
    pub position: f64,
    pub duration: f64,
    pub watched: bool,
    /// The file's size in bytes, to recognize it after it is moved or renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

impl Entry {
    /// Whether it has been (partly) watched.
    fn has_progress(&self) -> bool {
        self.watched || self.position > 0.0
    }

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

/// Track choices made for one video, restored when it is opened again.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackPrefs {
    /// mpv's `aid` / `sid` values ("no" for off). `None` keeps mpv's choice.
    pub audio: Option<String>,
    pub subtitle: Option<String>,
    /// Seconds subtitles are shifted by.
    pub sub_delay: f64,
    /// Seconds the sound is shifted by (positive: later).
    pub audio_delay: f64,
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
    /// The playlist, in play order.
    pub playlist: Vec<PathBuf>,
    /// The playlist entry that was playing last.
    pub current: Option<PathBuf>,
    /// The folder the playlist was opened from, if any.
    pub folder: Option<PathBuf>,
    /// Folders opened recently, most recent first.
    pub recent_folders: Vec<PathBuf>,
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
    #[serde(skip)]
    dirty: bool,
    /// Set when the saved file exists but couldn't be read: saving would
    /// replace it with this empty state, so nothing is saved.
    #[serde(skip)]
    read_failed: bool,
    #[serde(skip)]
    saver: Option<Saver>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            tracks: HashMap::new(),
            playlist: Vec::new(),
            current: None,
            folder: None,
            recent_folders: Vec::new(),
            include_subfolders: false,
            volume: 100.0,
            autoplay_next: true,
            sidebar_pinned: true,
            window: None,
            settings: Settings::default(),
            dirty: false,
            read_failed: false,
            saver: None,
        }
    }
}

/// The UTF-8 byte order mark.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Where the app keeps its data: a `data` folder next to the executable, so
/// the app is portable (or `AFTERGLOW_DATA_DIR`, e.g. for testing).
pub fn data_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("AFTERGLOW_DATA_DIR") {
            return Some(PathBuf::from(dir));
        }
        let dir = std::env::current_exe().ok()?.parent()?.join("data");
        copy_from_appdata(&dir);
        Some(dir)
    })
    .clone()
}

/// Earlier versions kept their data in the user's AppData folder (first as
/// "minimal-video-player"). Copies it next to the executable the first time,
/// leaving the original in place.
fn copy_from_appdata(dir: &Path) {
    let target = dir.join("progress.json");
    if target.exists() {
        return;
    }
    let Some(config) = dirs::config_dir() else { return };
    let old = ["Afterglow", "minimal-video-player"].iter().map(|name| config.join(name).join("progress.json")).find(|p| p.is_file());
    if let Some(old) = old {
        let copied = std::fs::create_dir_all(dir).and_then(|()| std::fs::copy(&old, &target));
        if let Err(e) = copied {
            eprintln!("could not copy {} to {}: {e}", old.display(), target.display());
        }
    }
}

fn file_path() -> Option<PathBuf> {
    Some(data_dir()?.join("progress.json"))
}

impl Store {
    /// Loads the saved state. A damaged file is set aside as
    /// `progress.bad.json` rather than overwritten, and a warning is returned.
    /// A file that can't be read at all (e.g. locked by another program) is
    /// left alone, and nothing is saved until the next start.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = file_path() else { return (Self::default(), None) };
        Self::load_from(&path)
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

    #[cfg(test)]
    fn save_to(&mut self, path: &Path) {
        let bytes = serde_json::to_vec_pretty(self).unwrap();
        write_atomically(path, &bytes).unwrap();
        self.dirty = false;
    }

    pub fn get(&self, path: &Path) -> Entry {
        self.entries.get(&key(path)).copied().unwrap_or_default()
    }

    pub fn tracks(&self, path: &Path) -> Option<&TrackPrefs> {
        self.tracks.get(&key(path))
    }

    pub fn tracks_mut(&mut self, path: &Path) -> &mut TrackPrefs {
        self.dirty = true;
        self.tracks.entry(key(path)).or_default()
    }

    /// Fraction of a video after which it counts as watched.
    pub fn watched_fraction(&self) -> f64 {
        self.settings.watched_percent.clamp(50.0, 100.0) / 100.0
    }

    /// Where a video should resume, if anywhere.
    pub fn resume_position(&self, path: &Path) -> Option<f64> {
        self.get(path).resume_position(self.watched_fraction())
    }

    pub fn update_position(&mut self, path: &Path, position: f64, duration: f64) {
        let watched_fraction = self.watched_fraction();
        let entry = self.entries.entry(key(path)).or_default();
        entry.position = position;
        entry.duration = duration;
        if duration > 0.0 && position >= duration * watched_fraction {
            entry.watched = true;
        }
        self.dirty = true;
    }

    pub fn set_watched(&mut self, path: &Path, watched: bool) {
        let entry = self.entries.entry(key(path)).or_default();
        entry.watched = watched;
        entry.position = if watched { entry.duration } else { 0.0 };
        self.dirty = true;
    }

    /// Forgets the position and watched state of every video, returning how
    /// many had any. Known lengths are kept, since they describe the files.
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

    /// Records a video file's size, by which it is recognized if it is moved
    /// or renamed.
    pub fn set_size(&mut self, path: &Path, size: u64) {
        let entry = self.entries.entry(key(path)).or_default();
        if entry.size != Some(size) {
            entry.size = Some(size);
            self.dirty = true;
        }
    }

    /// Carries progress over to videos that were moved or renamed. Each of
    /// `paths` without progress of its own takes that of a remembered video
    /// with exactly the same size whose file is gone, if just one such video
    /// and one such path have that size. Track choices go along. Also records
    /// the sizes of `paths`. Returns the recognized videos' old and new paths.
    pub fn adopt_moved(&mut self, paths: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
        let mut unknown: HashMap<u64, Vec<&PathBuf>> = HashMap::new();
        for path in paths {
            let entry = self.entries.get(&key(path)).copied();
            // A size recorded for this path means the file was seen here before.
            if entry.is_some_and(|e| e.size.is_some()) {
                continue;
            }
            let known = entry.is_some_and(|e| e.has_progress()) || self.tracks.contains_key(&key(path));
            let Ok(size) = std::fs::metadata(path).map(|m| m.len()) else { continue };
            self.set_size(path, size);
            if !known {
                unknown.entry(size).or_default().push(path);
            }
        }
        if unknown.is_empty() {
            return Vec::new();
        }
        let mut remembered: HashMap<u64, Vec<String>> = HashMap::new();
        for (k, entry) in &self.entries {
            let worth_keeping = entry.has_progress() || self.tracks.contains_key(k);
            if let Some(size) = entry.size.filter(|s| worth_keeping && unknown.contains_key(s)) {
                remembered.entry(size).or_default().push(k.clone());
            }
        }
        let mut adopted = Vec::new();
        for (size, keys) in remembered {
            let [path] = unknown[&size][..] else { continue };
            let gone: Vec<String> = keys.into_iter().filter(|k| !Path::new(k).exists()).collect();
            let [old] = &gone[..] else { continue };
            if let Some(entry) = self.entries.remove(old) {
                self.entries.insert(key(path), entry);
            }
            if let Some(tracks) = self.tracks.remove(old) {
                self.tracks.insert(key(path), tracks);
            }
            adopted.push((PathBuf::from(old), path.clone()));
        }
        if !adopted.is_empty() {
            self.dirty = true;
        }
        adopted
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
    queue: Option<Sender<Vec<u8>>>,
    thread: Option<JoinHandle<()>>,
}

impl Saver {
    fn new(path: PathBuf) -> Self {
        let (queue, saves) = mpsc::channel::<Vec<u8>>();
        let thread = std::thread::Builder::new()
            .name("save".into())
            .spawn(move || {
                while let Ok(mut bytes) = saves.recv() {
                    while let Ok(newer) = saves.try_recv() {
                        bytes = newer;
                    }
                    if let Err(e) = write_atomically(&path, &bytes) {
                        eprintln!("failed to save progress: {e}");
                    }
                }
            })
            .map_err(|e| eprintln!("could not start saving in the background: {e}"))
            .ok();
        Self { queue: Some(queue), thread }
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
    fn recognizes_moved_and_renamed_videos() {
        let dir = std::env::temp_dir().join(format!("afterglow-moved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, size: usize| {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; size]).unwrap();
            path
        };
        let mut store = Store::default();
        let gone = dir.join("old name.mkv");
        store.update_position(&gone, 60.0, 600.0);
        store.set_size(&gone, 10);
        store.tracks_mut(&gone).subtitle = Some("2".into());
        let still_there = file("still there.mkv", 20);
        store.set_watched(&still_there, true);
        store.set_size(&still_there, 20);
        // Two gone videos of the same size: can't tell which is which.
        for name in ["twin 1.mkv", "twin 2.mkv"] {
            store.set_watched(&dir.join(name), true);
            store.set_size(&dir.join(name), 30);
        }

        let renamed = file("new name.mkv", 10);
        let copy = file("copy.mkv", 20);
        let seen = file("seen before.mkv", 10);
        store.set_size(&seen, 10);
        let twin = file("twin.mkv", 30);
        let adopted = store.adopt_moved(&[renamed.clone(), copy.clone(), twin.clone(), seen.clone()]);
        assert_eq!(adopted, [(gone.clone(), renamed.clone())], "a video seen at its path before is no candidate");
        assert_eq!(store.get(&renamed).position, 60.0);
        assert_eq!(store.tracks(&renamed).and_then(|t| t.subtitle.as_deref()), Some("2"));
        assert!(store.entries.get(&key(&gone)).is_none());
        assert!(!store.get(&copy).watched, "a video whose file still exists keeps its progress to itself");
        assert!(!store.get(&twin).watched, "ambiguous matches are left alone");
        assert_eq!(store.get(&copy).size, Some(20), "sizes of new videos are recorded");
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
}
