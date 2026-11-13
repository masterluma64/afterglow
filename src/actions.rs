//! What the app can do (`Action`), the keys that trigger it, and carrying it out.

use super::*;

pub(crate) enum Action {
    OpenFolder,
    /// Picks a video to play by itself, in place of the playlist.
    OpenVideo,
    /// Makes one video the whole playlist, as if opened by itself (a recent video).
    OpenSingle(PathBuf),
    OpenVideos,
    /// Folders and videos dropped onto the window or passed on the command line.
    Open(Vec<PathBuf>),
    /// Folders and videos dropped onto the window.
    Drop(Vec<PathBuf>),
    /// Files a later start of Afterglow was given (e.g. a video opened from
    /// Explorer): opened like `Open`, and the first video is opened right
    /// away, paused.
    OpenAndShow(Vec<PathBuf>),
    /// Videos the folder watch found new in the playlist's folder.
    Arrived(Vec<PathBuf>),
    /// Answers what dropped videos should do; with `remember`, it becomes the
    /// default and isn't asked again.
    ChooseDropBehavior { behavior: progress::DroppedVideos, remember: bool },
    Play(usize),
    /// Plays a video from its start (the next one after a video finishes).
    PlayFromStart(usize),
    /// Plays a video again from its start, unwatched (Play from start).
    PlayOver(usize),
    Remove(usize),
    ClearPlaylist,
    /// Moves a playlist entry so it lands before the entry at `to` (or at the end).
    Move { from: usize, to: usize },
    FocusSearch,
    /// Rescans the playlist's folder now: adds new videos, drops missing ones.
    RefreshFolder,
    ClearRecentFolders,
    ClearRecentVideos,
    ShowHelp,
    ShowStats,
    /// Docks the playlist beside the video, or lets it slide out over it.
    TogglePin,
    OpenSettings,
    /// Opens the command palette (Ctrl+K).
    OpenPalette,
    /// Asks before resetting every video's progress.
    ConfirmResetProgress,
    SetSettings(progress::Settings),
    /// Puts a shortcut to Afterglow on the desktop or in the Start menu, or
    /// removes it.
    SetShortcut(shortcut::Place, bool),
    /// Moves the progress file next to afterglow.exe, or into AppData.
    SetPortable(bool),
    SetIncludeSubfolders(bool),
    SetWatched(PathBuf, bool),
    /// Marks several videos watched (or unwatched), such as the selected ones.
    SetWatchedMany(Vec<PathBuf>, bool),
    /// Removes several videos from the playlist.
    RemoveMany(Vec<PathBuf>),
    /// Removes the selected videos from the playlist (Delete).
    RemoveSelected,
    ClearSelection,
    ResetAllProgress,
    /// Asks before clearing all stats.
    ConfirmClearStats,
    ClearStats,
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
    /// Keeps the window on top of other windows, or stops doing so.
    ToggleOnTop,
    /// Turns the window into the mini player, or back.
    ToggleMiniPlayer,
    ToggleSidebar,
    SetTrack { kind: &'static str, id: Option<i64> },
    CycleSubtitles,
    CycleAudio,
    /// Copies the current frame, with subtitles, to the clipboard and saves
    /// it as a PNG in PicturesAfterglow.
    Screenshot,
    OpenScreenshotFolder,
    /// Opens the settings of a video's folder.
    OpenFolderSettings(PathBuf),
    /// Saves the settings of a video's folder (see `Store::set_folder_settings`).
    SetFolderSettings(PathBuf, progress::FolderSettings),
    /// Makes the playing video's subtitle delay and timing its folder's.
    UseSubTimingForFolder,
    /// Skips the opening or ending playing now.
    Skip,
    /// Marks the current time as where the opening ends or the ending
    /// starts, for the playing video's folder.
    MarkSkip(progress::Skip),
    /// Turns evening out the volume ("night mode") on or off.
    ToggleLevelVolume,
    /// Marks the playing video as watched, or unwatched if it is.
    ToggleWatched,
    /// Clears the selection, cancels the next video's countdown, hides the
    /// slid-out playlist, leaves fullscreen or the mini player, whichever
    /// applies first.
    Escape,
    LoadSubtitle,
    AdjustSubDelay(f64),
    SetSubDelay(f64),
    /// Shifts subtitles so the previous (-1) or next (1) line shows right now.
    SubStep(i32),
    /// Stretches subtitle timing, for subtitles made for another frame rate.
    SetSubSpeed(f64),
    /// Shifts the sound later (or earlier, when negative) by this many seconds.
    AdjustAudioDelay(f64),
    SetAudioDelay(f64),
    /// Makes the playing video's audio delay its folder's.
    UseAudioDelayForFolder,
    FrameStep { forward: bool },
    SetSpeed(f64),
    /// Jumps to the next chapter, or back to the start of this or the previous one.
    Chapter { forward: bool },
}

impl Action {
    /// Whether it starts disk work for the playlist (see `App::start_disk`).
    pub(crate) fn needs_disk(&self) -> bool {
        matches!(
            self,
            Action::Open(_)
                | Action::Drop(_)
                | Action::OpenAndShow(_)
                | Action::Arrived(_)
                | Action::RefreshFolder
                | Action::SetIncludeSubfolders(_)
        )
    }
}

impl App {
    pub(crate) fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.actions.push(Action::Drop(dropped));
        }

