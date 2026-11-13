# Releasing

Releases are tagged `vX.Y.Z` and published as GitHub releases on the private repo, with the
exe packed in a 7z archive.

Needs [GitHub CLI](https://cli.github.com) (logged in with `gh auth login`) and
[7-Zip](https://www.7-zip.org). If they aren't on PATH, they are at
`C:\Program Files\GitHub CLI\gh.exe` and `C:\Program Files\7-Zip\7z.exe`.

## Steps

1. On `develop`, set the version in `Cargo.toml`, then run `cargo check` so `Cargo.lock`
   follows. Commit both as "Version X.Y.Z" and push `develop`.
2. Move `main` to the same commit: `git push origin develop:main`.
3. Tag and push: `git tag -a vX.Y.Z -m "Version X.Y.Z"` and `git push origin vX.Y.Z`.
4. Build from a clean tree, so the commit in Settings doesn't end in `-dirty`:

   ```sh
   cargo build --release
   ```

5. Copy `target/release/afterglow.exe` to `afterglow-X.Y.Z-windows-x64.exe` and pack it:

   ```sh
   7z a -t7z -mx=9 afterglow-X.Y.Z-windows-x64.7z afterglow-X.Y.Z-windows-x64.exe
   ```

6. Sign the archive, so a future updater can check that a release came from you:

   ```sh
   cargo run --example sign_release -- afterglow-X.Y.Z-windows-x64.7z
   ```

   This writes `afterglow-X.Y.Z-windows-x64.7z.sig`, using the key in
   `%APPDATA%\afterglow-release\signing.key` (keep a copy of it somewhere safe). Its public
   half is `a34ddcca28d3d5f373a15ce82e5b9ffa4f73004ecdcf9b898feae8f18b892f8c`.

7. Create the release with the archive and its signature. Upload only the `.7z` and its
   `.sig`, never the bare exe:

   ```sh
   gh release create vX.Y.Z afterglow-X.Y.Z-windows-x64.7z afterglow-X.Y.Z-windows-x64.7z.sig --title "Afterglow X.Y.Z" --notes "..."
   ```

   The notes are a bullet list of what changed since the last release, from
   `git log vPREVIOUS..vX.Y.Z --oneline`, written for someone using the app.
