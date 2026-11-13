#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod library;
mod platform;
mod player;
mod ui;
mod update;

use library::{disk, playlist, progress, stats, watch};
use platform::{clock, instance, media_keys, power, screen, shortcut, taskbar};
use player::{dll_search, mpv, playback, thumbnails};
use ui::{app_icon, commands, controls, dialogs, gradient, icons, items, video};
use ui::icons::Bold;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, TextureHandle, vec2};

use mpv::Mpv;
use progress::Store;

use actions::*;
use commands::*;
use controls::*;
use dialogs::*;
use playback::*;
use playlist::*;
use video::*;

/// The name shown in the UI.
const APP_NAME: &str = "Afterglow";
/// The name used for files, folders and other technical names.
const APP_ID: &str = "afterglow";
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
/// Height of a playlist entry showing a still of its video.
const STILL_ROW_HEIGHT: f32 = 52.0;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Scroll distance, in points, of one mouse wheel notch.
const SCROLL_NOTCH: f32 = 50.0;
/// Tags the reply to a subtitle step sent to mpv.
const SUB_STEP_TAG: u64 = 1;
/// Tags the replies to subtitle files added back when a video is opened.
const SUB_RESTORE_TAG: u64 = 2;
/// Tags the reply to saving a screenshot as a file.
const SCREENSHOT_TAG: u64 = 3;
/// Tags the replies to opening a video (see `playback::Loads`).
const LOAD_TAG: u64 = 4;
/// Tags the reply to taking a screenshot's pixels, for the clipboard.
const SCREENSHOT_COPY_TAG: u64 = 5;
/// How long the next video is announced before it starts by itself.
const AUTOPLAY_DELAY: Duration = Duration::from_secs(5);
/// The smallest the window can be made: room for the docked playlist beside
/// the video, and for most of a menu.
const MIN_WINDOW_SIZE: [f32; 2] = [800.0, 500.0];

/// The icon in the title bar and taskbar (the executable's own icon is built
/// in by build.rs).
fn window_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    egui::IconData { rgba: app_icon::render(SIZE), width: SIZE, height: SIZE }
}

