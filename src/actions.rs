//! What the app can do (`Action`), the keys that trigger it, and carrying it out.

use super::*;

pub(crate) enum Action {
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
    /// Opens the command palette (Ctrl+K).
    OpenPalette,
    /// Asks before resetting every video's progress.
    ConfirmResetProgress,
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
    /// Turns evening out the volume ("night mode") on or off.
    ToggleLevelVolume,
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

impl App {
    pub(crate) fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.actions.push(Action::Drop(dropped));
        }

        // No shortcuts while typing or while a dialog is open (Esc closes it).
        if ctx.egui_wants_keyboard_input() || self.confirm_reset || self.settings_open || self.pending_drop.is_some() || self.palette.is_some() {
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

    pub(crate) fn apply_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            self.apply(action, ctx);
        }
    }

    pub(crate) fn apply(&mut self, action: Action, ctx: &egui::Context) {
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
            Action::OpenPalette => self.palette = Some(Palette::default()),
            Action::ConfirmResetProgress => self.confirm_reset = true,
            Action::SetSettings(settings) => {
                self.store.set_settings(settings);
                self.apply_settings(ctx);
            }
            Action::ToggleLevelVolume => {
                let mut settings = self.store.settings.clone();
                settings.level_volume = !settings.level_volume;
                self.notify(if settings.level_volume { "Evening out the volume" } else { "Volume as recorded" });
                self.apply(Action::SetSettings(settings), ctx);
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
}
