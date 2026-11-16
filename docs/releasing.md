# Releasing

Releases are tagged `vX.Y.Z` and published as GitHub releases, with a setup (the main
download) and the bare exe packed in a 7z archive, for those who'd rather not install.

Needs [GitHub CLI](https://cli.github.com) (logged in with `gh auth login`) and
[7-Zip](https://www.7-zip.org). If they aren't on PATH, they are at
`C:\Program Files\GitHub CLI\gh.exe` and `C:\Program Files\7-Zip\7z.exe`.

## Steps

1. On `develop`, set the version in `Cargo.toml`, then run `cargo check` so `Cargo.lock`
   follows. If dependencies changed since the last release, run
   `cargo run --example notices` to update `THIRD-PARTY-NOTICES.txt`. Commit as
   "Version X.Y.Z" and push `develop`.
2. Move `main` to the same commit: `git push origin develop:main`.
3. Tag and push: `git tag -a vX.Y.Z -m "Version X.Y.Z"` and `git push origin vX.Y.Z`.
4. Build from a clean tree, so the commit in Settings doesn't end in `-dirty`:

   ```sh
   cargo build --release
   ```

5. Build the setup, which has that exe, `LICENSE` and `THIRD-PARTY-NOTICES.txt` built in
   (it refuses an exe of another version):

   ```sh
   cargo build --release -p afterglow-setup
   ```

   Copy `target/release/afterglow-setup.exe` to `afterglow-X.Y.Z-windows-x64-setup.exe`. Its
   window asks where to install (`%LOCALAPPDATA%\Programs\afterglow` unless another folder
   is picked) and which shortcuts to add, without administrator rights, and updates an
   installed copy where it is; `--quiet` does that without a window. The installed exe
   uninstalls itself from Settings › Apps (`src/platform/install.rs`).

6. Copy `target/release/afterglow.exe` to `afterglow-X.Y.Z-windows-x64.exe` and pack it with
   the license and notices:

   ```sh
   7z a -t7z -mx=9 afterglow-X.Y.Z-windows-x64.7z afterglow-X.Y.Z-windows-x64.exe LICENSE THIRD-PARTY-NOTICES.txt
   ```

7. Sign both. Copies of Afterglow update themselves from the newest release's `.7z`
   (`src/update.rs`), and only install one with a valid `.sig`:

   ```sh
   cargo run --example sign_release -- afterglow-X.Y.Z-windows-x64-setup.exe
   cargo run --example sign_release -- afterglow-X.Y.Z-windows-x64.7z
   ```

   This writes a `.sig` next to each, using the key in
   `%APPDATA%\afterglow-release\signing.key` (keep a copy of it somewhere safe). Its public
   half is `a34ddcca28d3d5f373a15ce82e5b9ffa4f73004ecdcf9b898feae8f18b892f8c`.

8. Create the release with the setup, the archive and their signatures (not the bare
   exe):

   ```sh
   gh release create vX.Y.Z afterglow-X.Y.Z-windows-x64-setup.exe afterglow-X.Y.Z-windows-x64-setup.exe.sig afterglow-X.Y.Z-windows-x64.7z afterglow-X.Y.Z-windows-x64.7z.sig --title "Afterglow X.Y.Z" --notes "..."
   ```

   The notes are a bullet list of what changed since the last release, from
   `git log vPREVIOUS..vX.Y.Z --oneline`, written for someone using the app. Updated copies
   show them after restarting, under "Updated to Afterglow X.Y.Z". Copies find a release as
   soon as it is published, so check it before publishing (`--draft` first, if need be).
