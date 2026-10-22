#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icons;
mod library;
mod media_keys;
mod mpv;
mod power;
mod progress;
mod thumbnails;
mod watch;

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
/// Padding inside text fields.
const TEXT_MARGIN: egui::Margin = egui::Margin::symmetric(8, 5);
/// How close to the video's edge the pointer brings out the unpinned playlist.
const EDGE_WIDTH: f32 = 24.0;
/// How long the unpinned playlist stays after the pointer leaves it.
const PEEK_HIDE_DELAY: Duration = Duration::from_millis(300);
/// Height of a playlist entry, without the spacing between entries.
const PLAYLIST_ROW_HEIGHT: f32 = 26.0;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Scroll distance, in points, of one mouse wheel notch.
const SCROLL_NOTCH: f32 = 50.0;
/// Tags the reply to a subtitle step sent to mpv.
const SUB_STEP_TAG: u64 = 1;
/// Tags the replies to subtitle files added back when a video is opened.
const SUB_RESTORE_TAG: u64 = 2;
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
            set_spacing(&cc.egui_ctx);
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
    /// Folders and videos dropped onto the window.
    Drop(Vec<PathBuf>),
    /// Answers what dropped videos should do, and remembers it.
    ChooseDropBehavior(progress::DroppedVideos),
    Play(usize),
    Remove(usize),
    ClearPlaylist,
    /// Moves a playlist entry so it lands before the entry at `to` (or at the end).
    Move { from: usize, to: usize },
    Sort(SortBy),
    FocusSearch,
    ReverseOrder,
    /// Rescans the playlist's folder now: adds new videos, drops missing ones.
    RefreshFolder,
    ClearRecentFolders,
    ShowHelp,
    /// Docks the playlist beside the video, or lets it slide out over it.
    TogglePin,
    OpenSettings,
    SetSettings(progress::Settings),
    SetIncludeSubfolders(bool),
    SetWatched(PathBuf, bool),
    /// Marks every video before this playlist entry as watched.
    MarkWatchedBefore(usize),
    ResetAllProgress,
    SetAutoplayNext(bool),
    CancelUpNext,
    TogglePause,
    /// Pauses or plays (starting a video if none is loaded).
    SetPaused(bool),
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
    CycleAudio,
    /// Marks the playing video as watched, or unwatched if it is.
    ToggleWatched,
    /// Leaves fullscreen, or else closes the subtitle sync panel.
    Escape,
    LoadSubtitle,
    AdjustSubDelay(f64),
    SetSubDelay(f64),
    /// Shifts subtitles so the previous (-1) or next (1) line shows right now.
    SubStep(i32),
    /// Marks the moment a subtitle line appears, or when it is actually
    /// spoken. Once both are marked, subtitles shift by the difference.
    MarkLine { spoken: bool },
    /// Stretches subtitle timing, for subtitles made for another frame rate.
    SetSubSpeed(f64),
    ToggleSyncPanel,
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
    sub_speed: f64,
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

/// The unpinned playlist, slid out over the video.
#[derive(Default)]
struct Peek {
    /// Whether the pointer has been over it; only then does it hide when the
    /// pointer leaves (opened from the keyboard, it stays until then).
    entered: bool,
    /// When the pointer left it.
    left_at: Option<Instant>,
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
    /// The unpinned playlist while it is out.
    peek: Option<Peek>,
    /// Width of the docked playlist, which the slid-out one copies.
    sidebar_width: f32,
    /// Where the floating fullscreen controls were last drawn.
    controls_rect: Option<egui::Rect>,
    /// Scrolling over the video not yet turned into volume steps.
    scroll_volume: f32,
    notice: Option<Notice>,
    actions: Vec<Action>,
    durations: DurationProbe,
    /// Chapters of the loaded video.
    chapters: Vec<mpv::Chapter>,
    /// Audio, video and subtitle tracks of the loaded video.
    tracks: Vec<mpv::Track>,
    /// Whether track choices for this video were changed by hand, so they
    /// are saved as they come in.
    tracks_changed_by_user: bool,
    /// The kind of track ("sub" or "audio") to announce once mpv reports it.
    announce_track: Option<&'static str>,
    /// A subtitle step waiting for mpv to report whether there was a line.
    announce_sub_step: Option<&'static str>,
    /// Saved track choices being restored: how many subtitle files are still
    /// loading, and the choices to apply after.
    restoring: Option<(usize, progress::TrackPrefs)>,
    /// Seek-bar previews, once started in the background (see `thumbnails_ready`).
    thumbnails: Option<Result<thumbnails::Thumbnails, String>>,
    thumbnails_ready: Option<Receiver<Result<thumbnails::Thumbnails, String>>>,
    up_next: Option<UpNext>,
    watch: watch::FolderWatch,
    /// Playlist files found missing at startup.
    missing_files: Option<Receiver<HashSet<PathBuf>>>,
    /// The playlist entry being dragged to a new place.
    drag_row: Option<usize>,
    /// Only playlist entries whose name contains this are listed.
    search: String,
    focus_search: bool,
    /// Which search result Enter plays.
    search_pick: usize,
    /// Scroll the list to this search result on the next frame.
    scroll_to_pick: Option<usize>,
    /// Display names of the playlist entries, and their lowercase forms.
    names: Vec<(String, String)>,
    /// When the playlist summary was added up: total and watched seconds, and
    /// how many lengths are unknown.
    summary: Option<(Instant, f64, f64, usize)>,
    /// Whether the subtitle sync panel is open.
    sync_panel: bool,
    /// When a subtitle line appeared and when it was spoken, for syncing.
    line_marks: [Option<f64>; 2],
    /// Whether the "reset all progress" confirmation is open.
    confirm_reset: bool,
    /// Whether the keyboard shortcut overview is open.
    show_help: bool,
    /// Whether the settings dialog is open.
    settings_open: bool,
    /// Dropped videos waiting for the answer to what drops should do.
    pending_drop: Option<Vec<PathBuf>>,
    /// The system media controls, set up once the window exists.
    media_keys: Option<Result<media_keys::MediaKeys, String>>,
    sleep_blocker: power::SleepBlocker,
}

