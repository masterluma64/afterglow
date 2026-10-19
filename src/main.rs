#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icons;
mod library;
mod mpv;
mod progress;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, TextureHandle, TextureOptions, vec2};

use library::{EpisodeId, Library};
use mpv::Mpv;
use progress::Store;

const SAVE_INTERVAL: Duration = Duration::from_secs(5);
const CONTROLS_HIDE_DELAY: Duration = Duration::from_millis(2500);
const ICON_SIZE: f32 = 17.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Afterglow")
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([480.0, 300.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let open: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    eframe::run_native(
        "Afterglow",
        options,
        Box::new(|cc| {
            icons::install(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx, open)))
        }),
    )
}

/// Something that can be played: an episode of a playlist, or a video on its own.
#[derive(Clone, PartialEq, Debug)]
enum Item {
    Episode(usize, EpisodeId),
    Video(PathBuf),
}

enum Action {
    AddFolder,
    OpenVideo,
    /// A folder (becomes a playlist) or a video (plays on its own).
    Open(PathBuf),
    Play(Item),
    ContinuePlaylist(usize),
    RescanPlaylist(usize),
    RemovePlaylist(usize),
    RemoveVideo(PathBuf),
    SetWatched(PathBuf, bool),
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
}

struct App {
    mpv: Result<Mpv, String>,
    store: Store,
    playlists: Vec<Library>,
    current: Option<Item>,
    /// True between mpv reporting the file as loaded and its end.
    loaded: bool,
    playback: Playback,
    texture: Option<TextureHandle>,
    show_sidebar: bool,
    fullscreen: bool,
    /// Playlist whose header should be expanded on the next frame.
    reveal: Option<usize>,
    /// Seek-bar position while the user drags it.
    scrub: Option<f64>,
    last_save: Instant,
    last_activity: Instant,
    message: Option<String>,
    actions: Vec<Action>,
}

impl App {
    fn new(ctx: &egui::Context, open: Vec<PathBuf>) -> Self {
        let store = Store::load();
        let mpv = Mpv::new(ctx.clone());
        if let Ok(mpv) = &mpv {
            mpv.set("volume", &store.volume.to_string());
        }
        let playlists = store.playlists.iter().map(|p| Library::scan(p).unwrap_or_else(|_| Library::empty(p))).collect();
        let mut app = Self {
            mpv,
            store,
            playlists,
            current: None,
            loaded: false,
            playback: Playback::default(),
            texture: None,
            show_sidebar: true,
            fullscreen: false,
            reveal: None,
            scrub: None,
            last_save: Instant::now(),
            last_activity: Instant::now(),
            message: None,
            actions: Vec::new(),
        };
        for path in open {
            app.open(path, ctx);
        }
        app
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        if path.is_dir() {
            self.add_playlist(path);
        } else if library::is_video(&path) {
            self.open_video(path, ctx);
        } else {
            self.message = Some(format!("Not a folder or video: {}", path.display()));
        }
    }

    fn add_playlist(&mut self, folder: PathBuf) {
        if let Some(i) = self.playlists.iter().position(|l| l.root == folder) {
            self.reveal = Some(i);
            return;
        }
        match Library::scan(&folder) {
            Ok(lib) if lib.seasons.is_empty() => {
                self.message = Some(format!("No videos found in {}", folder.display()));
            }
            Ok(lib) => {
                self.store.add_playlist(&folder);
                self.store.save();
                self.playlists.push(lib);
                self.reveal = Some(self.playlists.len() - 1);
                self.show_sidebar = true;
                self.message = None;
            }
            Err(e) => self.message = Some(format!("Could not open {}: {e}", folder.display())),
        }
    }

    /// Plays a single video. One that is part of a playlist plays as that
    /// playlist's episode, so the next episode follows.
    fn open_video(&mut self, path: PathBuf, ctx: &egui::Context) {
        let in_playlist =
            self.playlists.iter().enumerate().find_map(|(i, lib)| lib.find(&path).map(|id| Item::Episode(i, id)));
        let item = in_playlist.unwrap_or_else(|| {
            self.store.add_video(&path);
            Item::Video(path)
        });
        self.play(item, ctx);
    }

