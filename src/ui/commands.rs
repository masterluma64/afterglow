//! The right-click menu on the video and the command palette (Ctrl+K).

use std::borrow::Cow;

use crate::*;

/// What is typed into the command palette, and which match Enter runs.
#[derive(Default)]
pub(crate) struct Palette {
    pub(crate) query: String,
    pub(crate) pick: usize,
}

/// An entry in the command palette: a command, or a playlist video to play.
pub(crate) struct Command {
    pub(crate) label: String,
    /// Its key (or a video's length), shown beside it.
    pub(crate) key: Cow<'static, str>,
    /// Other words it can be found by.
    pub(crate) keywords: &'static str,
    pub(crate) action: Action,
    /// Videos are only listed once something is typed, after the commands.
    pub(crate) video: bool,
    /// Shown in front of the label; set from the action (see `App::command_icon`)
    /// unless given.
    pub(crate) icon: &'static str,
}

pub(crate) fn command(label: impl Into<String>, key: &'static str, keywords: &'static str, action: Action) -> Command {
    Command { label: label.into(), key: key.into(), keywords, action, video: false, icon: "" }
}

/// Whether every word typed into the command palette appears in a command's
/// name or keywords (ignoring case). `query` is already lowercase.
pub(crate) fn command_matches(query: &str, command: &Command) -> bool {
    let text = format!("{} {}", command.label, command.keywords).to_lowercase();
    query.split_whitespace().all(|word| text.contains(word))
}

/// Orders palette matches: names starting with the text first, then names
/// with a word starting with it, then the rest (each in their usual order).
pub(crate) fn command_rank(query: &str, label: &str) -> u8 {
    let label = label.to_lowercase();
    let first = query.split_whitespace().next().unwrap_or("");
    if label.starts_with(query) {
        0
    } else if label.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(first)) {
        1
    } else {
        2
    }
}

impl App {
    /// The right-click menu on the video: what the controls and keys do,
    /// gathered in one place (also in fullscreen, where the controls hide),
    /// with each item's key beside it.
    pub(crate) fn video_menu(&mut self, ui: &mut egui::Ui) {
        let item = |ui: &mut egui::Ui, icon: &str, text: &str, key: &str| {
            ui.add(egui::Button::new(format!("{icon}  {text}")).shortcut_text(RichText::new(key).weak())).clicked()
        };
        if self.loaded {
            let paused = self.playback.paused;
            let (icon, text) = if paused { (icons::PLAY, "Play") } else { (icons::PAUSE, "Pause") };
            if item(ui, icon, text, "Space") {
                self.actions.push(Action::TogglePause);
            }
            if self.current.is_some_and(|i| i + 1 < self.playlist.len()) && item(ui, icons::SKIP_FORWARD, "Next video", "N") {
                self.actions.push(Action::Next);
            }
            if self.current.is_some_and(|i| i > 0) && item(ui, icons::SKIP_BACK, "Previous video", "P") {
                self.actions.push(Action::Prev);
            }
            ui.separator();
            ui.menu_button(format!("{}  Audio track", icons::WAVEFORM), |ui| self.track_items(ui, "audio"));
            ui.menu_button(format!("{}  Subtitles", icons::SUBTITLES), |ui| self.track_items(ui, "sub"));
            ui.menu_button(format!("{}  Speed  {}", icons::GAUGE, format_speed(self.playback.speed)), |ui| self.speed_items(ui));
            if !self.chapters.is_empty() {
                ui.menu_button(format!("{}  Chapters", icons::LIST_NUMBERS), |ui| {
                    let now = chapter_at(&self.chapters, self.playback.position);
                    for (i, chapter) in self.chapters.iter().enumerate() {
                        let label = egui::Button::selectable(now == Some(i), &chapter.title)
                            .shortcut_text(ui::secondary(ui, now == Some(i), format_time(chapter.time)));
                        if ui.add(label).clicked() {
                            self.actions.push(Action::Seek { to: chapter.time, exact: true });
                        }
                    }
                });
            }
            ui.separator();
            let watched = self.current_path().is_some_and(|p| self.store.get(p).watched);
            let (icon, text) = if watched { (icons::RESET, "Mark as unwatched") } else { (icons::CHECK, "Mark as watched") };
            if item(ui, icon, text, "W") {
                self.actions.push(Action::ToggleWatched);
            }
        } else {
            if item(ui, icons::FOLDER_PLUS, "Open folder…", "O") {
                self.actions.push(Action::OpenFolder);
            }
            if item(ui, icons::FILE_VIDEO, "Add videos…", "I") {
                self.actions.push(Action::OpenVideos);
            }
        }
        ui.separator();
        let (icon, text) = if self.fullscreen { (icons::CORNERS_IN, "Exit fullscreen") } else { (icons::CORNERS_OUT, "Fullscreen") };
        if item(ui, icon, text, "F") {
            self.actions.push(Action::ToggleFullscreen);
        }
        let shown = if self.sidebar_docked() { self.show_sidebar } else { self.peek.is_some() };
        if item(ui, icons::SIDEBAR, if shown { "Hide the playlist" } else { "Show the playlist" }, "B") {
            self.actions.push(Action::ToggleSidebar);
        }
        ui.separator();
        if item(ui, icons::COMMAND, "Find a command…", "Ctrl+K") {
            self.actions.push(Action::OpenPalette);
        }
        if item(ui, icons::KEYBOARD, "Keyboard shortcuts", "?") {
            self.actions.push(Action::ShowHelp);
        }
        if item(ui, icons::GEAR, "Settings…", "E") {
            self.actions.push(Action::OpenSettings);
        }
    }

