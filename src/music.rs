//! Music library: recursively scans folders for audio files, reads their tags
//! (cached on disk by modification time) and groups them into albums.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::UNIX_EPOCH;

use lofty::config::ParseOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "oga", "opus", "wav", "wma", "aiff", "aif", "ape", "wv", "mpc",
];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub duration: f64,
    pub modified: u64,
}

pub struct Album {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    /// Track indices, in disc/track order.
    pub tracks: Vec<usize>,
    /// Newest modification time of its tracks, for "recently added".
    pub added: u64,
}

impl Album {
    pub fn duration(&self, lib: &MusicLibrary) -> f64 {
        self.tracks.iter().map(|&t| lib.tracks[t].duration).sum()
    }
}

#[derive(Default)]
pub struct MusicLibrary {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    /// Artists with their number of tracks, sorted by name.
    pub artists: Vec<(String, usize)>,
    by_path: HashMap<PathBuf, usize>,
    album_by_key: HashMap<String, usize>,
}

impl MusicLibrary {
    pub fn new(mut tracks: Vec<Track>) -> Self {
        tracks.sort_by(|a, b| {
            natural_lower(&a.album_artist)
                .cmp(&natural_lower(&b.album_artist))
                .then_with(|| natural_lower(&a.album).cmp(&natural_lower(&b.album)))
                .then_with(|| a.disc.unwrap_or(1).cmp(&b.disc.unwrap_or(1)))
                .then_with(|| a.track.unwrap_or(u32::MAX).cmp(&b.track.unwrap_or(u32::MAX)))
                .then_with(|| a.path.cmp(&b.path))
        });

        let mut albums: Vec<Album> = Vec::new();
        let mut album_by_key = HashMap::new();
        let mut artist_counts: HashMap<&str, usize> = HashMap::new();
        for (i, t) in tracks.iter().enumerate() {
            let key = album_key(&t.album_artist, &t.album);
            let index = *album_by_key.entry(key.clone()).or_insert_with(|| {
                albums.push(Album {
                    key,
                    title: t.album.clone(),
                    artist: t.album_artist.clone(),
                    year: None,
                    tracks: Vec::new(),
                    added: 0,
                });
                albums.len() - 1
            });
            let album = &mut albums[index];
            album.tracks.push(i);
            album.year = album.year.or(t.year);
            album.added = album.added.max(t.modified);
            *artist_counts.entry(&t.artist).or_default() += 1;
            if t.album_artist != t.artist {
                *artist_counts.entry(&t.album_artist).or_default() += 1;
            }
        }

        let mut artists: Vec<(String, usize)> = artist_counts.into_iter().map(|(a, n)| (a.to_string(), n)).collect();
        artists.sort_by_key(|(a, _)| natural_lower(a));
        let by_path = tracks.iter().enumerate().map(|(i, t)| (t.path.clone(), i)).collect();
        Self { tracks, albums, artists, by_path, album_by_key }
    }

    pub fn track_index(&self, path: &Path) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    pub fn album(&self, key: &str) -> Option<&Album> {
        self.album_by_key.get(key).map(|&i| &self.albums[i])
    }

    pub fn album_of(&self, track: &Track) -> Option<&Album> {
        self.album(&album_key(&track.album_artist, &track.album))
    }
}

pub fn album_key(artist: &str, album: &str) -> String {
    format!("{}\u{1f}{}", artist.to_lowercase(), album.to_lowercase())
}

fn natural_lower(s: &str) -> String {
    let lower = s.to_lowercase();
    lower.strip_prefix("the ").map(str::to_string).unwrap_or(lower)
}

// ------------------------------------------------------------------ scanning

fn cache_path() -> Option<PathBuf> {
    Some(crate::progress::data_dir()?.join("music-cache.json"))
}

/// Scans `folders` on a background thread. Unchanged files reuse cached tags.
pub fn scan_in_background(folders: Vec<PathBuf>, ctx: eframe::egui::Context) -> Receiver<Vec<Track>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("music-scan".into())
        .spawn(move || {
            let cached: HashMap<PathBuf, Track> = cache_path()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| serde_json::from_slice::<Vec<Track>>(&b).ok())
                .map(|v| v.into_iter().map(|t| (t.path.clone(), t)).collect())
                .unwrap_or_default();

            let mut files = Vec::new();
            for folder in &folders {
                collect_audio(folder, &mut files);
            }
            files.sort();
            files.dedup();

            let tracks: Vec<Track> = files
                .into_iter()
                .filter_map(|path| {
                    let modified = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs());
                    match cached.get(&path) {
                        Some(t) if t.modified == modified => Some(t.clone()),
                        _ => Some(read_track(path, modified)),
                    }
                })
                .collect();

            if let Some(p) = cache_path() {
                let _ = std::fs::create_dir_all(p.parent().unwrap());
                if let Ok(bytes) = serde_json::to_vec(&tracks) {
                    let _ = std::fs::write(p, bytes);
                }
            }
            let _ = tx.send(tracks);
            ctx.request_repaint();
        })
        .expect("failed to spawn music scanner");
    rx
}

fn collect_audio(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_audio(&path, out);
        } else if is_audio(&path) {
            out.push(path);
        }
    }
}

