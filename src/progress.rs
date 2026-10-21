//! Persistent state: watch progress per video and the playlist, stored as
//! JSON in the user's config folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Fraction of a video after which it counts as watched.
const WATCHED_FRACTION: f64 = 0.9;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Entry {
    pub position: f64,
    pub duration: f64,
    pub watched: bool,
}

impl Entry {
    /// Where playback should resume, if anywhere.
    pub fn resume_position(&self) -> Option<f64> {
        (self.position > 5.0 && self.position < self.duration * WATCHED_FRACTION).then_some(self.position)
    }

    pub fn fraction(&self) -> f32 {
        if self.duration > 0.0 { (self.position / self.duration).clamp(0.0, 1.0) as f32 } else { 0.0 }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub entries: HashMap<String, Entry>,
    /// The playlist, in play order.
    pub playlist: Vec<PathBuf>,
    /// The playlist entry that was playing last.
    pub current: Option<PathBuf>,
    /// The folder the playlist was opened from, if any.
    pub folder: Option<PathBuf>,
    /// Whether opening a folder also includes videos in its subfolders.
    pub include_subfolders: bool,
    pub volume: f64,
    /// Whether the next video starts by itself when one finishes.
    pub autoplay_next: bool,
    #[serde(skip)]
    dirty: bool,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            playlist: Vec::new(),
            current: None,
            folder: None,
            include_subfolders: false,
            volume: 100.0,
            autoplay_next: true,
            dirty: false,
        }
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The app's folder in the user's config directory (or `AFTERGLOW_DATA_DIR`,
/// e.g. for testing). Data from before the rename to Afterglow is moved over
/// the first time.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AFTERGLOW_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let config = dirs::config_dir()?;
    let dir = config.join("Afterglow");
    let old = config.join("minimal-video-player");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    Some(dir)
}

fn file_path() -> Option<PathBuf> {
    Some(data_dir()?.join("progress.json"))
}

impl Store {
    /// Loads the saved state. A file that can't be read is set aside as
    /// `progress.bad.json` rather than overwritten, and a warning is returned.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = file_path() else { return (Self::default(), None) };
        Self::load_from(&path)
    }

    fn load_from(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => return (Self::default(), Some(format!("Could not read saved progress: {e}"))),
        };
        match serde_json::from_str(&text) {
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

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = file_path() else { return };
        self.save_to(&path);
    }

    /// Writes to a temporary file that replaces the old one only once it is
    /// fully on disk, so a crash mid-save never leaves a half-written file.
    fn save_to(&mut self, path: &Path) {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&tmp)?;
            std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(tmp, path)
        };
        match write() {
            Ok(()) => self.dirty = false,
            Err(e) => eprintln!("failed to save progress: {e}"),
        }
    }

    pub fn get(&self, path: &Path) -> Entry {
        self.entries.get(&key(path)).copied().unwrap_or_default()
    }

    pub fn update_position(&mut self, path: &Path, position: f64, duration: f64) {
        let entry = self.entries.entry(key(path)).or_default();
        entry.position = position;
        entry.duration = duration;
        if duration > 0.0 && position >= duration * WATCHED_FRACTION {
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

    pub fn set_autoplay_next(&mut self, autoplay: bool) {
        self.autoplay_next = autoplay;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn saves_and_sets_aside_damaged_files() {
        let dir = std::env::temp_dir().join(format!("afterglow-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("progress.json");

        let mut store = Store::default();
        store.update_position(Path::new("a.mkv"), 60.0, 600.0);
        store.save_to(&path);
        let (loaded, warning) = Store::load_from(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.get(Path::new("a.mkv")).position, 60.0);
        assert!(!dir.join("progress.json.tmp").exists());

        std::fs::write(&path, "{ not json").unwrap();
        let (loaded, warning) = Store::load_from(&path);
        assert!(warning.is_some_and(|w| w.contains("progress.bad.json")));
        assert!(loaded.entries.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("progress.bad.json")).unwrap(), "{ not json");
        assert!(!path.exists());

        let (_, warning) = Store::load_from(&dir.join("missing.json"));
        assert!(warning.is_none(), "a missing file is a fresh start, not an error");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
