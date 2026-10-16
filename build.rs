//! Copies the vendored libmpv DLL next to the built executable.

use std::path::PathBuf;

fn main() {
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
    let stale = std::fs::metadata(&dst).map(|m| m.len()).ok() != std::fs::metadata(&src).map(|m| m.len()).ok();
    if stale {
        std::fs::copy(&src, &dst).expect("failed to copy libmpv-2.dll");
    }
}
