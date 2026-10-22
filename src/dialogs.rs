//! Settings, the shortcut overview, confirmations and the subtitle sync panel.

use super::*;

/// A clickable color square, outlined when selected.
pub(crate) fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool, name: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    ui.painter().rect_filled(rect.shrink(3.0), 4.0, color);
    if selected || resp.hovered() {
        let outline = if selected { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
        ui.painter().rect_stroke(rect, 5.0, (2.0, outline), egui::StrokeKind::Inside);
    }
    resp.on_hover_text(name)
}

/// Selection colors for an accent: its fill, with text that stays readable on it.
pub(crate) fn accent_selection(accent: Option<[u8; 3]>, dark: bool) -> egui::style::Selection {
    let default = if dark { egui::Visuals::dark() } else { egui::Visuals::light() }.selection;
    let Some([r, g, b]) = accent else { return default };
    let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    let text = if luminance > 150.0 { Color32::BLACK } else { Color32::WHITE };
    egui::style::Selection { bg_fill: Color32::from_rgb(r, g, b), stroke: egui::Stroke::new(1.0, text) }
}

impl App {
    /// Tools for subtitles that are out of sync, in a panel over the video.
    pub(crate) fn sync_panel(&mut self, ctx: &egui::Context, video: egui::Rect) {
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
    pub(crate) fn apply_settings(&self, ctx: &egui::Context) {
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

    pub(crate) fn settings_dialog(&mut self, ctx: &egui::Context) {
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
    pub(crate) fn drop_dialog(&mut self, ctx: &egui::Context) {
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
    pub(crate) fn help_dialog(&mut self, ctx: &egui::Context) {
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
    pub(crate) fn reset_dialog(&mut self, ctx: &egui::Context) {
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
}
