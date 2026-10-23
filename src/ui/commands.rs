//! The right-click menu on the video and the command palette (Ctrl+K). Both
//! show actions as `App::item` describes them (see `items.rs`).

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
    /// Its key, "On" / "Off" for a switch, or a video's length, shown beside it.
    pub(crate) key: Cow<'static, str>,
    /// Other words it can be found by.
    pub(crate) keywords: &'static str,
    pub(crate) action: Action,
    /// Videos are only listed once something is typed, after the commands.
    pub(crate) video: bool,
    pub(crate) icon: &'static str,
    /// Commands that can't be done right now are left out of the palette.
    pub(crate) enabled: bool,
}

impl Command {
    fn key(self, key: &'static str) -> Self {
        Self { key: key.into(), ..self }
    }
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
    /// gathered in one place (also in fullscreen, where the controls hide).
    /// Its groups come in the same order as in the other menus: playback;
    /// tracks, speed and chapters; the playing video; opening videos; view;
    /// and the app.
    pub(crate) fn video_menu(&mut self, ui: &mut egui::Ui) {
        self.menu_item(ui, Action::TogglePause);
        self.menu_item(ui, Action::Next);
        self.menu_item(ui, Action::Prev);
        ui.separator();
        let loaded = self.loaded;
        ui.add_enabled_ui(loaded, |ui| {
            ui.menu_button(format!("{}  Audio track", icons::WAVEFORM), |ui| self.track_items(ui, "audio"));
            ui.menu_button(format!("{}  Subtitles", icons::SUBTITLES), |ui| self.track_items(ui, "sub"));
            ui.menu_button(format!("{}  Speed  {}", icons::GAUGE, format_speed(self.playback.speed)), |ui| self.speed_items(ui));
        });
        ui.add_enabled_ui(loaded && !self.chapters.is_empty(), |ui| {
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
        });
        ui.separator();
        // The playing video's choices, as in its playlist menu.
        let current = self.current.zip(self.current_path().map(Path::to_path_buf));
        let mut picked = Vec::new();
        match &current {
            Some((index, path)) => items::entry_items(ui, self, *index, path, &mut picked),
            None => ui.add_enabled_ui(false, |ui| items::entry_items(ui, self, 0, Path::new(""), &mut Vec::new())).inner,
        }
        self.actions.append(&mut picked);
        ui.separator();
        self.menu_item(ui, Action::OpenFolder);
        self.menu_item(ui, Action::OpenVideos);
        ui.separator();
        self.menu_item(ui, Action::ToggleFullscreen);
        self.menu_item(ui, Action::ToggleSidebar);
        ui.separator();
        self.app_items(ui);
    }

    /// The last group of the video menu and the ⋯ menu.
    pub(crate) fn app_items(&mut self, ui: &mut egui::Ui) {
        self.menu_item(ui, Action::OpenPalette);
        self.menu_item(ui, Action::ShowStats);
        self.menu_item(ui, Action::OpenSettings);
        self.menu_item(ui, Action::ShowHelp);
    }

    /// A palette entry for `action`, named and drawn as everywhere else.
    fn command(&self, action: Action, keywords: &'static str) -> Command {
        let item = self.item(&action);
        let key = match item.on {
            Some(on) => if on { "On" } else { "Off" }.into(),
            None => item.key.into(),
        };
        Command { label: item.label, key, keywords, action, video: false, icon: item.icon, enabled: item.enabled }
    }

    /// A palette entry for one of several choices of an action (a track, a
    /// chapter, a speed), which names it itself.
    fn command_named(&self, label: impl Into<String>, action: Action, keywords: &'static str) -> Command {
        Command { label: label.into(), ..self.command(action, keywords) }
    }

