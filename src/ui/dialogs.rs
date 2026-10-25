//! Settings, stats, the shortcut overview and confirmations.

use crate::*;

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

/// Text color for errors: softer than egui's pure red, which glares on a
/// dark background.
pub(crate) fn error_color(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(235, 115, 105) } else { Color32::from_rgb(185, 50, 40) }
}

/// Fill behind white text on buttons that destroy something.
pub(crate) const DANGER_FILL: Color32 = Color32::from_rgb(178, 62, 54);

impl App {
    /// Applies the settings to the UI and to mpv.
    pub(crate) fn apply_settings(&self, ctx: &egui::Context) {
        let settings = &self.store.settings;
        ctx.set_theme(match settings.theme {
            progress::Theme::System => egui::ThemePreference::System,
            progress::Theme::Dark => egui::ThemePreference::Dark,
            progress::Theme::Light => egui::ThemePreference::Light,
        });
        let accent = settings.accent;
        ctx.all_styles_mut(|style| {
            style.visuals.selection = accent_selection(accent, style.visuals.dark_mode);
            style.visuals.error_fg_color = error_color(style.visuals.dark_mode);
        });
        if let Ok(mpv) = &self.mpv {
            // With the playing video's folder settings in their place.
            let playing = self.store.settings_for(self.current_path());
            mpv.set("slang", &playing.sub_languages);
            mpv.set("alang", &playing.audio_languages);
            mpv.set("hwdec", if settings.hardware_decoding { "auto-safe" } else { "no" });
            mpv.set("af", if playing.level_volume { mpv::VOLUME_LEVELER } else { "" });
        }
    }