    /// Everything the command palette offers right now. Choices that don't
    /// apply (subtitles with no video loaded, clearing an empty playlist) are
    /// left out, and toggles are named for what they will do.
    pub(crate) fn commands(&self) -> Vec<Command> {
        let mut list = Vec::new();
        let Playback { paused, muted, speed, sub_delay, .. } = self.playback;
        if self.loaded {
            list.push(command(if paused { "Play" } else { "Pause" }, "Space", "resume stop", Action::TogglePause));
            if self.current.is_some_and(|i| i + 1 < self.playlist.len()) {
                list.push(command("Next video", "N", "skip", Action::Next));
            }
            if self.current.is_some_and(|i| i > 0) {
                list.push(command("Previous video", "P", "back", Action::Prev));
            }
            let start = command("Back to the start", "Home", "restart beginning rewind", Action::Seek { to: 0.0, exact: true });
            list.push(Command { icon: icons::RESET, ..start });
            if !self.chapters.is_empty() {
                list.push(command("Next chapter", "PgDn", "skip", Action::Chapter { forward: true }));
                list.push(command("Previous chapter", "PgUp", "back", Action::Chapter { forward: false }));
                for chapter in &self.chapters {
                    list.push(command(format!("Chapter: {}", chapter.title), "", "go to jump", Action::Seek { to: chapter.time, exact: true }));
                }
            }
            list.push(command("Next frame", ".", "step", Action::FrameStep { forward: true }));
            list.push(command("Previous frame", ",", "step back", Action::FrameStep { forward: false }));
            list.push(command("Faster", "]", "speed up", Action::SetSpeed(step_speed(speed, true))));
            list.push(command("Slower", "[", "speed down", Action::SetSpeed(step_speed(speed, false))));
            if speed != 1.0 {
                list.push(command("Normal speed", "Backspace", "reset 1×", Action::SetSpeed(1.0)));
            }
            for preset in SPEEDS {
                list.push(command(format!("Speed: {}", format_speed(preset)), "", "playback rate", Action::SetSpeed(preset)));
            }
            list.push(command(if muted { "Unmute" } else { "Mute" }, "M", "sound volume quiet", Action::ToggleMute));
            list.push(command("Next audio track", "A", "language cycle", Action::CycleAudio));
            for t in self.tracks("audio") {
                list.push(command(format!("Audio: {}", t.label), "", "track language", Action::SetTrack { kind: "audio", id: Some(t.id) }));
            }
            let label = if self.store.settings.level_volume { "Stop evening out the volume" } else { "Even out the volume (night mode)" };
            list.push(command(label, "", "level normalize loudness night quiet loud dialogue compress", Action::ToggleLevelVolume));
            list.push(command("Next subtitle track", "S", "captions cycle", Action::CycleSubtitles));
            list.push(command("Subtitles: Off", "", "captions hide", Action::SetTrack { kind: "sub", id: None }));
            for t in self.tracks("sub") {
                list.push(command(format!("Subtitles: {}", t.label), "", "captions track language", Action::SetTrack { kind: "sub", id: Some(t.id) }));
            }
            list.push(command("Load subtitle file…", "", "captions srt ass", Action::LoadSubtitle));
            list.push(command("Show subtitles earlier", "Z", "captions delay sync", Action::AdjustSubDelay(-0.1)));
            list.push(command("Show subtitles later", "X", "captions delay sync", Action::AdjustSubDelay(0.1)));
            if sub_delay != 0.0 {
                list.push(command("Reset the subtitle delay", "", "captions sync", Action::SetSubDelay(0.0)));
            }
            let sync = if self.sync_panel { "Close subtitle sync" } else { "Sync subtitles…" };
            list.push(command(sync, "T", "captions delay offset timing", Action::ToggleSyncPanel));
            let watched = self.current_path().is_some_and(|p| self.store.get(p).watched);
            list.push(command(if watched { "Mark as unwatched" } else { "Mark as watched" }, "W", "seen progress", Action::ToggleWatched));
        } else if self.current.is_none() && !self.playlist.is_empty() {
            list.push(command("Play", "Space", "resume continue start", Action::TogglePause));
        }

        list.push(command("Open folder…", "O", "playlist browse", Action::OpenFolder));
        list.push(command("Add videos…", "I", "files browse", Action::OpenVideos));
        for folder in self.store.recent_folders.iter().filter(|f| self.recent.found(f)) {
            list.push(command(format!("Open recent: {}", display_name(folder)), "", "folder", Action::Open(vec![folder.clone()])));
        }
        if !self.store.recent_folders.is_empty() {
            list.push(command("Clear recent folders", "", "forget history", Action::ClearRecentFolders));
        }
        if self.store.folder.is_some() {
            list.push(command("Refresh the folder", "R", "rescan reload", Action::RefreshFolder));
            let (label, include) = if self.store.include_subfolders {
                ("Stop including subfolders", false)
            } else {
                ("Include subfolders", true)
            };
            list.push(command(label, "", "folders scan recursive", Action::SetIncludeSubfolders(include)));
        }
        let shown = if self.sidebar_docked() { self.show_sidebar } else { self.peek.is_some() };
        list.push(command(if shown { "Hide the playlist" } else { "Show the playlist" }, "B", "sidebar", Action::ToggleSidebar));
        let pin = if self.store.sidebar_pinned { "Unpin the playlist" } else { "Pin the playlist" };
        list.push(command(pin, "", "sidebar dock", Action::TogglePin));
        if !self.playlist.is_empty() {
            list.push(command("Search the playlist", "/", "find filter video", Action::FocusSearch));
            list.push(command("Clear the playlist", "", "empty remove all", Action::ClearPlaylist));
        }
        let autoplay = self.store.autoplay_next;
        let label = if autoplay { "Stop playing the next video automatically" } else { "Play the next video automatically" };
        list.push(command(label, "", "autoplay up next countdown", Action::SetAutoplayNext(!autoplay)));

        list.push(command(if self.fullscreen { "Exit fullscreen" } else { "Fullscreen" }, "F", "full screen window maximize", Action::ToggleFullscreen));
        list.push(command("Settings…", "E", "preferences options", Action::OpenSettings));
        list.push(command("Stats", "", "statistics hours watched time streak history", Action::ShowStats));
        list.push(command("Keyboard shortcuts", "?", "help keys", Action::ShowHelp));
        list.push(command("Reset all progress…", "", "forget clear watched", Action::ConfirmResetProgress));
        // Every video in the playlist, to play by name.
        for (index, path) in self.playlist.iter().enumerate() {
            let entry = self.store.get(path);
            let length = if entry.duration > 0.0 { format_time(entry.duration) } else { String::new() };
            let label = display_name(path);
            list.push(Command { label, key: length.into(), keywords: "play video", action: Action::Play(index), video: true, icon: "" });
        }
        for c in list.iter_mut().filter(|c| c.icon.is_empty()) {
            c.icon = self.command_icon(&c.action);
        }
        list
    }