impl App {
    fn new(ctx: &egui::Context, open: Vec<PathBuf>, gl: Option<mpv::GetProcAddress>) -> Self {
        let (store, load_warning) = Store::load();
        let mpv = Mpv::new(ctx.clone(), gl);
        if let Ok(mpv) = &mpv {
            mpv.set("volume", &store.volume.to_string());
            // Reading properties can block while mpv waits for the UI thread to
            // draw a frame, so playback state comes in as events instead.
            for name in [
                "time-pos", "duration", "pause", "mute", "volume", "speed", "sub-delay", "sub-speed",
                "chapter-list", "track-list",
            ] {
                mpv.observe(name);
            }
        }
        let playlist: Vec<PathBuf> = store.playlist.clone();
        // Files that disappeared since last time are dropped from the playlist,
        // checked in the background since a network drive can take a while.
        let missing_files = {
            let (tx, rx) = mpsc::channel();
            let (paths, ctx) = (playlist.clone(), ctx.clone());
            let spawned = std::thread::Builder::new().name("check-files".into()).spawn(move || {
                let missing: HashSet<PathBuf> = paths.into_iter().filter(|p| !p.is_file()).collect();
                if !missing.is_empty() && tx.send(missing).is_ok() {
                    ctx.request_repaint();
                }
            });
            spawned.is_ok().then_some(rx)
        };
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
            controls_rect: None,
            peek: None,
            sidebar_width: 280.0,
            scroll_volume: 0.0,
            notice: None,
            actions: Vec::new(),
            durations: DurationProbe::new(ctx.clone()),
            chapters: Vec::new(),
            tracks: Vec::new(),
            tracks_changed_by_user: false,
            announce_track: None,
            announce_sub_step: None,
            restoring: None,
            thumbnails: None,
            thumbnails_ready: Some(thumbnails::Thumbnails::start(ctx)),
            up_next: None,
            watch: watch::FolderWatch::new(ctx.clone(), None),
            missing_files,
            drag_row: None,
            search: String::new(),
            focus_search: false,
            search_pick: 0,
            scroll_to_pick: None,
            names: Vec::new(),
            summary: None,
            sync_panel: false,
            line_marks: [None, None],
            confirm_reset: false,
            show_help: false,
            settings_open: false,
            pending_drop: None,
            media_keys: None,
            sleep_blocker: power::SleepBlocker::default(),
        };
        // Before opening anything, so language preferences apply to it.
        app.apply_settings(ctx);
        if !open.is_empty() {
            app.open(open, false, ctx);
        } else if let Some(index) = app.continue_target() {
            // Pre-open where we left off, paused on that frame.
            app.load(index, true, ctx);
        }
        app.request_durations();
        app.sync_watch();
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
        self.names.clear();
        self.summary = None;
        self.request_durations();
        self.store.set_playlist(&self.playlist);
        let current = self.current_path().map(Path::to_path_buf);
        if current.is_some() {
            self.store.set_current(current.as_deref());
        }
        self.store.save();
    }

    /// Opens dropped or passed paths. Folders replace the playlist with their
    /// videos (and start playing). Videos are added to the end, unless they
    /// were dropped: then the drop setting decides, asking the first time.
    fn open(&mut self, paths: Vec<PathBuf>, dropped: bool, ctx: &egui::Context) {
        let (folders, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.is_dir());
        let (videos, others): (Vec<_>, Vec<_>) = files.into_iter().partition(|p| library::is_video(p));
        if let Some(other) = others.first() {
            self.warn(format!("Not a video: {}", display_name(other)));
        }
        if folders.is_empty() {
            match self.store.settings.dropped_videos {
                _ if !dropped || videos.is_empty() || self.playlist.is_empty() => self.add_videos(videos, ctx),
                Some(behavior) => self.drop_videos(videos, behavior, ctx),
                None => self.pending_drop = Some(videos),
            }
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
        self.sync_watch();
        self.replace_playlist(list, ctx);
    }

    /// Adds dropped videos to the playlist, or makes them the playlist.
    fn drop_videos(&mut self, videos: Vec<PathBuf>, behavior: progress::DroppedVideos, ctx: &egui::Context) {
        match behavior {
            progress::DroppedVideos::Add => self.add_videos(videos, ctx),
            progress::DroppedVideos::Replace => {
                // The playlist no longer comes from a folder.
                self.store.set_folder(None);
                self.sync_watch();
                self.replace_playlist(videos, ctx);
            }
        }
    }

    /// Changes whether folders include their subfolders, and rescans the folder
    /// the playlist came from. Videos added by hand stay, and so does the
    /// playing video if it is still part of the list.
    fn set_include_subfolders(&mut self, include: bool, ctx: &egui::Context) {
        self.store.set_include_subfolders(include);
        self.store.save();
        self.sync_watch();
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

    /// Handles media keys and shows what's playing in Windows' media overlay.
    fn media_controls(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if self.media_keys.is_none() {
            let hwnd = match frame.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => h.hwnd.get() as *mut std::ffi::c_void,
                _ => return,
            };
            let keys = media_keys::MediaKeys::new(hwnd, ctx.clone());
            if let Err(e) = &keys {
                eprintln!("media keys unavailable: {e}");
            }
            self.media_keys = Some(keys);
        }
        let title = self.current_path().map(display_name);
        let Playback { position, duration, paused, .. } = self.playback;
        let playing = self.loaded && !paused;
        let seek_long = self.store.settings.seek_long;
        let Some(Ok(keys)) = &mut self.media_keys else { return };
        keys.show(title.as_deref(), playing, position, duration);
        use media_keys::{Event, SeekDirection};
        for event in keys.events() {
            let action = match event {
                Event::Play => Action::SetPaused(false),
                Event::Pause | Event::Stop => Action::SetPaused(true),
                Event::Toggle => Action::TogglePause,
                Event::Next => Action::Next,
                Event::Previous => Action::Prev,
                Event::Seek(direction) | Event::SeekBy(direction, _) => {
                    let by = match event {
                        Event::SeekBy(_, by) => by.as_secs_f64(),
                        _ => seek_long,
                    };
                    Action::SeekBy(if direction == SeekDirection::Backward { -by } else { by })
                }
                Event::SetPosition(position) => Action::Seek { to: position.0.as_secs_f64(), exact: true },
                Event::Raise => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    continue;
                }
                Event::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    continue;
                }
                Event::SetVolume(_) | Event::OpenUri(_) => continue,
            };
            self.actions.push(action);
        }
    }

    /// Points the folder watch at the folder the playlist came from.
    fn sync_watch(&self) {
        self.watch.set_target(self.store.folder.clone().map(|f| (f, self.store.include_subfolders)));
    }

    /// Takes in background rescans of the playlist's folder.
    /// Drops the playlist entries whose files were found missing at startup.
    fn receive_missing_files(&mut self) {
        let Some(missing) = self.missing_files.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.missing_files = None;
        let playing = self.current_path().map(Path::to_path_buf);
        self.reorder(|list| list.retain(|p| !missing.contains(p) || Some(p) == playing.as_ref()));
    }

    fn receive_scans(&mut self) {
        while let Ok((folder, found)) = self.watch.scans.try_recv() {
            if self.store.folder.as_ref() == Some(&folder) {
                self.append_new(found);
            }
        }
    }

    /// Adds the videos that aren't in the playlist yet to its end, returning
    /// how many there were.
    fn append_new(&mut self, found: Vec<PathBuf>) -> usize {
        let new: Vec<PathBuf> = found.into_iter().filter(|p| !self.playlist.contains(p)).collect();
        if new.is_empty() {
            return 0;
        }
        let count = new.len();
        self.names.clear();
        self.summary = None;
        // Appending shifts no indices, so a countdown to the next video stays.
        self.playlist.extend(new);
        self.store.set_playlist(&self.playlist);
        self.request_durations();
        self.store.save();
        self.notify(format!("{count} new video{} in the folder", if count == 1 { "" } else { "s" }));
        count
    }

    fn refresh_folder(&mut self) {
        let Some(folder) = self.store.folder.clone() else {
            return self.notify("The playlist wasn't opened from a folder");
        };
        let found = match library::scan_folder(&folder, self.store.include_subfolders) {
            Ok(found) => found,
            Err(e) => return self.warn(format!("Could not open {}: {e}", display_name(&folder))),
        };
        let before = self.playlist.len();
        let playing = self.current_path().map(Path::to_path_buf);
        self.reorder(|list| list.retain(|p| p.is_file() || Some(p) == playing.as_ref()));
        let removed = before - self.playlist.len();
        let added = self.append_new(found);
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        self.notify(match (added, removed) {
            (0, 0) => "No changes in the folder".to_string(),
            (a, 0) => format!("{a} new video{}", plural(a)),
            (0, r) => format!("Removed {r} missing video{}", plural(r)),
            (a, r) => format!("{a} new video{}, removed {r} missing", plural(a)),
        });
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
        self.sync_watch();
    }

    /// The video to offer when nothing is playing: the last one played if it is
    /// unfinished, otherwise the next unwatched one after it (wrapping around).
    /// `None` when everything is watched.
    fn continue_target(&self) -> Option<usize> {
        let watched = |i: usize| self.store.get(&self.playlist[i]).watched;
        let last = self.store.current.as_ref().and_then(|c| self.playlist.iter().position(|p| p == c));
        let start = match last {
            Some(i) if !watched(i) || self.store.resume_position(&self.playlist[i]).is_some() => return Some(i),
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
        if !path.is_file() {
            return self.warn(format!("{name} was not found (moved or deleted?)"));
        }
        let start = self.store.resume_position(&path);
        mpv.set("pause", if paused { "yes" } else { "no" });
        mpv.load_file(&path, start.unwrap_or(0.0));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — {APP_NAME}")));
        // Drop the previous frame so it never shows under the new video.
        self.texture = None;
        mpv.hide_frames();
        self.chapters.clear();
        // Until mpv reports the new video's values, don't let the previous
        // one's be saved as this one's progress.
        self.playback.position = start.unwrap_or(0.0);
        self.playback.duration = 0.0;
        self.restoring = None;
        self.tracks_changed_by_user = false;
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

    fn stop(&mut self, ctx: &egui::Context) {
        self.save_current_progress();
        if self.current.is_some() {
            if let Ok(mpv) = &self.mpv {
                mpv.send(&["stop"]);
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
        self.open_for_preview();
        let Ok(mpv) = &self.mpv else { return };
        self.loaded = true;
        // Restore this video's track choices. The subtitle delay is global in
        // mpv, so it is always set, back to 0 if nothing was chosen.
        let prefs = self.current_path().and_then(|p| self.store.tracks(p)).cloned().unwrap_or_default();
        mpv.set("sub-delay", &prefs.sub_delay.to_string());
        mpv.set("sub-speed", &prefs.sub_speed.unwrap_or(1.0).to_string());
        self.line_marks = [None, None];
        // Subtitle files load in the background when sent without waiting, so
        // the saved tracks are selected once they have all been added.
        let files: Vec<PathBuf> = prefs.sub_files.iter().filter(|f| f.is_file()).cloned().collect();
        for file in &files {
            mpv.send_tagged(SUB_RESTORE_TAG, &["sub-add", &file.to_string_lossy(), "auto"]);
        }
        self.restoring = Some((files.len(), prefs));
        self.select_saved_tracks();
    }

    /// Selects the saved audio and subtitle tracks once the saved subtitle
    /// files are loaded.
    fn select_saved_tracks(&mut self) {
        let (Ok(mpv), Some((0, prefs))) = (&self.mpv, &self.restoring) else { return };
        if let Some(aid) = &prefs.audio {
            mpv.set("aid", aid);
        }
        if let Some(sid) = &prefs.subtitle {
            mpv.set("sid", sid);
        }
        self.restoring = None;
    }

    /// Opens the playing video in the previewer too, so the first hover over
    /// the seek bar only has to seek.
    fn open_for_preview(&mut self) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else { return };
        if let Some(Ok(thumbnails)) = &mut self.thumbnails {
            thumbnails.request(&path, self.playback.position, 1.0);
        }
    }

    /// Takes in a changed property that was asked for with `observe`.
    fn property_changed(&mut self, name: &str, value: mpv::Node) {
        let p = &mut self.playback;
        match name {
            // Unavailable between videos: keep the last known position.
            "time-pos" => p.position = value.as_f64().unwrap_or(p.position),
            "duration" => p.duration = value.as_f64().unwrap_or(0.0),
            "pause" => p.paused = value.as_bool().unwrap_or(p.paused),
            "mute" => p.muted = value.as_bool().unwrap_or(p.muted),
            "volume" => p.volume = value.as_f64().unwrap_or(p.volume),
            "speed" => p.speed = value.as_f64().unwrap_or(1.0),
            "sub-delay" => p.sub_delay = value.as_f64().unwrap_or(0.0),
            "sub-speed" => p.sub_speed = value.as_f64().unwrap_or(1.0),
            "chapter-list" => self.chapters = mpv::Chapter::list(&value),
            "track-list" => {
                self.tracks = mpv::Track::list(&value);
                if let Some(kind) = self.announce_track.take() {
                    let what = if kind == "sub" { "Subtitles" } else { "Audio" };
                    let text = match self.tracks.iter().find(|t| t.kind == kind && t.selected) {
                        Some(track) => format!("{what}: {}", track.label),
                        None => format!("{what} off"),
                    };
                    self.notify(text);
                }
            }
            _ => {}
        }
        if self.tracks_changed_by_user && matches!(name, "track-list" | "sub-delay" | "sub-speed") {
            self.remember_tracks();
        }
    }

    /// The tracks of a kind ("audio" or "sub") in the loaded video.
    fn tracks(&self, kind: &str) -> impl Iterator<Item = &mpv::Track> {
        self.tracks.iter().filter(move |t| t.kind == kind)
    }

    /// Saves the playing video's current track choices.
    fn remember_tracks(&mut self) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else { return };
        if !self.loaded {
            return;
        }
        // In mpv's terms: a track id, or "no" for none.
        let selected = |kind| self.tracks(kind).find(|t| t.selected).map_or("no".to_string(), |t| t.id.to_string());
        let (audio, subtitle) = (selected("audio"), selected("sub"));
        let Playback { sub_delay, sub_speed, .. } = self.playback;
        let prefs = self.store.tracks_mut(&path);
        prefs.audio = Some(audio);
        prefs.subtitle = Some(subtitle);
        prefs.sub_delay = sub_delay;
        prefs.sub_speed = ((sub_speed - 1.0).abs() > 1e-6).then_some(sub_speed);
    }

    fn handle_mpv_events(&mut self, ctx: &egui::Context) {
        let Ok(mpv) = &self.mpv else { return };
        for event in mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.file_loaded(),
                mpv::Event::PlaybackRestart => {
                    if let Ok(mpv) = &self.mpv {
                        mpv.show_frames();
                    }
                }
                mpv::Event::Property { name, value } => self.property_changed(&name, value),
                mpv::Event::CommandDone { tag: SUB_STEP_TAG, ok } => {
                    if let Some(which) = self.announce_sub_step.take() {
                        self.notify(if ok { format!("{which} line shown now") } else { "No subtitle line to sync to".into() });
                    }
                }
                mpv::Event::CommandDone { tag: SUB_RESTORE_TAG, .. } => {
                    if let Some((left, _)) = &mut self.restoring {
                        *left = left.saturating_sub(1);
                    }
                    self.select_saved_tracks();
                }
                mpv::Event::CommandDone { .. } => {}
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

    /// Records the playing position (`playback` itself is kept up to date by
    /// mpv's property events) and saves now and then.
    fn sample_playback(&mut self) {
        if self.loaded {
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
            self.actions.push(Action::Drop(dropped));
        }

        // No shortcuts while typing or while a dialog is open (Esc closes it).
        if ctx.egui_wants_keyboard_input() || self.confirm_reset || self.settings_open || self.pending_drop.is_some() {
            return;
        }
        if self.show_help {
            // Any of the keys that opened it closes it again (Esc is handled by the dialog).
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Questionmark) || i.consume_key(egui::Modifiers::NONE, Key::F1)) {
                self.show_help = false;
            }
            return;
        }
        let Playback { volume, duration, speed, .. } = self.playback;
        let (short, long) = (self.store.settings.seek_short, self.store.settings.seek_long);
        let media_keys_work = matches!(self.media_keys, Some(Ok(_)));
        ctx.input_mut(|i| {
            use egui::Modifiers as M;
            // egui ignores extra Shift/Alt when matching, so more specific
            // combinations must come before their plain counterparts.
            let mut bindings = vec![
                // Single keys for everything; the Ctrl combinations are kept
                // for habit's sake.
                (M::CTRL | M::SHIFT, Key::O, Action::OpenFolder),
                (M::CTRL, Key::O, Action::OpenVideos),
                (M::CTRL, Key::F, Action::FocusSearch),
                (M::CTRL, Key::Comma, Action::OpenSettings),
                (M::CTRL, Key::ArrowLeft, Action::SubStep(-1)),
                (M::CTRL, Key::ArrowRight, Action::SubStep(1)),
                (M::ALT, Key::Enter, Action::ToggleFullscreen),
                (M::NONE, Key::O, Action::OpenFolder),
                (M::NONE, Key::I, Action::OpenVideos),
                (M::NONE, Key::Slash, Action::FocusSearch),
                (M::NONE, Key::E, Action::OpenSettings),
                (M::NONE, Key::R, Action::RefreshFolder),
                (M::NONE, Key::F5, Action::RefreshFolder),
                (M::NONE, Key::Questionmark, Action::ShowHelp),
                (M::NONE, Key::F1, Action::ShowHelp),
                (M::SHIFT, Key::ArrowLeft, Action::SeekBy(-long)),
                (M::SHIFT, Key::ArrowRight, Action::SeekBy(long)),
                (M::SHIFT, Key::J, Action::SeekBy(-long)),
                (M::SHIFT, Key::L, Action::SeekBy(long)),
                (M::NONE, Key::ArrowLeft, Action::SeekBy(-short)),
                (M::NONE, Key::ArrowRight, Action::SeekBy(short)),
                (M::NONE, Key::J, Action::SeekBy(-short)),
                (M::NONE, Key::L, Action::SeekBy(short)),
                (M::NONE, Key::ArrowUp, Action::SetVolume((volume + 5.0).min(150.0))),
                (M::NONE, Key::ArrowDown, Action::SetVolume((volume - 5.0).max(0.0))),
                (M::NONE, Key::Space, Action::TogglePause),
                (M::NONE, Key::K, Action::TogglePause),
                (M::NONE, Key::F11, Action::ToggleFullscreen),
                (M::NONE, Key::F, Action::ToggleFullscreen),
                (M::NONE, Key::Escape, Action::Escape),
                (M::NONE, Key::Home, Action::Seek { to: 0.0, exact: true }),
                (M::NONE, Key::Comma, Action::FrameStep { forward: false }),
                (M::NONE, Key::Period, Action::FrameStep { forward: true }),
                (M::NONE, Key::M, Action::ToggleMute),
                (M::NONE, Key::N, Action::Next),
                (M::NONE, Key::P, Action::Prev),
                (M::NONE, Key::W, Action::ToggleWatched),
                (M::NONE, Key::S, Action::CycleSubtitles),
                (M::NONE, Key::A, Action::CycleAudio),
                (M::NONE, Key::T, Action::ToggleSyncPanel),
                (M::NONE, Key::B, Action::ToggleSidebar),
                (M::SHIFT, Key::Z, Action::AdjustSubDelay(-1.0)),
                (M::SHIFT, Key::X, Action::AdjustSubDelay(1.0)),
                (M::NONE, Key::Z, Action::AdjustSubDelay(-0.1)),
                (M::NONE, Key::X, Action::AdjustSubDelay(0.1)),
                (M::NONE, Key::G, Action::MarkLine { spoken: false }),
                (M::NONE, Key::H, Action::MarkLine { spoken: true }),
                (M::NONE, Key::OpenBracket, Action::SetSpeed(step_speed(speed, false))),
                (M::NONE, Key::CloseBracket, Action::SetSpeed(step_speed(speed, true))),
                (M::NONE, Key::Backspace, Action::SetSpeed(1.0)),
                (M::NONE, Key::PageUp, Action::Chapter { forward: false }),
                (M::NONE, Key::PageDown, Action::Chapter { forward: true }),
            ];
            // The system media controls handle media keys, even without focus.
            // Without them, the keys still work while the window has focus.
            if !media_keys_work {
                bindings.push((M::NONE, Key::MediaPlayPause, Action::TogglePause));
                bindings.push((M::NONE, Key::MediaStop, Action::SetPaused(true)));
                bindings.push((M::NONE, Key::MediaTrackNext, Action::Next));
                bindings.push((M::NONE, Key::MediaTrackPrevious, Action::Prev));
            }
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
                    self.open(vec![folder], false, ctx);
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
            Action::Open(paths) => self.open(paths, false, ctx),
            Action::Drop(paths) => self.open(paths, true, ctx),
            Action::ChooseDropBehavior(behavior) => {
                let mut settings = self.store.settings.clone();
                settings.dropped_videos = Some(behavior);
                self.store.set_settings(settings);
                self.store.save();
                if let Some(videos) = self.pending_drop.take() {
                    self.drop_videos(videos, behavior, ctx);
                }
            }
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
            Action::RefreshFolder => self.refresh_folder(),
            Action::ShowHelp => self.show_help = true,
            Action::OpenSettings => self.settings_open = true,
            Action::SetSettings(settings) => {
                self.store.set_settings(settings);
                self.apply_settings(ctx);
            }
            Action::ClearRecentFolders => {
                self.store.clear_recent_folders();
                self.store.save();
            }
            Action::ToggleSyncPanel => self.sync_panel = !self.sync_panel,
            Action::FocusSearch => {
                self.show_sidebar = true;
                self.focus_search = true;
                if !self.sidebar_docked() && self.peek.is_none() {
                    self.peek = Some(Peek::default());
                }
            }
            Action::TogglePin => {
                let pinned = !self.store.sidebar_pinned;
                self.store.set_sidebar_pinned(pinned);
                self.store.save();
                // Unpinning from the panel leaves it out while the pointer is on it.
                self.peek = (!pinned).then_some(Peek { entered: true, left_at: None });
                self.show_sidebar = true;
            }
            Action::SetIncludeSubfolders(include) => self.set_include_subfolders(include, ctx),
            Action::SetWatched(path, watched) => {
                if self.loaded && self.current_path() == Some(path.as_path()) {
                    // Keep the live position from overwriting the change; a reset
                    // video that is loaded also jumps back to its start.
                    self.playback.position = if watched { self.playback.duration } else { 0.0 };
                    if !watched {
                        if let Ok(mpv) = &self.mpv {
                            mpv.send(&["seek", "0", "absolute+exact"]);
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
                        mpv.send(&["seek", "0", "absolute+exact"]);
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
            Action::ToggleSidebar if self.sidebar_docked() => self.show_sidebar = !self.show_sidebar,
            Action::ToggleSidebar => self.peek = if self.peek.is_some() { None } else { Some(Peek::default()) },
            Action::Escape => {
                if self.peek.is_some() {
                    self.peek = None;
                } else if self.fullscreen {
                    self.apply(Action::ExitFullscreen, ctx);
                } else {
                    self.sync_panel = false;
                }
            }
            Action::ToggleWatched => {
                if let Some(path) = self.current_path().map(Path::to_path_buf) {
                    let watched = self.store.get(&path).watched;
                    self.apply(Action::SetWatched(path, !watched), ctx);
                }
            }
            Action::SetPaused(paused) => {
                let playing = self.loaded && !self.playback.paused;
                if playing == paused {
                    self.apply(Action::TogglePause, ctx);
                }
            }
            Action::TogglePause => {
                if self.loaded {
                    if let Ok(mpv) = &self.mpv {
                        mpv.send(&["cycle", "pause"]);
                    }
                    self.notify(if self.playback.paused { "Playing" } else { "Paused" });
                } else if let Some(index) =
                    self.current.or(self.up_next.as_ref().map(|u| u.index)).or_else(|| self.continue_target())
                {
                    self.play(index, ctx);
                }
            }
            action => {
                let changes_tracks = matches!(
                    action,
                    Action::SetTrack { .. }
                        | Action::CycleSubtitles
                        | Action::CycleAudio
                        | Action::LoadSubtitle
                        | Action::AdjustSubDelay(_)
                        | Action::SetSubDelay(_)
                        | Action::SubStep(_)
                        | Action::MarkLine { .. }
                        | Action::SetSubSpeed(_)
                );
                // They are saved once mpv reports them (see `property_changed`).
                self.tracks_changed_by_user |= changes_tracks;
                self.apply_to_mpv(action);
            }
        }
    }

    fn apply_to_mpv(&mut self, action: Action) {
        let Ok(mpv) = &self.mpv else { return };
        let duration = self.playback.duration;
        match action {
            Action::Seek { to, exact } if self.loaded => {
                let flags = if exact { "absolute+exact" } else { "absolute+keyframes" };
                mpv.send(&["seek", &format!("{to:.3}"), flags]);
                if exact {
                    self.notify(format!("{} / {}", format_time(to), format_time(duration)));
                }
            }
            Action::SeekBy(by) if self.loaded => {
                mpv.send(&["seek", &by.to_string(), "relative"]);
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
                mpv.send(&["cycle", "mute"]);
                self.notify(if self.playback.muted { "Sound on" } else { "Muted" });
            }
            Action::SetTrack { kind, id } => {
                let prop = if kind == "sub" { "sid" } else { "aid" };
                mpv.set(prop, &id.map_or("no".into(), |id| id.to_string()));
            }
            Action::CycleSubtitles if self.loaded => {
                mpv.send(&["cycle", "sub"]);
                self.announce_track = Some("sub");
            }
            Action::CycleAudio if self.loaded => {
                mpv.send(&["cycle", "audio"]);
                self.announce_track = Some("audio");
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
                        Ok(()) => {
                            if let Some(video) = self.current.and_then(|i| self.playlist.get(i)).cloned() {
                                let files = &mut self.store.tracks_mut(&video).sub_files;
                                if !files.contains(&file) {
                                    files.push(file.clone());
                                }
                            }
                            self.notify(format!("Subtitles: {}", display_name(&file)));
                        }
                        Err(e) => self.warn(format!("Could not load subtitles: {e}")),
                    }
                }
            }
            Action::FrameStep { forward } if self.loaded => {
                // Steps one frame and leaves playback paused.
                mpv.send(&[if forward { "frame-step" } else { "frame-back-step" }]);
            }
            Action::AdjustSubDelay(by) if self.loaded => {
                mpv.send(&["add", "sub-delay", &by.to_string()]);
                self.notify(format!("Subtitle delay {:+.1} s", self.playback.sub_delay + by));
            }
            Action::SetSubDelay(delay) if self.loaded => {
                mpv.set("sub-delay", &delay.to_string());
                self.notify(format!("Subtitle delay {delay:+.1} s"));
            }
            Action::SubStep(_) if self.loaded && !self.tracks.iter().any(|t| t.kind == "sub" && t.selected) => {
                self.notify("Subtitles are off");
            }
            Action::SubStep(lines) if self.loaded => {
                mpv.send_tagged(SUB_STEP_TAG, &["sub-step", &lines.to_string()]);
                self.announce_sub_step = Some(if lines < 0 { "Previous" } else { "Next" });
            }
            Action::MarkLine { spoken } if self.loaded => {
                self.line_marks[usize::from(spoken)] = Some(self.playback.position);
                match self.line_marks {
                    [Some(appeared), Some(spoken_at)] => {
                        // A line that appears early must be delayed, and vice versa.
                        let by = spoken_at - appeared;
                        mpv.send(&["add", "sub-delay", &format!("{by:.3}")]);
                        self.line_marks = [None, None];
                        let delay = self.playback.sub_delay + by;
                        self.notify(format!("Subtitles shifted {by:+.1} s  ·  delay {delay:+.1} s"));
                    }
                    _ if spoken => self.notify("Marked when the line was spoken  ·  press G when its subtitle appears"),
                    _ => self.notify("Marked when the subtitle appeared  ·  press H when the line is spoken"),
                }
            }
            Action::SetSubSpeed(speed) if self.loaded => {
                mpv.set("sub-speed", &speed.to_string());
                self.notify(match SUB_TIMINGS.iter().find(|(s, _)| (s - speed).abs() < 1e-6) {
                    Some((_, label)) => format!("Subtitle timing: {label}"),
                    None => format!("Subtitle speed {speed:.4}"),
                });
            }
            Action::Chapter { forward } if self.loaded => {
                if self.chapters.is_empty() {
                    return self.notify("This video has no chapters");
                }
                match chapter_target(&self.chapters, self.playback.position, forward) {
                    Some(index) => {
                        let chapter = &self.chapters[index];
                        mpv.send(&["seek", &format!("{:.3}", chapter.time), "absolute+exact"]);
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
            if icon_button(ui, icons::SIDEBAR, "Show or hide the playlist (B)").clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
            ui.separator();
            if icon_button(ui, icons::FOLDER_PLUS, "Open folder: replace the playlist with its videos (O)").clicked() {
                self.actions.push(Action::OpenFolder);
            }
            if icon_button(ui, icons::FILE_VIDEO, "Add videos to the playlist (I)").clicked() {
                self.actions.push(Action::OpenVideos);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = egui::Button::new(RichText::new(icons::MORE).size(ICON_SIZE)).frame_when_inactive(false);
                let (response, _) = egui::containers::menu::MenuButton::from_button(more).ui(ui, |ui| {
                    let recent = self.store.recent_folders.clone();
                    ui.add_enabled_ui(!recent.is_empty(), |ui| {
                        ui.menu_button(format!("{}  Recent folders", icons::RECENT), |ui| {
                            for folder in &recent {
                                let exists = folder.is_dir();
                                let tip = if exists { folder.display().to_string() } else { format!("{} (not found)", folder.display()) };
                                let label = format!("{}  {}", icons::FOLDER, display_name(folder));
                                if ui.add_enabled(exists, egui::Button::new(label)).on_hover_text(tip).clicked() {
                                    self.actions.push(Action::Open(vec![folder.clone()]));
                                }
                            }
                            ui.separator();
                            if ui.button("Clear this list").clicked() {
                                self.actions.push(Action::ClearRecentFolders);
                            }
                        });
                    });
                    let refresh = egui::Button::new(format!("{}  Refresh folder", icons::REFRESH)).shortcut_text("R");
                    let tip = "Add new videos from the playlist's folder and drop missing ones. New videos are also picked up by themselves.";
                    if ui.add_enabled(self.store.folder.is_some(), refresh).on_hover_text(tip).clicked() {
                        self.actions.push(Action::RefreshFolder);
                    }
                    ui.separator();
                    let settings = egui::Button::new(format!("{}  Settings…", icons::GEAR)).shortcut_text("E");
                    if ui.add(settings).clicked() {
                        self.actions.push(Action::OpenSettings);
                    }
                    let help = egui::Button::new(format!("{}  Keyboard shortcuts", icons::KEYBOARD)).shortcut_text("?");
                    if ui.add(help).clicked() {
                        self.actions.push(Action::ShowHelp);
                    }
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

    /// Whether the playlist sits beside the video. Unpinned (and in fullscreen)
    /// it slides out over the video instead.
    fn sidebar_docked(&self) -> bool {
        self.store.sidebar_pinned && !self.fullscreen
    }

    /// The unpinned playlist: it slides out over the video when the pointer
    /// reaches its edge of the video (or from the keyboard) and hides once the
    /// pointer leaves it, unless it's in use.
    fn floating_sidebar(&mut self, ctx: &egui::Context, video: egui::Rect) {
        let pointer = ctx.pointer_hover_pos();
        let right = self.store.settings.sidebar_side == progress::Side::Right;
        let at_edge = pointer.is_some_and(|p| {
            video.contains(p) && if right { p.x > video.right() - EDGE_WIDTH } else { p.x < video.left() + EDGE_WIDTH }
        });
        if self.peek.is_none() && at_edge && !ctx.input(|i| i.pointer.any_down()) {
            self.peek = Some(Peek { entered: true, left_at: None });
        }
        if self.peek.is_none() {
            return;
        }
        let size = vec2(self.sidebar_width, video.height());
        let (pos, pivot) = if right { (video.right_top(), egui::Align2::RIGHT_TOP) } else { (video.left_top(), egui::Align2::LEFT_TOP) };
        let id = egui::Id::unique("floating-sidebar");
        // A new area is normally laid out invisibly on its first frame to
        // measure it, which flashes the video through when unpinning. Its size
        // is known, so skip that.
        if egui::AreaState::load(ctx, id).and_then(|s| s.size) != Some(size) {
            let state = egui::AreaState { pivot_pos: Some(pos), pivot, size: Some(size), interactable: true, last_became_visible_at: None };
            ctx.memory_mut(|m| m.areas_mut().set_state(egui::LayerId::new(egui::Order::Middle, id), state));
        }
        let area = egui::Area::new(id)
            .fade_in(false)
            .fixed_pos(pos)
            .pivot(pivot)
            .constrain(false)
            .show(ctx, |ui| {
                let frame = egui::Frame::side_top_panel(ui.style()).shadow(ui.visuals().popup_shadow);
                let inner = size - frame.total_margin().sum();
                frame.show(ui, |ui| {
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    self.sidebar(ui);
                });
            });
        let inside = pointer.is_some_and(|p| area.response.rect.expand(12.0).contains(p));
        if inside {
            // Keeps the cursor from hiding while it rests on the playlist.
            self.last_activity = Instant::now();
        }
        let searching = ctx.memory(|m| m.has_focus(egui::Id::unique("playlist-search")));
        let busy = searching || self.drag_row.is_some() || egui::Popup::is_any_open(ctx);
        let Some(peek) = &mut self.peek else { return };
        if inside || busy {
            peek.entered |= inside;
            peek.left_at = None;
        } else if peek.entered {
            let left_at = *peek.left_at.get_or_insert_with(Instant::now);
            if left_at.elapsed() >= PEEK_HIDE_DELAY {
                self.peek = None;
            } else {
                ctx.request_repaint_after(PEEK_HIDE_DELAY);
            }
        }
    }

    /// Tools for subtitles that are out of sync, in a panel over the video.
    fn sync_panel(&mut self, ctx: &egui::Context, video: egui::Rect) {
        let mut open = self.sync_panel;
        let Playback { sub_delay, sub_speed, .. } = self.playback;
        egui::Window::new("Sync subtitles")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(video.left_bottom() + vec2(16.0, -16.0))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .show(ctx, |ui| {
                ui.set_width(330.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Delay {sub_delay:+.1} s")).strong());
                    for (by, label, tip) in [
                        (-1.0, "−1 s", "Show subtitles 1 s earlier (Shift+Z)"),
                        (-0.1, "−0.1", "Show subtitles 0.1 s earlier (Z)"),
                        (0.1, "+0.1", "Show subtitles 0.1 s later (X)"),
                        (1.0, "+1 s", "Show subtitles 1 s later (Shift+X)"),
                    ] {
                        if ui.small_button(label).on_hover_text(tip).clicked() {
                            self.actions.push(Action::AdjustSubDelay(by));
                        }
                    }
                    if sub_delay != 0.0 && ui.small_button(icons::RESET).on_hover_text("No delay").clicked() {
                        self.actions.push(Action::SetSubDelay(0.0));
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("When you hear a line, show its subtitle now").small().weak());
                ui.horizontal(|ui| {
                    if ui.button("Previous line").on_hover_text("For subtitles that came too early (Ctrl+\u{E058})").clicked() {
                        self.actions.push(Action::SubStep(-1));
                    }
                    if ui.button("Next line").on_hover_text("For subtitles that haven't appeared yet (Ctrl+\u{E06C})").clicked() {
                        self.actions.push(Action::SubStep(1));
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Or mark one line, in either order").small().weak());
                ui.horizontal(|ui| {
                    let mark = |marked: Option<f64>, label: &str| match marked {
                        Some(at) => format!("{}  {label} at {}", icons::CHECK, format_time(at)),
                        None => label.to_string(),
                    };
                    let [appeared, spoken] = self.line_marks;
                    if ui.button(mark(appeared, "Subtitle appeared")).on_hover_text("G").clicked() {
                        self.actions.push(Action::MarkLine { spoken: false });
                    }
                    if ui.button(mark(spoken, "Line spoken")).on_hover_text("H").clicked() {
                        self.actions.push(Action::MarkLine { spoken: true });
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Drifting further out of sync over time?").small().weak());
                ui.horizontal(|ui| {
                    for (speed, label) in SUB_TIMINGS {
                        let selected = (sub_speed - speed).abs() < 1e-6;
                        let text = label[..1].to_uppercase() + &label[1..];
                        if ui.selectable_label(selected, text).clicked() && !selected {
                            self.actions.push(Action::SetSubSpeed(speed));
                        }
                    }
                });
                ui.add_space(4.0);
                ui.weak("Changes are remembered for this video.");
            });
        self.sync_panel = open;
    }

    /// Applies the settings to the UI and to mpv.
    fn apply_settings(&self, ctx: &egui::Context) {
        let settings = &self.store.settings;
        ctx.set_theme(match settings.theme {
            progress::Theme::System => egui::ThemePreference::System,
            progress::Theme::Dark => egui::ThemePreference::Dark,
            progress::Theme::Light => egui::ThemePreference::Light,
        });
        let accent = settings.accent;
        ctx.all_styles_mut(|style| style.visuals.selection = accent_selection(accent, style.visuals.dark_mode));
        if let Ok(mpv) = &self.mpv {
            mpv.set("slang", &settings.sub_languages);
            mpv.set("alang", &settings.audio_languages);
            mpv.set("hwdec", if settings.hardware_decoding { "auto-safe" } else { "no" });
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let mut settings = self.store.settings.clone();
        let mut autoplay = self.store.autoplay_next;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Settings").size(18.0).strong());
            ui.add_space(10.0);
            let section = |ui: &mut egui::Ui, title: &str| {
                ui.label(RichText::new(title).small().weak());
                ui.end_row();
            };
            egui::Grid::new("settings").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                section(ui, "PLAYBACK");
                ui.label("Seek step");
                ui.add(egui::DragValue::new(&mut settings.seek_short).range(1.0..=120.0).suffix(" s").speed(0.2));
                ui.end_row();
                ui.label("Seek step with Shift");
                ui.add(egui::DragValue::new(&mut settings.seek_long).range(1.0..=600.0).suffix(" s").speed(1.0));
                ui.end_row();
                ui.label("Count as watched at");
                ui.add(egui::Slider::new(&mut settings.watched_percent, 50.0..=100.0).suffix("%").integer());
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut autoplay, "Play the next video automatically");
                ui.end_row();
                ui.label("Dropped videos")
                    .on_hover_text("What dropping videos onto the window does (dropped folders always replace the playlist)");
                ui.horizontal(|ui| {
                    use progress::DroppedVideos as D;
                    for (choice, label) in [(None, "Ask"), (Some(D::Add), "Add to playlist"), (Some(D::Replace), "Replace playlist")] {
                        ui.selectable_value(&mut settings.dropped_videos, choice, label);
                    }
                });
                ui.end_row();
                section(ui, "LANGUAGES");
                ui.label("Subtitles");
                ui.add(egui::TextEdit::singleline(&mut settings.sub_languages).margin(TEXT_MARGIN).hint_text("e.g. en,eng").desired_width(200.0))
                    .on_hover_text("Language codes in order of preference, separated by commas. Applies to the next video.");
                ui.end_row();
                ui.label("Audio");
                ui.add(egui::TextEdit::singleline(&mut settings.audio_languages).margin(TEXT_MARGIN).hint_text("the file's default").desired_width(200.0))
                    .on_hover_text("Language codes in order of preference, e.g. ja,jpn. Applies to the next video.");
                ui.end_row();
                section(ui, "VIDEO AND APPEARANCE");
                ui.label("");
                ui.checkbox(&mut settings.hardware_decoding, "Decode video on the GPU")
                    .on_hover_text("Turn off if videos show glitches; uses more CPU");
                ui.end_row();
                ui.label("Theme");
                ui.horizontal(|ui| {
                    for (theme, label) in [
                        (progress::Theme::System, "System"),
                        (progress::Theme::Dark, "Dark"),
                        (progress::Theme::Light, "Light"),
                    ] {
                        ui.selectable_value(&mut settings.theme, theme, label);
                    }
                });
                ui.end_row();
                ui.label("Playlist side");
                ui.horizontal(|ui| {
                    for (side, label) in [(progress::Side::Left, "Left"), (progress::Side::Right, "Right")] {
                        ui.selectable_value(&mut settings.sidebar_side, side, label);
                    }
                });
                ui.end_row();
                ui.label("Accent color");
                ui.horizontal(|ui| {
                    let blue = accent_selection(None, ui.visuals().dark_mode).bg_fill;
                    if swatch(ui, blue, settings.accent.is_none(), "Blue (default)").clicked() {
                        settings.accent = None;
                    }
                    for (name, rgb) in ACCENTS {
                        if swatch(ui, Color32::from_rgb(rgb[0], rgb[1], rgb[2]), settings.accent == Some(rgb), name).clicked() {
                            settings.accent = Some(rgb);
                        }
                    }
                    let mut custom = settings.accent.unwrap_or([blue.r(), blue.g(), blue.b()]);
                    let picker = egui::color_picker::color_edit_button_srgb(ui, &mut custom);
                    if picker.on_hover_text("Pick any color").changed() {
                        settings.accent = Some(custom);
                    }
                });
                ui.end_row();
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Done").clicked() {
                    close = true;
                }
                if ui.button("Restore defaults").clicked() {
                    settings = progress::Settings::default();
                    autoplay = true;
                }
            });
        });
        if settings != self.store.settings {
            self.actions.push(Action::SetSettings(settings));
        }
        if autoplay != self.store.autoplay_next {
            self.actions.push(Action::SetAutoplayNext(autoplay));
        }
        if close || modal.should_close() {
            self.settings_open = false;
            self.store.save();
        }
    }

    /// Asks, the first time videos are dropped, whether they should be added
    /// or replace the playlist.
    fn drop_dialog(&mut self, ctx: &egui::Context) {
        let count = self.pending_drop.as_ref().map_or(0, Vec::len);
        let videos = if count == 1 { "this video".to_string() } else { format!("these {count} videos") };
        let modal = egui::Modal::new(egui::Id::unique("drop-behavior")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new("Dropped videos").size(18.0).strong());
            ui.add_space(8.0);
            ui.label(format!("Add {videos} to the playlist, or replace the playlist with {}?", if count == 1 { "it" } else { "them" }));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Add to the playlist", icons::PLUS)).clicked() {
                    self.actions.push(Action::ChooseDropBehavior(progress::DroppedVideos::Add));
                }
                if ui.button(format!("{}  Replace the playlist", icons::FILE_VIDEO)).clicked() {
                    self.actions.push(Action::ChooseDropBehavior(progress::DroppedVideos::Replace));
                }
            });
            ui.add_space(8.0);
            ui.weak("Your choice is remembered; you can change it in Settings.");
        });
        // Esc or a click outside cancels the drop without remembering anything.
        if modal.should_close() {
            self.pending_drop = None;
        }
    }

    /// An overview of the keyboard shortcuts.
    fn help_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("shortcuts")).show(ctx, |ui| {
            ui.label(RichText::new("Keyboard shortcuts").size(18.0).strong());
            ui.add_space(8.0);
            ui.columns(2, |columns| {
                for (column, groups) in columns.iter_mut().zip(SHORTCUTS.chunks(SHORTCUTS.len().div_ceil(2))) {
                    for (title, keys) in groups {
                        column.label(RichText::new(*title).small().weak());
                        egui::Grid::new(title).num_columns(2).spacing([16.0, 4.0]).min_col_width(112.0).show(column, |ui| {
                            for (key, action) in *keys {
                                ui.label(RichText::new(*key).monospace().strong());
                                ui.label(*action);
                                ui.end_row();
                            }
                        });
                        column.add_space(10.0);
                    }
                }
            });
            ui.weak("Press ? or Esc to close.");
        });
        if modal.should_close() {
            self.show_help = false;
        }
    }

    /// Confirmation before wiping every video's progress.
    fn reset_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("reset-progress")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("Reset all progress?").size(18.0).strong());
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
        // Right-clicking where there are no videos (registered first, so the
        // rows and buttons on top take precedence).
        let background = ui.interact(ui.max_rect(), egui::Id::unique("playlist-area"), Sense::click());
        // The same scope docked or slid out, so the list keeps its scroll position.
        ui.scope_builder(egui::UiBuilder::new().scope_id(egui::Id::unique("playlist")), |ui| self.sidebar_contents(ui));
        let include = self.store.include_subfolders;
        background.context_menu(|ui| subfolder_option(ui, include, &mut self.actions));
    }

    fn sidebar_contents(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("PLAYLIST").small().weak());
            let count = self.playlist.len();
            if count > 0 {
                ui.label(RichText::new(format!("{count} video{}", if count == 1 { "" } else { "s" })).small().weak());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pinned = self.store.sidebar_pinned;
                let (pin, tip) = if pinned {
                    (icons::fill(icons::PIN), "Unpin: hide the playlist until you point at the edge of the video")
                } else {
                    (RichText::new(icons::PIN), "Pin the playlist beside the video")
                };
                if ui.add(egui::Button::new(pin.size(14.0)).frame_when_inactive(false)).on_hover_text(tip).clicked() {
                    self.actions.push(Action::TogglePin);
                }
                if count > 0 {
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
                }
            });
        });
        if !self.playlist.is_empty() {
            self.playlist_summary(ui);
        }
        ui.separator();

        if self.playlist.is_empty() {
            ui.add_space(4.0);
            ui.weak("Empty. Open a folder to play its videos, or add videos one by one.");
            ui.add_space(4.0);
            ui.weak("You can also drop them onto the window.");
            return;
        }
        let query = self.search.trim().to_lowercase();
        self.refresh_names();
        // Indices of the entries to list, filtered by the search.
        let visible: Vec<usize> =
            (0..self.playlist.len()).filter(|&i| query.is_empty() || self.names[i].1.contains(&query)).collect();
        self.search_box(ui, &visible);
        if visible.is_empty() {
            ui.add_space(4.0);
            ui.weak(format!("No videos match \"{}\".", self.search.trim()));
            return;
        }
        let picked = (!query.is_empty()).then(|| visible.get(self.search_pick).copied()).flatten();
        let mut scroll = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(pick) = self.scroll_to_pick.take() {
            // Keep the picked result in the middle of the list.
            let row = PLAYLIST_ROW_HEIGHT + ui.spacing().item_spacing.y;
            let offset = pick as f32 * row - (ui.available_height() - PLAYLIST_ROW_HEIGHT) / 2.0;
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        // Only the rows in view are laid out.
        scroll.show_rows(ui, PLAYLIST_ROW_HEIGHT, visible.len(), |ui, range| {
            let mut rows = Vec::new();
            for &index in &visible[range] {
                let path = &self.playlist[index];
                let name = &self.names[index].0;
                let resp = playlist_row(ui, &self.store, &mut self.actions, index, path, name, self.current == Some(index));
                if picked == Some(index) {
                    // The search result Enter plays.
                    ui.painter().rect_stroke(resp.rect, 4.0, (1.5, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
                }
                if resp.drag_started() {
                    self.drag_row = Some(index);
                }
                rows.push((index, resp.rect));
            }
            self.drag_rows(ui, &rows);
        });
    }

    /// Rebuilds the cached display names (and their lowercase forms, for the
    /// search) after the playlist changed.
    fn refresh_names(&mut self) {
        if self.names.len() == self.playlist.len() {
            return;
        }
        let folder = self.store.folder.as_deref();
        self.names = self
            .playlist
            .iter()
            .map(|path| {
                // Videos from a subfolder of the opened folder show where they're from.
                let subfolder = folder.and_then(|f| path.parent()?.strip_prefix(f).ok());
                let name = match subfolder {
                    Some(sub) if !sub.as_os_str().is_empty() => {
                        format!("{} / {}", sub.to_string_lossy().replace(std::path::MAIN_SEPARATOR, " / "), display_name(path))
                    }
                    _ => display_name(path),
                };
                let lower = name.to_lowercase();
                (name, lower)
            })
            .collect();
    }

    /// The search box: / focuses it, ↑/↓ pick a result, Enter plays it and Esc
    /// clears the search.
    fn search_box(&mut self, ui: &mut egui::Ui, results: &[usize]) {
        let id = egui::Id::unique("playlist-search");
        if ui.memory(|m| m.has_focus(id)) {
            let (down, up) = ui.input_mut(|i| {
                (i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::ArrowUp))
            });
            let last = results.len().saturating_sub(1);
            if down {
                self.search_pick = (self.search_pick + 1).min(last);
            }
            if up {
                self.search_pick = self.search_pick.saturating_sub(1);
            }
            if down || up {
                self.scroll_to_pick = Some(self.search_pick);
            }
        }
        ui.horizontal(|ui| {
            let width = ui.available_width() - if self.search.is_empty() { 0.0 } else { 24.0 };
            let edit = egui::TextEdit::singleline(&mut self.search)
                .margin(TEXT_MARGIN)
                .id(id)
                .hint_text(format!("{}  Search the playlist (/)", icons::SEARCH))
                .desired_width(width);
            let resp = ui.add(edit);
            if resp.changed() {
                self.search_pick = 0;
                self.scroll_to_pick = Some(0);
            }
            if std::mem::take(&mut self.focus_search) {
                resp.request_focus();
            }
            // egui has already dropped focus by the time these show up.
            if resp.lost_focus() {
                let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if escape {
                    self.search.clear();
                }
                if enter && !self.search.trim().is_empty() {
                    if let Some(index) = results.get(self.search_pick) {
                        self.actions.push(Action::Play(*index));
                        self.search.clear();
                        self.peek = None;
                    }
                }
            }
            if !self.search.is_empty() && icon_button(ui, icons::X, "Clear the search (Esc)").clicked() {
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
    fn playlist_summary(&mut self, ui: &mut egui::Ui) {
        // Adding up the whole playlist every frame adds up; once a second is plenty.
        let fresh = self.summary.is_some_and(|(at, ..)| at.elapsed() < Duration::from_secs(1));
        if !fresh {
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
            self.summary = Some((Instant::now(), total, watched, unknown));
        }
        let Some((_, total, watched, unknown)) = self.summary else { return };
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
                    (icons::CORNERS_IN, "Exit fullscreen (F or Esc)")
                } else {
                    (icons::CORNERS_OUT, "Fullscreen (F)")
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
            // Dragging only moves the preview; the video jumps on release.
            if let Some(pos) = resp.interact_pointer_pos() {
                let to = frac_at(pos.x) * duration;
                if resp.dragged() {
                    self.scrub = Some(to);
                }
                if resp.drag_stopped() || resp.clicked() {
                    self.scrub = None;
                    self.actions.push(Action::Seek { to, exact: true });
                }
            }
            let pointer = if resp.dragged() { resp.interact_pointer_pos() } else { resp.hover_pos() };
            if let Some(pointer) = pointer {
                let at = frac_at(pointer.x) * duration;
                self.seek_preview(ui.ctx(), egui::pos2(pointer.x, rect.top() - 8.0), at, duration);
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

    /// A popup above the seek bar at `anchor` with a preview frame, the time
    /// and the chapter at `at`.
    fn seek_preview(&mut self, ctx: &egui::Context, anchor: egui::Pos2, at: f64, duration: f64) {
        let text = match chapter_at(&self.chapters, at) {
            Some(i) => format!("{}  ·  {}", format_time(at), self.chapters[i].title),
            None => format_time(at),
        };
        let preview = self.preview(at, duration);
        egui::Area::new(egui::Id::unique("seek-preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(anchor)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .constrain(true)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if let Some(texture) = preview {
                            let size = vec2(thumbnails::SIZE[0] as f32, thumbnails::SIZE[1] as f32);
                            ui.image((texture, size / ctx.pixels_per_point().max(1.0)));
                        }
                        ui.label(text);
                    });
                });
            });
    }

    /// A preview frame of the playing video near `at`, once the previewer has
    /// started. The frame may still be from a moment ago.
    fn preview(&mut self, at: f64, duration: f64) -> Option<egui::TextureId> {
        let path = self.current_path()?.to_path_buf();
        let thumbnails = self.thumbnails.as_mut()?.as_mut().ok()?;
        // About 300 distinct previews along the video, at most one per second.
        thumbnails.request(&path, at, (duration / 300.0).max(1.0));
        thumbnails.texture(&path).map(TextureHandle::id)
    }

    fn track_menu(&mut self, ui: &mut egui::Ui, kind: &'static str) {
        let (icon, tip) = if kind == "sub" { (icons::SUBTITLES, "Subtitles (S to cycle)") } else { (icons::WAVEFORM, "Audio track (A to cycle)") };
        ui.add_enabled_ui(self.loaded, |ui| {
            let button = egui::Button::new(RichText::new(icon).size(ICON_SIZE)).frame_when_inactive(false);
            let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                let tracks: Vec<mpv::Track> = self.tracks(kind).cloned().collect();
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
                    if ui.add(egui::Button::new(format!("{}  Sync subtitles…", icons::TIMER)).shortcut_text("T")).clicked() {
                        self.actions.push(Action::ToggleSyncPanel);
                    }
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
            if resp.middle_clicked() {
                self.actions.push(Action::TogglePause);
            }
            // Scrolling changes the volume, 5% per wheel notch.
            if resp.hovered() {
                self.scroll_volume += ui.input(|i| {
                    let wheel = |e: &egui::Event| match e {
                        egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => delta.y,
                        egui::Event::MouseWheel { delta, .. } => delta.y * SCROLL_NOTCH,
                        _ => 0.0,
                    };
                    i.raw.events.iter().map(wheel).sum::<f32>()
                });
                let notches = (self.scroll_volume / SCROLL_NOTCH).trunc();
                if notches != 0.0 {
                    self.scroll_volume -= notches * SCROLL_NOTCH;
                    let volume = (self.playback.volume + 5.0 * f64::from(notches)).clamp(0.0, 150.0);
                    self.actions.push(Action::SetVolume(volume));
                }
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
        // The video area is black in either theme.
        child.style_mut().visuals = video_visuals(self.store.settings.accent);
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
        // Opening a video: just black until it shows.
        if self.current.is_some() {
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
            let resume = self.store.resume_position(path);
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let window = ui.max_rect();
        self.handle_mpv_events(&ctx);
        self.sample_playback();
        self.handle_input(&ctx);
        self.media_controls(&ctx, frame);
        self.sleep_blocker.set(self.loaded && !self.playback.paused);
        self.receive_durations();
        self.receive_scans();
        self.receive_missing_files();
        self.update_frame(&ctx);
        if let Some(ready) = &self.thumbnails_ready {
            if let Ok(thumbnails) = ready.try_recv() {
                self.thumbnails = Some(thumbnails);
                self.thumbnails_ready = None;
                self.open_for_preview();
            }
        }
        if let Some(Ok(thumbnails)) = &mut self.thumbnails {
            thumbnails.update(&ctx);
        }

        // While a video plays and the mouse is still, the cursor hides over
        // the video, and in fullscreen so do the controls, which float over
        // the video there. They stay up while paused and while the pointer
        // rests on the controls.
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down());
        let hover = ctx.pointer_hover_pos();
        let over_controls = self.controls_rect.zip(hover).is_some_and(|(r, p)| r.contains(p));
        if moved || over_controls {
            self.last_activity = Instant::now();
        }
        let idle = self.loaded && !self.playback.paused && self.last_activity.elapsed() >= CONTROLS_HIDE_DELAY;

        if !self.fullscreen {
            egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
            egui::Panel::bottom("controls")
                .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6)))
                .show(ui, |ui| self.controls(ui));
        }
        if self.sidebar_docked() && self.show_sidebar {
            let panel = match self.store.settings.sidebar_side {
                progress::Side::Left => egui::Panel::left("sidebar"),
                progress::Side::Right => egui::Panel::right("sidebar"),
            };
            let panel = panel
                .resizable(true)
                .default_size(280.0)
                .size_range(180.0..=600.0)
                .show(ui, |ui| self.sidebar(ui));
            self.sidebar_width = panel.response.rect.width();
        }
        let video = egui::CentralPanel::no_frame()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| self.video_area(ui))
            .response
            .rect;
        if !self.sidebar_docked() {
            self.floating_sidebar(&ctx, video);
        }
        self.controls_rect = None;
        if self.fullscreen && !idle {
            let accent = self.store.settings.accent;
            let area = egui::Area::new(egui::Id::unique("controls-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(window.left_bottom())
                .pivot(egui::Align2::LEFT_BOTTOM)
                .show(&ctx, |ui| {
                    ui.style_mut().visuals = video_visuals(accent);
                    egui::Frame::NONE
                        .fill(Color32::from_black_alpha(190))
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(window.width() - 24.0);
                            self.controls(ui);
                        });
                });
            self.controls_rect = Some(area.response.rect);
        }
        if idle && (self.fullscreen || hover.is_some_and(|p| video.contains(p))) {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        // Hidden, but kept open, while switching videos.
        if self.sync_panel && self.loaded {
            self.sync_panel(&ctx, video);
        }
        self.drop_overlay(&ctx, window);
        if self.confirm_reset {
            self.reset_dialog(&ctx);
        }
        if self.show_help {
            self.help_dialog(&ctx);
        }
        if self.settings_open {
            self.settings_dialog(&ctx);
        }
        if self.pending_drop.is_some() {
            self.drop_dialog(&ctx);
        }

        if let Some(up_next) = &self.up_next {
            if Instant::now() >= up_next.at {
                let index = up_next.index;
                self.actions.push(Action::Play(index));
            }
        }
        self.apply_actions(&ctx);

        // Also hides the controls and cursor once the mouse stops moving.
        if self.loaded && !self.playback.paused {
            ctx.request_repaint_after(Duration::from_millis(250));
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
        .right_text(if playing {
            // Readable on the accent-colored background.
            RichText::new(length).color(ui.visuals().selection.stroke.color)
        } else {
            RichText::new(length).weak()
        })
        // One line per entry (cut off with …), so rows have a fixed height.
        .truncate()
        .sense(Sense::click_and_drag());
    let resp = ui.add_sized([ui.available_width(), PLAYLIST_ROW_HEIGHT], button);
    if (!entry.watched && entry.fraction() > 0.0) || store.resume_position(path).is_some() {
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
        ui.separator();
        subfolder_option(ui, store.include_subfolders, actions);
    });
    resp
}

/// The "Include subfolders" switch in the playlist's right-click menus.
fn subfolder_option(ui: &mut egui::Ui, include: bool, actions: &mut Vec<Action>) {
    let mut include = include;
    let tip = "Also play videos in subfolders when opening a folder (rescans the opened folder now)";
    if ui.checkbox(&mut include, "Include subfolders").on_hover_text(tip).changed() {
        actions.push(Action::SetIncludeSubfolders(include));
        ui.close();
    }
}

/// Accent colors offered in the settings, besides egui's default blue.
const ACCENTS: [(&str, [u8; 3]); 6] = [
    ("Purple", [140, 95, 225]),
    ("Pink", [214, 78, 150]),
    ("Red", [205, 62, 58]),
    ("Orange", [222, 120, 30]),
    ("Green", [52, 150, 82]),
    ("Teal", [22, 145, 150]),
];

/// A clickable color square, outlined when selected.
fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool, name: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    ui.painter().rect_filled(rect.shrink(3.0), 4.0, color);
    if selected || resp.hovered() {
        let outline = if selected { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
        ui.painter().rect_stroke(rect, 5.0, (2.0, outline), egui::StrokeKind::Inside);
    }
    resp.on_hover_text(name)
}

/// Selection colors for an accent: its fill, with text that stays readable on it.
fn accent_selection(accent: Option<[u8; 3]>, dark: bool) -> egui::style::Selection {
    let default = if dark { egui::Visuals::dark() } else { egui::Visuals::light() }.selection;
    let Some([r, g, b]) = accent else { return default };
    let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    let text = if luminance > 150.0 { Color32::BLACK } else { Color32::WHITE };
    egui::style::Selection { bg_fill: Color32::from_rgb(r, g, b), stroke: egui::Stroke::new(1.0, text) }
}

/// The dark look of everything drawn over the video, whatever the theme.
fn video_visuals(accent: Option<[u8; 3]>) -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.selection = accent_selection(accent, true);
    visuals
}

/// Roomier than egui's defaults, so buttons and fields are easier to hit.
fn set_spacing(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let spacing = &mut style.spacing;
        spacing.button_padding = vec2(8.0, 4.0);
        spacing.interact_size.y = 24.0;
        spacing.item_spacing = vec2(8.0, 5.0);
        spacing.menu_margin = egui::Margin::same(8);
        spacing.icon_width = 16.0;
        spacing.icon_width_inner = 10.0;
    });
}

fn transport_button(icon: &str) -> egui::Button<'static> {
    egui::Button::new(icons::fill(icon).size(ICON_SIZE)).frame_when_inactive(false).min_size(vec2(34.0, 30.0))
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

/// The keyboard shortcuts, by topic, as shown with `?`.
/// Arrows are Phosphor icons (see `icons::ARROW_*`); the text font has none.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("PLAYBACK", &[
        ("Space / K", "Play / pause"),
        ("J / L", "Seek back / forward (Shift: further)"),
        ("\u{E058} / \u{E06C}", "Seek back / forward (Shift: further)"),
        ("Home", "Back to the start"),
        ("0–9", "Jump to 0%–90%"),
        (", / .", "Previous / next frame"),
        ("[ / ]", "Slower / faster"),
        ("Backspace", "Normal speed"),
        ("PgUp / PgDn", "Previous / next chapter"),
    ]),
    ("SOUND", &[
        ("\u{E08E} / \u{E03E}", "Volume"),
        ("M", "Mute"),
        ("A", "Cycle audio tracks"),
    ]),
    ("SUBTITLES", &[
        ("S", "Cycle subtitles"),
        ("T", "Sync subtitles panel"),
        ("Z / X", "Delay −/+ 0.1 s (Shift: 1 s)"),
        ("Ctrl+\u{E058} / \u{E06C}", "Show previous / next line now"),
        ("G / H", "Mark subtitle shown / line spoken"),
    ]),
    ("PLAYLIST", &[
        ("N / P", "Next / previous video"),
        ("W", "Mark watched / unwatched"),
        ("/", "Search (\u{E08E} \u{E03E} pick, Enter plays)"),
        ("O", "Open folder"),
        ("I", "Add videos"),
        ("R", "Refresh the folder"),
        ("B", "Show or hide the playlist"),
    ]),
    ("WINDOW", &[
        ("F", "Fullscreen"),
        ("Esc", "Leave fullscreen, close panels"),
        ("E", "Settings"),
        ("?", "This overview"),
    ]),
];