    fn remove_playlist(&mut self, index: usize) {
        match self.current {
            Some(Item::Episode(l, _)) if l == index => self.stop(),
            Some(Item::Episode(ref mut l, _)) if *l > index => *l -= 1,
            _ => {}
        }
        let lib = self.playlists.remove(index);
        self.store.remove_playlist(&lib.root);
        self.store.save();
    }

    fn rescan_playlist(&mut self, index: usize) {
        let current_path = self.current_path().map(Path::to_path_buf);
        let root = self.playlists[index].root.clone();
        self.playlists[index] = Library::scan(&root).unwrap_or_else(|_| Library::empty(&root));
        if let Some(Item::Episode(l, id)) = &mut self.current {
            if *l == index {
                match current_path.and_then(|p| self.playlists[index].find(&p)) {
                    Some(new_id) => *id = new_id,
                    None => self.stop(),
                }
            }
        }
    }

    fn item_path<'a>(&'a self, item: &'a Item) -> Option<&'a Path> {
        match item {
            Item::Episode(l, id) => Some(&self.playlists.get(*l)?.get(*id)?.path),
            Item::Video(path) => Some(path),
        }
    }

    fn current_path(&self) -> Option<&Path> {
        self.item_path(self.current.as_ref()?)
    }

    /// The episode to offer for a playlist: the last one played if it is
    /// unfinished, otherwise the first unwatched episode after it.
    fn continue_in(&self, index: usize) -> Option<EpisodeId> {
        let lib = self.playlists.get(index)?;
        let last = self.store.last_episode(&lib.root).and_then(|p| lib.find(p));
        let start = match last {
            Some(id) => {
                let entry = self.store.get(&lib.get(id)?.path);
                if !entry.watched || entry.resume_position().is_some() {
                    return Some(id);
                }
                lib.next(id)?
            }
            None => lib.ids().next()?,
        };
        lib.ids()
            .skip_while(|&i| i != start)
            .find(|&i| lib.get(i).is_some_and(|e| !self.store.get(&e.path).watched))
            .or(Some(start))
    }

    /// What to offer when nothing is playing, based on what was played last.
    fn continue_item(&self) -> Option<Item> {
        let last = self.store.last_played.as_ref()?;
        if let Some(index) = self.playlists.iter().position(|lib| lib.find(last).is_some()) {
            return self.continue_in(index).map(|id| Item::Episode(index, id));
        }
        let resumable = self.store.get(last).resume_position().is_some();
        (resumable && last.is_file()).then(|| Item::Video(last.clone()))
    }

    fn save_current_progress(&mut self) {
        if !self.loaded || self.playback.duration <= 0.0 {
            return;
        }
        if let Some(path) = self.current_path().map(Path::to_path_buf) {
            self.store.update_position(&path, self.playback.position, self.playback.duration);
        }
    }

    fn play(&mut self, item: Item, ctx: &egui::Context) {
        self.save_current_progress();
        let Ok(mpv) = &self.mpv else { return };
        let (path, title, playlist) = match &item {
            Item::Episode(l, id) => {
                let Some(lib) = self.playlists.get(*l) else { return };
                let Some(ep) = lib.get(*id) else { return };
                (ep.path.clone(), format!("{} — {}", ep.name, lib.name), Some(lib.root.clone()))
            }
            Item::Video(path) => (path.clone(), display_name(path), None),
        };

        let start = self.store.get(&path).resume_position().unwrap_or(0.0);
        match mpv.load_file(&path, start) {
            Ok(()) => {
                mpv.set("pause", "no");
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
                self.store.set_played(&path, playlist.as_deref());
                self.store.save();
                // Drop the previous frame so it never shows under new content.
                self.texture = None;
                self.current = Some(item);
                self.loaded = false;
                self.message = None;
            }
            Err(e) => self.message = Some(format!("Could not play {}: {e}", display_name(&path))),
        }
    }

    fn stop(&mut self) {
        self.save_current_progress();
        if let Ok(mpv) = &self.mpv {
            let _ = mpv.command(&["stop"]);
        }
        self.current = None;
        self.loaded = false;
    }