    /// The icon for a palette entry, matching the menus where they overlap.
    /// Toggles show what they will do, like their labels.
    fn command_icon(&self, action: &Action) -> &'static str {
        match action {
            Action::TogglePause if self.loaded && !self.playback.paused => icons::PAUSE,
            Action::TogglePause => icons::PLAY,
            Action::Next => icons::SKIP_FORWARD,
            Action::Prev => icons::SKIP_BACK,
            Action::Play(_) => icons::FILE_VIDEO,
            Action::Seek { .. } | Action::Chapter { .. } => icons::LIST_NUMBERS,
            Action::FrameStep { .. } => icons::FILM_STRIP,
            Action::SetSpeed(_) => icons::GAUGE,
            Action::ToggleMute if self.playback.muted => icons::SPEAKER_HIGH,
            Action::ToggleMute => icons::SPEAKER_X,
            Action::CycleAudio => icons::WAVEFORM,
            Action::SetTrack { kind: "audio", .. } => icons::WAVEFORM,
            Action::SetTrack { .. } | Action::CycleSubtitles | Action::LoadSubtitle => icons::SUBTITLES,
            Action::AdjustSubDelay(_) | Action::SetSubDelay(_) | Action::ToggleSyncPanel => icons::TIMER,
            Action::ToggleLevelVolume => icons::MOON,
            Action::ConfirmResetProgress => icons::RESET,
            Action::ToggleWatched => icons::CHECK,
            Action::OpenFolder => icons::FOLDER_PLUS,
            Action::OpenVideos => icons::FILE_VIDEO,
            Action::Open(_) => icons::FOLDER,
            Action::ClearRecentFolders => icons::RECENT,
            Action::RefreshFolder => icons::REFRESH,
            Action::SetIncludeSubfolders(_) => icons::FOLDER,
            Action::ToggleSidebar => icons::SIDEBAR,
            Action::TogglePin => icons::PIN,
            Action::FocusSearch => icons::SEARCH,
            Action::ClearPlaylist => icons::TRASH,
            Action::SetAutoplayNext(_) => icons::SKIP_FORWARD,
            Action::ToggleFullscreen if self.fullscreen => icons::CORNERS_IN,
            Action::ToggleFullscreen => icons::CORNERS_OUT,
            Action::OpenSettings => icons::GEAR,
            Action::ShowHelp => icons::KEYBOARD,
            Action::ShowStats => icons::CHART,
            _ => icons::COMMAND,
        }
    }

    /// Ctrl+K: type to find any command, including ones without a key or a
    /// button of their own, then run it with Enter or a click.
    pub(crate) fn command_palette(&mut self, ctx: &egui::Context) {
        let commands = self.commands();
        let Some(palette) = &mut self.palette else { return };
        let query = palette.query.trim().to_lowercase();
        let mut matches: Vec<usize> = (0..commands.len())
            .filter(|&i| !(query.is_empty() && commands[i].video) && command_matches(&query, &commands[i]))
            .collect();
        matches.sort_by_key(|&i| (commands[i].video, command_rank(&query, &commands[i].label)));

        // Taken before the text field sees them, so it keeps focus on Enter.
        let (down, up, enter) = ctx.input_mut(|i| {
            use egui::Modifiers as M;
            (i.consume_key(M::NONE, Key::ArrowDown), i.consume_key(M::NONE, Key::ArrowUp), i.consume_key(M::NONE, Key::Enter))
        });
        let last = matches.len().saturating_sub(1);
        if down {
            palette.pick = (palette.pick + 1).min(last);
        }
        if up {
            palette.pick = palette.pick.saturating_sub(1);
        }
        palette.pick = palette.pick.min(last);
        let mut chosen = if enter { matches.get(palette.pick).copied() } else { None };

        let id = egui::Id::unique("command-palette");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 72.0));
        let modal = egui::Modal::new(id).area(area).show(ctx, |ui| {
            ui.set_width(480.0);
            let resp = ui
                .horizontal(|ui| {
                    ui.label(RichText::new(icons::SEARCH).size(ICON_SIZE).weak());
                    let edit = egui::TextEdit::singleline(&mut palette.query)
                        .margin(TEXT_MARGIN)
                        .hint_text("Type a command or a video's name")
                        .desired_width(f32::INFINITY);
                    ui.add(edit)
                })
                .inner;
            resp.request_focus();
            if resp.changed() {
                palette.pick = 0;
                // The list above was filtered by the previous text.
                ui.ctx().request_repaint();
            }
            ui.add_space(4.0);
            if matches.is_empty() {
                ui.weak("No matching commands");
                return;
            }
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                for (n, &i) in matches.iter().enumerate() {
                    let c = &commands[i];
                    let button = egui::Button::selectable(n == palette.pick, format!("{}  {}", c.icon, c.label))
                        .shortcut_text(ui::secondary(ui, n == palette.pick, c.key.as_ref()))
                        .min_size(vec2(ui.available_width(), PLAYLIST_ROW_HEIGHT));
                    let row = ui.add(button);
                    if row.clicked() {
                        chosen = Some(i);
                    }
                    if n == palette.pick && (down || up) {
                        row.scroll_to_me(None);
                    }
                }
            });
        });
        if let Some(i) = chosen {
            self.palette = None;
            if let Some(c) = commands.into_iter().nth(i) {
                self.actions.push(c.action);
            }
        } else if modal.should_close() {
            self.palette = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_commands_by_any_word() {
        let mute = command("Mute", "M", "sound volume quiet", Action::ToggleMute);
        assert!(command_matches("", &mute));
        assert!(command_matches("mu", &mute));
        assert!(command_matches("vol", &mute), "keywords count");
        assert!(command_matches("quiet sound", &mute), "words in any order");
        assert!(!command_matches("mute subtitles", &mute), "every word must match");

        assert_eq!(command_rank("sub", "Subtitles: English"), 0);
        assert_eq!(command_rank("sub", "Next subtitle track"), 1);
        assert_eq!(command_rank("sub", "Load subtitle file…"), 1);
        assert_eq!(command_rank("delay", "Show subtitles later"), 2, "found by a keyword only");
    }
}