/// Moves the item at `from` so it lands before the item now at `to`, or at
/// the end when `to` is the length.
fn move_item<T>(list: &mut Vec<T>, from: usize, to: usize) {
    if from >= list.len() || to > list.len() {
        return;
    }
    let item = list.remove(from);
    list.insert(if to > from { to - 1 } else { to }, item);
}

/// Subtitle speeds that fix subtitles timed for a different frame rate,
/// whose lines drift further out of sync as the video goes on.
const SUB_TIMINGS: [(f64, &str); 3] = [
    (1.0, "normal"),
    (25.0 / 23.976, "made for 25 fps"),
    (23.976 / 25.0, "made for 23.976 fps"),
];

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
            icons::CHECK, icons::PLUS, icons::MINUS, icons::X, icons::TRASH, icons::RESET, icons::MORE, icons::WARNING, icons::PLAY, icons::SORT, icons::SEARCH, icons::CHECKS, icons::TIMER, icons::REFRESH, icons::RECENT,
            icons::FOLDER, icons::KEYBOARD, icons::GEAR, icons::ARROW_LEFT, icons::ARROW_RIGHT, icons::ARROW_UP,
            icons::ARROW_DOWN, "–",
            "—", "·", "…", "−", "×",
        ];
        let fill = [icons::PLAY, icons::PAUSE, icons::SKIP_BACK, icons::SKIP_FORWARD];
        let has = |family: FontFamily, s: &str| ctx.fonts_mut(|f| f.has_glyphs(&FontId::new(14.0, family), s));
        let missing: Vec<_> = regular.iter().filter(|s| !has(FontFamily::Proportional, s)).collect();
        assert!(missing.is_empty(), "missing regular glyphs: {missing:?}");
        let arrows = [icons::ARROW_LEFT, icons::ARROW_RIGHT, icons::ARROW_UP, icons::ARROW_DOWN];
        let missing: Vec<_> = arrows.iter().filter(|s| !has(FontFamily::Monospace, s)).collect();
        assert!(missing.is_empty(), "missing monospace glyphs: {missing:?}");
        let missing: Vec<_> = fill.iter().filter(|s| !has(FontFamily::Name("phosphor-fill".into()), s)).collect();
        assert!(missing.is_empty(), "missing fill glyphs: {missing:?}");
    }
}
