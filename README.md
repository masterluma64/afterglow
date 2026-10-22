<img src="assets/icon.png" width="96" alt="">

# Afterglow

A minimalist video player that remembers where you left off, written in Rust with
[egui](https://github.com/emilk/egui).
Decoding, audio and subtitle rendering are handled by [libmpv](https://mpv.io), loaded at runtime.

## Features

- **Playlist**: one playlist in the sidebar, restored on restart.
  - The pin icon in its header docks it beside the video, or unpins it: then it stays
    hidden and slides out over the video when you point at its edge (or press B), hiding
    again when the pointer leaves. In fullscreen it always works that way. Settings can
    put it on the left or the right.
  - *Open folder* (O, or drop a folder) replaces it with the videos in that
    folder, sorted naturally (`Part 2` before `Part 10`), and starts where you left off.
    Turn on *Include subfolders* (right-click the playlist) to also pick up videos in
    subfolders; changing it rescans the opened folder right away.
    New videos saved into the folder are added to the end of the playlist by themselves;
    *Refresh folder* (R, in the ⋯ menu) also drops videos whose files are gone.
  - The ⋯ menu lists recently opened folders.
  - *Add videos* (I) appends them; if nothing is playing, the first one starts.
  - Dropping videos onto the window adds them or replaces the playlist: the first time,
    Afterglow asks which, and remembers it (changeable in Settings).
  - Each video shows its length, and the header shows how much of the playlist's total
    running time you've watched. Lengths of unplayed videos are read in the background
    by a headless mpv and remembered.
  - Right-click an entry to play it from the start, mark it (or everything above it)
    watched, mark it unwatched or remove it; the trash icon clears the playlist. Files on
    disk are never touched.
  - Drag entries to reorder them, or sort the playlist by name, date modified or length
    (or reverse it) from the sort icon.
  - The search box (/) filters the playlist by name; ↑ / ↓ pick a result and Enter plays
    it. Esc clears it.
- **Progress tracking**: every video remembers its position, wherever it was opened from.
  Moving or renaming a video keeps its progress and track choices: it is recognized by its
  exact file size once it shows up in the playlist again (if no other video has that size).
  Videos count as watched at 90% (adjustable) or when they finish. The next one then starts after a
  5-second countdown (this can be turned off in Settings).
  On startup the video you were watching is opened, paused on the exact frame where you
  left off.
- **Subtitles**: embedded tracks and matching external files (`Movie.srt`,
  `Movie.en.ass`, …) are picked up automatically, with English preferred. The subtitle
  menu switches tracks, loads a subtitle file, or adjusts the delay.
- **Subtitle sync**: the *Sync subtitles* panel (T, or the subtitle menu) fixes subtitles that are out
  of sync. When you hear a line, show the previous or next subtitle line right now
  (Ctrl+← / Ctrl+→); or mark when a subtitle appears (G) and when the line is actually
  spoken (H) to shift by the difference; or nudge the delay by 0.1 s or 1 s. Subtitles that
  drift further off over time (made for 25 fps vs 23.976 fps video) can be retimed.
- **Per-video choices**: the audio track, subtitle track, subtitle and audio delays, subtitle
  files you load and picture adjustments are remembered for each video and restored when you open it again.
- **Right-click the video** for a menu with play/pause, next and previous video, audio and
  subtitle tracks, speed, chapters, marking the video watched, fullscreen, the playlist and
  Settings. Each item shows its key, and it also works in fullscreen while the controls are
  hidden.
- **Command palette** (Ctrl+K, or the ⋯ and right-click menus): type a few letters to find
  any command, including the ones without a key or button, such as a specific audio or
  subtitle track, a chapter, a speed, sorting, a recent folder or turning auto-play off.
  Typing part of a video's name also finds it in the playlist, to play it.
  ↑ / ↓ pick one and Enter runs it.
- **On-screen feedback**: seeking, volume, mute, subtitle changes and video changes show a
  short notice over the video, so keyboard control works without the controls visible.
- **Seek bar previews**: hovering or dragging the seek bar shows a preview frame and the
  time (and chapter name). While dragging, only the preview moves; the video jumps there
  when you let go. Chapters show as marks on the seek bar.
- Playback speed from 0.25× to 3× (pitch-corrected), audio track selection, volume
  (remembered), fullscreen with controls that float over the video and hide while playing
  (they stay up while paused or while the pointer rests on them). The cursor also hides over
  the video when the mouse is still.
- **Audio delay** (audio menu or palette): plays the sound earlier or later in 0.1 s steps,
  for videos whose sound is out of sync with the picture.
- **Picture** (right-click menu or palette): show the video at another aspect ratio (4:3,
  16:9, 21:9) when it looks stretched, rotate it a quarter turn (a phone video recorded
  sideways), zoom in or out, or fill the window by cropping the black bars. Remembered per video.
- **Even out the volume** (night mode, in the audio menu, the palette or Settings): quiet
  dialogue gets louder and loud scenes quieter.
- **Screenshots** (Shift+S, the right-click menu or the palette) save the current frame with
  its subtitles as a PNG in `Pictures\Afterglow`, named after the video and the time.
- **Media keys**: play/pause, next, previous and stop work from the keyboard's media keys,
  even when another window has focus, and Windows' media overlay shows what's playing.
- The PC and display stay awake while a video plays (not while paused).
- **One window**: opening a video or folder (from Explorer, say) while Afterglow is already
  running plays it in the open window instead of starting a second one.
- The window opens where you left it, at the same size (and maximized if it was). If that
  spot is on a monitor that is no longer connected, it opens on the main screen instead.
- **GPU playback**: video is decoded on the GPU where supported and drawn by mpv straight
  into the window through OpenGL. Without a working OpenGL driver Afterglow restarts in
  software mode (also available by setting `AFTERGLOW_SOFTWARE_VIDEO=1`).

- **Settings** (⋯ menu or E): seek steps, when a video counts as watched, auto-play,
  evening out the volume,
  preferred subtitle and audio languages, hardware decoding, the theme and the accent color
  (a few presets or any color).

Afterglow is portable: its state is stored in `data\progress.json` next to `afterglow.exe`,
so the whole folder can be moved or carried on a USB stick. Data from earlier versions in
`%APPDATA%\Afterglow` is copied over on first start (the original is left in place). Set
`AFTERGLOW_DATA_DIR` to use another folder, e.g. for testing.

## Building

Requires `libmpv-2.dll` in `vendor/` (from the `mpv-dev-x86_64-*.7z` archive at
<https://github.com/shinchiro/mpv-winbuild-cmake/releases>). The build script copies it
next to the executable. The build Afterglow is tested with is release
[20261016](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20261016), and
`vendor/libmpv-2.dll.sha256` pins it: since libmpv decodes every video Afterglow opens, the
packaging script below only ships that exact DLL. To update libmpv, test the new one and put
its SHA-256 in that file.

```sh
cargo run --release
cargo run --release -- "D:\Shows\Foo"    # play a folder
```

To run it elsewhere, keep `afterglow.exe` and `libmpv-2.dll` together in a folder the user
can write to (the `data` folder is created next to them).

To make a zip of it, run `scripts\package.cmd` (or double-click it). It runs the tests,
builds, and writes `dist\Afterglow-<version>-windows-x64.zip` with the exe, the DLL, this
README and a notice saying which commit and libmpv it was built from. The version comes
from `Cargo.toml`; building the same version again replaces its zip. `-SkipTests` skips the
tests.

**Licensing:** Afterglow is closed source and meant for private use. The libmpv builds it
uses (from shinchiro) are GPL builds, so don't share builds publicly while that's the
case: the GPL would then require Afterglow's source to be offered under the GPL too. To
distribute it more widely, switch to an LGPL build of libmpv (mpv built with
`-Dgpl=false`, with an FFmpeg built without `--enable-gpl`), which a closed-source program
may load as a separate, replaceable DLL.

Icons are [Phosphor](https://phosphoricons.com) (MIT), embedded from `assets/fonts`.
The app icon is drawn in code (`src/app_icon.rs`); `cargo run --example make_icon`
regenerates `assets/icon.ico`, which the build script puts into the executable.

## Keyboard shortcuts

Nearly everything has a single-key shortcut (the older Ctrl combinations still work). Press `?`
in the app for an overview.

| Key | Action |
| --- | --- |
| Space / K | Play / pause |
| J / L or ← / → | Seek 5 s (Shift: 30 s; adjustable in Settings) |
| Home | Back to the start |
| 0–9 | Jump to 0%–90% |
| , / . | Previous / next frame (pauses) |
| Shift+S | Save a screenshot (in Pictures\Afterglow) |
| [ / ] | Slower / faster (Backspace: normal speed) |
| PgUp / PgDn | Previous / next chapter |
| ↑ / ↓ | Volume |
| M | Mute |
| A | Cycle audio tracks |
| S | Cycle subtitles |
| T | Subtitle sync panel |
| Z / X | Subtitle delay −/+ 0.1 s (Shift: 1 s) |
| Ctrl+← / Ctrl+→ | Show the previous / next subtitle line now |
| G / H | Mark when a subtitle appears / when its line is spoken |
| N / P | Next / previous video |
| W | Mark the playing video watched / unwatched |
| / | Search the playlist: ↑ / ↓ pick a result, Enter plays it, Esc clears |
| O | Open folder as playlist |
| I | Add videos to the playlist |
| R | Refresh the playlist's folder |
| B | Show or hide the playlist (slides it out when unpinned) |
| F / double-click video | Fullscreen |
| Esc | Leave fullscreen, or close the subtitle sync panel |
| E | Settings |
| ? | Keyboard shortcut overview (also in the ⋯ menu) |
| Ctrl+K | Command palette: find any command by typing |
| Scroll over the video | Volume |
| Middle-click the video | Play / pause |
| Right-click the video | Menu with tracks, speed, chapters and more |
