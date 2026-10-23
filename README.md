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
    put it on the left or the right. A single video opened by itself (not from a folder), such as a movie, hides
    the playlist; B still shows it.
  - *Open folder* (O, or drop a folder) replaces it with the videos in that
    folder, sorted naturally (`Part 2` before `Part 10`), and starts where you left off.
    Turn on *Include subfolders* (right-click the playlist) to also pick up videos in
    subfolders; changing it rescans the opened folder right away.
    New videos saved into the folder are added to the end of the playlist by themselves;
    *Refresh folder* (R, in the ⋯ menu) also drops videos whose files are gone.
  - The ⋯ menu lists recently opened folders.
  - *Add videos* (I) appends them; if nothing is playing, the first one starts.
  - Dropping videos onto the window adds them or replaces the playlist. Afterglow asks
    which, until you tick *Save as default* (changeable in Settings).
  - Each video shows its length, and the header shows how much of the playlist's total
    running time you've watched. Lengths of unplayed videos are read in the background
    by a headless mpv and remembered.
  - Right-click an entry to play it from the start, mark it (or everything above it)
    watched, mark it unwatched or remove it; the trash icon clears the playlist. Files on
    disk are never touched.
  - Ctrl+click entries to select several (Shift+click selects a range), then right-click
    one of them to mark them all watched or unwatched or remove them; Delete also removes
    them, and Esc clears the selection.
  - Drag entries to reorder them.
  - The search box (/) filters the playlist by name; ↑ / ↓ pick a result and Enter plays
    it. Esc clears it.
- **Progress tracking**: every video remembers its position, wherever it was opened from.
  What was saved about videos whose files were deleted is forgotten by itself, on startup
  and when refreshing the folder. Videos on a drive that isn't connected right now are left
  alone.
  Videos count as watched at 90% (adjustable) or when they finish. The next one then starts after a
  5-second countdown (this can be turned off in Settings), from its beginning. Marking the playing
  video watched (W) skips straight to the next one.
  On startup the video you were watching is opened, paused on the exact frame where you
  left off.
- **Subtitles**: embedded tracks and matching external files (`Movie.srt`,
  `Movie.en.ass`, …) are picked up automatically, with English preferred. The subtitle
  menu switches tracks, loads a subtitle file, or adjusts the delay.
- **Subtitle timing** (the subtitle menu's *Timing* section, or the palette): shift
  subtitles earlier or later with the delay (Z / X, Shift: 1 s), or, when you hear a
  line, show the previous or next subtitle line right now (Ctrl+← / Ctrl+→). Subtitles
  that drift further off over time (made for 25 fps vs 23.976 fps video) can be retimed.
- **Per-video choices**: the audio track, subtitle track, subtitle delay and subtitle files you
  load are remembered for each video and restored when you open it again.
- **Right-click the video** for a menu with play/pause, next and previous video, audio and
  subtitle tracks, speed, chapters, the playing video's playlist choices (play from start,
  mark it or everything above it watched, mark it unwatched, remove it), fullscreen, the
  playlist and Settings. Each item shows its key, and it also works in fullscreen while the controls are
  hidden.
- **Command palette** (Ctrl+K, or the ⋯ and right-click menus): type a few letters to find
  any command, including the ones without a key or button, such as a specific audio or
  subtitle track, a chapter, a speed, a recent folder or turning auto-play off.
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
- **Even out volume** (night mode, in the audio menu, the palette or Settings): quiet
  dialogue gets louder and loud scenes quieter.
- **Media keys**: play/pause, next, previous and stop work from the keyboard's media keys,
  even when another window has focus, and Windows' media overlay shows what's playing.
- The PC and display stay awake while a video plays (not while paused).
- **One window**: opening a video or folder (from Explorer, say) while Afterglow is already
  running plays it in the open window instead of starting a second one.
- The window opens where you left it, at the same size (and maximized if it was). If that
  spot is on a monitor that is no longer connected, it opens on the main screen instead.
- **GPU playback**: video is decoded on the GPU where supported and drawn by mpv straight
  into the window through OpenGL, which Afterglow needs.
- **Stats** (⋯ menu or the palette): total time watched, videos finished, the last 7 and 30
  days, streaks, your best day, a chart of the last 30 days and the most watched series
  (by folder). They are kept apart from each video's progress, so deleting watched videos
  doesn't lower them. Seeking doesn't count, and an hour of video counts as an hour at any
  speed. When stats were first kept, what was watched before was estimated from the saved
  progress. *Clear stats…* at the bottom of the dialog starts them over, after asking.
