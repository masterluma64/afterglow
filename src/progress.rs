//! Persistent state: watch progress, playlists and loose videos, stored as
//! JSON in the user's config folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Fraction of a video after which it counts as watched.
const WATCHED_FRACTION: f64 = 0.9;
/// How many individually opened videos are remembered.
const MAX_VIDEOS: usize = 50;

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
    /// Folders added as playlists, in the order they were added.
    pub playlists: Vec<PathBuf>,
    /// Videos opened on their own, most recent first.
    pub videos: Vec<PathBuf>,
    /// Last played episode per playlist folder.
    pub last_episode: HashMap<String, PathBuf>,
    /// The most recently played video, in a playlist or not.
    pub last_played: Option<PathBuf>,
    pub volume: f64,
    /// Folders scanned for music.
    pub music_folders: Vec<PathBuf>,
    pub visualizer: crate::music::Visualizer,
    /// Whether the music view was open last.
    pub music_mode: bool,
    /// Whether the visualizer moves to the next preset by itself.
    pub viz_auto: bool,
    #[serde(skip)]
    dirty: bool,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            playlists: Vec::new(),
            videos: Vec::new(),
            last_episode: HashMap::new(),
            last_played: None,
            volume: 100.0,
            music_folders: Vec::new(),
            visualizer: Default::default(),
            music_mode: false,
            viz_auto: true,
            dirty: false,
        }
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The app's folder in the user's config directory. Data from before the
/// rename to Afterglow is moved over the first time.
pub fn data_dir() -> Option<PathBuf> {
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
    pub fn load() -> Self {
        file_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = file_path() else { return };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(tmp, &path)
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

    pub fn add_playlist(&mut self, folder: &Path) {
        if !self.playlists.iter().any(|p| p == folder) {
            self.playlists.push(folder.to_path_buf());
            self.dirty = true;
        }
    }

    /// Forgets a playlist. Progress of its episodes is kept, so re-adding the
    /// folder later restores it.
    pub fn remove_playlist(&mut self, folder: &Path) {
        self.playlists.retain(|p| p != folder);
        self.last_episode.remove(&key(folder));
        self.dirty = true;
    }

    /// Remembers an individually opened video, moving it to the top.
    pub fn add_video(&mut self, path: &Path) {
        self.videos.retain(|p| p != path);
        self.videos.insert(0, path.to_path_buf());
        self.videos.truncate(MAX_VIDEOS);
        self.dirty = true;
    }

    pub fn remove_video(&mut self, path: &Path) {
        self.videos.retain(|p| p != path);
        self.dirty = true;
    }

    pub fn set_played(&mut self, path: &Path, playlist: Option<&Path>) {
        if let Some(folder) = playlist {
            self.last_episode.insert(key(folder), path.to_path_buf());
        }
        self.last_played = Some(path.to_path_buf());
        self.dirty = true;
    }

    pub fn last_episode(&self, folder: &Path) -> Option<&PathBuf> {
        self.last_episode.get(&key(folder))
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume;
        self.dirty = true;
    }
}

impl Store {
    pub fn add_music_folder(&mut self, folder: &Path) -> bool {
        if self.music_folders.iter().any(|p| p == folder) {
            return false;
        }
        self.music_folders.push(folder.to_path_buf());
        self.dirty = true;
        true
    }

    pub fn remove_music_folder(&mut self, folder: &Path) {
        self.music_folders.retain(|p| p != folder);
        self.dirty = true;
    }

    pub fn set_visualizer(&mut self, visualizer: crate::music::Visualizer) {
        self.visualizer = visualizer;
        self.dirty = true;
    }

    pub fn set_music_mode(&mut self, music: bool) {
        self.music_mode = music;
        self.dirty = true;
    }
}

impl Store {
    pub fn set_viz_auto(&mut self, auto: bool) {
        self.viz_auto = auto;
        self.dirty = true;
    }
}