fn main() -> eframe::Result {
    dll_search::restrict();
    // After an update, this waits for the old copy to quit first.
    let args = update::tidy_up(std::env::args_os().skip(1).collect(), progress::data_dir().as_deref());
    let open = absolute_paths(args.into_iter());
    // With Afterglow already running (using the same data folder), hand it
    // the files instead: two at once would save over each other's progress.
    let pipe = progress::data_dir().map(|dir| instance::pipe_name(&dir));
    let other_starts = match pipe.as_deref().map(instance::claim) {
        Some(instance::Claim::First(listener)) => Some(listener),
        Some(instance::Claim::Running) => {
            match instance::forward(pipe.as_deref().unwrap_or_default(), &open) {
                Ok(()) => return Ok(()),
                // Better two at once than none.
                Err(e) => eprintln!("could not hand the files to the running Afterglow: {e}"),
            }
            None
        }
        _ => None,
    };
    let (store, load_warning) = Store::load();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1280.0, 760.0])
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_drag_and_drop(true)
        .with_icon(std::sync::Arc::new(window_icon()));
    if store.on_top {
        viewport = viewport.with_always_on_top();
    }
    // Open the window where it was, unless that is no longer on screen.
    if let Some(window) = store.window {
        viewport = viewport
            .with_inner_size([window.width.max(MIN_WINDOW_SIZE[0]), window.height.max(MIN_WINDOW_SIZE[1])])
            .with_maximized(window.maximized);
        if screen::title_bar_visible(&window) {
            viewport = viewport.with_position([window.x, window.y]);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        // OpenGL lets mpv draw video straight into the window.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(|cc| {
            icons::install(&cc.egui_ctx);
            set_style(&cc.egui_ctx);
            let other_starts = other_starts.map(|listener| listener.listen(cc.egui_ctx.clone()));
            Ok(Box::new(App::new(&cc.egui_ctx, (store, load_warning), open, other_starts, cc.get_proc_address.clone())))
        }),
    )
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
    /// Seconds the sound is shifted by (later when positive).
    audio_delay: f64,
    speed: f64,
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
    /// Which video an end-of-file event is about.
    loads: Loads,
    playback: Playback,
    show_sidebar: bool,
    /// Whether the playlist was a single video opened by itself, as of the
    /// last frame: the playlist hides for one (see `single_video`).
    was_single_video: bool,
    fullscreen: bool,
    /// Seek-bar position while the user drags it.
    scrub: Option<f64>,
    last_save: Instant,
    last_activity: Instant,
    /// When the video was last seeked with the keys, which keeps the controls
    /// up in fullscreen, as pausing does.
    seeked: Option<Instant>,
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
    /// Reads folders and files for the playlist without freezing the window.
    disk: disk::Disk,
    /// The disk work under way, if any.
    disk_busy: Option<DiskBusy>,
    recent: RecentFolders,
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
    /// Screenshots being taken, in order, to announce once mpv has done both
    /// halves of each.
    screenshots: Vec<actions::Screenshot>,
    /// Saved track choices being restored: how many subtitle files are still
    /// loading, and the choices to apply after.
    restoring: Option<(usize, progress::TrackPrefs)>,
    /// The position and time of the last playback sample, to count the
    /// time watched since then in the stats.
    watch_mark: Option<(f64, Instant)>,
    /// Set when the playing video is marked unwatched, so playing on doesn't
    /// count it as watched again (until it is opened again).
    held_unwatched: bool,
    /// Seek-bar previews, once started in the background (see `thumbnails_ready`).
    thumbnails: Option<Result<thumbnails::Thumbnails, String>>,
    thumbnails_ready: Option<Receiver<Result<thumbnails::Thumbnails, String>>>,
    /// A still of each video, for the playlist.
    stills: player::stills::Stills,
    up_next: Option<UpNext>,
    /// The video that just played to its end, when no next video starts by
    /// itself: the video area shows what can be done next (see `end_screen`).
    ended: Option<usize>,
    watch: watch::FolderWatch,
    /// Playlist files found missing at startup.
    missing_files: Option<Receiver<HashSet<PathBuf>>>,
    /// The playlist entry being dragged to a new place.
    drag_row: Option<usize>,
    /// Playlist entries selected with Ctrl / Shift+click, to act on together.
    selected: HashSet<PathBuf>,
    /// Where a Shift+click selection extends from.
    select_anchor: Option<PathBuf>,
    /// Only playlist entries whose name contains this are listed.
    search: String,
    focus_search: bool,
    /// Which search result Enter plays.
    search_pick: usize,
    /// Scroll the list to this search result on the next frame.
    scroll_to_pick: Option<usize>,
    /// What the window's title says now.
    window_title: String,
    /// Display names of the playlist entries, and their lowercase forms.
    names: Vec<(String, String)>,
    /// When the playlist summary was added up: total and watched seconds, and
    /// how many lengths are unknown.
    summary: Option<(Instant, f64, f64, usize)>,
    /// Whether the "reset all progress" confirmation is open.
    confirm_reset: bool,
    /// Whether the "clear stats" confirmation is open.
    confirm_clear_stats: bool,
    /// The startup search for remembered videos whose files are gone.
    gone_scan: Option<Receiver<Vec<String>>>,
    /// Whether the keyboard shortcut overview is open.
    show_help: bool,
    /// Whether the settings dialog is open.
    settings_open: bool,
    /// The executables the shortcuts start, by `shortcut::Place`, as of opening
    /// Settings.
    shortcuts: [Option<PathBuf>; 2],
    /// The video whose folder settings are open.
    folder_dialog: Option<PathBuf>,
    /// The speed the playing video's folder set, if it set one.
    folder_speed: Option<f64>,
    stats_open: bool,
    /// The command palette, while it is open.
    palette: Option<Palette>,
    /// Dropped videos waiting for the answer to what drops should do.
    pending_drop: Option<Vec<PathBuf>>,
    /// Whether the answer to the dropped-videos question becomes the default.
    drop_remember: bool,
    /// The system media controls, set up once the window exists.
    media_keys: Option<Result<media_keys::MediaKeys, String>>,
    sleep_blocker: power::SleepBlocker,
    /// Checking for, downloading and installing updates.
    updater: update::Updater,
    /// The version just updated to and its release notes, while they show.
    whats_new: Option<progress::UpdateNotes>,
    /// Whether the home screen shows over the video (it always shows while
    /// nothing plays).
    home: bool,
    /// The home screen's cards, and when they were worked out.
    home_cards: Option<(Instant, Vec<ui::home::Card>)>,
    /// How the window was before the mini player, while it is on.
    mini: Option<ui::mini::Restore>,
    /// The window's handle, once it exists.
    hwnd: Option<isize>,
    /// The playing video's progress on the taskbar button and the buttons in
    /// its preview, set up once the window exists.
    taskbar: Option<Result<taskbar::Taskbar, String>>,
    /// Files from later starts of Afterglow (see `instance`): one pipe per
    /// data folder it used since starting.
    other_starts: Vec<Receiver<Vec<PathBuf>>>,
}