fn read_track(path: PathBuf, modified: u64) -> Track {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut track = Track {
        title: stem,
        artist: "Unknown Artist".into(),
        album: folder,
        album_artist: String::new(),
        track: None,
        disc: None,
        year: None,
        duration: 0.0,
        modified,
        path,
    };

    let options = ParseOptions::new().read_cover_art(false);
    if let Ok(file) = Probe::open(&track.path).and_then(|p| p.options(options).read()) {
        track.duration = file.properties().duration().as_secs_f64();
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            let text = |s: Option<std::borrow::Cow<str>>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            if let Some(v) = text(tag.title()) {
                track.title = v;
            }
            if let Some(v) = text(tag.artist()) {
                track.artist = v;
            }
            if let Some(v) = text(tag.album()) {
                track.album = v;
            }
            if let Some(v) = tag.get_string(ItemKey::AlbumArtist).map(str::trim).filter(|s| !s.is_empty()) {
                track.album_artist = v.to_string();
            }
            track.track = tag.track();
            track.disc = tag.disk();
            track.year = tag.date().map(|d| d.year as u32).filter(|&y| y > 0);
        }
    }
    if track.album_artist.is_empty() {
        track.album_artist = track.artist.clone();
    }
    track
}

/// Reads the embedded front cover of a file, if it has one.
pub fn embedded_cover(path: &Path) -> Option<Vec<u8>> {
    let file = Probe::open(path).ok()?.read().ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pictures = tag.pictures();
    let front = pictures
        .iter()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| pictures.first())?;
    Some(front.data().to_vec())
}

/// Finds a cover image file next to the tracks (cover.jpg, folder.png, …).
pub fn folder_cover(dir: &Path) -> Option<PathBuf> {
    const NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart", "albumartsmall"];
    let images: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp" | "bmp"))
        })
        .collect();
    let stem = |p: &PathBuf| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    NAMES
        .iter()
        .find_map(|name| images.iter().find(|p| stem(p) == *name))
        .or_else(|| images.first())
        .cloned()
}

// ---------------------------------------------------------------- visualizer

/// Visualizer presets (see `viz.wgsl`), or just the album cover.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Visualizer {
    #[default]
    Nebula,
    Vortex,
    Aurora,
    Starburst,
    Liquid,
    Prism,
    Off,
    /// Names from older versions load as the default.
    #[serde(other)]
    Unknown,
}

impl Visualizer {
    /// Presets in shader order.
    pub const PRESETS: [Visualizer; 6] = [Self::Nebula, Self::Vortex, Self::Aurora, Self::Starburst, Self::Liquid, Self::Prism];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nebula | Self::Unknown => "Nebula",
            Self::Vortex => "Vortex",
            Self::Aurora => "Aurora",
            Self::Starburst => "Starburst",
            Self::Liquid => "Liquid",
            Self::Prism => "Prism",
            Self::Off => "Cover",
        }
    }

    /// Shader preset index, or `None` for the cover view.
    pub fn preset(self) -> Option<usize> {
        match self {
            Self::Off => None,
            Self::Unknown => Some(0),
            v => Self::PRESETS.iter().position(|&p| p == v),
        }
    }

    /// Next preset; cycling also passes through the cover view.
    pub fn next(self) -> Self {
        match self.preset() {
            Some(i) if i + 1 < Self::PRESETS.len() => Self::PRESETS[i + 1],
            Some(_) => Self::Off,
            None => Self::PRESETS[0],
        }
    }

    /// Next preset for auto-cycling, never the cover.
    pub fn next_preset(self) -> Self {
        let i = self.preset().map_or(0, |i| i + 1);
        Self::PRESETS[i % Self::PRESETS.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, title: &str, artist: &str, album: &str, album_artist: &str, n: u32) -> Track {
        Track {
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: album_artist.into(),
            track: Some(n),
            disc: None,
            year: Some(2020),
            duration: 60.0,
            modified: 0,
        }
    }

    #[test]
    fn groups_albums_and_artists() {
        let lib = MusicLibrary::new(vec![
            track("b2", "B2", "Band", "Second", "Band", 2),
            track("a1", "A1", "Band", "First", "Band", 1),
            track("b1", "B1", "Band feat. Guest", "Second", "Band", 1),
            track("c1", "C1", "The Abc", "Solo", "The Abc", 1),
        ]);
        assert_eq!(lib.albums.len(), 3);
        // "The Abc" sorts as "Abc", before "Band".
        assert_eq!(lib.albums[0].title, "Solo");
        let second = lib.album(&album_key("Band", "Second")).unwrap();
        let titles: Vec<&str> = second.tracks.iter().map(|&t| lib.tracks[t].title.as_str()).collect();
        assert_eq!(titles, ["B1", "B2"], "tracks follow track numbers");
        let artists: Vec<&str> = lib.artists.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(artists, ["The Abc", "Band", "Band feat. Guest"]);
        let b1 = &lib.tracks[lib.track_index(Path::new("b1")).unwrap()];
        assert_eq!(lib.album_of(b1).unwrap().title, "Second");
    }

    #[test]
    fn visualizer_cycles_and_loads_old_names() {
        let mut v = Visualizer::default();
        for _ in 0..=Visualizer::PRESETS.len() {
            v = v.next();
        }
        assert_eq!(v, Visualizer::default(), "cycles through every preset and the cover");
        assert_eq!(Visualizer::Prism.next_preset(), Visualizer::Nebula, "auto-cycle skips the cover");
        let old: Visualizer = serde_json::from_str("\"Spectrum\"").unwrap();
        assert_eq!(old.preset(), Some(0));
    }
}
