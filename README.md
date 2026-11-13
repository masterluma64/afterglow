<p align="center"><img src="assets/icon.png" width="96" alt=""></p>

# Afterglow

A minimalist video player for Windows that remembers where you left off. Open a folder of a
show and it plays the videos in order, picking up where you stopped; open a single video, like
a movie, and it plays just that. Written in Rust with [egui](https://github.com/emilk/egui);
[libmpv](https://mpv.io) does the decoding, sound and subtitles.

![Afterglow playing a video, with the playlist beside it](assets/screenshot.png)

## Features

- **Progress**: every video remembers its position and counts as watched at 90% (adjustable).
  On startup, the last video opens paused on the frame where you left off.
- **Playlist**: open a folder (optionally with its subfolders) or add videos; new files in the
  folder show up by themselves. Search, drag to reorder, and select several (Ctrl / Shift+click)
  to mark or remove them. It can dock beside the video or slide out over it.
- **Single videos**: *Open video* plays one video by itself, with the playlist hidden, its own
  settings and its own place in the stats as a movie.
- **Up next**: the next video starts after a short countdown (or not, if you turn it off); an
  end screen offers what to do once nothing is left.
- **Subtitles and audio**: embedded and matching external subtitles are picked up, with
  preferred languages. Shift the timing, sync to the line you just heard, or fix subtitles made
  for another frame rate. Sound that is out of sync with the picture can be delayed or brought
  forward (G / H). Track choices and delays are remembered per video.
- **Folder settings**: skip a show's opening and ending, and give a folder its own languages,
  subtitle timing, speed and more.
- **Titles**: videos are shown by the title stored in the file when it tells them apart.
- **Stats**: time watched, streaks, the last 30 days, most watched series and recent movies.
- **Screenshots** (C or F12): the current frame, with its subtitles, is copied to the clipboard
  and saved as a PNG in `Pictures\Afterglow`.
- **Everything at hand**: a command palette (Ctrl+K), single-key shortcuts, a right-click menu
  on the video, seek bar previews, media keys and an option to even out the volume.

Progress and settings are saved in `data\progress.json` next to `afterglow.exe`, so the folder
is portable; Settings can move it to `%LOCALAPPDATA%\afterglow` instead. `AFTERGLOW_DATA_DIR`
overrides both (for testing).

## Building

Put `libmpv-2.dll` from shinchiro's
[mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake/releases) (the
`mpv-dev-x86_64` archive) in `vendor/`. Release builds only accept the DLL whose SHA-256 is
in `vendor/libmpv-2.dll.sha256` (release 20261016); to update libmpv, test the new one and
put its hash there.

```sh
cargo run --release
cargo run --release -- "D:\Shows\Foo"    # play a folder
```

The release exe (about 43 MB) has libmpv packed in and unpacks it into its `data` folder on
the first start, so it needs a folder it can write to.

**Licensing**: Afterglow is closed source and for private use. Its libmpv build is GPL, so
builds must not be shared publicly. Icons are [Phosphor](https://phosphoricons.com) (MIT);
bold text is Ubuntu Medium (`assets/fonts/UFL.txt`).

## Keyboard shortcuts

Press `?` in the app for all of them. The main ones:

| Key | Action |
| --- | --- |
| Space | Play / pause |
| ← / → | Seek 5 s (Shift: 30 s) |
| ↑ / ↓ | Volume |
| N / P | Next / previous video |
| W | Mark watched / unwatched |
| O / Shift+O | Open folder / open video |
| B | Show or hide the playlist |
| F | Fullscreen |
| C / F12 | Screenshot (copied, and saved in Pictures) |
| Ctrl+K | Find any command |