    fn next_item(&self, item: &Item) -> Option<Item> {
        match item {
            Item::Episode(l, id) => self.playlists.get(*l)?.next(*id).map(|n| Item::Episode(*l, n)),
            Item::Video(_) => None,
        }
    }

    fn prev_item(&self, item: &Item) -> Option<Item> {
        match item {
            Item::Episode(l, id) => self.playlists.get(*l)?.prev(*id).map(|n| Item::Episode(*l, n)),
            Item::Video(_) => None,
        }
    }

    fn handle_mpv_events(&mut self, ctx: &egui::Context) {
        let Ok(mpv) = &self.mpv else { return };
        for event in mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.loaded = true,
                mpv::Event::EndFile { eof, error } => {
                    self.loaded = false;
                    if error {
                        self.message = Some("Playback failed: the file could not be decoded.".into());
                    }
                    if !eof {
                        continue;
                    }
                    let Some(current) = self.current.clone() else { continue };
                    if let Some(path) = self.item_path(&current).map(Path::to_path_buf) {
                        self.store.set_watched(&path, true);
                    }
                    match self.next_item(&current) {
                        Some(next) => self.play(next, ctx),
                        None => self.current = None,
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
        self.actions.extend(dropped.into_iter().map(Action::Open));

        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let Playback { volume, duration, .. } = self.playback;
        ctx.input_mut(|i| {
            use egui::Modifiers as M;
            // egui ignores extra Shift/Alt when matching, so more specific
            // combinations must come before their plain counterparts.
            let mut bindings = vec![
                (M::CTRL, Key::O, Action::OpenVideo),
                (M::CTRL, Key::N, Action::AddFolder),
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
            Action::AddFolder => {
                if let Some(folder) = rfd::FileDialog::new().set_title("Add folder as playlist").pick_folder() {
                    self.add_playlist(folder);
                }
            }
            Action::OpenVideo => {
                let file = rfd::FileDialog::new()
                    .set_title("Open video")
                    .add_filter("Videos", library::VIDEO_EXTENSIONS)
                    .pick_file();
                if let Some(file) = file {
                    self.open_video(file, ctx);
                }
            }
            Action::Open(path) => self.open(path, ctx),
            Action::Play(item) => self.play(item, ctx),
            Action::ContinuePlaylist(index) => {
                if let Some(id) = self.continue_in(index) {
                    self.play(Item::Episode(index, id), ctx);
                }
            }
            Action::RescanPlaylist(index) => self.rescan_playlist(index),
            Action::RemovePlaylist(index) => self.remove_playlist(index),
            Action::RemoveVideo(path) => {
                if self.current == Some(Item::Video(path.clone())) {
                    self.stop();
                }
                self.store.remove_video(&path);
                self.store.save();
            }
            Action::SetWatched(path, watched) => {
                if self.loaded && self.current_path() == Some(path.as_path()) {
                    // Keep the live position from overwriting the change.
                    self.playback.position = if watched { self.playback.duration } else { 0.0 };
                }
                self.store.set_watched(&path, watched);
                self.store.save();
            }
            Action::Next | Action::Prev => {
                let target = match (&action, &self.current) {
                    (Action::Next, Some(item)) => self.next_item(item),
                    (Action::Prev, Some(item)) => self.prev_item(item),
                    _ => self.continue_item(),
                };
                if let Some(item) = target {
                    self.play(item, ctx);
                }
            }
            Action::ToggleFullscreen | Action::ExitFullscreen => {
                let fullscreen = matches!(action, Action::ToggleFullscreen) && !self.fullscreen;
                if fullscreen != self.fullscreen {
                    self.fullscreen = fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
                }
            }
            Action::ToggleSidebar => self.show_sidebar = !self.show_sidebar,
            Action::TogglePause => {
                if self.loaded {
                    if let Ok(mpv) = &self.mpv {
                        let _ = mpv.command(&["cycle", "pause"]);
                    }
                } else if let Some(item) = self.current.clone().or_else(|| self.continue_item()) {
                    self.play(item, ctx);
                }
            }
            action => self.apply_to_mpv(action),
        }
    }

    fn apply_to_mpv(&mut self, action: Action) {
        let Ok(mpv) = &self.mpv else { return };
        match action {
            Action::Seek { to, exact } if self.loaded => {
                let flags = if exact { "absolute+exact" } else { "absolute+keyframes" };
                let _ = mpv.command(&["seek", &format!("{to:.3}"), flags]);
            }
            Action::SeekBy(by) if self.loaded => {
                let _ = mpv.command(&["seek", &by.to_string(), "relative"]);
            }
            Action::SetVolume(v) => {
                mpv.set("volume", &format!("{v:.0}"));
                mpv.set("mute", "no");
                self.store.set_volume(v.round());
            }
            Action::ToggleMute => {
                let _ = mpv.command(&["cycle", "mute"]);
            }
            Action::SetTrack { kind, id } => {
                let prop = if kind == "sub" { "sid" } else { "aid" };
                mpv.set(prop, &id.map_or("no".into(), |id| id.to_string()));
            }
            Action::CycleSubtitles => {
                let _ = mpv.command(&["cycle", "sub"]);
            }
            Action::LoadSubtitle if self.loaded => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Load subtitle file")
                    .add_filter("Subtitles", &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"]);
                if let Some(dir) = self.current_path().and_then(Path::parent) {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(file) = dialog.pick_file() {
                    if let Err(e) = mpv.command(&["sub-add", &file.to_string_lossy(), "select"]) {
                        self.message = Some(format!("Could not load subtitles: {e}"));
                    }
                }
            }
            Action::FrameStep { forward } if self.loaded => {
                // Steps one frame and leaves playback paused.
                let _ = mpv.command(&[if forward { "frame-step" } else { "frame-back-step" }]);
            }
            Action::AdjustSubDelay(by) => {
                let _ = mpv.command(&["add", "sub-delay", &by.to_string()]);
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- UI

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if icon_button(ui, icons::SIDEBAR, "Toggle sidebar (L)").clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
            if ui.button(format!("{}  Add folder", icons::FOLDER_PLUS)).on_hover_text("Add a folder as playlist (Ctrl+N)").clicked() {
                self.actions.push(Action::AddFolder);
            }
            if ui.button(format!("{}  Open video", icons::FILE_VIDEO)).on_hover_text("Play a single video (Ctrl+O)").clicked() {
                self.actions.push(Action::OpenVideo);
            }
            let title = match &self.current {
                Some(Item::Episode(l, id)) => self.playlists[*l].get(*id).map(|ep| (ep.name.clone(), Some(&self.playlists[*l].name))),
                Some(Item::Video(path)) => Some((display_name(path), None)),
                _ => None,
            };
            if let Some((name, playlist)) = title {
                ui.separator();
                if let Some(playlist) = playlist {
                    ui.label(RichText::new(playlist).strong());
                }
                ui.label(RichText::new(name).weak());
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        if self.playlists.is_empty() && self.store.videos.is_empty() {
            ui.add_space(12.0);
            ui.weak("Drop a folder here to add it as a playlist, or a video to play it.");
            return;
        }
        let reveal = self.reveal.take();
        // Season to expand per playlist: the one playing, else the one to continue.
        let focus: Vec<Option<usize>> = (0..self.playlists.len())
            .map(|l| match &self.current {
                Some(Item::Episode(cl, id)) if *cl == l => Some(id.season),
                _ => self.continue_in(l).map(|id| id.season),
            })
            .collect();

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if !self.playlists.is_empty() {
                section_label(ui, "Playlists");
            }
            for (l, lib) in self.playlists.iter().enumerate() {
                let episodes = || lib.seasons.iter().flat_map(|s| &s.episodes);
                let watched = episodes().filter(|e| self.store.get(&e.path).watched).count();
                let header = RichText::new(format!("{}   {watched}/{}", lib.name, episodes().count())).strong();
                let playing_here = matches!(&self.current, Some(Item::Episode(cl, _)) if *cl == l);
                let mut collapsing = egui::CollapsingHeader::new(header)
                    .id_salt(("playlist", &lib.root))
                    .default_open(playing_here);
                if reveal == Some(l) {
                    collapsing = collapsing.open(Some(true));
                }
                let resp = collapsing.show(ui, |ui| {
                    if lib.seasons.is_empty() {
                        ui.weak("Folder is missing or has no videos.");
                    }
                    let single = lib.seasons.len() == 1;
                    for (s, season) in lib.seasons.iter().enumerate() {
                        let mut rows = |ui: &mut egui::Ui| {
                            for (e, ep) in season.episodes.iter().enumerate() {
                                let item = Item::Episode(l, EpisodeId { season: s, episode: e });
                                let selected = self.current.as_ref() == Some(&item);
                                media_row(ui, &self.store, &mut self.actions, item, &ep.name, &ep.path, selected, false);
                            }
                        };
                        if single {
                            rows(ui);
                            continue;
                        }
                        let watched = season.episodes.iter().filter(|e| self.store.get(&e.path).watched).count();
                        egui::CollapsingHeader::new(format!("{}   {watched}/{}", season.name, season.episodes.len()))
                            .id_salt(("season", &lib.root, s))
                            .default_open(focus[l] == Some(s))
                            .show(ui, |ui| rows(ui));
                    }
                });
                resp.header_response.on_hover_text(lib.root.display().to_string()).context_menu(|ui| {
                    if ui.button(format!("{}  Continue", icons::PLAY_CIRCLE)).clicked() {
                        self.actions.push(Action::ContinuePlaylist(l));
                    }
                    if ui.button(format!("{}  Rescan folder", icons::REFRESH)).clicked() {
                        self.actions.push(Action::RescanPlaylist(l));
                    }
                    if ui.button(format!("{}  Remove playlist", icons::TRASH)).clicked() {
                        self.actions.push(Action::RemovePlaylist(l));
                    }
                });
            }

            if !self.store.videos.is_empty() {
                ui.add_space(8.0);
                section_label(ui, "Videos");
                for path in &self.store.videos {
                    let item = Item::Video(path.clone());
                    let selected = self.current.as_ref() == Some(&item);
                    media_row(ui, &self.store, &mut self.actions, item, &display_name(path), path, selected, true);
                }
            }
        });
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let Playback { position, duration, paused, muted, volume, .. } = self.playback;
        let position = self.scrub.unwrap_or(position);
        let in_playlist = matches!(self.current, Some(Item::Episode(..)));
        ui.horizontal(|ui| {
            if ui.add_enabled(in_playlist, transport_button(icons::SKIP_BACK)).on_hover_text("Previous (P)").clicked() {
                self.actions.push(Action::Prev);
            }
            let icon = if self.loaded && !paused { icons::PAUSE } else { icons::PLAY };
            if ui.add(transport_button(icon)).on_hover_text("Play/pause (Space)").clicked() {
                self.actions.push(Action::TogglePause);
            }
            if ui.add_enabled(in_playlist, transport_button(icons::SKIP_FORWARD)).on_hover_text("Next (N)").clicked() {
                self.actions.push(Action::Next);
            }
            ui.add_space(4.0);
            ui.monospace(format_time(if self.loaded { position } else { 0.0 }));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let fs_icon = if self.fullscreen { icons::CORNERS_IN } else { icons::CORNERS_OUT };
                if icon_button(ui, fs_icon, "Fullscreen (F11)").clicked() {
                    self.actions.push(Action::ToggleFullscreen);
                }
                self.track_menu(ui, "sub");
                self.track_menu(ui, "audio");

                let mut volume = volume;
                let slider = egui::Slider::new(&mut volume, 0.0..=150.0).show_value(false);
                if ui.add_sized([80.0, 18.0], slider).on_hover_text(format!("Volume {volume:.0}%")).changed() {
                    self.actions.push(Action::SetVolume(volume));
                }
                let speaker = match () {
                    _ if muted || volume == 0.0 => icons::SPEAKER_X,
                    _ if volume < 50.0 => icons::SPEAKER_LOW,
                    _ => icons::SPEAKER_HIGH,
                };
                if icon_button(ui, speaker, "Mute (M)").clicked() {
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
                resp.clone().on_hover_text_at_pointer(format_time(frac_at(hover.x) * duration));
            }
        }

        let visuals = ui.visuals();
        let track = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
        ui.painter().rect_filled(track, 2.0, visuals.widgets.inactive.bg_fill);
        if active {
            let frac = (position / duration).clamp(0.0, 1.0) as f32;
            let filled = egui::Rect::from_min_max(track.min, egui::pos2(track.left() + track.width() * frac, track.bottom()));
            ui.painter().rect_filled(filled, 2.0, visuals.selection.bg_fill);
            if resp.hovered() || resp.dragged() {
                ui.painter().circle_filled(egui::pos2(filled.right(), track.center().y), 6.0, visuals.selection.bg_fill);
            }
        }
    }

    fn track_menu(&mut self, ui: &mut egui::Ui, kind: &'static str) {
        let (icon, tip) = if kind == "sub" { (icons::SUBTITLES, "Subtitles (S to cycle)") } else { (icons::WAVEFORM, "Audio track") };
        ui.add_enabled_ui(self.loaded, |ui| {
            let button = egui::Button::new(RichText::new(icon).size(ICON_SIZE)).frame_when_inactive(false);
            let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                let Ok(mpv) = &self.mpv else { return };
                let tracks = mpv.tracks(kind);
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
                if tracks.is_empty() {
                    ui.weak("No tracks");
                }
                if kind == "sub" {
                    ui.separator();
                    if ui.button(format!("{}  Load subtitle file…", icons::PLUS)).clicked() {
                        self.actions.push(Action::LoadSubtitle);
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!("Delay {:+.1}s", self.playback.sub_delay));
                        if ui.small_button(icons::MINUS).on_hover_text("Z").clicked() {
                            self.actions.push(Action::AdjustSubDelay(-0.1));
                        }
                        if ui.small_button(icons::PLUS).on_hover_text("X").clicked() {
                            self.actions.push(Action::AdjustSubDelay(0.1));
                        }
                    });
                }
            });
            response.on_hover_text(tip);
        });
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
        if let Ok(mpv) = &self.mpv {
            let ppp = ui.ctx().pixels_per_point();
            mpv.set_render_size((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        }
        if let Some(tex) = &self.texture {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let img_rect = egui::Rect::from_center_size(rect.center(), size * scale);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex.id(), img_rect, uv, Color32::WHITE);
        }
    }

    fn placeholder(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)));
        child.add_space(rect.height() * 0.36);
        if let Some(msg) = &self.message {
            child.colored_label(child.visuals().warn_fg_color, msg);
            child.add_space(8.0);
        }
        if let Err(e) = &self.mpv {
            child.colored_label(child.visuals().error_fg_color, e);
            child.label("Place libmpv-2.dll next to the executable.");
            return;
        }
        if self.current.is_some() {
            child.weak("Loading…");
            return;
        }
        if let Some(item) = self.continue_item() {
            let Some(path) = self.item_path(&item) else { return };
            let entry = self.store.get(path);
            let (name, context) = match &item {
                Item::Episode(l, id) => {
                    let lib = &self.playlists[*l];
                    let season = &lib.seasons[id.season].name;
                    let context = if lib.seasons.len() > 1 { format!("{} · {season}", lib.name) } else { lib.name.clone() };
                    (lib.get(*id).map(|e| e.name.clone()).unwrap_or_default(), Some(context))
                }
                Item::Video(path) => (display_name(path), None),
            };
            let verb = if entry.resume_position().is_some() { "Continue" } else { "Play" };
            let text = RichText::new(format!("{}   {verb}: {name}", icons::PLAY)).size(18.0);
            if child.button(text).clicked() {
                self.actions.push(Action::Play(item.clone()));
            }
            let detail = [context, entry.resume_position().map(|p| format!("from {}", format_time(p)))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            child.weak(detail);
            return;
        }
        child.label(RichText::new(icons::FOLDER_PLUS).size(44.0).weak());
        child.add_space(6.0);
        child.label(RichText::new("Drop a folder or video here").size(18.0));
        child.weak("A folder becomes a playlist; its subfolders are seasons.");
        child.add_space(10.0);
        child.horizontal(|ui| {
            // Center the two buttons.
            let width = 260.0;
            ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
            if ui.button(format!("{}  Add folder", icons::FOLDER_PLUS)).clicked() {
                self.actions.push(Action::AddFolder);
            }
            if ui.button(format!("{}  Open video", icons::FILE_VIDEO)).clicked() {
                self.actions.push(Action::OpenVideo);
            }
        });
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
            "Drop a folder to add a playlist, or a video to play it",
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
        self.update_frame(&ctx);

        // In fullscreen, controls appear on mouse movement and hide after a moment.
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down());
        if moved {
            self.last_activity = Instant::now();
        }
        let show_chrome = !self.fullscreen || self.last_activity.elapsed() < CONTROLS_HIDE_DELAY || !self.loaded;
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

        self.apply_actions(&ctx);

        if self.loaded && !self.playback.paused {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.fullscreen && show_chrome {
            ctx.request_repaint_after(CONTROLS_HIDE_DELAY);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_current_progress();
        self.store.save();
    }
}

/// A row in the sidebar for an episode or a loose video, with its progress.
#[allow(clippy::too_many_arguments)]
fn media_row(
    ui: &mut egui::Ui,
    store: &Store,
    actions: &mut Vec<Action>,
    item: Item,
    name: &str,
    path: &Path,
    selected: bool,
    removable: bool,
) {
    let entry = store.get(path);
    let text = if entry.watched {
        RichText::new(format!("{}  {name}", icons::CHECK)).weak()
    } else {
        RichText::new(name)
    };
    let resp = ui.add_sized([ui.available_width(), 22.0], egui::Button::selectable(selected, "").left_text(text));
    if (!entry.watched && entry.fraction() > 0.0) || entry.resume_position().is_some() {
        let r = resp.rect.shrink2(vec2(4.0, 1.0));
        let end = egui::pos2(r.left() + r.width() * entry.fraction(), r.bottom());
        ui.painter().line_segment([r.left_bottom(), end], (2.0, ui.visuals().selection.bg_fill));
    }
    let resp = resp.on_hover_text(hover_text(&entry));
    if resp.clicked() {
        actions.push(Action::Play(item.clone()));
    }
    resp.context_menu(|ui| {
        if ui.button(format!("{}  Play from start", icons::PLAY)).clicked() {
            actions.push(Action::SetWatched(path.to_path_buf(), false));
            actions.push(Action::Play(item.clone()));
        }
        let (label, watched) = if entry.watched { ("Mark as unwatched", false) } else { ("Mark as watched", true) };
        if ui.button(format!("{}  {label}", icons::CHECK)).clicked() {
            actions.push(Action::SetWatched(path.to_path_buf(), watched));
        }
        if removable && ui.button(format!("{}  Remove from list", icons::X)).clicked() {
            actions.push(Action::RemoveVideo(path.to_path_buf()));
        }
    });
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text.to_uppercase()).small().weak());
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
        "Watched".into()
    } else if entry.position > 0.0 {
        format!("{} / {}", format_time(entry.position), format_time(entry.duration))
    } else {
        "Not started".into()
    }
}

fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::icons;
    use eframe::egui::{Context, FontFamily, FontId};

    #[test]
    fn icons_and_text_glyphs_exist() {
        let ctx = Context::default();
        icons::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let regular = [
            icons::SIDEBAR, icons::FOLDER_PLUS, icons::FILE_VIDEO, icons::CORNERS_OUT, icons::CORNERS_IN,
            icons::SPEAKER_HIGH, icons::SPEAKER_LOW, icons::SPEAKER_X, icons::SUBTITLES, icons::WAVEFORM,
            icons::CHECK, icons::PLUS, icons::MINUS, icons::X, icons::REFRESH, icons::TRASH, icons::PLAY_CIRCLE,
            icons::PLAY, "—", "·", "…",
        ];
        let fill = [icons::PLAY, icons::PAUSE, icons::SKIP_BACK, icons::SKIP_FORWARD];
        let has = |family: FontFamily, s: &str| ctx.fonts_mut(|f| f.has_glyphs(&FontId::new(14.0, family), s));
        let missing: Vec<_> = regular.iter().filter(|s| !has(FontFamily::Proportional, s)).collect();
        assert!(missing.is_empty(), "missing regular glyphs: {missing:?}");
        let missing: Vec<_> = fill.iter().filter(|s| !has(FontFamily::Name("phosphor-fill".into()), s)).collect();
        assert!(missing.is_empty(), "missing fill glyphs: {missing:?}");
    }
}
