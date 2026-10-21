//! Finding videos on disk.

use std::cmp::Ordering;
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
/// videos kept together after the folder's own).
pub fn scan_folder(dir: &Path, include_subfolders: bool) -> std::io::Result<Vec<PathBuf>> {
    std::fs::read_dir(dir)?;
    let mut videos = Vec::new();
    collect(dir, include_subfolders, &mut videos);
    // Files directly in the folder come first, then subfolders in order.
    let key = |p: &PathBuf| {
        let rel = p.strip_prefix(dir).unwrap_or(p);
        (rel.components().count() > 1, rel.to_string_lossy().replace('\\', "/"))
    };
    videos.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then_with(|| natural_cmp(&ka.1, &kb.1))
    });
    Ok(videos)
}

fn collect(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            if recursive {
                collect(&path, true, out);
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
}
