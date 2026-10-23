//! How each action is shown wherever it is offered (the menus and the
//! command palette), so its name, icon and key are the same everywhere.
//!
//! The rules: names drop "the" ("Clear playlist"); a name ends in "…" when the
//! action opens a dialog or a file picker; each icon means one thing; on/off
//! switches are checkboxes in menus; and an action that can't be done right
//! now is greyed out rather than left out, so menus keep their layout.

use crate::*;

/// An action as menus and the palette show it.
pub(crate) struct Item {
    pub(crate) icon: &'static str,
    pub(crate) label: String,
    /// Its keyboard shortcut, if it has one.
    pub(crate) key: &'static str,
    /// For on/off switches, whether it is on now (menus show a checkbox).
    pub(crate) on: Option<bool>,
    /// Whether it can be done right now.
    pub(crate) enabled: bool,
}

fn item(icon: &'static str, label: impl Into<String>, key: &'static str) -> Item {
    Item { icon, label: label.into(), key, on: None, enabled: true }
}

impl Item {
    fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    fn on(self, on: bool) -> Self {
        Self { on: Some(on), ..self }
    }
}

impl App {
    /// How `action` is shown, given the current state.
    pub(crate) fn item(&self, action: &Action) -> Item {
        let loaded = self.loaded;
        let has_videos = !self.playlist.is_empty();
        let progress = |path: &Path| self.store.get(path);
        let is_current = |path: &Path| self.current_path() == Some(path);
        match action {
            Action::TogglePause if loaded && !self.playback.paused => item(icons::PAUSE, "Pause", "Space"),
            Action::TogglePause => item(icons::PLAY, "Play", "Space").enabled(loaded || (self.current.is_none() && has_videos)),
            Action::Next => item(icons::SKIP_FORWARD, "Next video", "N").enabled(self.current.is_some_and(|i| i + 1 < self.playlist.len())),
            Action::Prev => item(icons::SKIP_BACK, "Previous video", "P").enabled(self.current.is_some_and(|i| i > 0)),
            Action::Seek { to, .. } if *to == 0.0 => item(icons::REWIND, "Rewind to start", "Home").enabled(loaded),
            Action::Seek { to, .. } => item(icons::LIST_NUMBERS, format!("Go to {}", format_time(*to)), "").enabled(loaded),
            Action::Chapter { forward } => {
                let (label, key) = if *forward { ("Next chapter", "PgDn") } else { ("Previous chapter", "PgUp") };
                item(icons::LIST_NUMBERS, label, key).enabled(loaded && !self.chapters.is_empty())
            }
            Action::FrameStep { forward } => {
                let (label, key) = if *forward { ("Next frame", ".") } else { ("Previous frame", ",") };
                item(icons::FILM_STRIP, label, key).enabled(loaded)
            }
            Action::SetSpeed(speed) if *speed == 1.0 => item(icons::GAUGE, "Normal speed", "Backspace").enabled(loaded),
            Action::SetSpeed(speed) => item(icons::GAUGE, format!("Speed {}", format_speed(*speed)), "").enabled(loaded),
            Action::ToggleMute if self.playback.muted => item(icons::SPEAKER_HIGH, "Unmute", "M"),
            Action::ToggleMute => item(icons::SPEAKER_X, "Mute", "M"),
            Action::CycleAudio => item(icons::WAVEFORM, "Next audio track", "A").enabled(loaded),
            Action::SetTrack { kind: "audio", .. } => item(icons::WAVEFORM, "Audio track", "").enabled(loaded),
            Action::SetTrack { .. } => item(icons::SUBTITLES, "Subtitles", "").enabled(loaded),
            Action::CycleSubtitles => item(icons::SUBTITLES, "Next subtitle track", "S").enabled(loaded),
            Action::LoadSubtitle => item(icons::FILE_PLUS, "Load subtitle file…", "").enabled(loaded),
            Action::ToggleLevelVolume => item(icons::MOON, "Even out volume", "").on(self.store.settings_for(self.current_path()).level_volume),
            Action::AdjustSubDelay(by) => {
                let label = if *by < 0.0 { "Show subtitles earlier" } else { "Show subtitles later" };
                let key = match by {
                    b if *b <= -1.0 => "Shift+Z",
                    b if *b < 0.0 => "Z",
                    b if *b >= 1.0 => "Shift+X",
                    _ => "X",
                };
                item(icons::TIMER, label, key).enabled(loaded)
            }
            Action::SetSubDelay(_) => item(icons::RESET, "Reset subtitle delay", "").enabled(loaded && self.playback.sub_delay != 0.0),
            Action::SubStep(lines) => {
                let (icon, label, key) = if *lines < 0 {
                    (icons::ARROW_LEFT, "Show previous line now", "Ctrl+\u{E058}")
                } else {
                    (icons::ARROW_RIGHT, "Show next line now", "Ctrl+\u{E06C}")
                };
                item(icon, label, key).enabled(loaded)
            }
            Action::SetSubSpeed(speed) => {
                let name = SUB_TIMINGS.iter().find(|(s, _)| (s - speed).abs() < 1e-6).map_or("custom", |(_, name)| *name);
                item(icons::TIMER, format!("Subtitle timing: {name}"), "").enabled(loaded)
            }
            Action::ToggleWatched => {
                let watched = self.current_path().is_some_and(|p| progress(p).watched);
                let (icon, label) = if watched { (icons::EYE_SLASH, "Mark as unwatched") } else { (icons::CHECK, "Mark as watched") };
                item(icon, label, "W").enabled(self.current.is_some())
            }
            Action::SetWatched(path, true) => {
                let key = if is_current(path) { "W" } else { "" };
                item(icons::CHECK, "Mark as watched", key).enabled(!progress(path).watched)
            }
            Action::SetWatched(path, false) => {
                let entry = progress(path);
                let key = if is_current(path) && entry.watched { "W" } else { "" };
                item(icons::EYE_SLASH, "Mark as unwatched", key).enabled(entry.watched || entry.position > 0.0)
            }
            Action::SetWatchedMany(_, true) => item(icons::CHECK, "Mark as watched", ""),
            Action::SetWatchedMany(_, false) => item(icons::EYE_SLASH, "Mark as unwatched", ""),
            Action::MarkWatchedBefore(index) => item(icons::CHECKS, "Mark all above as watched", "").enabled(*index > 0),
            Action::PlayOver(_) => item(icons::REPEAT, "Play from start", ""),
            Action::Play(index) => item(icons::FILE_VIDEO, self.playlist.get(*index).map(|p| display_name(p)).unwrap_or_default(), ""),
            Action::Remove(_) => item(icons::X, "Remove from playlist", ""),
            Action::RemoveMany(_) | Action::RemoveSelected => item(icons::X, "Remove from playlist", "Delete").enabled(!self.selected.is_empty() || matches!(action, Action::RemoveMany(_))),
            Action::ClearSelection => item(icons::SELECTION_SLASH, "Clear selection", "Esc").enabled(!self.selected.is_empty()),
            Action::ClearPlaylist => item(icons::TRASH, "Clear playlist", "").enabled(has_videos),
            Action::OpenFolder => item(icons::FOLDER_PLUS, "Open folder…", "O"),
            Action::OpenVideos => item(icons::FILE_VIDEO, "Add videos…", "I"),
            Action::Open(paths) => {
                let folder = paths.first();
                let name = folder.map(|f| display_name(f)).unwrap_or_default();
                item(icons::FOLDER, name, "").enabled(folder.is_some_and(|f| self.recent.found(f)))
            }
            Action::ClearRecentFolders => item(icons::TRASH, "Clear recent folders", "").enabled(!self.store.recent_folders.is_empty()),
            Action::RefreshFolder => item(icons::REFRESH, "Refresh folder", "R").enabled(self.store.folder.is_some()),
            Action::SetIncludeSubfolders(_) => item(icons::FOLDER, "Include subfolders", "").on(self.store.include_subfolders),
            Action::ToggleSidebar => {
                let shown = if self.sidebar_docked() { self.show_sidebar } else { self.peek.is_some() };
                item(icons::SIDEBAR, if shown { "Hide playlist" } else { "Show playlist" }, "B")
            }
            Action::TogglePin => item(icons::PIN, "Pin playlist", "").on(self.store.sidebar_pinned),
            Action::FocusSearch => item(icons::SEARCH, "Search playlist", "/").enabled(has_videos),
            Action::SetAutoplayNext(_) => item(icons::SKIP_FORWARD, "Play next video automatically", "").on(self.autoplay_next()),
            Action::ToggleFullscreen if self.fullscreen => item(icons::CORNERS_IN, "Exit fullscreen", "F"),
            Action::ToggleFullscreen => item(icons::CORNERS_OUT, "Fullscreen", "F"),
            Action::Skip => {
                let label = match self.skip_now() {
                    Some((progress::Skip::Ending, ..)) => "Skip ending",
                    _ => "Skip opening",
                };
                item(icons::FAST_FORWARD, label, "Enter").enabled(self.skip_now().is_some())
            }
            Action::MarkSkip(progress::Skip::Opening) => item(icons::FLAG, "Mark end of opening here", "").enabled(loaded),
            Action::MarkSkip(progress::Skip::Ending) => item(icons::FLAG_CHECKERED, "Mark start of ending here", "").enabled(loaded),
            Action::OpenFolderSettings(video) => {
                let has_folder = video.parent().is_some_and(|f| !f.as_os_str().is_empty());
                item(icons::SLIDERS, "Folder settings…", "").enabled(has_folder)
            }
            Action::UseSubTimingForFolder => item(icons::SLIDERS, "Use this timing for whole folder", "").enabled(loaded),
            Action::OpenPalette =>item(icons::COMMAND, "Find a command…", "Ctrl+K"),
            Action::ShowStats => item(icons::CHART, "Stats…", ""),
            Action::OpenSettings => item(icons::GEAR, "Settings…", "E"),
            Action::ShowHelp => item(icons::KEYBOARD, "Keyboard shortcuts…", "?"),
            Action::ConfirmResetProgress => item(icons::BROOM, "Reset all progress…", ""),
            Action::ConfirmClearStats => {
                let total = self.store.stats.total;
                item(icons::TRASH, "Clear stats…", "").enabled(total.seconds >= 1.0 || total.finished > 0)
            }
            _ => item(icons::COMMAND, "", ""),
        }
    }

