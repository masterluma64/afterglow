//! Signs release archives, so that once afterglow updates itself from GitHub
//! releases, it can tell an update really came from its maintainer.
//!
//! ```sh
//! cargo run --example sign_release -- new-key       # once: makes the key
//! cargo run --example sign_release -- <file.7z>     # writes <file.7z>.sig
//! ```
//!
//! The private key is kept in `%APPDATA%\afterglow-release\signing.key`,
//! outside the repo; `AFTERGLOW_SIGNING_KEY` points elsewhere. Its public half
//! is in docs/releasing.md, for the updater to check against.

use std::path::PathBuf;

use ed25519_dalek::{Signer, SigningKey};

fn key_path() -> PathBuf {
    if let Some(path) = std::env::var_os("AFTERGLOW_SIGNING_KEY") {
        return PathBuf::from(path);
    }
    let appdata = std::env::var_os("APPDATA").expect("APPDATA is not set");
    PathBuf::from(appdata).join("afterglow-release").join("signing.key")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: sign_release new-key | sign_release <file>");
        std::process::exit(2);
    });
    let path = key_path();
    if arg == "new-key" {
        if path.exists() {
            eprintln!("{} already exists; it stays, so updates signed with it keep installing.", path.display());
            std::process::exit(1);
        }
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("no randomness");
        std::fs::create_dir_all(path.parent().unwrap()).expect("could not create the key's folder");
        std::fs::write(&path, hex(&seed)).expect("could not save the key");
        let public = SigningKey::from_bytes(&seed).verifying_key();
        println!("Saved the signing key in {}. Keep a copy somewhere safe.", path.display());
        println!("Public key, for docs/releasing.md:\n{}", hex(public.as_bytes()));
        return;
    }
    let seed = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let seed: [u8; 32] = unhex(&seed).and_then(|s| s.try_into().ok()).expect("the key file is damaged");
    let key = SigningKey::from_bytes(&seed);
    let file = PathBuf::from(&arg);
    let bytes = std::fs::read(&file).unwrap_or_else(|e| panic!("could not read {}: {e}", file.display()));
    let signature = key.sign(&bytes);
    let mut sig = file.clone().into_os_string();
    sig.push(".sig");
    std::fs::write(&sig, hex(&signature.to_bytes())).expect("could not write the signature");
    println!("Signed with public key {}:\n{}", hex(key.verifying_key().as_bytes()), PathBuf::from(sig).display());
}