- **Folder settings** (⋯ menu, a video's right-click menu or the palette): settings for the
  playing video's folder. Set where the opening ends (counted from the start) and where the
  ending starts (counted back from the end), and a *Skip opening* / *Skip ending* button shows
  in the bottom-right corner while it plays (Enter skips; skipping the ending goes to the next
  video). *Mark end of opening here* and *Mark start of ending here* in the video's right-click
  menu or the palette set them from the current time. Both show as bands on the seek bar. A
  folder can also have its own
  subtitle and audio languages, subtitle delay and timing (the starting point for each video;
  *Use this timing for whole folder* in the subtitle menu sets them), when a video counts as
  watched, auto-play, evening out the volume and speed. *Also use for other folders in …*
  shares them with the parent folder's other subfolders, such as a show's seasons.
- **Settings** (⋯ menu or E): seek steps, when a video counts as watched, auto-play,
  evening out the volume,
  preferred subtitle and audio languages, hardware decoding, the theme and the accent color
  (a few presets or any color), and where progress is saved.
- **Desktop shortcut** (in Settings). If another copy of Afterglow (or the same one, moved or
  renamed) made it, *Use this copy* points it here.

Afterglow is portable by default: its state is stored in `data\progress.json` next to
`afterglow.exe`, so the whole folder can be moved or carried on a USB stick. Settings can move
it to `%LOCALAPPDATA%\afterglow` instead. Set `AFTERGLOW_DATA_DIR` to use another folder,
e.g. for testing.

## Building

Requires `libmpv-2.dll` in `vendor/` (from the `mpv-dev-x86_64-*.7z` archive at
<https://github.com/shinchiro/mpv-winbuild-cmake/releases>). The build script copies it
next to the executable, and release builds also pack it into the executable, compressed with
xz (this takes a minute or two the first time, and again only when the DLL changes).
The build Afterglow is tested with is release
[20261016](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20261016), and
`vendor/libmpv-2.dll.sha256` pins it: since libmpv decodes every video Afterglow opens,
release builds refuse any other DLL. To update libmpv, test the new one and put its SHA-256
in that file.

```sh
cargo run --release
cargo run --release -- "D:\Shows\Foo"    # play a folder
```

The release build, `target\release\afterglow.exe` (about 45 MB, 36 MB in a 7-Zip archive), runs on its own: on
the first start it unpacks libmpv into its `data` folder (about 120 MB, taking a second or
two), and uses that from then on. A `libmpv-2.dll` next to the exe is used instead if there
is one. Put the exe in a folder the user can write to, since the `data` folder is created
next to it.

**Licensing:** Afterglow is closed source and meant for private use. The libmpv builds it
uses (from shinchiro) are GPL builds, so don't share builds publicly while that's the
case: the GPL would then require Afterglow's source to be offered under the GPL too. To
distribute it more widely, switch to an LGPL build of libmpv (mpv built with
`-Dgpl=false`, with an FFmpeg built without `--enable-gpl`), which a closed-source program
may load as a separate, replaceable DLL.

Icons are [Phosphor](https://phosphoricons.com) (MIT) and bold text is Ubuntu Medium (Ubuntu
Font Licence, `assets/fonts/UFL.txt`), both embedded from `assets/fonts`.
The app icon is drawn in code (`src/ui/app_icon.rs`); `cargo run --example make_icon`
regenerates `assets/icon.ico`, which the build script puts into the executable.

## Keyboard shortcuts

Nearly everything has a single-key shortcut (the older Ctrl combinations still work). Press `?`
in the app for an overview.

| Key | Action |
| --- | --- |
| Space / K | Play / pause |
| J / L or ← / → | Seek 5 s (Shift: 30 s; adjustable in Settings) |
| Home | Rewind to start |
| 0–9 | Jump to 0%–90% |
| , / . | Previous / next frame (pauses) |
| [ / ] | Slower / faster (Backspace: normal speed) |
| PgUp / PgDn | Previous / next chapter |
| Enter | Skip the opening or ending (see Folder settings) |
| ↑ / ↓ | Volume |
| M | Mute |
| A | Next audio track |
| S | Next subtitle track |
| Z / X | Show subtitles earlier / later (Shift: 1 s) |
| Ctrl+← / Ctrl+→ | Show the previous / next subtitle line now |
| N / P | Next / previous video |
| W | Mark the playing video watched (and play the next) / unwatched |
| / | Search playlist: ↑ / ↓ pick a result, Enter plays it, Esc clears |
| O | Open folder as playlist |
| I | Add videos to the playlist |
| R | Refresh the playlist's folder |
| B | Show or hide the playlist (slides it out when unpinned) |
| F / double-click video | Fullscreen |
| Esc | Clear selection, cancel the next video's countdown, or leave fullscreen |
| E | Settings |
| ? | Keyboard shortcut overview (also in the ⋯ menu) |
| Ctrl+K | Command palette: find any command by typing |
| Scroll over the video | Volume |
| Middle-click the video | Play / pause |
| Right-click the video | Menu with tracks, speed, chapters and more |