impl App {
    fn new(
        ctx: &egui::Context,
        (store, load_warning): (Store, Option<String>),
        open: Vec<PathBuf>,
        other_starts: Option<Receiver<Vec<PathBuf>>>,
        gl: Option<mpv::GetProcAddress>,
    ) -> Self {
        let mpv = gl.ok_or_else(|| "OpenGL is unavailable".to_string()).and_then(|gl| Mpv::new(ctx.clone(), gl));
        if let Ok(mpv) = &mpv {
            mpv.set("volume", &store.volume.to_string());
            // Reading properties can block while mpv waits for the UI thread to
            // draw a frame, so playback state comes in as events instead.
            for name in [
                "time-pos", "duration", "pause", "mute", "volume", "speed", "sub-delay", "sub-speed", "audio-delay",
                "chapter-list", "track-list", "metadata/by-key/title",
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
                if tx.send(missing).is_ok() {
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
            loads: Loads::default(),
            playback: Playback::default(),
            show_sidebar: true,
            was_single_video: false,
            fullscreen: false,
            scrub: None,
            last_save: Instant::now(),
            last_activity: Instant::now(),
            seeked: None,
            controls_rect: None,
            peek: None,
            sidebar_width: 280.0,
            scroll_volume: 0.0,
            notice: None,
            actions: Vec::new(),
            disk: disk::Disk::new(ctx.clone()),
            disk_busy: None,
            recent: RecentFolders::default(),
            durations: DurationProbe::new(ctx.clone()),
            chapters: Vec::new(),
            tracks: Vec::new(),
            tracks_changed_by_user: false,
            announce_track: None,
            announce_sub_step: None,
            screenshots: Vec::new(),
            restoring: None,
            watch_mark: None,
            held_unwatched: false,
            thumbnails: None,
            thumbnails_ready: Some(thumbnails::Thumbnails::start(ctx)),
            stills: player::stills::Stills::new(ctx.clone()),
            up_next: None,
            ended: None,
            watch: watch::FolderWatch::new(ctx.clone(), None),
            missing_files,
            drag_row: None,
            selected: HashSet::new(),
            select_anchor: None,
            search: String::new(),
            focus_search: false,
            search_pick: 0,
            scroll_to_pick: None,
            window_title: APP_NAME.to_string(),
            names: Vec::new(),
            summary: None,
            confirm_reset: false,
            confirm_clear_stats: false,
            gone_scan: None,
            show_help: false,
            settings_open: false,
            shortcuts: [None, None],
            folder_dialog: None,
            folder_speed: None,
            stats_open: false,
            palette: None,
            pending_drop: None,
            drop_remember: false,
            media_keys: None,
            sleep_blocker: power::SleepBlocker::default(),
            other_starts: other_starts.into_iter().collect(),
            updater: update::Updater::new(ctx.clone()),
            whats_new: None,
            home: false,
            home_cards: None,
            mini: None,
            hwnd: None,
            taskbar: None,
        };
        // Before opening anything, so language preferences apply to it.
        app.apply_settings(ctx);
        if !open.is_empty() {
            app.open(open, false, None);
        } else if let Some(index) = app.continue_target() {
            // Pre-open where we left off, paused on that frame.
            app.load(index, true, true, ctx);
        }
        app.request_durations();
        app.find_gone(ctx);
        app.check_recent(ctx);
        app.sync_watch();
        if let Some(warning) = load_warning {
            app.warn(warning);
        }
        // Updated since the last start: say what's new.
        app.whats_new = app.store.take_update_notes().filter(|n| n.version == update::VERSION);
        if app.store.settings.check_for_updates && !app.store.settings.update_server.trim().is_empty() {
            app.check_for_updates();
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

    /// Whether the controls hide in fullscreen: while a video plays and
    /// nothing has happened for a moment.
    fn controls_idle(&self) -> bool {
        let recent = |t: Instant| t.elapsed() < CONTROLS_HIDE_DELAY;
        self.loaded && !self.playback.paused && !recent(self.last_activity) && !self.seeked.is_some_and(recent)
    }

    /// Whether the playlist is one video opened by itself (not from a folder).
    fn single_video(&self) -> bool {
        self.playlist.first().is_some_and(|video| self.store.is_movie(video))
    }

    /// Keeps the window's place and size up to date in the store, so it opens
    /// the same way next time. Fullscreen and minimized don't count, and while
    /// maximized the size it had before is kept for un-maximizing.
    fn remember_window(&mut self, ctx: &egui::Context) {
        if self.mini() {
            return self.remember_mini(ctx);
        }
        if self.fullscreen {
            return;
        }
        let (outer, inner, maximized, minimized, scale) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized, v.minimized, i.pixels_per_point)
        });
        let (Some(outer), Some(inner), Some(maximized)) = (outer, inner, maximized) else { return };
        if minimized == Some(true) {
            return;
        }
        let window = match self.store.window {
            Some(before) if maximized => progress::WindowPlacement { maximized, ..before },
            _ => progress::WindowPlacement {
                x: outer.min.x,
                y: outer.min.y,
                width: inner.width(),
                height: inner.height(),
                maximized,
                scale,
            },
        };
        self.store.set_window(window);
    }

    /// Opens the files later starts of Afterglow were given (such as a video
    /// opened from Explorer), and comes to the front for them.
    fn receive_other_starts(&mut self, ctx: &egui::Context) {
        let starts: Vec<_> = self.other_starts.iter().flat_map(Receiver::try_iter).collect();
        for paths in starts {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if !paths.is_empty() {
                self.actions.push(Action::OpenAndShow(paths));
            }
        }
    }

    /// Shows how far the playing video is on the taskbar button, and takes
    /// clicks on the buttons in its preview.
    fn taskbar_progress(&mut self, ctx: &egui::Context) {
        if self.taskbar.is_none() {
            let Some(hwnd) = self.hwnd else { return };
            let taskbar = taskbar::Taskbar::new(hwnd, ctx.clone());
            if let Err(e) = &taskbar {
                eprintln!("taskbar progress unavailable: {e}");
            }
            self.taskbar = Some(taskbar);
        }
        let Playback { position, duration, paused, .. } = self.playback;
        let progress = (self.loaded && duration > 0.0).then(|| position / duration);
        let buttons = taskbar::Buttons {
            playing: self.loaded && !paused,
            has_previous: self.current.is_some_and(|i| i > 0),
            has_next: self.current.is_some_and(|i| i + 1 < self.playlist.len()),
        };
        let Some(Ok(taskbar)) = &mut self.taskbar else { return };
        taskbar.show(progress, paused);
        taskbar.set_buttons(buttons);
        for button in taskbar.clicks() {
            self.actions.push(match button {
                taskbar::Button::Previous => Action::Prev,
                taskbar::Button::PlayPause => Action::TogglePause,
                taskbar::Button::Next => Action::Next,
            });
        }
    }

    /// Handles media keys and shows what's playing in Windows' media overlay.
    fn media_controls(&mut self, ctx: &egui::Context) {
        if self.media_keys.is_none() {
            let Some(hwnd) = self.hwnd else { return };
            let keys = media_keys::MediaKeys::new(hwnd as *mut std::ffi::c_void, ctx.clone());
            if let Err(e) = &keys {
                eprintln!("media keys unavailable: {e}");
            }
            self.media_keys = Some(keys);
        }
        let title = self.current_path().map(|path| self.video_name(path));
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

}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let window = ui.max_rect();
        self.handle_mpv_events(&ctx);
        self.sample_playback();
        self.handle_input(&ctx);
        if self.hwnd.is_none() {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(RawWindowHandle::Win32(h)) = frame.window_handle().map(|h| h.as_raw()) {
                self.hwnd = Some(h.hwnd.get());
            }
        }
        self.media_controls(&ctx);
        self.taskbar_progress(&ctx);
        self.receive_other_starts(&ctx);
        self.remember_window(&ctx);
        self.update_window_title(&ctx);
        self.sleep_blocker.set(self.loaded && !self.playback.paused);
        self.receive_durations();
        self.receive_disk(&ctx);
        let gone = self.recent.receive();
        self.store.forget_recent(&gone);
        self.receive_scans();
        self.receive_missing_files();
        self.receive_gone();
        if let Some(release) = self.updater.poll() {
            self.notify(format!("Afterglow {} is ready: restart to update", release.version));
        }
        if let Some(ready) = &self.thumbnails_ready
            && let Ok(thumbnails) = ready.try_recv()
        {
            self.thumbnails = Some(thumbnails);
            self.thumbnails_ready = None;
            self.open_for_preview();
        }
        if let Some(Ok(thumbnails)) = &mut self.thumbnails {
            thumbnails.update(&ctx);
        }
        self.stills.update();

        // While a video plays and the mouse is still, the cursor hides over
        // the video, and in fullscreen so do the controls, which float over
        // the video there. They stay up while paused and while the pointer
        // rests on the controls.
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down());
        let hover = ctx.pointer_hover_pos();
        let over_controls = self.controls_rect.zip(hover).is_some_and(|(r, p)| r.contains(p));
        // An open menu or the command palette also counts, so the cursor stays while choosing.
        if moved || over_controls || egui::Popup::is_any_open(&ctx) || self.palette.is_some() {
            self.last_activity = Instant::now();
        }
        let idle = self.loaded && !self.playback.paused && self.last_activity.elapsed() >= CONTROLS_HIDE_DELAY;
        // Seeking with the keys keeps the controls up, but not the cursor.
        let controls_idle = self.controls_idle();

