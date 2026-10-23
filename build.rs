//! Builds the app icon and version into the executable and copies the
//! vendored libmpv DLL next to it. Release builds also pack the DLL into the
//! executable (see `src/player/packed.rs`), so the exe works on its own.

use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    // The commit the build came from, shown in Settings, with "-dirty" when
    // the sources differ from it. Watching src notices edits made since.
    for path in [".git/HEAD", ".git/index", "src"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rustc-env=AFTERGLOW_COMMIT={}", commit());

    // The icon Explorer, shortcuts and the taskbar show for afterglow.exe,
    // and the version its Properties show. Regenerate assets/icon.ico with
    // `cargo run --example make_icon`.
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let rc = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("afterglow.rc");
    std::fs::write(&rc, resources()).expect("failed to write afterglow.rc");
    if let Err(e) = embed_resource::compile(&rc, embed_resource::NONE).manifest_optional() {
        println!("cargo:warning=could not build the icon and version into the executable: {e}");
    }

    println!("cargo::rustc-check-cfg=cfg(packed_mpv)");
    let src = PathBuf::from("vendor/libmpv-2.dll");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed=vendor/libmpv-2.dll.sha256");
    if !src.exists() {
        println!("cargo:warning=vendor/libmpv-2.dll not found; the player needs it at runtime");
        return;
    }
    // OUT_DIR is target/<profile>/build/<pkg>/out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let profile_dir = out.ancestors().nth(3).unwrap();
    let dst = profile_dir.join("libmpv-2.dll");
    // Copying keeps the modified time, so another build of the DLL differs in
    // it even when its size happens to be the same.
    let stamp = |path: &Path| std::fs::metadata(path).map(|m| (m.len(), m.modified().ok())).ok();
    if stamp(&dst) != stamp(&src) {
        std::fs::copy(&src, &dst).expect("failed to copy libmpv-2.dll");
    }

    // Packing takes a minute or so, so debug builds skip it.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        check_pinned(&src);
        // The stamp names the packing method too, so changing it packs again.
        pack(&src, &out, &format!("xz x86 9e {:?}", stamp(&src)));
        println!("cargo:rustc-cfg=packed_mpv");
    }
}

/// libmpv decodes every video Afterglow opens, so release builds only pack the
/// DLL pinned in `vendor/libmpv-2.dll.sha256`. To update libmpv, test the new
/// one and put its SHA-256 there.
fn check_pinned(src: &Path) {
    use sha2::Digest;
    let pinned = std::fs::read_to_string("vendor/libmpv-2.dll.sha256").expect("vendor/libmpv-2.dll.sha256 is missing");
    let pinned = pinned.split_whitespace().next().unwrap_or_default().to_lowercase();
    let hash: String = sha2::Sha256::digest(std::fs::read(src).expect("failed to read libmpv-2.dll")).iter().map(|b| format!("{b:02x}")).collect();
    if hash != pinned {
        panic!("vendor/libmpv-2.dll has SHA-256 {hash}, not the pinned {pinned} (vendor/libmpv-2.dll.sha256). If you meant to update libmpv, test it and put its checksum there.");
    }
}

/// Compresses the DLL into `out` as `libmpv-2.dll.xz`, with its hash in
/// `libmpv-2.dll.hash`, unless that was already done for this `stamp`.
fn pack(src: &Path, out: &Path, stamp: &str) {
    let (packed, hash, stamp_file) = (out.join("libmpv-2.dll.xz"), out.join("libmpv-2.dll.hash"), out.join("libmpv-2.dll.stamp"));
    if packed.exists() && hash.exists() && std::fs::read_to_string(&stamp_file).is_ok_and(|s| s == stamp) {
        return;
    }
    let dll = std::fs::read(src).expect("failed to read libmpv-2.dll");
    // The x86 filter makes machine code compress better.
    let mut filters = liblzma::stream::Filters::new();
    filters.x86().lzma2(&liblzma::stream::LzmaOptions::new_preset(9 | liblzma::stream::PRESET_EXTREME).unwrap());
    let stream = liblzma::stream::Stream::new_stream_encoder(&filters, liblzma::stream::Check::Crc64).unwrap();
    let mut encoder = liblzma::write::XzEncoder::new_stream(Vec::new(), stream);
    encoder.write_all(&dll).unwrap();
    std::fs::write(&packed, encoder.finish().unwrap()).expect("failed to write the packed libmpv-2.dll");
    std::fs::write(&hash, format!("{:016x}", fnv1a(&dll))).unwrap();
    std::fs::write(&stamp_file, stamp).unwrap();
}

/// A quick hash, to tell builds of the DLL apart.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// The short hash of the checked-out commit, plus "-dirty" when tracked files
/// have changed since, or "unknown" outside a git checkout.
fn commit() -> String {
    let git = |args: &[&str]| std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success());
    let Some(head) = git(&["rev-parse", "--short", "HEAD"]) else {
        return "unknown".into();
    };
    let hash = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|o| !o.stdout.is_empty());
    if dirty { format!("{hash}-dirty") } else { hash }
}

/// The Windows resources built into afterglow.exe: its icon, and the version
/// Explorer shows under Properties > Details.
fn resources() -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split(['.', '-']).map(|p| p.parse::<u16>().unwrap_or(0));
    let numbers: Vec<String> = (0..4).map(|_| parts.next().unwrap_or(0).to_string()).collect();
    let numbers = numbers.join(",");
    let icon = std::env::current_dir().unwrap().join("assets/icon.ico").display().to_string().replace('\\', "/");
    format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Afterglow"
      VALUE "ProductName", "Afterglow"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "afterglow.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