    /// Everything the command palette offers right now, in the menus' order.
    /// Commands that can't be done right now are left out, and switches show
    /// whether they are on.
    pub(crate) fn commands(&self) -> Vec<Command> {
        let speed = self.playback.speed;
        let mut list = vec![
            self.command(Action::TogglePause, "resume stop continue start"),
            self.command(Action::Next, "skip"),
            self.command(Action::Prev, "back"),
            self.command(Action::Seek { to: 0.0, exact: true }, "restart beginning"),
            self.command(Action::Chapter { forward: true }, "skip"),
            self.command(Action::Chapter { forward: false }, "back"),
        ];
        for chapter in &self.chapters {
            list.push(self.command_named(format!("Chapter: {}", chapter.title), Action::Seek { to: chapter.time, exact: true }, "go to jump"));
        }
        list.extend([
            self.command(Action::FrameStep { forward: true }, "step"),
            self.command(Action::FrameStep { forward: false }, "step back"),
            self.command_named("Faster", Action::SetSpeed(step_speed(speed, true)), "speed up").key("]"),
            self.command_named("Slower", Action::SetSpeed(step_speed(speed, false)), "speed down").key("["),
        ]);
        if speed != 1.0 {
            list.push(self.command(Action::SetSpeed(1.0), "reset 1×"));
        }
        for preset in SPEEDS {
            list.push(self.command_named(format!("Speed: {}", format_speed(preset)), Action::SetSpeed(preset), "playback rate"));
        }
        list.extend([self.command(Action::ToggleMute, "sound volume quiet"), self.command(Action::CycleAudio, "language cycle")]);
        for t in self.tracks("audio") {
            list.push(self.command_named(format!("Audio: {}", t.label), Action::SetTrack { kind: "audio", id: Some(t.id) }, "track language"));
        }
        list.extend([
            self.command(Action::ToggleLevelVolume, "level normalize loudness night mode quiet loud dialogue compress"),
            self.command(Action::CycleSubtitles, "captions cycle"),
            self.command_named("Subtitles: Off", Action::SetTrack { kind: "sub", id: None }, "captions hide"),
        ]);
        for t in self.tracks("sub") {
            list.push(self.command_named(format!("Subtitles: {}", t.label), Action::SetTrack { kind: "sub", id: Some(t.id) }, "captions track language"));
        }
        list.extend([
            self.command(Action::LoadSubtitle, "captions srt ass"),
            self.command(Action::AdjustSubDelay(-0.1), "captions delay sync"),
            self.command(Action::AdjustSubDelay(0.1), "captions delay sync"),
            self.command(Action::SetSubDelay(0.0), "captions delay sync"),
            self.command(Action::SubStep(-1), "captions sync subtitle step heard"),
            self.command(Action::SubStep(1), "captions sync subtitle step heard"),
        ]);
        for (speed, _) in SUB_TIMINGS {
            list.push(self.command(Action::SetSubSpeed(speed), "captions sync drift frame rate fps speed"));
        }
        // The playing video, as in its playlist menu.
        if let Some(index) = self.current {
            list.extend([
                self.command(Action::PlayOver(index), "restart again beginning unwatched"),
                self.command(Action::ToggleWatched, "seen progress"),
                self.command(Action::MarkWatchedBefore(index), "seen progress previous earlier"),
            ]);
        }
        list.extend([self.command(Action::OpenFolder, "playlist browse"), self.command(Action::OpenVideos, "files browse")]);
        for folder in &self.store.recent_folders {
            list.push(self.command_named(format!("Open recent: {}", display_name(folder)), Action::Open(vec![folder.clone()]), "folder"));
        }
        list.extend([
            self.command(Action::ClearRecentFolders, "forget history"),
            self.command(Action::RefreshFolder, "rescan reload"),
            self.command(Action::SetIncludeSubfolders(!self.store.include_subfolders), "folders scan recursive"),
            self.command(Action::FocusSearch, "find filter video"),
            self.command(Action::RemoveSelected, "selection delete"),
            self.command(Action::ClearSelection, "deselect"),
            self.command(Action::ClearPlaylist, "empty remove all"),
            self.command(Action::SetAutoplayNext(!self.store.autoplay_next), "autoplay up next countdown"),
            self.command(Action::ToggleFullscreen, "full screen window maximize"),
            self.command(Action::ToggleSidebar, "sidebar"),
            self.command(Action::TogglePin, "sidebar dock"),
            self.command(Action::ShowStats, "statistics hours watched time streak history"),
            self.command(Action::OpenSettings, "preferences options"),
            self.command(Action::ShowHelp, "help keys"),
            self.command(Action::ConfirmResetProgress, "forget clear watched"),
        ]);
        list.retain(|c| c.enabled);
        // Every video in the playlist, to play by name.
        for (index, path) in self.playlist.iter().enumerate() {
            let entry = self.store.get(path);
            let length = if entry.duration > 0.0 { format_time(entry.duration) } else { String::new() };
            list.push(Command { key: length.into(), video: true, ..self.command(Action::Play(index), "play video") });
        }
        list
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
            // Taller than other text fields: it is what the palette is for.
            let edit = egui::TextEdit::singleline(&mut palette.query)
                .hint_text("Type a command or a video's name")
                .desired_width(f32::INFINITY);
            let resp = ui::text_field(ui, edit, egui::Id::unique("palette-query"), egui::Margin::symmetric(10, 9));
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
        let mute = Command {
            label: "Mute".into(),
            key: "M".into(),
            keywords: "sound volume quiet",
            action: Action::ToggleMute,
            video: false,
            icon: icons::SPEAKER_X,
            enabled: true,
        };
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
