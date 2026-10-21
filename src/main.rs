#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icons;
mod library;
mod mpv;
mod progress;
mod thumbnails;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, TextureHandle, TextureOptions, vec2};

use mpv::Mpv;
use progress::Store;

const APP_NAME: &str = "Afterglow";
const SAVE_INTERVAL: Duration = Duration::from_secs(5);
const CONTROLS_HIDE_DELAY: Duration = Duration::from_millis(2500);
const NOTICE_DURATION: Duration = Duration::from_millis(1500);
const WARNING_DURATION: Duration = Duration::from_secs(6);
const ICON_SIZE: f32 = 17.0;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the next video is announced before it starts by itself.
const AUTOPLAY_DELAY: Duration = Duration::from_secs(5);

/// Set to run without OpenGL: the UI uses wgpu and video is rendered in software.
const SOFTWARE_VIDEO_ENV: &str = "AFTERGLOW_SOFTWARE_VIDEO";

fn main() -> eframe::Result {
    // OpenGL lets mpv draw video straight into the window.
    let software = std::env::var_os(SOFTWARE_VIDEO_ENV).is_some();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([480.0, 300.0])
            .with_drag_and_drop(true),
        renderer: if software { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow },
        ..Default::default()
    };
    let open: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            icons::install(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx, open, cc.get_proc_address.clone())))
        }),
    );
    // Without a usable OpenGL driver, start over without it. A window's event
    // loop can't be created twice in one process, hence a new process.
    let gl_failed = matches!(
        result,
        Err(eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..) | eframe::Error::OpenGL(_))
    );
    if gl_failed && !software {
        if let Err(e) = &result {
            eprintln!("OpenGL is unavailable ({e}); restarting with software video");
        }
        if let Ok(exe) = std::env::current_exe() {
            let restarted = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .env(SOFTWARE_VIDEO_ENV, "1")
                .status();
            if let Ok(status) = restarted {
                std::process::exit(status.code().unwrap_or(0));
            }
        }
    }
    result
}

