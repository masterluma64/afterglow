# Afterglow

A minimalist media player for shows, movies and music, written in Rust with
[egui](https://github.com/emilk/egui).
Decoding, audio and subtitle rendering are handled by [libmpv](https://mpv.io), loaded at runtime.

## Features

- **Playlists**: add a folder (button, Ctrl+N, or drag it onto the window) and it is saved
  as a playlist in the sidebar. Every subfolder with videos is a season; episodes are sorted
  naturally (`Episode 2` before `Episode 10`). Right-click a playlist to continue it, rescan
  the folder, or remove it.
- **Single videos**: open a video (button, Ctrl+O, or drag it in) to watch it without a
  playlist. It is listed under *Videos* and keeps its own progress. A video that belongs to
  a playlist plays as part of that playlist.
- **Progress tracking**: positions are saved continuously. Videos count as watched at 90%
  or when they finish, after which the next episode of a playlist starts automatically.
  The player offers to continue whatever you watched last. Right-click any entry to mark it
  (un)watched or play it from the start.
- **Subtitles**: embedded tracks and matching external files (`Episode 1.srt`,
  `Episode 1.en.ass`, …) are picked up automatically, with English preferred. The subtitle
  menu switches tracks, loads a subtitle file, or adjusts the delay.
- **Music**: switch to the Music view and add music folders (scanned recursively in the
  background; tags are cached). Browse an album grid with covers (embedded art, or
  `cover.jpg`/`folder.jpg` next to the files), open album pages, or use the songs table,
  sortable by title, artist, album, year or length. Search, filter by artist from the
  sidebar, and sort albums by artist, title, year or recently added. Playing a track queues
  the rest of the album or list. Dropping a folder that only holds audio adds it as music.
- **Visualizer**: while music plays, the now-playing view runs a GPU visualizer in the
  spirit of MilkDrop: each frame warps the previous one (zoom, swirl, mirror) and draws
  audio-reactive waveforms and spectrum shapes on top, leaving flowing trails. Six presets
  (Nebula, Vortex, Aurora, Starburst, Liquid, Prism) react to bass, mids, treble and beats;
  *Auto* moves to the next preset on a beat every half minute or so. *Cover* shows just the
  album art. Press V to cycle.

  How it works: libmpv doesn't expose decoded audio, so an FFmpeg filter graph renders a
  tiny spectrum + waveform image that mpv presents in sync with playback (`src/audio.rs`);
  the shaders live in `src/viz.wgsl`.
- Audio track selection, volume (remembered), fullscreen with auto-hiding controls.

State is stored in `%APPDATA%\Afterglow\`: `progress.json`, plus
`music-cache.json` for scanned music tags.

## Building

Requires `libmpv-2.dll` in `vendor/` (from the `mpv-dev-x86_64-*.7z` archive at
<https://github.com/shinchiro/mpv-winbuild-cmake/releases>). The build script copies it
next to the executable.

```sh
cargo run --release
cargo run --release -- "D:\Shows\Foo" "D:\Movies\Bar.mkv"   # add a playlist, play a video
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
| N / P | Next / previous episode |
| S | Cycle subtitles |
| Z / X | Subtitle delay −/+ 0.1 s |
| F11 / F / Alt+Enter / double-click video | Fullscreen (Esc to exit) |
| Home | Back to the start |
| 0–9 | Jump to 0%–90% |
| , / . | Previous / next frame (pauses) |
| L | Toggle the sidebar |
| Ctrl+N | Add folder as playlist |
| Ctrl+O | Open video |
| V | Cycle music visualizer |
