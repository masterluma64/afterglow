//! Writes THIRD-PARTY-NOTICES.txt: the licenses of everything built into
//! afterglow.exe, which its own license and theirs ask to ship with it.
//!
//! ```sh
//! cargo run --example notices
//! ```
//!
//! Run it again whenever dependencies change (it is a step in
//! docs/releasing.md). The crates come from `cargo metadata`: those the app
//! is built from on Windows, without the ones only builds and tests use. Their
//! license texts are the files they ship; libmpv, FFmpeg and the fonts are
//! described by hand below.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const TARGET: &str = "x86_64-pc-windows-msvc";

/// What isn't a crate: libmpv and FFmpeg, packed into release builds, and
/// the fonts and icons.
const BUNDLED: &str = "\
libmpv and FFmpeg
=================

Release builds of Afterglow include libmpv-2.dll, a build of mpv and FFmpeg from
shinchiro's mpv-winbuild-cmake. That build is licensed under the GNU General Public
License, version 3 or later (see LICENSE). Its source code:

  mpv:              https://github.com/mpv-player/mpv
  FFmpeg:           https://ffmpeg.org/download.html#get-sources
  The build itself: https://github.com/shinchiro/mpv-winbuild-cmake
                    (the release dated in vendor/libmpv-2.dll.sha256)


Phosphor Icons
==============

https://phosphoricons.com, under the MIT License:

";

const UBUNTU_FONT: &str = "

Ubuntu font (Ubuntu Medium, for bold text)
==========================================

https://design.ubuntu.com/font, under the Ubuntu Font Licence 1.0:

";

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--filter-platform", TARGET, "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("could not run cargo metadata");
    assert!(output.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr));
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata's output");

    let packages: HashMap<&str, &Value> = metadata["packages"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap(), p)).collect();
    let nodes: HashMap<&str, &Value> = metadata["resolve"]["nodes"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap(), n)).collect();
    let app = metadata["resolve"]["root"].as_str().expect("the app's package");

    // Everything the app is built from: normal dependencies, all the way down.
    let (mut used, mut queue) = (HashSet::new(), vec![app]);
    while let Some(id) = queue.pop() {
        for dep in nodes[id]["deps"].as_array().unwrap() {
            let normal = dep["dep_kinds"].as_array().unwrap().iter().any(|k| k["kind"].is_null());
            let dep_id = dep["pkg"].as_str().unwrap();
            if normal && used.insert(dep_id) {
                queue.push(dep_id);
            }
        }
    }

    // Grouped by name, so versions of one crate share their texts.
    let mut crates: BTreeMap<String, Crate> = BTreeMap::new();
    for id in used {
        let package = packages[id];
        let name = package["name"].as_str().unwrap().to_string();
        let license = package["license"].as_str().unwrap_or("see its license file").to_string();
        let dir = Path::new(package["manifest_path"].as_str().unwrap()).parent().unwrap().to_path_buf();
        let authors: Vec<String> = package["authors"].as_array().unwrap().iter().filter_map(|a| a.as_str()).map(String::from).collect();
        let entry = crates.entry(name).or_insert_with(|| Crate { versions: Vec::new(), license, files: license_files(&dir), authors });
        entry.versions.push(package["version"].as_str().unwrap().to_string());
    }
    // The Apache License text, for crates under it that don't ship it.
    let apache = crates.values().flat_map(|c| &c.files).find(|f| f.file_name().unwrap().to_string_lossy().to_uppercase().starts_with("LICENSE-APACHE"));
    let apache = apache.map(|f| std::fs::read_to_string(f).unwrap_or_default()).expect("some crate ships the Apache License");
    let mut needs_apache = false;

    let mut out = String::from(
        "Third-party notices for Afterglow\n\
         =================================\n\n\
         Afterglow is free software under the GNU General Public License, version 3 or later\n\
         (see LICENSE). It is built with the software below, under their own licenses.\n\n\n",
    );
    out.push_str(BUNDLED);
    out.push_str(&std::fs::read_to_string(root.join("assets/fonts/LICENSE.txt")).expect("Phosphor's license"));
    out.push_str(UBUNTU_FONT);
    out.push_str(&std::fs::read_to_string(root.join("assets/fonts/UFL.txt")).expect("the Ubuntu Font Licence"));
    out.push_str("\n\nRust crates\n===========\n");
    for (name, mut krate) in crates {
        krate.versions.sort();
        let license = &krate.license;
        out.push_str(&format!("\n\n{name} {} ({license})\n{}\n", krate.versions.join(", "), "-".repeat(60)));
        // Fonts it bundles have their own licenses; the crate needs one too.
        let own = krate.files.iter().any(|f| f.parent().and_then(Path::file_name).is_none_or(|d| d != "fonts"));
        if !own {
            // The crate doesn't ship its license (it is in its repository):
            // the license it names, with its authors.
            let holders = if krate.authors.is_empty() { format!("the {name} authors") } else { krate.authors.join(", ") };
            if license.contains("MIT") {
                out.push_str(&format!("\nUsed under the MIT License:\n\n{}\n", MIT.replace("{holders}", &holders)));
            } else if license.contains("BSL-1.0") {
                out.push_str(&format!("\nCopyright (c) {holders}. Used under the Boost Software License 1.0:\n\n{BSL}\n"));
            } else if license.contains("Apache-2.0") {
                needs_apache = true;
                out.push_str(&format!("\nCopyright (c) {holders}. Used under the Apache License 2.0, whose text is at the end.\n"));
            } else {
                panic!("{name} ships no license text and its license ({license}) isn't one this knows");
            }
        }
        for file in krate.files {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let shown = file.strip_prefix(file.parent().unwrap().parent().unwrap()).unwrap_or(&file);
            out.push_str(&format!("\n{}:\n\n{}\n", shown.display(), text.trim_end()));
        }
    }
    if needs_apache {
        out.push_str(&format!("\n\nApache License 2.0\n==================\n\n{}\n", apache.trim_end()));
    }
    let path = root.join("THIRD-PARTY-NOTICES.txt");
    std::fs::write(&path, out.replace("\r\n", "\n")).expect("could not write the notices");
    println!("Wrote {}", path.display());
}

struct Crate {
    versions: Vec<String>,
    license: String,
    files: Vec<PathBuf>,
    authors: Vec<String>,
}

const MIT: &str = "\
Copyright (c) {holders}

Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the \"Software\"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.";

const BSL: &str = "\
Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization obtaining a copy of
the software and accompanying documentation covered by this license (the \"Software\") to use,
reproduce, display, distribute, execute, and transmit the Software, and to prepare derivative
works of the Software, and to permit third-parties to whom the Software is furnished to do so,
all subject to the following:

The copyright notices in the Software and this entire statement, including the above license
grant, this restriction and the following disclaimer, must be included in all copies of the
Software, in whole or in part, and all derivative works of the Software, unless such copies or
derivative works are solely in the form of machine-executable object code generated by a source
language processor.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE AND
NON-INFRINGEMENT. IN NO EVENT SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE
LIABLE FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.";

/// The license texts a crate ships: LICENSE*, LICENCE*, COPYING*, NOTICE*,
/// and the licenses of fonts it bundles (the .txt files in its `fonts`).
fn license_files(dir: &Path) -> Vec<PathBuf> {
    let files_in = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default()
    };
    let mut files: Vec<PathBuf> = files_in(dir)
        .into_iter()
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_uppercase();
            ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"].iter().any(|n| name.starts_with(n))
        })
        .collect();
    files.extend(files_in(&dir.join("fonts")).into_iter().filter(|p| p.extension().is_some_and(|e| e == "txt")));
    files.sort();
    files
}