    pub(crate) fn settings_dialog(&mut self, ctx: &egui::Context) {
        let mut settings = self.store.settings.clone();
        let mut autoplay = self.store.autoplay_next;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(470.0);
            ui.label(RichText::new("Settings").bold());
            ui.add_space(10.0);
            // Labels as wide in every card, so the controls line up.
            let grid = |id: &str| egui::Grid::new(id).num_columns(2).spacing([16.0, 10.0]).min_col_width(150.0);
            let max_height = (ui.ctx().content_rect().height() - 180.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                card(ui, "Playback", |ui| {
                    grid("settings-playback").show(ui, |ui| {
                        ui.label("Seek step");
                        ui.add(egui::DragValue::new(&mut settings.seek_short).range(1.0..=120.0).suffix(" s").speed(0.2));
                        ui.end_row();
                        ui.label("Seek step with Shift");
                        ui.add(egui::DragValue::new(&mut settings.seek_long).range(1.0..=600.0).suffix(" s").speed(1.0));
                        ui.end_row();
                        ui.label("Count as watched at");
                        ui.add(egui::Slider::new(&mut settings.watched_percent, 50.0..=100.0).suffix("%").integer());
                        ui.end_row();
                        ui.label("Auto-play");
                        ui.checkbox(&mut autoplay, "Play next video automatically");
                        ui.end_row();
                        ui.label("Volume");
                        ui.checkbox(&mut settings.level_volume, "Even out volume")
                            .on_hover_text("Quiet dialogue gets louder and loud scenes quieter");
                        ui.end_row();
                        ui.label("Dropped videos")
                            .on_hover_text("What dropping videos onto the window does (dropped folders always replace the playlist)");
                        ui.horizontal(|ui| {
                            use progress::DroppedVideos as D;
                            for (choice, label) in [(None, "Ask"), (Some(D::Add), "Add"), (Some(D::Replace), "Replace playlist")] {
                                ui.selectable_value(&mut settings.dropped_videos, choice, label);
                            }
                        });
                        ui.end_row();
                    });
                });
                ui.add_space(8.0);
                card(ui, "Languages", |ui| {
                    grid("settings-languages").show(ui, |ui| {
                        ui.label("Subtitles");
                        ui::text_field(ui, egui::TextEdit::singleline(&mut settings.sub_languages).hint_text("the file's default").desired_width(200.0), egui::Id::unique("sub-languages"), TEXT_MARGIN)
                            .on_hover_text("Applies to the next video");
                        ui.end_row();
                        ui.label("Audio");
                        ui::text_field(ui, egui::TextEdit::singleline(&mut settings.audio_languages).hint_text("the file's default").desired_width(200.0), egui::Id::unique("audio-languages"), TEXT_MARGIN)
                            .on_hover_text("Applies to the next video");
                        ui.end_row();
                    });
                    ui.weak(LANGUAGES_HINT);
                });
                ui.add_space(8.0);
                card(ui, "Video and appearance", |ui| {
                    grid("settings-appearance").show(ui, |ui| {
                        ui.label("Hardware decoding");
                        ui.checkbox(&mut settings.hardware_decoding, "Decode video on the GPU")
                            .on_hover_text("Turn off if videos show glitches; uses more CPU");
                        ui.end_row();
                        ui.label("Screenshots");
                        // (copy, save); one of the two stays on, so taking a screenshot always does something.
                        const SCREENSHOT_MODES: [((bool, bool), &str); 3] = [
                            ((true, true), "Copy to clipboard and save"),
                            ((true, false), "Copy to clipboard only"),
                            ((false, true), "Save only"),
                        ];
                        let mut mode = (settings.copy_screenshots, settings.save_screenshots);
                        let name = SCREENSHOT_MODES.iter().find(|(m, _)| *m == mode).map_or("Copy to clipboard and save", |(_, name)| name);
                        egui::ComboBox::from_id_salt("screenshot-mode").selected_text(name).show_ui(ui, |ui| {
                            for (m, name) in SCREENSHOT_MODES {
                                ui.selectable_value(&mut mode, m, name);
                            }
                        })
                        .response
                        .on_hover_text("Screenshots (C or F12) are saved in Pictures\\Afterglow");
                        (settings.copy_screenshots, settings.save_screenshots) = mode;
                        ui.end_row();
                        ui.label("Seek bar");
                        ui.checkbox(&mut settings.show_remaining, "Show time remaining")
                            .on_hover_text("At the end of the seek bar, instead of the video's length");
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
                });
                ui.add_space(8.0);
                card(ui, "Data", |ui| {
                    grid("settings-data").show(ui, |ui| {
                        let folder = self.store.folder();
                        let fixed = progress::data_dir_fixed();
                        let portable = folder.is_some() && folder == progress::data_dir_for(true);
                        ui.label("Save progress");
                        ui.horizontal(|ui| {
                            for (choice, label) in [(true, "Next to afterglow.exe"), (false, "In AppData")] {
                                let option = ui.add_enabled(!fixed, egui::Button::selectable(!fixed && portable == choice, label));
                                if option.on_disabled_hover_text("Chosen with AFTERGLOW_DATA_DIR").clicked() && portable != choice {
                                    self.actions.push(Action::SetPortable(choice));
                                }
                            }
                        });
                        ui.end_row();
                        if let Some(folder) = folder {
                            ui.label("");
                            let open = ui.add_enabled(folder.is_dir(), egui::Button::new("Open folder"));
                            if open.on_hover_text(folder.display().to_string()).clicked() {
                                let _ = std::process::Command::new("explorer").arg(&folder).spawn();
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.add_space(8.0);
                // Changes Windows rather than the settings, so it takes effect right away.
                card(ui, "Windows", |ui| {
                    grid("settings-windows").show(ui, |ui| {
                        for place in platform::shortcut::Place::ALL {
                            ui.label(place.name());
                            ui.horizontal(|ui| {
                                let current = &self.shortcuts[place as usize];
                                let label = if current.is_some() { "Remove shortcut" } else { "Add shortcut" };
                                if ui.button(label).clicked() {
                                    self.actions.push(Action::SetShortcut(place, current.is_none()));
                                }
                                // Another copy of Afterglow (or this one, moved or renamed) made it.
                                if let Some(exe) = current
                                    && !platform::is_this_exe(exe)
                                    && ui.button("Use this copy").on_hover_text(format!("It opens {}", exe.display())).clicked()
                                {
                                    self.actions.push(Action::SetShortcut(place, true));
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Restore defaults").clicked() {
                    settings = progress::Settings::default();
                    autoplay = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(primary_button(ui, "Done")).clicked() {
                        close = true;
                    }
                    ui.add_space(8.0);
                    ui.weak(concat!("Afterglow ", env!("CARGO_PKG_VERSION"), " (", env!("AFTERGLOW_COMMIT"), ")"));
                });
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
            ui.set_width(420.0);
            ui.label(RichText::new("Dropped videos").bold());
            ui.add_space(6.0);
            ui.label(format!("What should happen with {videos}?"));
            ui.add_space(10.0);
            let gap = 8.0;
            let width = (ui.available_width() - gap) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                use progress::DroppedVideos as D;
                let them = if count == 1 { "it" } else { "them" };
                let add = format!("Put {them} at the end of the playlist.");
                let replace = format!("Start a new playlist with {them}.");
                for (choice, icon, title, detail) in [(D::Add, icons::PLUS, "Add", add), (D::Replace, icons::FILE_VIDEO, "Replace", replace)] {
                    if choice_card(ui, width, icon, title, &detail).clicked() {
                        self.actions.push(Action::ChooseDropBehavior { behavior: choice, remember: self.drop_remember });
                    }
                }
            });
            ui.add_space(10.0);
            ui.checkbox(&mut self.drop_remember, "Save as default")
                .on_hover_text("Do this every time videos are dropped, without asking. You can change it in Settings.");
        });
        // Esc or a click outside cancels the drop without remembering anything.
        if modal.should_close() {
            self.pending_drop = None;
        }
    }

    /// An overview of the keyboard shortcuts.
    pub(crate) fn help_dialog(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("shortcuts")).show(ctx, |ui| {
            ui.set_width(720.0);
            ui.label(RichText::new("Keyboard shortcuts").bold());
            ui.add_space(10.0);
            let max_height = (ui.ctx().content_rect().height() - 140.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                ui.columns(2, |columns| {
                    for (column, groups) in columns.iter_mut().zip(SHORTCUTS.chunks(SHORTCUTS.len().div_ceil(2))) {
                        for (title, keys) in groups {
                            card(column, title, |ui| {
                                egui::Grid::new(title).num_columns(2).spacing([12.0, 3.0]).min_col_width(110.0).show(ui, |ui| {
                                    for (key, action) in *keys {
                                        keycaps(ui, key);
                                        ui.label(*action);
                                        ui.end_row();
                                    }
                                });
                            });
                            column.add_space(8.0);
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Press ? or Esc to close.").weak());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| close = ui.button("Close").clicked());
            });
        });
        if close || modal.should_close() {
            self.show_help = false;
        }
    }

    /// Confirmation before wiping every video's progress.
    pub(crate) fn reset_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("reset-progress")).show(ctx, |ui| {
            ui.set_width(400.0);
            let title = format!("{}  Reset all progress?", icons::WARNING);
            ui.label(RichText::new(title).bold().color(error_color(ui.visuals().dark_mode)));
            ui.add_space(8.0);
            ui.label("This forgets the saved position and watched state of every video.");
            ui.add_space(4.0);
            ui.label(RichText::new("Your playlist, settings and stats stay. This can't be undone.").weak());
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let reset = egui::Button::new(RichText::new("Reset all progress").color(Color32::WHITE)).fill(DANGER_FILL);
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

impl App {
    /// Watching stats: totals, streaks, the last 30 days, the most watched
    /// series and the movies watched last.
    pub(crate) fn stats_dialog(&mut self, ctx: &egui::Context) {
        use crate::ui::sidebar::format_total;
        let s = self.store.stats.summary(clock::today());
        let days = |n: u32| format!("{n} day{}", if n == 1 { "" } else { "s" });
        let videos = |n: u32| format!("{n} video{}", if n == 1 { "" } else { "s" });
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("stats")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Stats").bold());
            ui.add_space(10.0);

            let gap = 8.0;
            let tile_width = (ui.available_width() - 3.0 * gap) / 4.0;
            let best = s.best_day.map(|(day, seconds)| (format_total(seconds), stats::format_date(day)));
            let movie_time = (s.movie_seconds >= 1.0).then(|| format!("Movies: {}", format_total(s.movie_seconds)));
            let tiles = [
                ("Total watched", format_total(s.total.seconds), movie_time),
                ("Videos finished", s.videos_finished.to_string(), None),
                ("Movies watched", s.movies_watched.to_string(), None),
                ("Best day", best.as_ref().map_or("–".into(), |b| b.0.clone()), best.map(|b| b.1)),
                ("Current streak", days(s.current_streak), None),
                ("Longest streak", days(s.longest_streak), None),
                ("Last 7 days", format_total(s.last_7_days), None),
                ("Last 30 days", format_total(s.last_30_days), Some(format!("{} a day", format_total(s.last_30_days / 30.0)))),
            ];
            for row in tiles.chunks(4) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (label, value, detail) in row {
                        stat_tile(ui, tile_width, label, value, detail.as_deref());
                    }
                });
                ui.add_space(gap - ui.spacing().item_spacing.y);
            }

            ui.add_space(12.0);
            daily_chart(ui, &s.daily);

            if let Some((_, top)) = s.top_series.first() {
                ui.add_space(14.0);
                ui.label(RichText::new("Most watched").bold());
                ui.add_space(2.0);
                for (name, tally) in &s.top_series {
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), ui.spacing().interact_size.y), Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(videos(tally.finished)).weak());
                        ui.label(RichText::new(format_total(tally.seconds)).bold());
                        // The name takes the room left, cut off with … when long.
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(name.as_str()).truncate()).on_hover_text(name.as_str());
                        });
                    });
                    let share = (tally.seconds / top.seconds.max(1.0)) as f32;
                    meter(ui, share);
                    ui.add_space(4.0);
                }
            }

            if !s.recent_movies.is_empty() {
                ui.add_space(14.0);
                ui.label(RichText::new("Recent movies").bold());
                ui.add_space(2.0);
                for (name, movie) in &s.recent_movies {
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), ui.spacing().interact_size.y), Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(stats::format_date(movie.last_day)).weak());
                        ui.label(RichText::new(format_total(movie.seconds)).bold());
                        if movie.finished > 0 {
                            ui.label(RichText::new(icons::CHECK).weak()).on_hover_text("Watched");
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(name.as_str()).truncate()).on_hover_text(name.as_str());
                        });
                    });
                }
            }

            ui.add_space(10.0);
            if s.before.seconds >= 60.0 {
                let before = format!(
                    "Includes about {} ({}) watched before stats were kept, estimated from saved progress.",
                    format_total(s.before.seconds),
                    videos(s.before.finished)
                );
                ui.label(RichText::new(before).weak());
                ui.add_space(6.0);
            }
            ui.allocate_ui_with_layout(vec2(ui.available_width(), ui.spacing().interact_size.y), Layout::right_to_left(Align::Center), |ui| {
                close = ui.button("Close").clicked();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| self.menu_item(ui, Action::ConfirmClearStats));
            });
        });
        // While the confirmation to clear them is open on top, clicks and Esc
        // are meant for that.
        if close || (modal.should_close() && !self.confirm_clear_stats) {
            self.stats_open = false;
        }
    }

    /// Confirmation before clearing all stats.
    pub(crate) fn clear_stats_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("clear-stats")).show(ctx, |ui| {
            ui.set_width(400.0);
            let title = format!("{}  Clear all stats?", icons::WARNING);
            ui.label(RichText::new(title).bold().color(error_color(ui.visuals().dark_mode)));
            ui.add_space(8.0);
            ui.label("This forgets all time watched, videos finished, streaks, series and movies, and starts counting again from nothing.");
            ui.add_space(4.0);
            ui.label(RichText::new("Your videos' progress stays. This can't be undone.").weak());
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let clear = egui::Button::new(RichText::new("Clear stats").color(Color32::WHITE)).fill(DANGER_FILL);
                if ui.add(clear).clicked() {
                    self.actions.push(Action::ClearStats);
                    self.confirm_clear_stats = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_clear_stats = false;
                }
            });
        });
        // Esc or a click outside cancels.
        if modal.should_close() {
            self.confirm_clear_stats = false;
        }
    }
}

