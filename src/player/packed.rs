//! libmpv packed into the executable (in release builds), so afterglow.exe
//! works on its own. The first start unpacks it into the data folder, named
//! after its hash so a new build of it is unpacked again.

use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(packed_mpv)]
static PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libmpv-2.dll.xz"));
#[cfg(packed_mpv)]
const HASH: &str = include_str!(concat!(env!("OUT_DIR"), "/libmpv-2.dll.hash"));

/// Where the packed libmpv was unpacked; `None` if this build has none.
/// Several threads load libmpv, so it is unpacked only once.
pub fn unpacked() -> Option<Result<PathBuf, String>> {
    static PATH: OnceLock<Option<Result<PathBuf, String>>> = OnceLock::new();
    PATH.get_or_init(unpack).clone()
}

#[cfg(not(packed_mpv))]
fn unpack() -> Option<Result<PathBuf, String>> {
    None
}

#[cfg(packed_mpv)]
fn unpack() -> Option<Result<PathBuf, String>> {
    let Some(dir) = crate::progress::data_dir() else {
        return Some(Err("there is no folder to unpack libmpv into".into()));
    };
    tidy_other_data_dir(&dir);
    let name = format!("libmpv-{HASH}.dll");
    let path = dir.join(&name);
    if path.is_file() {
        return Some(Ok(path));
    }
    let result = (|| {
        std::fs::create_dir_all(&dir)?;
        // Written under another name first, so a start cut short never leaves
        // half a DLL behind.
        let tmp = dir.join(format!("{name}.tmp"));
        let mut file = std::fs::File::create(&tmp)?;
        std::io::copy(&mut liblzma::read::XzDecoder::new(PACKED), &mut file)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = result {
        return Some(Err(format!("libmpv could not be unpacked into {}: {e}", dir.display())));
    }
    // Earlier builds' copies are no longer needed (one still loaded by a
    // running afterglow can't be deleted, and stays until next time).
    remove_unpacked(&dir, Some(&name));
    Some(Ok(path))
}

/// Removes the copies of libmpv in `dir`, except the one called `keep`.
#[cfg(packed_mpv)]
fn remove_unpacked(dir: &std::path::Path, keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if Some(other.as_str()) != keep && other != super::mpv::DLL_NAME && other.starts_with("libmpv-") && (other.ends_with(".dll") || other.ends_with(".dll.tmp")) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// After the data folder moved (portable or not), the copy of libmpv left in
/// the old one is removed, and the folder too if nothing else is in it.
#[cfg(packed_mpv)]
fn tidy_other_data_dir(dir: &std::path::Path) {
    if crate::progress::data_dir_fixed() {
        return;
    }
    for other in [true, false].into_iter().filter_map(crate::progress::data_dir_for) {
        if other != dir {
            remove_unpacked(&other, None);
            let _ = std::fs::remove_dir(&other);
        }
    }
}