    /// Adds `action` to a menu, shown as [`App::item`] describes it; clicking
    /// it carries it out.
    pub(crate) fn menu_item(&mut self, ui: &mut egui::Ui, action: Action) -> egui::Response {
        let mut picked = Vec::new();
        let resp = offer(ui, self, action, &mut picked);
        self.actions.append(&mut picked);
        resp
    }
}

/// Adds `action` to a menu, shown as [`App::item`] describes it, for code
/// that only reads the app: clicking it adds it to `picked`.
pub(crate) fn offer(ui: &mut egui::Ui, app: &App, action: Action, picked: &mut Vec<Action>) -> egui::Response {
    let resp = show_item(ui, &app.item(&action));
    if resp.clicked() {
        picked.push(action);
    }
    resp
}

/// A playlist entry's choices, the same in its playlist menu and (for the
/// playing video) in the video's menu.
pub(crate) fn entry_items(ui: &mut egui::Ui, app: &App, index: usize, path: &Path, picked: &mut Vec<Action>) {
    offer(ui, app, Action::PlayOver(index), picked);
    offer(ui, app, Action::SetWatched(path.to_path_buf(), true), picked);
    offer(ui, app, Action::MarkWatchedBefore(index), picked);
    offer(ui, app, Action::SetWatched(path.to_path_buf(), false), picked);
    ui.separator();
    offer(ui, app, Action::OpenFolderSettings(path.to_path_buf()), picked);
    ui.separator();
    offer(ui, app, Action::Remove(index), picked).on_hover_text("The file is not touched");
}

/// Shows an item in a menu: a checkbox for an on/off switch, otherwise a
/// button with its icon and key, greyed out when it can't be done now.
pub(crate) fn show_item(ui: &mut egui::Ui, item: &Item) -> egui::Response {
    match item.on {
        Some(on) => {
            // Clicking it reports a click; the action does the switching.
            let mut on = on;
            ui.add_enabled(item.enabled, egui::Checkbox::new(&mut on, item.label.as_str()))
        }
        None => {
            let button = egui::Button::new(format!("{}  {}", item.icon, item.label)).shortcut_text(RichText::new(item.key).weak());
            ui.add_enabled(item.enabled, button)
        }
    }
}