        // No shortcuts while typing or while a dialog is open (Esc closes it).
        if ctx.egui_wants_keyboard_input() || self.confirm_reset || self.confirm_clear_stats || self.settings_open || self.folder_dialog.is_some() || self.stats_open || self.pending_drop.is_some() || self.palette.is_some() {
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
                (M::CTRL, Key::K, Action::OpenPalette),
                (M::CTRL, Key::ArrowLeft, Action::SubStep(-1)),
                (M::CTRL, Key::ArrowRight, Action::SubStep(1)),
                (M::ALT, Key::Enter, Action::ToggleFullscreen),
                (M::SHIFT, Key::O, Action::OpenVideo),
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
                (M::SHIFT, Key::T, Action::ToggleMiniPlayer),
                (M::NONE, Key::T, Action::ToggleOnTop),
                (M::NONE, Key::F12, Action::Screenshot),
                (M::NONE, Key::C, Action::Screenshot),
                (M::NONE, Key::Escape, Action::Escape),
                (M::NONE, Key::Delete, Action::RemoveSelected),
                (M::NONE, Key::Home, Action::Seek { to: 0.0, exact: true }),
                (M::NONE, Key::Comma, Action::FrameStep { forward: false }),
                (M::NONE, Key::Period, Action::FrameStep { forward: true }),
                (M::NONE, Key::M, Action::ToggleMute),
                (M::NONE, Key::N, Action::Next),
                (M::NONE, Key::P, Action::Prev),
                (M::NONE, Key::W, Action::ToggleWatched),
                (M::NONE, Key::S, Action::CycleSubtitles),
                (M::NONE, Key::A, Action::CycleAudio),
                (M::NONE, Key::B, Action::ToggleSidebar),
                (M::SHIFT, Key::Z, Action::AdjustSubDelay(-1.0)),
                (M::SHIFT, Key::X, Action::AdjustSubDelay(1.0)),
                (M::NONE, Key::Z, Action::AdjustSubDelay(-0.1)),
                (M::NONE, Key::X, Action::AdjustSubDelay(0.1)),
                (M::SHIFT, Key::G, Action::AdjustAudioDelay(-1.0)),
                (M::SHIFT, Key::H, Action::AdjustAudioDelay(1.0)),
                (M::NONE, Key::G, Action::AdjustAudioDelay(-0.1)),
                (M::NONE, Key::H, Action::AdjustAudioDelay(0.1)),
                (M::NONE, Key::OpenBracket, Action::SetSpeed(step_speed(speed, false))),
                (M::NONE, Key::CloseBracket, Action::SetSpeed(step_speed(speed, true))),
                (M::NONE, Key::Backspace, Action::SetSpeed(1.0)),
                (M::NONE, Key::PageUp, Action::Chapter { forward: false }),
                (M::NONE, Key::PageDown, Action::Chapter { forward: true }),
                (M::NONE, Key::Enter, Action::Skip),
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

    pub(crate) fn apply_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            self.apply(action, ctx);
        }
    }

    pub(crate) fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            // Disk work for the playlist goes one job at a time: these wait for
            // the one under way, and run on a later frame.
            action if action.needs_disk() && self.disk_busy.is_some() => self.actions.push(action),
            Action::OpenFolder => {
                if let Some(folder) = rfd::FileDialog::new().set_title("Open folder as playlist").pick_folder() {
                    self.actions.push(Action::Open(vec![folder]));
                }
            }
            Action::OpenVideo => {
                let file = rfd::FileDialog::new()
                    .set_title("Open video")
                    .add_filter("Videos", library::VIDEO_EXTENSIONS)
                    .pick_file();
                if let Some(file) = file {
                    self.actions.push(Action::OpenSingle(file));
                }
            }
            Action::OpenSingle(video) => self.replace_with_videos(vec![video], ctx),
            Action::OpenVideos => {
                let files = rfd::FileDialog::new()
                    .set_title("Add videos to the playlist")
                    .add_filter("Videos", library::VIDEO_EXTENSIONS)
                    .pick_files();
                if let Some(files) = files {
                    self.actions.push(Action::Open(files));
                }
            }
            Action::Open(paths) => self.open(paths, false, None),
            Action::Drop(paths) => self.open(paths, true, None),
            Action::OpenAndShow(paths) => {
                let video = paths.iter().find(|p| library::is_video(p)).cloned();
                self.open(paths, false, video);
            }
            Action::Arrived(videos) => {
                let job = disk::Job::Arrived(videos);
                self.start_disk(job, "new videos".into());
            }
            Action::ChooseDropBehavior { behavior, remember } => {
                if remember {
                    let mut settings = self.store.settings.clone();
                    settings.dropped_videos = Some(behavior);
                    self.store.set_settings(settings);
                    self.store.save();
                }
                if let Some(videos) = self.pending_drop.take() {
                    self.drop_videos(videos, behavior, ctx);
                }
            }
            Action::Play(index) => self.play(index, ctx),
            Action::PlayFromStart(index) => self.play_from_start(index, ctx),
            Action::PlayOver(index) => {
                if let Some(path) = self.playlist.get(index).cloned() {
                    self.apply(Action::SetWatched(path, false), ctx);
                    self.play_from_start(index, ctx);
                }
            }
            Action::Remove(index) => self.remove(index, ctx),
            Action::ClearPlaylist => self.clear_playlist(ctx),
            Action::Move { from, to } => self.reorder(|list| move_item(list, from, to)),
            Action::RefreshFolder => self.refresh_folder(),
            Action::ShowHelp => self.show_help = true,
            Action::ShowStats => self.stats_open = true,
            Action::OpenSettings => {
                self.settings_open = true;
                self.shortcuts = shortcut::Place::ALL.map(shortcut::target);
            }
            Action::SetShortcut(place, on) => {
                let result = if on { shortcut::create(place) } else { shortcut::remove(place) };
                self.shortcuts[place as usize] = shortcut::target(place);
                // "Start menu" keeps its capital.
                let name = if place == shortcut::Place::Desktop { "desktop" } else { place.name() };
                match result {
                    Ok(()) => self.notify(if on { format!("Shortcut added to {name}") } else { format!("Shortcut removed from {name}") }),
                    Err(e) => self.warn(format!("Couldn't change {name} shortcut: {e}")),
                }
            }
            Action::SetPortable(portable) => {
                let Some(dir) = progress::data_dir_for(portable) else { return };
                match self.store.move_to(&dir) {
                    // Later starts look for this Afterglow by its data folder.
                    Ok(()) => {
                        if let instance::Claim::First(listener) = instance::claim(&instance::pipe_name(&dir)) {
                            self.other_starts.push(listener.listen(ctx.clone()));
                        }
                        self.notify(if portable { "Progress is now saved next to afterglow.exe" } else { "Progress is now saved in AppData" });
                    }
                    Err(e) => self.warn(format!("Couldn't move progress: {e}")),
                }
            }
            Action::OpenPalette => {
                // It offers the recent folders that are there.
                self.check_recent(ctx);
                self.palette = Some(Palette::default());
            }
            Action::ConfirmResetProgress => self.confirm_reset = true,
            Action::ConfirmClearStats => self.confirm_clear_stats = true,
            Action::ClearStats => {
                self.store.clear_stats();
                self.store.save();
                self.notify("Stats cleared");
            }
            Action::SetSettings(settings) => {
                self.store.set_settings(settings);
                self.apply_settings(ctx);
            }
            Action::OpenFolderSettings(video) => self.folder_dialog = Some(video),
            Action::SetFolderSettings(video, settings) => {
                self.store.set_folder_settings(&video, settings);
                if let Some(current) = self.current_path().map(Path::to_path_buf) {
                    self.apply_folder_settings(&current);
                }
            }
            Action::UseSubTimingForFolder => {
                if let Some((video, mut folder)) = self.playing_folder() {
                    let Playback { sub_delay, sub_speed, .. } = self.playback;
                    folder.sub_delay = (sub_delay != 0.0).then_some(sub_delay);
                    folder.sub_speed = ((sub_speed - 1.0).abs() > 1e-6).then_some(sub_speed);
                    self.apply(Action::SetFolderSettings(video, folder), ctx);
                    self.store.save();
                    self.notify("Subtitle timing saved for this folder");
                }
            }
            Action::UseAudioDelayForFolder => {
                if let Some((video, mut folder)) = self.playing_folder() {
                    let delay = self.playback.audio_delay;
                    folder.audio_delay = (delay != 0.0).then_some(delay);
                    self.apply(Action::SetFolderSettings(video, folder), ctx);
                    self.store.save();
                    self.notify("Audio delay saved for this folder");
                }
            }
            Action::Skip => {
                if let (Ok(mpv), Some((skip, _, end))) = (&self.mpv, self.skip_now()) {
                    // Skipping the ending finishes the video, unless a scene
                    // follows the credits. It stops rather than seeking to the
                    // end, where a paused video would stay on its last frame.
                    if skip == progress::Skip::Ending && end >= self.playback.duration - 0.5 {
                        mpv.send(&["stop"]);
                        self.played_to_end(ctx);
                    } else {
                        mpv.send(&["seek", &format!("{end:.3}"), "absolute+exact"]);
                    }
                    self.notify(if skip == progress::Skip::Opening { "Skipped opening" } else { "Skipped ending" });
                } else if let Some(action) = self.main_button() {
                    // While nothing plays, Enter presses the button over the video.
                    self.apply(action, ctx);
                }
            }
            Action::MarkSkip(skip) => {
                if let Some((video, mut folder)) = self.playing_folder().filter(|_| self.loaded) {
                    let Playback { position, duration, .. } = self.playback;
                    let place = if self.store.is_movie(&video) { "for this video" } else { "in this folder" };
                    let text = match skip {
                        progress::Skip::Opening => {
                            folder.opening_end = Some(position.round());
                            format!("Opening ends at {} {place}", format_time(position))
                        }
                        progress::Skip::Ending => {
                            let before_end = (duration - position).max(0.0).round();
                            folder.ending_start = Some(before_end);
                            format!("Ending starts {} before the end {place}", format_time(before_end))
                        }
                    };
                    self.apply(Action::SetFolderSettings(video, folder), ctx);
                    self.store.save();
                    self.notify(text);
                }
            }
            // Switches a folder setting, if the playing video's folder has
            // one, otherwise the global setting.
            Action::ToggleLevelVolume => {
                let on = !self.store.settings_for(self.current_path()).level_volume;
                self.notify(if on { "Evening out the volume" } else { "Volume as recorded" });
                match self.playing_folder().filter(|(_, f)| f.level_volume.is_some()) {
                    Some((video, folder)) => {
                        self.apply(Action::SetFolderSettings(video, progress::FolderSettings { level_volume: Some(on), ..folder }), ctx);
                    }
                    None => {
                        let settings = progress::Settings { level_volume: on, ..self.store.settings.clone() };
                        self.apply(Action::SetSettings(settings), ctx);
                    }
                }
            }
            Action::ClearRecentFolders => {
                self.store.clear_recent_folders();
                self.store.save();
                self.notify("Recent folders cleared");
            }
            Action::ClearRecentVideos => {
                self.store.clear_recent_videos();
                self.store.save();
                self.notify("Recent videos cleared");
            }
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
            Action::SetIncludeSubfolders(include) => self.set_include_subfolders(include),
            Action::SetWatched(path, watched) => {
                let playing = self.loaded && self.current_path() == Some(path.as_path());
                self.store.set_watched(&path, watched);
                if playing {
                    self.keep_watched_mark(watched);
                }
                self.store.save();
                let name = self.video_name(&path);
                self.notify(match (watched, playing) {
                    (true, _) => format!("{name}: marked as watched"),
                    (false, true) => format!("{name}: marked as unwatched"),
                    (false, false) => format!("{name}: progress reset"),
                });
                // Done with the playing video, so move on to the next one, as N does.
                if let Some(index) = self.current.filter(|_| watched && self.current_path() == Some(path.as_path())) {
                    if index + 1 < self.playlist.len() {
                        self.play(index + 1, ctx);
                        if let Some(notice) = self.notice.as_mut().filter(|n| !n.warning) {
                            notice.text = format!("{name}: marked as watched  ·  next: {}", notice.text);
                        }
                    } else {
                        self.notify(format!("{name}: marked as watched  ·  this is the last video"));
                    }
                }
            }
            Action::SetWatchedMany(paths, watched) => {
                let playing = self.loaded && self.current_path().is_some_and(|current| paths.iter().any(|p| p == current));
                for path in &paths {
                    self.store.set_watched(path, watched);
                }
                // As for one video (see `SetWatched`), but without moving on.
                if playing {
                    self.keep_watched_mark(watched);
                }
                self.store.save();
                self.selected.clear();
                let videos = format!("{} video{}", paths.len(), if paths.len() == 1 { "" } else { "s" });
                self.notify(if watched { format!("Marked {videos} as watched") } else { format!("Progress reset for {videos}") });
            }
            Action::RemoveMany(paths) => {
                // From the end, so the indices of the rest stay put.
                let indices: Vec<usize> = (0..self.playlist.len()).rev().filter(|&i| paths.contains(&self.playlist[i])).collect();
                for &index in &indices {
                    self.remove(index, ctx);
                }
                self.selected.clear();
                self.notify(format!("Removed {} video{} from the playlist", indices.len(), if indices.len() == 1 { "" } else { "s" }));
            }
            Action::ClearSelection => self.selected.clear(),
            Action::RemoveSelected => {
                let paths = self.selected_paths();
                if !paths.is_empty() {
                    self.apply(Action::RemoveMany(paths), ctx);
                }
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
                match self.playing_folder().filter(|(_, f)| f.autoplay_next.is_some()) {
                    Some((video, folder)) => {
                        self.store.set_folder_settings(&video, progress::FolderSettings { autoplay_next: Some(autoplay), ..folder });
                    }
                    None => self.store.set_autoplay_next(autoplay),
                }
                self.store.save();
                if !autoplay {
                    self.cancel_up_next();
                }
                // The Settings checkbox shows it already.
                if !self.settings_open {
                    self.notify(if autoplay { "Auto-play on" } else { "Auto-play off" });
                }
            }
            Action::CancelUpNext => self.cancel_up_next(),
            Action::Next | Action::Prev => {
                let next = matches!(action, Action::Next);
                if let Some(up_next) = self.up_next.as_ref().filter(|_| next && self.current.is_none()) {
                    self.play_from_start(up_next.index, ctx);
                    return;
                }
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
            Action::ToggleMiniPlayer if self.mini() => self.exit_mini(ctx),
            Action::ToggleMiniPlayer => self.enter_mini(ctx),
            // Fullscreen from the mini player goes by the window it came from.
            Action::ToggleFullscreen if self.mini() => {
                self.exit_mini(ctx);
                if !self.fullscreen {
                    self.apply(Action::ToggleFullscreen, ctx);
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
            Action::ToggleOnTop => {
                let on_top = !self.store.on_top;
                self.store.set_on_top(on_top);
                self.store.save();
                // The mini player stays on top either way.
                if !self.mini() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(window_level(on_top)));
                }
                self.notify(if on_top { "Kept on top of other windows" } else { "No longer kept on top" });
            }
            Action::ToggleSidebar if self.sidebar_docked() => self.show_sidebar = !self.show_sidebar,
            Action::ToggleSidebar => self.peek = if self.peek.is_some() { None } else { Some(Peek::default()) },
            Action::Escape => {
                if !self.selected.is_empty() {
                    self.selected.clear();
                } else if self.up_next.is_some() {
                    self.cancel_up_next();
                } else if self.peek.is_some() {
                    self.peek = None;
                } else if self.fullscreen {
                    self.apply(Action::ExitFullscreen, ctx);
                } else if self.mini() {
                    self.exit_mini(ctx);
                }
            }
            Action::Screenshot => self.screenshot(),
            Action::OpenScreenshotFolder => match screenshot_folder() {
                Some(folder) => {
                    let _ = std::fs::create_dir_all(&folder);
                    let _ = std::process::Command::new("explorer").arg(folder).spawn();
                }
                None => self.warn("There is no Pictures folder to keep screenshots in"),
            },
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
                } else if let Some(up_next) = self.up_next.as_ref().filter(|_| self.current.is_none()) {
                    self.play_from_start(up_next.index, ctx);
                } else if let Some(next) = self.ended.map(|i| i + 1).filter(|&i| i < self.playlist.len() && self.current.is_none()) {
                    // What the end screen offers first.
                    self.play_from_start(next, ctx);
                } else if let Some(index) = self.current.or_else(|| self.continue_target()) {
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
                        | Action::SetSubSpeed(_)
                        | Action::AdjustAudioDelay(_)
                        | Action::SetAudioDelay(_)
                );
                // They are saved once mpv reports them (see `property_changed`).
                self.tracks_changed_by_user |= changes_tracks;
                self.apply_to_mpv(action);
            }
        }
    }

    pub(crate) fn apply_to_mpv(&mut self, action: Action) {
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
                // Shows where it got to, as while paused.
                self.seeked = Some(Instant::now());
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
                if self.loaded {
                    self.announce_track = Some(kind);
                }
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
                self.notify(if forward { "Next frame" } else { "Previous frame" });
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
            Action::SetSubSpeed(speed) if self.loaded => {
                mpv.set("sub-speed", &speed.to_string());
                self.notify(match SUB_TIMINGS.iter().find(|(s, _)| (s - speed).abs() < 1e-6) {
                    Some((_, label)) => format!("Subtitle timing: {label}"),
                    None => format!("Subtitle speed {speed:.4}"),
                });
            }
            Action::AdjustAudioDelay(by) if self.loaded => {
                mpv.send(&["add", "audio-delay", &by.to_string()]);
                self.notify(audio_delay_notice(self.playback.audio_delay + by));
            }
            Action::SetAudioDelay(delay) if self.loaded => {
                mpv.set("audio-delay", &delay.to_string());
                self.notify(audio_delay_notice(delay));
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
}


impl App {
    /// The playing video, with the settings of its folder (empty if it has
    /// none), to change them.
    pub(crate) fn playing_folder(&self) -> Option<(PathBuf, progress::FolderSettings)> {
        let video = self.current_path()?;
        let folder = self.store.folder_settings(video).map(|(_, f)| f.clone()).unwrap_or_default();
        Some((video.to_path_buf(), folder))
    }

    /// Stops the countdown to the next video, which leaves the end screen
    /// for the video that finished.
    fn cancel_up_next(&mut self) {
        if let Some(up_next) = self.up_next.take() {
            self.ended = up_next.index.checked_sub(1);
        }
    }

    /// Whether the next video starts by itself after the playing one (or
    /// after any video, when none is playing).
    pub(crate) fn autoplay_next(&self) -> bool {
        self.current_path().map_or(self.store.autoplay_next, |p| self.store.autoplay_after(p))
    }

    /// After the playing video was marked watched or unwatched in the store,
    /// keeps playing on from overwriting that. Marked watched, its saved
    /// position goes to the end. Marked unwatched, it plays on from where it
    /// is (Play from start is for starting over), its position is kept, and
    /// it doesn't count as watched again until it is opened again.
    fn keep_watched_mark(&mut self, watched: bool) {
        self.held_unwatched = !watched;
        if watched {
            self.playback.position = self.playback.duration;
        } else {
            self.save_current_progress();
        }
    }
}

/// "Audio delay +0.2 s (sound later)".
fn audio_delay_notice(delay: f64) -> String {
    // Rounded, so steps of 0.1 adding up to nothing don't show as −0.0.
    let delay = (delay * 10.0).round() / 10.0;
    match delay {
        d if d > 0.0 => format!("Audio delay {d:+.1} s (sound later)"),
        d if d < 0.0 => format!("Audio delay {d:+.1} s (sound earlier)"),
        _ => "Audio delay 0.0 s".into(),
    }
}

/// The window's level for keeping it on top of other windows, or not.
pub(crate) fn window_level(on_top: bool) -> egui::WindowLevel {
    if on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal }
}