/// A faint background for the stat tiles, a step away from the dialog's own.
fn tile_fill(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { Color32::from_white_alpha(10) } else { Color32::from_black_alpha(10) }
}

/// A tile with a label, a big value and an optional detail line below it.
fn stat_tile(ui: &mut egui::Ui, width: f32, label: &str, value: &str, detail: Option<&str>) {
    egui::Frame::new().fill(tile_fill(ui)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.set_width(width - 24.0);
            // All tiles as tall as one with a detail line.
            ui.set_min_height(58.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(label).weak());
            ui.label(RichText::new(value).bold());
            if let Some(detail) = detail {
                ui.label(RichText::new(detail).weak());
            }
        });
    });
}

/// A thin bar filled to `share` (0 to 1) in the accent color.
pub(crate) fn meter(ui: &mut egui::Ui, share: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, tile_fill(ui));
    let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * share.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(filled, 2.0, ui.visuals().selection.bg_fill);
}

/// Bars for the time watched on each day, oldest first, in a tile with the
/// busiest day's time as its scale; hovering a bar shows its day and time.
fn daily_chart(ui: &mut egui::Ui, daily: &[(stats::Day, f64)]) {
    use crate::ui::sidebar::format_total;
    let most = daily.iter().map(|(_, s)| *s).fold(0.0, f64::max);
    egui::Frame::new().fill(tile_fill(ui)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Last 30 days").bold());
            if most >= 1.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("Most in a day: {}", format_total(most))).weak());
                });
            }
        });
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let slot = rect.width() / daily.len().max(1) as f32;
        let hovered = resp.hover_pos().map(|p| (((p.x - rect.left()) / slot) as usize).min(daily.len().saturating_sub(1)));
        let visuals = ui.visuals();
        for (i, (_, seconds)) in daily.iter().enumerate() {
            let x = rect.left() + slot * i as f32;
            if *seconds < 1.0 {
                // Days with nothing get a dot, so all 30 days stay visible.
                let c = egui::pos2(x + slot / 2.0, rect.bottom() - 1.5);
                ui.painter().circle_filled(c, 1.5, visuals.widgets.inactive.bg_fill);
                continue;
            }
            let height = ((*seconds / most.max(1.0)) as f32 * rect.height()).max(4.0);
            let bar = egui::Rect::from_min_max(egui::pos2(x + 1.5, rect.bottom() - height), egui::pos2(x + slot - 1.5, rect.bottom()));
            let color = if hovered == Some(i) { visuals.selection.bg_fill.gamma_multiply(1.3) } else { visuals.selection.bg_fill };
            ui.painter().rect_filled(bar, 2.0, color);
        }
        if let Some((day, seconds)) = hovered.and_then(|i| daily.get(i)) {
            resp.on_hover_text_at_pointer(format!("{}: {}", stats::format_date(*day), format_total(*seconds)));
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            // "Sep 23", without the year.
            let first = daily.first().map(|(d, _)| stats::format_date(*d)).unwrap_or_default();
            ui.label(RichText::new(first.split(',').next().unwrap_or_default()).weak());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new("Today").weak()));
        });
    });
}

