# Afterglow

A minimalist video player that remembers where you left off, written in Rust with
[egui](https://github.com/emilk/egui).
Decoding, audio and subtitle rendering are handled by [libmpv](https://mpv.io), loaded at runtime.

## Features

- **Playlist**: one playlist in the sidebar, restored on restart.
  - *Open folder* (Ctrl+Shift+O, or drop a folder) replaces it with the videos in that
    folder, sorted naturally (`Part 2` before `Part 10`), and starts where you left off.
    Tick *Include subfolders* to also pick up videos in subfolders; changing it rescans
    the opened folder right away.
  - *Add videos* (Ctrl+O, or drop videos) appends them; if nothing is playing, the first
    one starts.
  - Each video shows its length, and the header shows how much of the playlist's total
    running time you've watched. Lengths of unplayed videos are read in the background
    by a headless mpv and remembered.
  - Right-click an entry to play it from the start, mark it (un)watched or remove it; the
    trash icon clears the playlist. Files on disk are never touched.
- **Progress tracking**: every video remembers its position, wherever it was opened from.
  Videos count as watched at 90% or when they finish, after which the next one starts.
  On startup the video you were watching is opened, paused on the exact frame where you
  left off.
- **Subtitles**: embedded tracks and matching external files (`Movie.srt`,
  `Movie.en.ass`, …) are picked up automatically, with English preferred. The subtitle
  menu switches tracks, loads a subtitle file, or adjusts the delay.
- **On-screen feedback**: seeking, volume, mute, subtitle changes and video changes show a
  short notice over the video, so keyboard control works without the controls visible.
- Audio track selection, volume (remembered), fullscreen with controls that hide while
  playing and stay up while paused.
- **GPU playback**: video is decoded on the GPU where supported and drawn by mpv straight
  into the window through OpenGL. Without a working OpenGL driver Afterglow restarts in
  software mode (also available by setting `AFTERGLOW_SOFTWARE_VIDEO=1`).

State is stored in `%APPDATA%\Afterglow\progress.json` (set `AFTERGLOW_DATA_DIR` to use
another folder, e.g. for testing).

## Building

Requires `libmpv-2.dll` in `vendor/` (from the `mpv-dev-x86_64-*.7z` archive at
<https://github.com/shinchiro/mpv-winbuild-cmake/releases>). The build script copies it
next to the executable.

```sh
cargo run --release
cargo run --release -- "D:\Shows\Foo"    # play a folder
```

To distribute, ship `afterglow.exe` together with `libmpv-2.dll`.

Icons are [Phosphor](https://phosphoricons.com) (MIT), embedded from `assets/fonts`.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Space / K | Play / pause |
| ← / → | Seek 5 s (Shift: 30 s) |
| ↑ / ↓ | Volume |
| M | Mute |
| N / P | Next / previous video |
| S | Cycle subtitles |
| Z / X | Subtitle delay −/+ 0.1 s |
| F11 / F / Alt+Enter / double-click video | Fullscreen (Esc to exit) |
| Home | Back to the start |
| 0–9 | Jump to 0%–90% |
| , / . | Previous / next frame (pauses) |
| L | Show or hide the playlist |
| Ctrl+Shift+O | Open folder as playlist |
| Ctrl+O | Add videos to the playlist |