/// A screenshot under way: mpv saves it as a file (if it has a `path`) and
/// (if `copy`) copies its pixels separately, and it is announced once both
/// are done.
pub(crate) struct Screenshot {
    path: Option<PathBuf>,
    copy: bool,
    copied: Option<bool>,
    saved: Option<bool>,
}

impl App {
    /// Saves the current frame in the screenshots folder and copies it to
    /// the clipboard, each unless turned off.
    fn screenshot(&mut self) {
        let (Ok(mpv), true) = (&self.mpv, self.loaded) else { return };
        let copy = self.store.settings.copy_screenshots;
        let path = match self.store.settings.save_screenshots.then(|| self.screenshot_path()) {
            Some(Ok(path)) => Some(path),
            Some(Err(e)) => return self.warn(e),
            None if copy => None,
            None => return self.warn("Screenshots are neither copied nor saved; turn one on in Settings"),
        };
        // Both are taken from the frame showing now.
        if copy {
            mpv.send_tagged(SCREENSHOT_COPY_TAG, &["screenshot-raw", "subtitles", "rgba"]);
        }
        if let Some(path) = &path {
            mpv.send_tagged(SCREENSHOT_TAG, &["screenshot-to-file", &path.to_string_lossy(), "subtitles"]);
        }
        self.screenshots.push(Screenshot { path, copy, copied: None, saved: None });
    }