/// A full-width card with a small title, for grouping a dialog's parts.
pub(crate) fn card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    egui::Frame::new().fill(tile_fill(ui)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(width - 24.0);
        ui.label(RichText::new(title).bold());
        ui.add_space(4.0);
        add_contents(ui);
    });
}

/// What the language fields expect, under them.
pub(crate) const LANGUAGES_HINT: &str =
    "Language codes in order of preference, separated by commas, like en,eng for English or ja,jpn for Japanese. Left empty, the file's default is used.";

/// The main button of a dialog or screen, in the accent color.
pub(crate) fn primary_button(ui: &egui::Ui, text: impl Into<RichText>) -> egui::Button<'static> {
    let selection = ui.visuals().selection;
    egui::Button::new(text.into().color(selection.stroke.color)).fill(selection.bg_fill)
}

/// A key as a keycap; keys given as "A / B" are drawn as two.
fn keycaps(ui: &mut egui::Ui, keys: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, key) in keys.split(" / ").enumerate() {
            if i > 0 {
                ui.weak("/");
            }
            let visuals = ui.visuals();
            egui::Frame::new()
                .fill(visuals.widgets.inactive.weak_bg_fill)
                .stroke((1.0, visuals.widgets.noninteractive.bg_stroke.color))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(6, 1))
                .show(ui, |ui| ui.label(RichText::new(key).monospace()));
        }
    });
}

/// A clickable card for one of a few choices: an icon, a title and a line
/// saying what it does, outlined in the accent color on hover.
fn choice_card(ui: &mut egui::Ui, width: f32, icon: &str, title: &str, detail: &str) -> egui::Response {
    let inner = egui::Frame::new().fill(tile_fill(ui)).corner_radius(8).inner_margin(12).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.set_width(width - 24.0);
            ui.label(RichText::new(icon).size(22.0));
            ui.add_space(4.0);
            ui.label(RichText::new(title).bold());
            ui.label(RichText::new(detail).weak());
        });
    });
    let resp = ui.interact(inner.response.rect, ui.scope_id().with(title), Sense::click());
    if resp.hovered() {
        ui.painter().rect_stroke(inner.response.rect, 8.0, (1.5, ui.visuals().selection.bg_fill), egui::StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
