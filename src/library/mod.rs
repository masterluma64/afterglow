//! The playlist, finding videos on disk, and what is remembered about them.

pub(crate) mod disk;
pub(crate) mod playlist;
pub(crate) mod progress;
pub(crate) mod stats;
pub(crate) mod watch;

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The videos in a folder, optionally including its subfolders, in natural
/// order of their paths ("Part 2" before "Part 10", and each subfolder's
/// videos kept together after the folder's own). A folder with no videos of
/// its own always includes its subfolders: it's a show with a folder per
/// season.
pub fn scan_folder(dir: &Path, include_subfolders: bool) -> std::io::Result<Vec<PathBuf>> {
    std::fs::read_dir(dir)?;
    let mut videos = Vec::new();
    collect(dir, include_subfolders, &mut HashSet::new(), &mut videos);
    if videos.is_empty() && !include_subfolders {
        collect(dir, true, &mut HashSet::new(), &mut videos);
    }
    sort_in_folder(dir, &mut videos);
    Ok(videos)
}

/// The folder a video is in, relative to the opened folder `dir`: its
/// season, when `dir` is a show with a folder per season. `None` for a
/// video outside `dir`, and an empty path for one directly in it.
pub fn season_of<'a>(dir: &Path, video: &'a Path) -> Option<&'a Path> {
    video.parent()?.strip_prefix(dir).ok()
}

/// Puts videos in a folder in the order opening it plays them: the folder's
/// own files first, then each subfolder's, in natural order.
pub fn sort_in_folder(dir: &Path, videos: &mut [PathBuf]) {
    let key = |p: &PathBuf| {
        let rel = p.strip_prefix(dir).unwrap_or(p);
        (rel.components().count() > 1, rel.to_string_lossy().replace('\\', "/"))
    };
    videos.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then_with(|| natural_cmp(&ka.1, &kb.1))
    });
}

/// `seen` holds the folders already scanned, so a link back up the tree
/// (a symbolic link or junction) is followed only once.
fn collect(dir: &Path, recursive: bool, seen: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    if let Ok(real) = std::fs::canonicalize(dir)
        && !seen.insert(real)
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // The entry's own type needs no extra disk access; only links are
        // looked up to see what they point to.
        let is_dir = match entry.file_type() {
            Ok(t) if t.is_symlink() => path.is_dir(),
            Ok(t) => t.is_dir(),
            Err(_) => continue,
        };
        if is_dir {
            if recursive {
                collect(&path, true, seen, out);
            }
        } else if is_video(&path) {
            out.push(path);
        }
    }
}

/// Compares strings so that "Part 2" sorts before "Part 10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
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
    use super::*;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("Part 2", "Part 10"), Ordering::Less);
        assert_eq!(natural_cmp("S01E09", "s01e10"), Ordering::Less);
        assert_eq!(natural_cmp("Season 010", "Season 9"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
    }

    #[test]
    fn scans_folders_with_and_without_subfolders() {
        let root = std::env::temp_dir().join(format!("afterglow-scan-{}", std::process::id()));
        let files = ["b 10.mkv", "b 2.mp4", "notes.txt", "Part 10/x.mkv", "Part 2/y.mkv", "Part 2/cover.jpg"];
        for f in files {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let names = |include| -> Vec<String> {
            scan_folder(&root, include)
                .unwrap()
                .iter()
                .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
                .collect()
        };
        let (flat, nested) = (names(false), names(true));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(flat, ["b 2.mp4", "b 10.mkv"]);
        assert_eq!(nested, ["b 2.mp4", "b 10.mkv", "Part 2/y.mkv", "Part 10/x.mkv"]);
    }

    /// A show with a folder per season opens with all of them, even with
    /// subfolders turned off.
    #[test]
    fn opens_seasons() {
        let root = std::env::temp_dir().join(format!("afterglow-seasons-{}", std::process::id()));
        for f in ["Season 10/01.mkv", "Season 2/02.mkv", "Season 2/01.mkv", "cover.jpg"] {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let found = scan_folder(&root, false).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        let names: Vec<String> = found.iter().map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, ["Season 2/01.mkv", "Season 2/02.mkv", "Season 10/01.mkv"]);
        assert_eq!(season_of(&root, &found[2]), Some(Path::new("Season 10")));
        assert_eq!(season_of(&root, &root.join("a.mkv")), Some(Path::new("")));
        assert_eq!(season_of(&root, Path::new(r"D:\elsewhere\a.mkv")), None);
    }

    /// A link back up the tree is followed once, not forever.
    #[test]
    fn survives_folder_loops() {
        let root = std::env::temp_dir().join(format!("afterglow-loop-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.mkv"), b"").unwrap();
        std::fs::write(root.join("sub/b.mkv"), b"").unwrap();
        // A junction needs no special rights, unlike a symbolic link.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("sub").join("loop"))
            .arg(&root)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink failed: {}", String::from_utf8_lossy(&made.stderr));
        let found = scan_folder(&root, true);
        std::fs::remove_dir_all(&root).unwrap();
        let names: Vec<String> = found
            .unwrap()
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(names, ["a.mkv", "sub/b.mkv"]);
    }
}