    /// A free `<video name> <time>.png` in the screenshots folder, which is
    /// created if needed.
    fn screenshot_path(&self) -> Result<PathBuf, String> {
        let folder = screenshot_folder().ok_or("There is no Pictures folder to keep screenshots in")?;
        std::fs::create_dir_all(&folder).map_err(|e| format!("Could not create {}: {e}", folder.display()))?;
        let name = self.current_path().map(display_name).unwrap_or_default();
        let time = format_time(self.playback.position).replace(':', ".");
        let mut path = folder.join(format!("{name} {time}.png"));
        // Several of the same second (stepping through frames) are numbered,
        // also while earlier ones are still being written.
        let mut n = 2;
        while path.exists() || self.screenshots.iter().any(|s| s.path.as_ref() == Some(&path)) {
            path = folder.join(format!("{name} {time} ({n}).png"));
            n += 1;
        }
        Ok(path)
    }

    /// mpv finished copying (`copy`) or saving the oldest screenshot that
    /// waits for it; `result` holds the pixels of a copy.
    pub(crate) fn screenshot_done(&mut self, ctx: &egui::Context, copy: bool, ok: bool, result: &mpv::Node) {
        let waiting = |s: &Screenshot| if copy { s.copy && s.copied.is_none() } else { s.path.is_some() && s.saved.is_none() };
        let Some(index) = self.screenshots.iter().position(waiting) else { return };
        let shot = &mut self.screenshots[index];
        if copy {
            let image = mpv::screenshot_image(result).filter(|_| ok);
            shot.copied = Some(image.is_some());
            if let Some(image) = image {
                ctx.copy_image(image);
            }
        } else {
            shot.saved = Some(ok);
        }
        // Not copied or not saved, that part counts as done.
        let copied = if shot.copy { shot.copied } else { Some(true) };
        let saved = if shot.path.is_some() { shot.saved } else { Some(true) };
        let (Some(copied), Some(saved)) = (copied, saved) else { return };
        let shot = self.screenshots.remove(index);
        let Some(folder) = shot.path.as_deref().map(short_folder) else {
            return if copied { self.notify("Screenshot copied to clipboard") } else { self.warn("Could not copy the screenshot") };
        };
        match (shot.copy, copied, saved) {
            (false, _, true) => self.notify(format!("Screenshot saved in {folder}")),
            (false, _, false) => self.warn(format!("Could not save the screenshot in {folder}")),
            (true, true, true) => self.notify(format!("Screenshot copied to clipboard and saved in {folder}")),
            (true, true, false) => self.warn(format!("Screenshot copied to clipboard, but could not be saved in {folder}")),
            (true, false, true) => self.warn(format!("Screenshot saved in {folder}, but could not be copied")),
            (true, false, false) => self.warn("Could not take a screenshot"),
        }
    }
}

/// Where screenshots go: an Afterglow folder in the user's Pictures.
pub(crate) fn screenshot_folder() -> Option<PathBuf> {
    platform::pictures().map(|pictures| pictures.join(APP_NAME))
}

/// A file's folder and the one above it, e.g. `Pictures\Afterglow`.
fn short_folder(path: &Path) -> String {
    let folder = path.parent().unwrap_or(path);
    match (folder.parent().and_then(Path::file_name), folder.file_name()) {
        (Some(above), Some(name)) => Path::new(above).join(name).display().to_string(),
        _ => folder.display().to_string(),
    }
}
