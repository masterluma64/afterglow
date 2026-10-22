//! Builds the app icon into the executable and copies the vendored libmpv DLL
//! next to it. Release builds also pack the DLL into the executable (see
//! `src/player/packed.rs`), so the exe works on its own.

use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    // The icon Explorer, shortcuts and the taskbar show for afterglow.exe.
    // Regenerate assets/icon.ico with `cargo run --example make_icon`.
    println!("cargo:rerun-if-changed=assets/afterglow.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if let Err(e) = embed_resource::compile("assets/afterglow.rc", embed_resource::NONE).manifest_optional() {
        println!("cargo:warning=could not build the app icon into the executable: {e}");
    }

    println!("cargo::rustc-check-cfg=cfg(packed_mpv)");
    let src = PathBuf::from("vendor/libmpv-2.dll");
    println!("cargo:rerun-if-changed={}", src.display());
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
        // The stamp names the packing method too, so changing it packs again.
        pack(&src, &out, &format!("xz x86 9e {:?}", stamp(&src)));
        println!("cargo:rustc-cfg=packed_mpv");
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
