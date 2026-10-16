//! Scans a show folder: every subfolder with videos is a season.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
];

pub struct Episode {
    pub path: PathBuf,
    pub name: String,
}

pub struct Season {
    pub name: String,
    pub episodes: Vec<Episode>,
}

pub struct Library {
    pub root: PathBuf,
    pub name: String,
    pub seasons: Vec<Season>,
}

/// Position of an episode inside a library.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EpisodeId {
    pub season: usize,
    pub episode: usize,
}

impl Library {
    pub fn scan(root: &Path) -> std::io::Result<Self> {
        let mut dirs = Vec::new();
        let mut loose = Vec::new();
        for entry in std::fs::read_dir(root)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if is_video(&path) {
                loose.push(path);
            }
        }
        dirs.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));

        let mut seasons: Vec<Season> = dirs
            .iter()
            .filter_map(|dir| {
                let videos: Vec<PathBuf> = std::fs::read_dir(dir)
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && is_video(p))
                    .collect();
                (!videos.is_empty()).then(|| Season { name: file_name(dir), episodes: episodes(videos) })
            })
            .collect();

        let name = file_name(root);
        if !loose.is_empty() {
            let season_name = if seasons.is_empty() { name.clone() } else { "Other".into() };
            seasons.push(Season { name: season_name, episodes: episodes(loose) });
        }

        Ok(Self { root: root.to_path_buf(), name, seasons })
    }

    /// A playlist whose folder could not be read.
    pub fn empty(root: &Path) -> Self {
        Self { root: root.to_path_buf(), name: file_name(root), seasons: Vec::new() }
    }

    pub fn get(&self, id: EpisodeId) -> Option<&Episode> {
        self.seasons.get(id.season)?.episodes.get(id.episode)
    }

    pub fn find(&self, path: &Path) -> Option<EpisodeId> {
        self.ids().find(|&id| self.get(id).is_some_and(|e| e.path == path))
    }

    pub fn ids(&self) -> impl Iterator<Item = EpisodeId> + '_ {
        self.seasons.iter().enumerate().flat_map(|(s, season)| {
            (0..season.episodes.len()).map(move |e| EpisodeId { season: s, episode: e })
        })
    }

    pub fn next(&self, id: EpisodeId) -> Option<EpisodeId> {
        self.ids().skip_while(|&i| i != id).nth(1)
    }

    pub fn prev(&self, id: EpisodeId) -> Option<EpisodeId> {
        self.ids().take_while(|&i| i != id).last()
    }
}

fn episodes(mut paths: Vec<PathBuf>) -> Vec<Episode> {
    paths.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            Episode { path, name }
        })
        .collect()
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Compares strings so that "Episode 2" sorts before "Episode 10".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
                        s.push(c);
                    }
                    s
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("Episode 2", "Episode 10"), Ordering::Less);
        assert_eq!(natural_cmp("S01E09", "s01e10"), Ordering::Less);
        assert_eq!(natural_cmp("Season 010", "Season 9"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
    }
}