enum Action {
    OpenFolder,
    OpenVideos,
    /// Folders and videos dropped onto the window or passed on the command line.
    Open(Vec<PathBuf>),
    Play(usize),
    Remove(usize),
    ClearPlaylist,
    /// Moves a playlist entry so it lands before the entry at `to` (or at the end).
    Move { from: usize, to: usize },
    Sort(SortBy),
    FocusSearch,
    ReverseOrder,
    SetIncludeSubfolders(bool),
    SetWatched(PathBuf, bool),
    /// Marks every video before this playlist entry as watched.
    MarkWatchedBefore(usize),
    ResetAllProgress,
    SetAutoplayNext(bool),
    CancelUpNext,
    TogglePause,
    Seek { to: f64, exact: bool },
    SeekBy(f64),
    Next,
    Prev,
    SetVolume(f64),
    ToggleMute,
    ToggleFullscreen,
    ExitFullscreen,
    ToggleSidebar,
    SetTrack { kind: &'static str, id: Option<i64> },
    CycleSubtitles,
    LoadSubtitle,
    AdjustSubDelay(f64),
    FrameStep { forward: bool },
    SetSpeed(f64),
    /// Jumps to the next chapter, or back to the start of this or the previous one.
    Chapter { forward: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Name,
    Modified,
    Length,
}

/// Playback state sampled from mpv once per frame.
#[derive(Default)]
struct Playback {
    position: f64,
    duration: f64,
    paused: bool,
    muted: bool,
    volume: f64,
    sub_delay: f64,
    speed: f64,
}

/// Finds the length of videos that haven't been played yet, on a background
/// thread with its own headless mpv.
struct DurationProbe {
    requests: Sender<PathBuf>,
    results: Receiver<(PathBuf, Option<f64>)>,
    pending: HashSet<PathBuf>,
}

impl DurationProbe {
    fn new(ctx: egui::Context) -> Self {
        let (requests, request_rx) = mpsc::channel::<PathBuf>();
        let (result_tx, results) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("duration-probe".into()).spawn(move || {
            let Ok(prober) = mpv::Prober::new() else { return };
            for path in request_rx {
                let duration = prober.duration(&path, PROBE_TIMEOUT);
                if result_tx.send((path, duration)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not start the duration probe: {e}");
        }
        Self { requests, results, pending: HashSet::new() }
    }

    fn request(&mut self, path: &Path) {
        // If the probe thread isn't running (e.g. mpv failed to start), the
        // send fails and the video simply stays without a length.
        if self.pending.insert(path.to_path_buf()) && self.requests.send(path.to_path_buf()).is_err() {
            self.pending.remove(path);
        }
    }
}

/// The video that starts by itself once the countdown ends.
struct UpNext {
    index: usize,
    at: Instant,
}

/// A short message shown over the video: feedback for an action, or a warning.
struct Notice {
    text: String,
    warning: bool,
    until: Instant,
}

struct App {
    mpv: Result<Mpv, String>,
    store: Store,
    playlist: Vec<PathBuf>,
    /// Index into `playlist` of the video that is playing (or loading).
    current: Option<usize>,
    /// True between mpv reporting the file as loaded and its end.
    loaded: bool,
    playback: Playback,
    texture: Option<TextureHandle>,
    show_sidebar: bool,
    fullscreen: bool,
    /// Seek-bar position while the user drags it.
    scrub: Option<f64>,
    last_save: Instant,
    last_activity: Instant,
    notice: Option<Notice>,
    actions: Vec<Action>,
    durations: DurationProbe,
    /// Chapters of the loaded video.
    chapters: Vec<mpv::Chapter>,
    /// Seek-bar previews, started the first time they are needed.
    thumbnails: Option<Result<thumbnails::Thumbnails, String>>,
    up_next: Option<UpNext>,
    /// The playlist entry being dragged to a new place.
    drag_row: Option<usize>,
    /// Only playlist entries whose name contains this are listed.
    search: String,
    focus_search: bool,
    /// Whether the "reset all progress" confirmation is open.
    confirm_reset: bool,
}

impl App {
    fn new(ctx: &egui::Context, open: Vec<PathBuf>, gl: Option<mpv::GetProcAddress>) -> Self {
        let (store, load_warning) = Store::load();
        let mpv = Mpv::new(ctx.clone(), gl);
        if let Ok(mpv) = &mpv {
            mpv.set("volume", &store.volume.to_string());
        }
        // Files that disappeared since last time are dropped from the playlist.
        let playlist = store.playlist.iter().filter(|p| p.is_file()).cloned().collect();
        let mut app = Self {
            mpv,
            store,
            playlist,
            current: None,
            loaded: false,
            playback: Playback::default(),
            texture: None,
            show_sidebar: true,
            fullscreen: false,
            scrub: None,
            last_save: Instant::now(),
            last_activity: Instant::now(),
            notice: None,
            actions: Vec::new(),
            durations: DurationProbe::new(ctx.clone()),
            chapters: Vec::new(),
            thumbnails: None,
            up_next: None,
            drag_row: None,
            search: String::new(),
            focus_search: false,
            confirm_reset: false,
        };
        if !open.is_empty() {
            app.open(open, ctx);
        } else if let Some(index) = app.continue_target() {
            // Pre-open where we left off, paused on that frame.
            app.load(index, true, ctx);
        }
        app.request_durations();
        if let Some(warning) = load_warning {
            app.warn(warning);
        }
        app
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), warning: false, until: Instant::now() + NOTICE_DURATION });
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), warning: true, until: Instant::now() + WARNING_DURATION });
    }

    fn current_path(&self) -> Option<&Path> {
        self.playlist.get(self.current?).map(PathBuf::as_path)
    }

    /// Queues every playlist video whose length isn't known yet.
    fn request_durations(&mut self) {
        for path in &self.playlist {
            if self.store.get(path).duration <= 0.0 {
                self.durations.request(path);
            }
        }
    }

    fn receive_durations(&mut self) {
        while let Ok((path, duration)) = self.durations.results.try_recv() {
            self.durations.pending.remove(&path);
            if let Some(d) = duration {
                self.store.set_duration(&path, d);
            }
        }
    }

    fn save_playlist(&mut self) {
        // Indices may have shifted.
        self.up_next = None;
        self.request_durations();
        self.store.set_playlist(&self.playlist);
        let current = self.current_path().map(Path::to_path_buf);
        if current.is_some() {
            self.store.set_current(current.as_deref());
        }
        self.store.save();
    }

    /// Opens dropped or passed paths. Folders replace the playlist with their
    /// videos (and start playing); videos are added to the end.
    fn open(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let (folders, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.is_dir());
        let (videos, others): (Vec<_>, Vec<_>) = files.into_iter().partition(|p| library::is_video(p));
        if let Some(other) = others.first() {
            self.warn(format!("Not a video: {}", display_name(other)));
        }
        if folders.is_empty() {
            self.add_videos(videos, ctx);
            return;
        }
        let mut list = Vec::new();
        for folder in &folders {
            match library::scan_folder(folder, self.store.include_subfolders) {
                Ok(found) => list.extend(found),
                Err(e) => self.warn(format!("Could not open {}: {e}", display_name(folder))),
            }
        }
        list.extend(videos);
        if list.is_empty() {
            let hint = if self.store.include_subfolders { "" } else { " (subfolders are not included)" };
            self.warn(format!("No videos found in {}{hint}", display_name(&folders[0])));
            return;
        }
        // Remember a single opened folder, so the subfolder setting can rescan it.
        self.store.set_folder((folders.len() == 1).then(|| folders[0].as_path()));
        self.replace_playlist(list, ctx);
    }

    /// Changes whether folders include their subfolders, and rescans the folder
    /// the playlist came from. Videos added by hand stay, and so does the
    /// playing video if it is still part of the list.
    fn set_include_subfolders(&mut self, include: bool, ctx: &egui::Context) {
        self.store.set_include_subfolders(include);
        self.store.save();
        let Some(folder) = self.store.folder.clone() else { return };
        let scanned = match library::scan_folder(&folder, include) {
            Ok(scanned) => scanned,
            Err(e) => return self.warn(format!("Could not open {}: {e}", display_name(&folder))),
        };
        let added_by_hand = self.playlist.iter().filter(|p| !p.starts_with(&folder)).cloned();
        let list: Vec<PathBuf> = scanned.into_iter().chain(added_by_hand).collect();
        let playing = self.current_path().map(Path::to_path_buf);
        if playing.as_ref().is_some_and(|p| !list.contains(p)) {
            self.stop(ctx);
        }
        self.current = playing.and_then(|p| list.iter().position(|q| *q == p));
        self.playlist = list;
        self.save_playlist();
        let what = if include { "with subfolders" } else { "without subfolders" };
        self.notify(format!("{}: {} videos {what}", display_name(&folder), self.playlist.len()));
    }

    fn replace_playlist(&mut self, list: Vec<PathBuf>, ctx: &egui::Context) {
        self.stop(ctx);
        self.playlist = list;
        self.show_sidebar = true;
        self.save_playlist();
        match self.continue_target() {
            Some(index) => self.play(index, ctx),
            None => self.notify(format!("{} videos, all watched", self.playlist.len())),
        }
    }

    /// Appends videos (skipping ones already listed). Starts the first of them
    /// when nothing is playing.
    fn add_videos(&mut self, videos: Vec<PathBuf>, ctx: &egui::Context) {
        if videos.is_empty() {
            return;
        }
        let mut first = None;
        let mut added = 0;
        for video in videos {
            let index = match self.playlist.iter().position(|p| *p == video) {
                Some(i) => i,
                None => {
                    self.playlist.push(video);
                    added += 1;
                    self.playlist.len() - 1
                }
            };
            first.get_or_insert(index);
        }
        self.save_playlist();
        match first {
            Some(index) if self.current.is_none() => self.play(index, ctx),
            _ if added > 0 => self.notify(format!("Added {added} video{} to the playlist", if added == 1 { "" } else { "s" })),
            _ => self.notify("Already in the playlist"),
        }
    }

    fn remove(&mut self, index: usize, ctx: &egui::Context) {
        if index >= self.playlist.len() {
            return;
        }
        match self.current {
            Some(c) if c == index => self.stop(ctx),
            Some(c) if c > index => self.current = Some(c - 1),
            _ => {}
        }
        self.playlist.remove(index);
        self.save_playlist();
    }

    /// Rearranges the playlist, keeping track of the playing video.
    fn reorder(&mut self, rearrange: impl FnOnce(&mut Vec<PathBuf>)) {
        let playing = self.current_path().map(Path::to_path_buf);
        rearrange(&mut self.playlist);
        self.current = playing.and_then(|p| self.playlist.iter().position(|q| *q == p));
        self.save_playlist();
    }

    fn sort_playlist(&mut self, by: SortBy) {
        match by {
            SortBy::Name => self.reorder(|list| {
                list.sort_by(|a, b| library::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()))
            }),
            SortBy::Modified => self.reorder(|list| {
                // Oldest first, like episodes saved in the order they came out.
                list.sort_by_cached_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
            }),
            SortBy::Length => {
                let lengths: std::collections::HashMap<PathBuf, f64> =
                    self.playlist.iter().map(|p| (p.clone(), self.store.get(p).duration)).collect();
                // Videos of unknown length go last.
                let key = |p: &PathBuf| match lengths[p] {
                    d if d > 0.0 => d,
                    _ => f64::INFINITY,
                };
                self.reorder(|list| list.sort_by(|a, b| key(a).total_cmp(&key(b))));
            }
        }
    }

    fn clear_playlist(&mut self, ctx: &egui::Context) {
        self.stop(ctx);
        self.playlist.clear();
        self.store.set_current(None);
        self.store.set_folder(None);
        self.save_playlist();
    }

    /// The video to offer when nothing is playing: the last one played if it is
    /// unfinished, otherwise the next unwatched one after it (wrapping around).
    /// `None` when everything is watched.
    fn continue_target(&self) -> Option<usize> {
        let watched = |i: usize| self.store.get(&self.playlist[i]).watched;
        let last = self.store.current.as_ref().and_then(|c| self.playlist.iter().position(|p| p == c));
        let start = match last {
            Some(i) if !watched(i) || self.store.get(&self.playlist[i]).resume_position().is_some() => return Some(i),
            Some(i) => i + 1,
            None => 0,
        };
        let n = self.playlist.len();
        (0..n).map(|k| (start + k) % n).find(|&i| !watched(i))
    }

    fn save_current_progress(&mut self) {
        if !self.loaded || self.playback.duration <= 0.0 {
            return;
        }
        if let Some(path) = self.current_path().map(Path::to_path_buf) {
            self.store.update_position(&path, self.playback.position, self.playback.duration);
        }
    }

    fn play(&mut self, index: usize, ctx: &egui::Context) {
        self.load(index, false, ctx);
    }

    /// Loads a playlist video at its saved position. With `paused`, it only
    /// shows that frame, ready to continue.
    fn load(&mut self, index: usize, paused: bool, ctx: &egui::Context) {
        self.up_next = None;
        self.save_current_progress();
        let Ok(mpv) = &self.mpv else { return };
        let Some(path) = self.playlist.get(index).cloned() else { return };
        let name = display_name(&path);
        let start = self.store.get(&path).resume_position();
        mpv.set("pause", if paused { "yes" } else { "no" });
        match mpv.load_file(&path, start.unwrap_or(0.0)) {
            Ok(()) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — {APP_NAME}")));
                // Drop the previous frame so it never shows under the new video.
                self.texture = None;
                mpv.forget_frame();
                self.chapters.clear();
                self.current = Some(index);
                self.loaded = false;
                self.store.set_current(Some(&path));
                self.store.save();
                match (paused, start) {
                    (true, Some(pos)) => self.notify(format!("{name}  ·  paused at {}", format_time(pos))),
                    (true, None) => self.notify(format!("{name}  ·  press Space to play")),
                    (false, Some(pos)) => self.notify(format!("{name}  ·  resuming at {}", format_time(pos))),
                    (false, None) => self.notify(name),
                }
            }
            Err(e) => self.warn(format!("Could not play {name}: {e}")),
        }
    }

    fn stop(&mut self, ctx: &egui::Context) {
        self.save_current_progress();
        if self.current.is_some() {
            if let Ok(mpv) = &self.mpv {
                let _ = mpv.command(&["stop"]);
            }
        }
        self.finish(ctx);
    }

    /// Clears the playing state once nothing is playing any more.
    fn finish(&mut self, ctx: &egui::Context) {
        self.current = None;
        self.loaded = false;
        self.chapters.clear();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(APP_NAME.into()));
    }

    fn file_loaded(&mut self) {
        let Ok(mpv) = &self.mpv else { return };
        self.loaded = true;
        self.chapters = mpv.chapters();
    }

    fn handle_mpv_events(&mut self, ctx: &egui::Context) {
        let Ok(mpv) = &self.mpv else { return };
        for event in mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.file_loaded(),
                mpv::Event::EndFile { eof, error } => {
                    self.loaded = false;
                    if error {
                        self.warn("Playback failed: the file could not be decoded.");
                        self.finish(ctx);
                        continue;
                    }
                    if !eof {
                        continue;
                    }
                    let Some(index) = self.current else { continue };
                    if let Some(path) = self.playlist.get(index).cloned() {
                        self.store.set_watched(&path, true);
                    }
                    self.finish(ctx);
                    if index + 1 >= self.playlist.len() {
                        self.notify("End of the playlist");
                    } else if self.store.autoplay_next {
                        self.up_next = Some(UpNext { index: index + 1, at: Instant::now() + AUTOPLAY_DELAY });
                    }
                    self.store.save();
                }
            }
        }
    }

    fn sample_playback(&mut self) {
        let Ok(mpv) = &self.mpv else { return };
        let p = &mut self.playback;
        p.paused = mpv.get_flag("pause");
        p.muted = mpv.get_flag("mute");
        p.volume = mpv.get_f64("volume").unwrap_or(p.volume);
        p.sub_delay = mpv.get_f64("sub-delay").unwrap_or(0.0);
        p.speed = mpv.get_f64("speed").unwrap_or(1.0);
        if self.loaded {
            p.position = mpv.get_f64("time-pos").unwrap_or(p.position);
            p.duration = mpv.get_f64("duration").unwrap_or(0.0);
            self.save_current_progress();
        }
        if self.last_save.elapsed() >= SAVE_INTERVAL {
            self.store.save();
            self.last_save = Instant::now();
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.actions.push(Action::Open(dropped));
        }

        // No shortcuts while typing or while a dialog is open (Esc closes it).
        if ctx.egui_wants_keyboard_input() || self.confirm_reset {
            return;
        }
        let Playback { volume, duration, speed, .. } = self.playback;
        ctx.input_mut(|i| {
            use egui::Modifiers as M;
            // egui ignores extra Shift/Alt when matching, so more specific
            // combinations must come before their plain counterparts.
            let mut bindings = vec![
                (M::CTRL | M::SHIFT, Key::O, Action::OpenFolder),
                (M::CTRL, Key::O, Action::OpenVideos),
                (M::CTRL, Key::F, Action::FocusSearch),
                (M::ALT, Key::Enter, Action::ToggleFullscreen),
                (M::SHIFT, Key::ArrowLeft, Action::SeekBy(-30.0)),
                (M::SHIFT, Key::ArrowRight, Action::SeekBy(30.0)),
                (M::NONE, Key::ArrowLeft, Action::SeekBy(-5.0)),
                (M::NONE, Key::ArrowRight, Action::SeekBy(5.0)),
                (M::NONE, Key::ArrowUp, Action::SetVolume((volume + 5.0).min(150.0))),
                (M::NONE, Key::ArrowDown, Action::SetVolume((volume - 5.0).max(0.0))),
                (M::NONE, Key::Space, Action::TogglePause),
                (M::NONE, Key::K, Action::TogglePause),
                (M::NONE, Key::F11, Action::ToggleFullscreen),
                (M::NONE, Key::F, Action::ToggleFullscreen),
                (M::NONE, Key::Escape, Action::ExitFullscreen),
                (M::NONE, Key::Home, Action::Seek { to: 0.0, exact: true }),
                (M::NONE, Key::Comma, Action::FrameStep { forward: false }),
                (M::NONE, Key::Period, Action::FrameStep { forward: true }),
                (M::NONE, Key::M, Action::ToggleMute),
                (M::NONE, Key::N, Action::Next),
                (M::NONE, Key::P, Action::Prev),
                (M::NONE, Key::S, Action::CycleSubtitles),
                (M::NONE, Key::L, Action::ToggleSidebar),
                (M::NONE, Key::Z, Action::AdjustSubDelay(-0.1)),
                (M::NONE, Key::X, Action::AdjustSubDelay(0.1)),
                (M::NONE, Key::OpenBracket, Action::SetSpeed(step_speed(speed, false))),
                (M::NONE, Key::CloseBracket, Action::SetSpeed(step_speed(speed, true))),
                (M::NONE, Key::Backspace, Action::SetSpeed(1.0)),
                (M::NONE, Key::PageUp, Action::Chapter { forward: false }),
                (M::NONE, Key::PageDown, Action::Chapter { forward: true }),
            ];
            // 0–9 jump to 0%–90% of the video.
            let digits = [
                Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4,
                Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
            ];
            for (n, key) in digits.into_iter().enumerate() {
                bindings.push((M::NONE, key, Action::Seek { to: duration * n as f64 / 10.0, exact: true }));
            }
            for (mods, key, action) in bindings {
                if i.consume_key(mods, key) {
                    self.actions.push(action);
                }
            }
        });
    }

    fn apply_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            self.apply(action, ctx);
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::OpenFolder => {
                if let Some(folder) = rfd::FileDialog::new().set_title("Open folder as playlist").pick_folder() {
                    self.open(vec![folder], ctx);
                }
            }
            Action::OpenVideos => {
                let files = rfd::FileDialog::new()
                    .set_title("Add videos to the playlist")
                    .add_filter("Videos", library::VIDEO_EXTENSIONS)
                    .pick_files();
                if let Some(files) = files {
                    self.add_videos(files, ctx);
                }
            }
            Action::Open(paths) => self.open(paths, ctx),
            Action::Play(index) => self.play(index, ctx),
            Action::Remove(index) => self.remove(index, ctx),
            Action::ClearPlaylist => self.clear_playlist(ctx),
            Action::Move { from, to } => self.reorder(|list| move_item(list, from, to)),
            Action::Sort(by) => {
                self.sort_playlist(by);
                let what = match by {
                    SortBy::Name => "name",
                    SortBy::Modified => "date modified",
                    SortBy::Length => "length",
                };
                self.notify(format!("Sorted by {what}"));
            }
            Action::ReverseOrder => self.reorder(|list| list.reverse()),
            Action::FocusSearch => {
                self.show_sidebar = true;
                self.focus_search = true;
            }
            Action::SetIncludeSubfolders(include) => self.set_include_subfolders(include, ctx),
            Action::SetWatched(path, watched) => {
                if self.loaded && self.current_path() == Some(path.as_path()) {
                    // Keep the live position from overwriting the change; a reset
                    // video that is loaded also jumps back to its start.
                    self.playback.position = if watched { self.playback.duration } else { 0.0 };
                    if !watched {
                        if let Ok(mpv) = &self.mpv {
                            let _ = mpv.command(&["seek", "0", "absolute+exact"]);
                        }
                    }
                }
                self.store.set_watched(&path, watched);
                self.store.save();
                let name = display_name(&path);
                self.notify(if watched { format!("{name}: marked as watched") } else { format!("{name}: progress reset") });
            }
            Action::MarkWatchedBefore(index) => {
                let before: Vec<PathBuf> = self.playlist.iter().take(index).cloned().collect();
                for path in &before {
                    self.store.set_watched(path, true);
                }
                if self.loaded && self.current.is_some_and(|c| c < index) {
                    // Keep the live position from overwriting the change.
                    self.playback.position = self.playback.duration;
                }
                self.store.save();
                self.notify(format!("Marked {} video{} as watched", before.len(), if before.len() == 1 { "" } else { "s" }));
            }
            Action::ResetAllProgress => {
                if self.loaded {
                    // The loaded video goes back to its start too, so the live
                    // position doesn't write its progress back.
                    self.playback.position = 0.0;
                    if let Ok(mpv) = &self.mpv {
                        let _ = mpv.command(&["seek", "0", "absolute+exact"]);
                    }
                }
                let reset = self.store.reset_progress();
                self.store.save();
                self.notify(format!("Progress reset for {reset} video{}", if reset == 1 { "" } else { "s" }));
            }
            Action::SetAutoplayNext(autoplay) => {
                self.store.set_autoplay_next(autoplay);
                self.store.save();
                if !autoplay {
                    self.up_next = None;
                }
            }
            Action::CancelUpNext => self.up_next = None,
            Action::Next | Action::Prev => {
                let next = matches!(action, Action::Next);
                let target = match self.current {
                    Some(i) if next => (i + 1 < self.playlist.len()).then_some(i + 1),
                    Some(i) => i.checked_sub(1),
                    None => self.continue_target(),
                };
                match target {
                    Some(index) => self.play(index, ctx),
                    None if self.current.is_some() => {
                        self.notify(if next { "This is the last video" } else { "This is the first video" });
                    }
                    None => {}
                }
            }
            Action::ToggleFullscreen | Action::ExitFullscreen => {
                let fullscreen = matches!(action, Action::ToggleFullscreen) && !self.fullscreen;
                if fullscreen != self.fullscreen {
                    self.fullscreen = fullscreen;
                    self.last_activity = Instant::now();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
                }
            }
            Action::ToggleSidebar => self.show_sidebar = !self.show_sidebar,
            Action::TogglePause => {
                if self.loaded {
                    if let Ok(mpv) = &self.mpv {
                        let _ = mpv.command(&["cycle", "pause"]);
                    }
                    self.notify(if self.playback.paused { "Playing" } else { "Paused" });
                } else if let Some(index) =
                    self.current.or(self.up_next.as_ref().map(|u| u.index)).or_else(|| self.continue_target())
                {
                    self.play(index, ctx);
                }
            }
            action => self.apply_to_mpv(action),
        }
    }

    fn apply_to_mpv(&mut self, action: Action) {
        let Ok(mpv) = &self.mpv else { return };
        let duration = self.playback.duration;
        match action {
            Action::Seek { to, exact } if self.loaded => {
                let flags = if exact { "absolute+exact" } else { "absolute+keyframes" };
                let _ = mpv.command(&["seek", &format!("{to:.3}"), flags]);
                if exact {
                    self.notify(format!("{} / {}", format_time(to), format_time(duration)));
                }
            }
            Action::SeekBy(by) if self.loaded => {
                let _ = mpv.command(&["seek", &by.to_string(), "relative"]);
                let to = (self.playback.position + by).clamp(0.0, duration);
                let sign = if by < 0.0 { "−" } else { "+" };
                self.notify(format!("{sign}{} s  ·  {} / {}", by.abs(), format_time(to), format_time(duration)));
            }
            Action::SetVolume(v) => {
                mpv.set("volume", &format!("{v:.0}"));
                mpv.set("mute", "no");
                self.store.set_volume(v.round());
                self.notify(format!("Volume {v:.0}%"));
            }
            Action::ToggleMute => {
                let _ = mpv.command(&["cycle", "mute"]);
                self.notify(if self.playback.muted { "Sound on" } else { "Muted" });
            }
            Action::SetTrack { kind, id } => {
                let prop = if kind == "sub" { "sid" } else { "aid" };
                mpv.set(prop, &id.map_or("no".into(), |id| id.to_string()));
            }
            Action::CycleSubtitles if self.loaded => {
                let _ = mpv.command(&["cycle", "sub"]);
                let text = match mpv.tracks("sub").into_iter().find(|t| t.selected) {
                    Some(track) => format!("Subtitles: {}", track.label),
                    None => "Subtitles off".into(),
                };
                self.notify(text);
            }
            Action::LoadSubtitle if self.loaded => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Load subtitle file")
                    .add_filter("Subtitles", &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"]);
                if let Some(dir) = self.current_path().and_then(Path::parent) {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(file) = dialog.pick_file() {
                    match mpv.command(&["sub-add", &file.to_string_lossy(), "select"]) {
                        Ok(()) => self.notify(format!("Subtitles: {}", display_name(&file))),
                        Err(e) => self.warn(format!("Could not load subtitles: {e}")),
                    }
                }
            }
            Action::FrameStep { forward } if self.loaded => {
                // Steps one frame and leaves playback paused.
                let _ = mpv.command(&[if forward { "frame-step" } else { "frame-back-step" }]);
            }
            Action::AdjustSubDelay(by) if self.loaded => {
                let _ = mpv.command(&["add", "sub-delay", &by.to_string()]);
                self.notify(format!("Subtitle delay {:+.1} s", self.playback.sub_delay + by));
            }
            Action::Chapter { forward } if self.loaded => {
                if self.chapters.is_empty() {
                    return self.notify("This video has no chapters");
                }
                match chapter_target(&self.chapters, self.playback.position, forward) {
                    Some(index) => {
                        let chapter = &self.chapters[index];
                        let _ = mpv.command(&["seek", &format!("{:.3}", chapter.time), "absolute+exact"]);
                        let text = format!("{}/{}  ·  {}", index + 1, self.chapters.len(), chapter.title);
                        self.notify(text);
                    }
                    None => self.notify("This is the last chapter"),
                }
            }
            Action::SetSpeed(speed) => {
                mpv.set("speed", &speed.to_string());
                self.notify(format!("Speed {}", format_speed(speed)));
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- UI

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if icon_button(ui, icons::SIDEBAR, "Show or hide the playlist (L)").clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
            ui.separator();
            if icon_button(ui, icons::FOLDER_PLUS, "Open folder: replace the playlist with its videos (Ctrl+Shift+O)").clicked() {
                self.actions.push(Action::OpenFolder);
            }
            if icon_button(ui, icons::FILE_VIDEO, "Add videos to the playlist (Ctrl+O)").clicked() {
                self.actions.push(Action::OpenVideos);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = egui::Button::new(RichText::new(icons::MORE).size(ICON_SIZE)).frame_when_inactive(false);
                let (response, _) = egui::containers::menu::MenuButton::from_button(more).ui(ui, |ui| {
                    let mut autoplay = self.store.autoplay_next;
                    let tip = "When a video ends, start the next one after a short countdown";
                    if ui.checkbox(&mut autoplay, "Play next video automatically").on_hover_text(tip).changed() {
                        self.actions.push(Action::SetAutoplayNext(autoplay));
                    }
                    ui.separator();
                    if ui.button(format!("{}  Reset all progress…", icons::RESET)).clicked() {
                        self.confirm_reset = true;
                    }
                });
                response.on_hover_text("More");
                // The title takes the space left between the buttons.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if let Some(path) = self.current_path() {
                        ui.separator();
                        ui.add(egui::Label::new(RichText::new(display_name(path)).strong()).truncate());
                    }
                });
            });
        });
    }

    /// Confirmation before wiping every video's progress.
    fn reset_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("reset-progress")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("{}  Reset all progress?", icons::WARNING)).size(18.0).strong());
            ui.add_space(8.0);
            ui.label("This forgets the saved position and watched state of every video. Your playlist and settings stay.");
            ui.add_space(4.0);
            ui.label(RichText::new("This can't be undone.").strong());
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let reset = egui::Button::new(RichText::new("Reset all progress").color(Color32::WHITE))
                    .fill(ui.visuals().error_fg_color);
                if ui.add(reset).clicked() {
                    self.actions.push(Action::ResetAllProgress);
                    self.confirm_reset = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_reset = false;
                }
            });
        });
        // Esc or a click outside cancels.
        if modal.should_close() {
            self.confirm_reset = false;
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("PLAYLIST").small().weak());
            if !self.playlist.is_empty() {
                let count = self.playlist.len();
                ui.label(RichText::new(format!("{count} video{}", if count == 1 { "" } else { "s" })).small().weak());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let clear = egui::Button::new(RichText::new(icons::TRASH).size(14.0)).frame_when_inactive(false);
                    if ui.add(clear).on_hover_text("Clear the playlist (files are not touched)").clicked() {
                        self.actions.push(Action::ClearPlaylist);
                    }
                    let sort = egui::Button::new(RichText::new(icons::SORT).size(14.0)).frame_when_inactive(false);
                    let (response, _) = egui::containers::menu::MenuButton::from_button(sort).ui(ui, |ui| {
                        ui.label(RichText::new("Sort by").small().weak());
                        for (by, label) in [(SortBy::Name, "Name"), (SortBy::Modified, "Date modified"), (SortBy::Length, "Length")] {
                            if ui.button(label).clicked() {
                                self.actions.push(Action::Sort(by));
                            }
                        }
                        ui.separator();
                        if ui.button("Reverse order").clicked() {
                            self.actions.push(Action::ReverseOrder);
                        }
                    });
                    response.on_hover_text("Sort the playlist (or drag videos to reorder)");
                });
            }
        });
        if !self.playlist.is_empty() {
            self.playlist_summary(ui);
        }
        let mut include = self.store.include_subfolders;
        let tip = "Also play videos in subfolders when opening a folder";
        if ui.checkbox(&mut include, "Include subfolders").on_hover_text(tip).changed() {
            self.actions.push(Action::SetIncludeSubfolders(include));
        }
        ui.separator();

        if self.playlist.is_empty() {
            ui.add_space(4.0);
            ui.weak("Empty. Open a folder to play its videos, or add videos one by one.");
            ui.add_space(4.0);
            ui.weak("You can also drop them onto the window.");
            return;
        }
        self.search_box(ui);
        let query = self.search.trim().to_lowercase();
        let folder = self.store.folder.clone();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut rows = Vec::new();
            for (index, path) in self.playlist.iter().enumerate() {
                // Videos from a subfolder of the opened folder show where they're from.
                let subfolder = folder.as_deref().and_then(|f| path.parent()?.strip_prefix(f).ok());
                let name = match subfolder {
                    Some(sub) if !sub.as_os_str().is_empty() => {
                        format!("{} / {}", sub.to_string_lossy().replace('\\', " / "), display_name(path))
                    }
                    _ => display_name(path),
                };
                if !query.is_empty() && !name.to_lowercase().contains(&query) {
                    continue;
                }
                let resp = playlist_row(ui, &self.store, &mut self.actions, index, path, &name, self.current == Some(index));
                if resp.drag_started() {
                    self.drag_row = Some(index);
                }
                rows.push((index, resp.rect));
            }
            if rows.is_empty() {
                ui.add_space(4.0);
                ui.weak(format!("No videos match \"{}\".", self.search.trim()));
            }
            self.drag_rows(ui, &rows);
        });
    }

    fn search_box(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let width = ui.available_width() - if self.search.is_empty() { 0.0 } else { 24.0 };
            let edit = egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search the playlist (Ctrl+F)", icons::SEARCH))
                .desired_width(width);
            let resp = ui.add(edit);
            if std::mem::take(&mut self.focus_search) {
                resp.request_focus();
            }
            // Esc clears the search; egui has already dropped focus by then.
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                self.search.clear();
            }
            if !self.search.is_empty() && icon_button(ui, icons::X, "Clear the search").clicked() {
                self.search.clear();
            }
        });
        ui.add_space(2.0);
    }

    /// While a row is dragged, shows where it would land, and moves it there
    /// when released.
    fn drag_rows(&mut self, ui: &mut egui::Ui, rows: &[(usize, egui::Rect)]) {
        let Some(from) = self.drag_row else { return };
        let (Some(pointer), Some(&(last, last_rect))) = (ui.ctx().pointer_interact_pos(), rows.last()) else {
            self.drag_row = None;
            return;
        };
        // Land before the first row whose middle is below the pointer.
        let (to, y) = match rows.iter().find(|(_, r)| pointer.y < r.center().y) {
            Some(&(index, rect)) => (index, rect.top()),
            None => (last + 1, last_rect.bottom()),
        };
        let unchanged = to == from || to == from + 1;
        if !unchanged {
            let x = last_rect.x_range();
            ui.painter().hline(x, y, (2.0, ui.visuals().selection.bg_fill));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        // Scroll when dragging near the top or bottom edge.
        let clip = ui.clip_rect();
        if pointer.y < clip.top() + 24.0 {
            ui.scroll_with_delta(vec2(0.0, 6.0));
        } else if pointer.y > clip.bottom() - 24.0 {
            ui.scroll_with_delta(vec2(0.0, -6.0));
        }
        if !ui.input(|i| i.pointer.any_down()) {
            self.drag_row = None;
            if !unchanged {
                self.actions.push(Action::Move { from, to });
            }
        }
    }

    /// Total and watched running time of the playlist, with a progress bar.
    fn playlist_summary(&self, ui: &mut egui::Ui) {
        let (mut total, mut watched, mut unknown) = (0.0, 0.0, 0);
        for path in &self.playlist {
            let entry = self.store.get(path);
            if entry.duration <= 0.0 {
                unknown += 1;
                continue;
            }
            total += entry.duration;
            watched += if entry.watched { entry.duration } else { entry.position.clamp(0.0, entry.duration) };
        }
        let measuring = unknown > 0 && !self.durations.pending.is_empty();
        let mut text = format!("{} of {} watched", format_total(watched), format_total(total));
        if measuring {
            text.push_str("  ·  measuring…");
        } else if unknown > 0 {
            text.push_str(&format!("  ·  {unknown} unknown"));
        }
        let tip = if unknown > 0 && !measuring {
            format!("The length of {unknown} video(s) couldn't be read; they aren't counted.")
        } else {
            format!("{} left to watch", format_total(total - watched))
        };
        ui.label(RichText::new(text).weak()).on_hover_text(tip);
        let fraction = if total > 0.0 { (watched / total) as f32 } else { 0.0 };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.inactive.bg_fill);
        let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
        ui.painter().rect_filled(filled, 2.0, ui.visuals().selection.bg_fill);
        ui.add_space(2.0);
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let Playback { position, duration, paused, muted, volume, .. } = self.playback;
        let position = self.scrub.unwrap_or(position);
        let has_prev = self.current.is_some_and(|i| i > 0);
        let has_next = self.current.is_some_and(|i| i + 1 < self.playlist.len());
        ui.horizontal(|ui| {
            if ui.add_enabled(has_prev, transport_button(icons::SKIP_BACK)).on_hover_text("Previous video (P)").clicked() {
                self.actions.push(Action::Prev);
            }
            let (icon, tip) = if self.loaded && !paused { (icons::PAUSE, "Pause (Space)") } else { (icons::PLAY, "Play (Space)") };
            if ui.add(transport_button(icon)).on_hover_text(tip).clicked() {
                self.actions.push(Action::TogglePause);
            }
            if ui.add_enabled(has_next, transport_button(icons::SKIP_FORWARD)).on_hover_text("Next video (N)").clicked() {
                self.actions.push(Action::Next);
            }
            ui.add_space(4.0);
            ui.monospace(format_time(if self.loaded { position } else { 0.0 }));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (fs_icon, fs_tip) = if self.fullscreen {
                    (icons::CORNERS_IN, "Exit fullscreen (F11 or Esc)")
                } else {
                    (icons::CORNERS_OUT, "Fullscreen (F11)")
                };
                if icon_button(ui, fs_icon, fs_tip).clicked() {
                    self.actions.push(Action::ToggleFullscreen);
                }
                self.track_menu(ui, "sub");
                self.track_menu(ui, "audio");
                self.speed_menu(ui);

                let mut volume = volume;
                let slider = egui::Slider::new(&mut volume, 0.0..=150.0).show_value(false);
                if ui.add_sized([80.0, 18.0], slider).on_hover_text(format!("Volume {volume:.0}% (Up/Down)")).changed() {
                    self.actions.push(Action::SetVolume(volume));
                }
                let speaker = match () {
                    _ if muted || volume == 0.0 => icons::SPEAKER_X,
                    _ if volume < 50.0 => icons::SPEAKER_LOW,
                    _ => icons::SPEAKER_HIGH,
                };
                if icon_button(ui, speaker, if muted { "Unmute (M)" } else { "Mute (M)" }).clicked() {
                    self.actions.push(Action::ToggleMute);
                }
                ui.monospace(format_time(if self.loaded { duration } else { 0.0 }));
                self.seek_bar(ui, position);
            });
        });
    }

    fn seek_bar(&mut self, ui: &mut egui::Ui, position: f64) {
        let duration = self.playback.duration;
        let size = vec2(ui.available_width().max(40.0), 20.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let active = self.loaded && duration > 0.0;
        let frac_at = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;

        if active {
            if let Some(pos) = resp.interact_pointer_pos() {
                let to = frac_at(pos.x) * duration;
                if resp.dragged() {
                    self.scrub = Some(to);
                    self.actions.push(Action::Seek { to, exact: false });
                }
                if resp.drag_stopped() || resp.clicked() {
                    self.scrub = None;
                    self.actions.push(Action::Seek { to, exact: true });
                }
            }
            if let Some(hover) = resp.hover_pos() {
                let at = frac_at(hover.x) * duration;
                let text = match chapter_at(&self.chapters, at) {
                    Some(i) => format!("{}  ·  {}", format_time(at), self.chapters[i].title),
                    None => format_time(at),
                };
                let preview = self.preview(ui.ctx(), at, duration);
                resp.clone().on_hover_ui_at_pointer(|ui| {
                    if let Some(texture) = preview {
                        let size = vec2(thumbnails::SIZE[0] as f32, thumbnails::SIZE[1] as f32);
                        ui.image((texture, size / ui.ctx().pixels_per_point().max(1.0)));
                    }
                    ui.label(text);
                });
            }
        }

        let visuals = ui.visuals();
        let track = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
        ui.painter().rect_filled(track, 2.0, visuals.widgets.inactive.bg_fill);
        if active {
            let frac = (position / duration).clamp(0.0, 1.0) as f32;
            let filled = egui::Rect::from_min_max(track.min, egui::pos2(track.left() + track.width() * frac, track.bottom()));
            ui.painter().rect_filled(filled, 2.0, visuals.selection.bg_fill);
            // Chapter starts show as small gaps in the track.
            for chapter in self.chapters.iter().filter(|c| c.time > 0.5 && c.time < duration) {
                let x = track.left() + track.width() * (chapter.time / duration) as f32;
                let gap = egui::Rect::from_center_size(egui::pos2(x, track.center().y), vec2(2.0, track.height() + 2.0));
                ui.painter().rect_filled(gap, 0.0, visuals.panel_fill);
            }
            if resp.hovered() || resp.dragged() {
                ui.painter().circle_filled(egui::pos2(filled.right(), track.center().y), 6.0, visuals.selection.bg_fill);
            }
        }
    }

    /// A preview frame of the playing video near `at`, starting the previewer
    /// on first use. The frame may still be from a moment ago.
    fn preview(&mut self, ctx: &egui::Context, at: f64, duration: f64) -> Option<egui::TextureId> {
        let path = self.current_path()?.to_path_buf();
        let thumbnails = self.thumbnails.get_or_insert_with(|| thumbnails::Thumbnails::new(ctx)).as_mut().ok()?;
        // About 300 distinct previews along the video, at most one per second.
        thumbnails.request(&path, at, (duration / 300.0).max(1.0));
        thumbnails.texture(&path).map(TextureHandle::id)
    }

    fn track_menu(&mut self, ui: &mut egui::Ui, kind: &'static str) {
        let (icon, tip) = if kind == "sub" { (icons::SUBTITLES, "Subtitles (S to cycle)") } else { (icons::WAVEFORM, "Audio track") };
        ui.add_enabled_ui(self.loaded, |ui| {
            let button = egui::Button::new(RichText::new(icon).size(ICON_SIZE)).frame_when_inactive(false);
            let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                let Ok(mpv) = &self.mpv else { return };
                let tracks = mpv.tracks(kind);
                ui.label(RichText::new(if kind == "sub" { "Subtitles" } else { "Audio track" }).small().weak());
                if kind == "sub" {
                    let none_selected = !tracks.iter().any(|t| t.selected);
                    if ui.selectable_label(none_selected, "Off").clicked() {
                        self.actions.push(Action::SetTrack { kind, id: None });
                    }
                }
                for t in &tracks {
                    if ui.selectable_label(t.selected, &t.label).clicked() {
                        self.actions.push(Action::SetTrack { kind, id: Some(t.id) });
                    }
                }
                if tracks.is_empty() && kind != "sub" {
                    ui.weak("No audio tracks");
                }
                if kind == "sub" {
                    ui.separator();
                    if ui.button(format!("{}  Load subtitle file…", icons::PLUS)).clicked() {
                        self.actions.push(Action::LoadSubtitle);
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!("Delay {:+.1} s", self.playback.sub_delay));
                        if ui.small_button(icons::MINUS).on_hover_text("Show earlier (Z)").clicked() {
                            self.actions.push(Action::AdjustSubDelay(-0.1));
                        }
                        if ui.small_button(icons::PLUS).on_hover_text("Show later (X)").clicked() {
                            self.actions.push(Action::AdjustSubDelay(0.1));
                        }
                    });
                }
            });
            response.on_hover_text(tip);
        });
    }

    fn speed_menu(&mut self, ui: &mut egui::Ui) {
        let speed = self.playback.speed;
        let label = RichText::new(format_speed(speed)).monospace();
        let label = if speed == 1.0 { label.weak() } else { label.strong() };
        let button = egui::Button::new(label).frame_when_inactive(false);
        let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
            ui.label(RichText::new("Speed").small().weak());
            for preset in SPEEDS {
                if ui.selectable_label((speed - preset).abs() < 0.001, format_speed(preset)).clicked() {
                    self.actions.push(Action::SetSpeed(preset));
                }
            }
        });
        response.on_hover_text("Playback speed ([ and ], Backspace to reset)");
    }

    fn video_area(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, egui::Id::unique("video"), Sense::click());
        if self.loaded {
            self.paint_frame(ui, rect);
            if resp.double_clicked() {
                self.actions.push(Action::ToggleFullscreen);
            }
        } else {
            self.placeholder(ui, rect);
        }
        self.paint_notice(ui, rect);
    }

    /// Takes mpv's latest frame into the video texture.
    fn update_frame(&mut self, ctx: &egui::Context) {
        let Ok(mpv) = &self.mpv else { return };
        if let Some(image) = mpv.take_frame() {
            match &mut self.texture {
                Some(tex) => tex.set(image, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("video", image, TextureOptions::LINEAR)),
            }
        }
    }

    /// Paints the latest video frame from mpv fitted into `rect`.
    fn paint_frame(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Ok(mpv) = &self.mpv else { return };
        // On the GPU, mpv fits the video into the rect itself.
        if let Some(callback) = mpv.paint_callback(rect) {
            ui.painter().add(callback);
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        mpv.set_render_size((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        if let Some(tex) = &self.texture {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let img_rect = egui::Rect::from_center_size(rect.center(), size * scale);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex.id(), img_rect, uv, Color32::WHITE);
        }
    }

    /// The current notice, as a pill in the top-left corner of the video.
    fn paint_notice(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Some(notice) = &self.notice else { return };
        let now = Instant::now();
        if now >= notice.until {
            self.notice = None;
            return;
        }
        // Fade out over the last 300 ms.
        let left = (notice.until - now).as_secs_f32();
        let alpha = (left / 0.3).min(1.0);
        let color = if notice.warning { Color32::from_rgb(255, 196, 92) } else { Color32::WHITE };
        let galley = ui.painter().layout(
            notice.text.clone(),
            egui::FontId::proportional(16.0),
            color.gamma_multiply(alpha),
            (rect.width() - 64.0).max(100.0),
        );
        let pos = rect.left_top() + vec2(20.0, 18.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(vec2(12.0, 7.0));
        ui.painter().rect_filled(bg, 8.0, Color32::from_black_alpha((170.0 * alpha) as u8));
        ui.painter().galley(pos, galley, Color32::WHITE);
        ui.ctx().request_repaint_after(Duration::from_millis(30).min(notice.until - now));
    }

    fn placeholder(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)));
        child.add_space(rect.height() * 0.36);
        if let Err(e) = &self.mpv {
            child.label(RichText::new(format!("{}  Afterglow can't play videos", icons::WARNING)).size(18.0));
            child.add_space(6.0);
            child.colored_label(child.visuals().error_fg_color, e);
            child.add_space(6.0);
            child.label(format!(
                "It needs {} from an mpv build (the \"mpv-dev-x86_64\" archive), placed next to afterglow.exe.",
                mpv::DLL_NAME
            ));
            child.add_space(6.0);
            child.hyperlink_to("Download libmpv", mpv::DOWNLOAD_URL);
            if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
                if child.button("Open Afterglow's folder").clicked() {
                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                }
            }
            child.weak("Then restart Afterglow.");
            return;
        }
        if self.current.is_some() {
            child.weak("Loading…");
            return;
        }
        if let Some(up_next) = &self.up_next {
            let (index, left) = (up_next.index, up_next.at.saturating_duration_since(Instant::now()));
            child.label(RichText::new("Up next").weak());
            child.add_space(4.0);
            child.label(RichText::new(display_name(&self.playlist[index])).size(20.0).strong());
            child.add_space(4.0);
            child.weak(format!("Starts in {} s", left.as_secs_f32().ceil() as u32));
            child.add_space(10.0);
            child.horizontal(|ui| {
                // Centre the two buttons under the text.
                ui.add_space((ui.available_width() - 150.0).max(0.0) / 2.0);
                if ui.button(format!("{}  Play now", icons::PLAY)).on_hover_text("Space").clicked() {
                    self.actions.push(Action::Play(index));
                }
                if ui.button("Cancel").clicked() {
                    self.actions.push(Action::CancelUpNext);
                }
            });
            child.ctx().request_repaint_after(Duration::from_millis(200).min(left));
            return;
        }
        if let Some(index) = self.continue_target() {
            let path = &self.playlist[index];
            let resume = self.store.get(path).resume_position();
            let verb = if resume.is_some() { "Continue" } else { "Play" };
            let text = RichText::new(format!("{}   {verb}: {}", icons::PLAY, display_name(path))).size(18.0);
            if child.button(text).on_hover_text("Space").clicked() {
                self.actions.push(Action::Play(index));
            }
            if let Some(pos) = resume {
                child.weak(format!("from {}", format_time(pos)));
            }
            return;
        }

        let (title, hint) = if self.playlist.is_empty() {
            ("Drop a folder or videos here", "A folder plays all of its videos in order.")
        } else {
            ("Everything in the playlist is watched", "Pick a video to watch it again, or open something new.")
        };
        child.label(RichText::new(icons::FOLDER_PLUS).size(44.0).weak());
        child.add_space(6.0);
        child.label(RichText::new(title).size(18.0));
        child.weak(hint);
    }

    fn drop_overlay(&self, ctx: &egui::Context, rect: egui::Rect) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop")));
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(190));
        let accent = ctx.global_style().visuals.selection.bg_fill;
        painter.rect_stroke(rect.shrink(16.0), 12.0, (2.0, accent), egui::StrokeKind::Inside);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop a folder to play it, or videos to add them to the playlist",
            egui::FontId::proportional(20.0),
            Color32::WHITE,
        );
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let window = ui.max_rect();
        self.handle_mpv_events(&ctx);
        self.sample_playback();
        self.handle_input(&ctx);
        self.receive_durations();
        self.update_frame(&ctx);
        if let Some(Ok(thumbnails)) = &mut self.thumbnails {
            thumbnails.update(&ctx);
        }

        // In fullscreen, controls appear on mouse movement and hide after a
        // moment, but stay up while paused or idle.
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down());
        if moved {
            self.last_activity = Instant::now();
        }
        let show_chrome = !self.fullscreen
            || !self.loaded
            || self.playback.paused
            || self.last_activity.elapsed() < CONTROLS_HIDE_DELAY;
        if !show_chrome {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }

        if !self.fullscreen {
            egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        }
        if show_chrome {
            egui::Panel::bottom("controls")
                .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6)))
                .show(ui, |ui| self.controls(ui));
        }
        if !self.fullscreen && self.show_sidebar {
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(280.0)
                .size_range(180.0..=600.0)
                .show(ui, |ui| self.sidebar(ui));
        }
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| self.video_area(ui));
        self.drop_overlay(&ctx, window);
        if self.confirm_reset {
            self.reset_dialog(&ctx);
        }

        if let Some(up_next) = &self.up_next {
            if Instant::now() >= up_next.at {
                let index = up_next.index;
                self.actions.push(Action::Play(index));
            }
        }
        self.apply_actions(&ctx);

        if self.loaded && !self.playback.paused {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.fullscreen && show_chrome {
            ctx.request_repaint_after(CONTROLS_HIDE_DELAY);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let (Ok(mpv), Some(gl)) = (&self.mpv, gl) {
            mpv.release_gl(gl);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_current_progress();
        self.store.save();
    }
}

/// A playlist entry with its watch progress.
fn playlist_row(
    ui: &mut egui::Ui,
    store: &Store,
    actions: &mut Vec<Action>,
    index: usize,
    path: &Path,
    name: &str,
    playing: bool,
) -> egui::Response {
    let entry = store.get(path);
    let text = if entry.watched {
        RichText::new(format!("{}  {name}", icons::CHECK)).weak()
    } else {
        RichText::new(name)
    };
    let length = if entry.duration > 0.0 { format_time(entry.duration) } else { String::new() };
    let button = egui::Button::selectable(playing, "")
        .left_text(text)
        .right_text(RichText::new(length).weak())
        .sense(Sense::click_and_drag());
    let resp = ui.add_sized([ui.available_width(), 22.0], button);
    if (!entry.watched && entry.fraction() > 0.0) || entry.resume_position().is_some() {
        let r = resp.rect.shrink2(vec2(4.0, 1.0));
        let end = egui::pos2(r.left() + r.width() * entry.fraction(), r.bottom());
        ui.painter().line_segment([r.left_bottom(), end], (2.0, ui.visuals().selection.bg_fill));
    }
    let resp = resp.on_hover_text(format!("{}\n{}", path.display(), hover_text(&entry)));
    if resp.clicked() {
        actions.push(Action::Play(index));
    }
    resp.context_menu(|ui| {
        if ui.button(format!("{}  Play from start", icons::PLAY)).clicked() {
            actions.push(Action::SetWatched(path.to_path_buf(), false));
            actions.push(Action::Play(index));
        }
        if !entry.watched && ui.button(format!("{}  Mark as watched", icons::CHECK)).clicked() {
            actions.push(Action::SetWatched(path.to_path_buf(), true));
        }
        if index > 0 && ui.button(format!("{}  Mark all above as watched", icons::CHECKS)).clicked() {
            actions.push(Action::MarkWatchedBefore(index));
        }
        // Also offered for half-watched videos: resets progress without playing.
        if entry.watched || entry.position > 0.0 {
            let reset = ui.button(format!("{}  Mark as unwatched", icons::RESET)).on_hover_text("Reset its progress to the start");
            if reset.clicked() {
                actions.push(Action::SetWatched(path.to_path_buf(), false));
            }
        }
        ui.separator();
        if ui.button(format!("{}  Remove from playlist", icons::X)).on_hover_text("The file is not touched").clicked() {
            actions.push(Action::Remove(index));
        }
    });
    resp
}

fn transport_button(icon: &str) -> egui::Button<'static> {
    egui::Button::new(icons::fill(icon).size(ICON_SIZE)).frame_when_inactive(false).min_size(vec2(30.0, 26.0))
}

fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(icon).size(ICON_SIZE)).frame_when_inactive(false))
        .on_hover_text(tooltip)
}

fn display_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn hover_text(entry: &progress::Entry) -> String {
    if entry.watched {
        "Watched · right-click for options".into()
    } else if entry.position > 0.0 {
        format!("{} / {} · right-click for options", format_time(entry.position), format_time(entry.duration))
    } else {
        "Not started · right-click for options".into()
    }
}

/// A running time for totals: "4 h 12 min", "42 min", "30 s".
fn format_total(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    match (s / 3600, (s / 60) % 60) {
        (0, 0) => format!("{s} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Moves the item at `from` so it lands before the item now at `to`, or at
/// the end when `to` is the length.
fn move_item<T>(list: &mut Vec<T>, from: usize, to: usize) {
    if from >= list.len() || to > list.len() {
        return;
    }
    let item = list.remove(from);
    list.insert(if to > from { to - 1 } else { to }, item);
}

/// Going back within this many seconds of a chapter's start goes to the
/// previous chapter instead of restarting this one.
const CHAPTER_BACK_THRESHOLD: f64 = 3.0;

/// The chapter playing at `time`.
fn chapter_at(chapters: &[mpv::Chapter], time: f64) -> Option<usize> {
    chapters.iter().rposition(|c| c.time <= time + 0.01)
}

/// The chapter to jump to from `position`: the next one, or going back, the
/// start of the current one (or the previous one if it just started).
fn chapter_target(chapters: &[mpv::Chapter], position: f64, forward: bool) -> Option<usize> {
    let current = chapter_at(chapters, position);
    if forward {
        let next = current.map_or(0, |i| i + 1);
        return (next < chapters.len()).then_some(next);
    }
    match current {
        Some(i) if position - chapters[i].time > CHAPTER_BACK_THRESHOLD => Some(i),
        Some(i) => Some(i.saturating_sub(1)),
        None => (!chapters.is_empty()).then_some(0),
    }
}

const SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

/// The next preset speed above (or below) `speed`, staying within the presets.
fn step_speed(speed: f64, up: bool) -> f64 {
    let next = if up {
        SPEEDS.iter().copied().find(|&s| s > speed + 0.001)
    } else {
        SPEEDS.iter().rev().copied().find(|&s| s < speed - 0.001)
    };
    next.unwrap_or(if up { SPEEDS[SPEEDS.len() - 1] } else { SPEEDS[0] })
}

/// "1×", "1.25×", "0.5×".
fn format_speed(speed: f64) -> String {
    let text = format!("{speed:.2}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
}

fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::{chapter_at, chapter_target, format_speed, format_time, format_total, icons, move_item, step_speed};
    use eframe::egui::{Context, FontFamily, FontId};

    #[test]
    fn formats_times() {
        assert_eq!(format_total(30.0), "30 s");
        assert_eq!(format_total(42.0 * 60.0), "42 min");
        assert_eq!(format_total(2.0 * 3600.0), "2 h");
        assert_eq!(format_total(4.0 * 3600.0 + 12.0 * 60.0 + 20.0), "4 h 12 min");
        assert_eq!(format_time(65.0), "01:05");
        assert_eq!(format_time(3725.0), "1:02:05");
    }

    #[test]
    fn moves_items() {
        let moved = |from, to| {
            let mut list = vec!['a', 'b', 'c', 'd'];
            move_item(&mut list, from, to);
            list.into_iter().collect::<String>()
        };
        assert_eq!(moved(0, 2), "bacd", "before c");
        assert_eq!(moved(3, 1), "adbc", "before b");
        assert_eq!(moved(1, 4), "acdb", "to the end");
        assert_eq!(moved(2, 0), "cabd", "to the start");
        assert_eq!(moved(1, 1), "abcd");
        assert_eq!(moved(1, 2), "abcd", "before the next one is where it already is");
        assert_eq!(moved(9, 0), "abcd", "out of range is ignored");
    }

    #[test]
    fn finds_chapter_targets() {
        let chapters: Vec<_> = [0.0, 60.0, 120.0]
            .into_iter()
            .map(|time| crate::mpv::Chapter { title: String::new(), time })
            .collect();
        assert_eq!(chapter_at(&chapters, 59.0), Some(0));
        assert_eq!(chapter_at(&chapters, 60.0), Some(1));
        assert_eq!(chapter_target(&chapters, 30.0, true), Some(1));
        assert_eq!(chapter_target(&chapters, 130.0, true), None, "no chapter after the last");
        assert_eq!(chapter_target(&chapters, 90.0, false), Some(1), "back to the start of this one");
        assert_eq!(chapter_target(&chapters, 61.0, false), Some(0), "just started: the previous one");
        assert_eq!(chapter_target(&chapters, 1.0, false), Some(0));
        assert_eq!(chapter_target(&[], 1.0, false), None);
    }

    #[test]
    fn steps_through_speeds() {
        assert_eq!(format_speed(1.0), "1×");
        assert_eq!(format_speed(1.25), "1.25×");
        assert_eq!(format_speed(0.5), "0.5×");
        assert_eq!(step_speed(1.0, true), 1.25);
        assert_eq!(step_speed(1.0, false), 0.75);
        assert_eq!(step_speed(1.1, false), 1.0, "off-preset speeds snap to the next preset");
        assert_eq!(step_speed(3.0, true), 3.0);
        assert_eq!(step_speed(0.25, false), 0.25);
    }

    #[test]
    fn icons_and_text_glyphs_exist() {
        let ctx = Context::default();
        icons::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let regular = [
            icons::SIDEBAR, icons::FOLDER_PLUS, icons::FILE_VIDEO, icons::CORNERS_OUT, icons::CORNERS_IN,
            icons::SPEAKER_HIGH, icons::SPEAKER_LOW, icons::SPEAKER_X, icons::SUBTITLES, icons::WAVEFORM,
            icons::CHECK, icons::PLUS, icons::MINUS, icons::X, icons::TRASH, icons::RESET, icons::MORE, icons::WARNING, icons::PLAY, icons::SORT, icons::SEARCH, icons::CHECKS,
            "—", "·", "…", "−", "×",
        ];
        let fill = [icons::PLAY, icons::PAUSE, icons::SKIP_BACK, icons::SKIP_FORWARD];
        let has = |family: FontFamily, s: &str| ctx.fonts_mut(|f| f.has_glyphs(&FontId::new(14.0, family), s));
        let missing: Vec<_> = regular.iter().filter(|s| !has(FontFamily::Proportional, s)).collect();
        assert!(missing.is_empty(), "missing regular glyphs: {missing:?}");
        let missing: Vec<_> = fill.iter().filter(|s| !has(FontFamily::Name("phosphor-fill".into()), s)).collect();
        assert!(missing.is_empty(), "missing fill glyphs: {missing:?}");
    }
}