        // A video opened by itself has no use for the playlist, so it hides
        // (B still shows it); it comes back once there is more to list.
        // Playing again closes the home screen over the video.
        if self.home && self.loaded && !self.playback.paused {
            self.home = false;
        }
        let single = self.single_video();
        if single != self.was_single_video {
            self.was_single_video = single;
            self.show_sidebar = !single;
            self.peek = None;
        }
        if !self.controls_overlay() {
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
        // The mini player shows its controls only while the pointer is on it
        // (or while paused), so they don't cover the little video there is.
        let away = self.mini() && hover.is_none() && self.loaded && !self.playback.paused;
        if self.controls_overlay() && !controls_idle && !away {
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
        if self.mini() {
            self.mini_resize(&ctx, window);
        }
        self.drop_overlay(&ctx, window);
        if self.confirm_reset {
            self.reset_dialog(&ctx);
        }
        if self.show_help {
            self.help_dialog(&ctx);
        }
        if self.whats_new.is_some() {
            self.whats_new_dialog(&ctx);
        }
        if self.settings_open {
            self.settings_dialog(&ctx);
        }
        if self.folder_dialog.is_some() {
            self.folder_dialog(&ctx);
        }
        if self.stats_open {
            self.stats_dialog(&ctx);
        }
        // After the stats, so it shows on top of them.
        if self.confirm_clear_stats {
            self.clear_stats_dialog(&ctx);
        }
        if self.palette.is_some() {
            self.command_palette(&ctx);
        }
        if self.pending_drop.is_some() {
            self.drop_dialog(&ctx);
        }

        if let Some(up_next) = &self.up_next
            && Instant::now() >= up_next.at
        {
            let index = up_next.index;
            self.actions.push(Action::PlayFromStart(index));
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

/// Accent colors offered in the settings, besides egui's default blue.
const ACCENTS: [(&str, [u8; 3]); 6] = [
    ("Purple", [140, 95, 225]),
    ("Pink", [214, 78, 150]),
    ("Red", [205, 62, 58]),
    ("Orange", [222, 120, 30]),
    ("Green", [52, 150, 82]),
    ("Teal", [22, 145, 150]),
];

/// Roomier than egui's defaults, so buttons and fields are easier to hit,
/// and one text size everywhere: titles stand out by weight, hints by color.
fn set_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let body = style.text_styles[&egui::TextStyle::Body].size;
        for font in style.text_styles.values_mut() {
            font.size = body;
        }
        let spacing = &mut style.spacing;
        spacing.button_padding = vec2(8.0, 4.0);
        spacing.interact_size.y = 24.0;
        spacing.item_spacing = vec2(8.0, 5.0);
        spacing.menu_margin = egui::Margin::same(8);
        spacing.icon_width = 16.0;
        spacing.icon_width_inner = 10.0;
    });
}

/// Paths from the command line, made absolute: the playlist and progress are
/// remembered by path, and a relative one would mean something else once
/// Afterglow is started from another folder.
fn absolute_paths(args: impl Iterator<Item = std::ffi::OsString>) -> Vec<PathBuf> {
    args.map(PathBuf::from).map(|p| std::path::absolute(&p).unwrap_or(p)).collect()
}

/// A video's file name without its extension, or a folder's name.
fn display_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The keyboard shortcuts, by topic, as shown with `?`.
/// Arrows are Phosphor icons (see `icons::ARROW_*`); the text font has none.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Playback", &[
        ("Space / K", "Play / pause"),
        ("J / L", "Seek back / forward (Shift: further)"),
        ("\u{E058} / \u{E06C}", "Seek back / forward (Shift: further)"),
        ("Home", "Rewind to start"),
        ("0–9", "Jump to 0%–90%"),
        (", / .", "Previous / next frame"),
        ("[ / ]", "Slower / faster"),
        ("Backspace", "Normal speed"),
        ("PgUp / PgDn", "Previous / next chapter"),
        ("Enter", "Skip opening / ending, or press the button over the video"),
    ]),
    ("Sound", &[
        ("\u{E08E} / \u{E03E}", "Volume"),
        ("M", "Mute"),
        ("A", "Next audio track"),
        ("G / H", "Play sound earlier / later (Shift: 1 s)"),
    ]),
    ("Subtitles", &[
        ("S", "Next subtitle track"),
        ("Z / X", "Show subtitles earlier / later (Shift: 1 s)"),
        ("Ctrl+\u{E058} / \u{E06C}", "Show previous / next line now"),
    ]),
    ("Playlist", &[
        ("N / P", "Next / previous video"),
        ("W", "Mark as watched / unwatched"),
        ("/", "Search playlist (\u{E08E} \u{E03E} pick, Enter plays)"),
        ("O", "Open folder"),
        ("Shift+O", "Open video"),
        ("I", "Add videos"),
        ("R", "Refresh folder"),
        ("B", "Show or hide playlist"),
        ("Ctrl+click", "Select several videos (Shift: a range)"),
        ("Delete", "Remove selected videos"),
    ]),
    ("Window", &[
        ("F", "Fullscreen"),
        ("T", "Keep on top of other windows"),
        ("Shift+T", "Mini player: a small window kept on top"),
        ("C / F12", "Take screenshot"),
        ("Esc", "Clear selection, leave fullscreen"),
        ("E", "Settings"),
        ("Ctrl+K", "Find a command"),
        ("?", "This overview"),
    ]),
];

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

const SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

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
    use super::*;
    use eframe::egui::{Context, FontFamily, FontId};
    use crate::ui::sidebar::format_total;

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
    fn makes_command_line_paths_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let args = ["Shows\\Foo", "D:\\Movies\\Bar.mkv"].map(std::ffi::OsString::from);
        let paths = absolute_paths(args.into_iter());
        assert_eq!(paths, [cwd.join("Shows\\Foo"), PathBuf::from("D:\\Movies\\Bar.mkv")]);
    }

    #[test]
    fn icons_and_text_glyphs_exist() {
        let ctx = Context::default();
        icons::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let regular = [
            icons::SIDEBAR, icons::FOLDER_PLUS, icons::FILE_VIDEO, icons::CORNERS_OUT, icons::CORNERS_IN,
            icons::SPEAKER_HIGH, icons::SPEAKER_LOW, icons::SPEAKER_X, icons::SUBTITLES, icons::WAVEFORM,
            icons::CHECK, icons::PLUS, icons::MINUS, icons::X, icons::TRASH, icons::RESET, icons::MORE, icons::WARNING, icons::PLAY, icons::SEARCH, icons::TIMER, icons::REFRESH, icons::RECENT,
            icons::FOLDER, icons::KEYBOARD, icons::GEAR, icons::GAUGE, icons::CHART, icons::REWIND, icons::REPEAT, icons::EYE_SLASH, icons::BROOM, icons::SELECTION_SLASH, icons::FILE_PLUS, icons::LIST_NUMBERS, icons::COMMAND, icons::FILM_STRIP, icons::MOON, icons::FAST_FORWARD, icons::SLIDERS, icons::FLAG, icons::FLAG_CHECKERED, icons::FILM_SLATE, icons::MONITOR_PLAY, icons::CAMERA, icons::IMAGES, icons::STACK, icons::PICTURE_IN_PICTURE, icons::HOUSE, icons::DOWNLOAD, icons::SPARKLE, icons::ARROW_LEFT, icons::ARROW_RIGHT, icons::ARROW_UP,
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
