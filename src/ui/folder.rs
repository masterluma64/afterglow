//! Folder settings: settings that differ from the global ones for a
//! folder's videos. A video opened by itself has the same settings for just
//! that video.

use crate::*;
use progress::FolderSettings;

impl App {
    /// The settings of the folder of the video in `folder_dialog`, or of the
    /// video itself when it was opened by itself.
    pub(crate) fn folder_dialog(&mut self, ctx: &egui::Context) {
        let Some(video) = self.folder_dialog.clone() else { return };
        let Some(own) = video.parent().map(Path::to_path_buf) else {
            self.folder_dialog = None;
            return;
        };
        let saved = self.store.folder_settings(&video).map(|(_, f)| f.clone()).unwrap_or_default();
        let mut folder = saved.clone();
        let movie = self.store.is_movie(&video);
        let global = self.store.settings.clone();
        let global_autoplay = self.store.autoplay_next;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("folder-settings")).show(ctx, |ui| {
            ui.set_width(540.0);
            ui.label(RichText::new(if movie { "Video settings" } else { "Folder settings" }).bold());
            let parent = own.parent().filter(|p| p.file_name().is_some() && !movie);
            let subject = match parent {
                _ if movie => self.video_name(&video),
                Some(parent) if folder.include_subfolders => format!("{} and the folders in it", folder_name(parent)),
                _ => folder_name(&own),
            };
            let place = if movie { video.display() } else { own.display() };
            ui.label(RichText::new(subject).weak()).on_hover_text(place.to_string());
            if let Some(parent) = parent {
                ui.checkbox(&mut folder.include_subfolders, format!("Also use for other folders in {}", folder_name(parent)))
                    .on_hover_text(format!(
                        "For a show with a folder per season: the settings are kept for {} and used by every folder directly in it that has no settings of its own.",
                        folder_name(parent)
                    ));
            }
            ui.add_space(10.0);
            let grid = |id: &str| egui::Grid::new(id).num_columns(2).spacing([16.0, 10.0]).min_col_width(150.0);
            let max_height = (ui.ctx().content_rect().height() - 220.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                card(ui, "Skipping", |ui| {
                    ui.weak("While an opening or ending plays, a Skip button shows in the bottom-right corner of the video (Enter skips).");
                    ui.weak("They're found from chapters named like Intro, Opening, Credits or Ending.");
                    if global.listen_for_openings {
                        let heard = match self.store.heard_opening(&video) {
                            Some(Some((start, end))) => format!(" In this video, it heard one from {} to {}.", format_time(start), format_time(end)),
                            Some(None) => " It heard none in this video.".to_string(),
                            None => String::new(),
                        };
                        ui.weak(format!("Without those, Afterglow listens for the theme song episodes share.{heard}"));
                    }
                });
                ui.add_space(8.0);
                card(ui, "Subtitles and audio", |ui| {
                    grid("folder-languages").show(ui, |ui| {
                        ui.label("Subtitle languages");
                        custom_field(ui, &mut folder.sub_languages, global.sub_languages.clone(), |v| if v.is_empty() { "the file's default".into() } else { v.clone() }, |ui, v| {
                            ui::text_field(ui, egui::TextEdit::singleline(v).hint_text("e.g. en,eng").desired_width(140.0), egui::Id::unique("folder-sub-languages"), TEXT_MARGIN);
                        });
                        ui.end_row();
                        ui.label("Audio languages");
                        custom_field(ui, &mut folder.audio_languages, global.audio_languages.clone(), |v| if v.is_empty() { "the file's default".into() } else { v.clone() }, |ui, v| {
                            ui::text_field(ui, egui::TextEdit::singleline(v).hint_text("e.g. ja,jpn").desired_width(140.0), egui::Id::unique("folder-audio-languages"), TEXT_MARGIN);
                        });
                        ui.end_row();
                        ui.label("Subtitle delay").on_hover_text("Where each video starts, until you change it for that video");
                        custom_field(ui, &mut folder.sub_delay, 0.0, |v| format!("{v:+.1} s"), |ui, v| {
                            ui.add(egui::DragValue::new(v).range(-600.0..=600.0).speed(0.05).fixed_decimals(1).suffix(" s"));
                        });
                        ui.end_row();
                        ui.label("Audio delay").on_hover_text("Where each video starts, until you change it for that video. More plays the sound later.");
                        custom_field(ui, &mut folder.audio_delay, 0.0, |v| format!("{v:+.1} s"), |ui, v| {
                            ui.add(egui::DragValue::new(v).range(-10.0..=10.0).speed(0.01).fixed_decimals(1).suffix(" s"));
                        });
                        ui.end_row();
                        ui.label("Subtitle timing").on_hover_text("For subtitles that drift further off over time");
                        custom_field(ui, &mut folder.sub_speed, 1.0, |v| sub_timing_name(*v), |ui, v| {
                            egui::ComboBox::from_id_salt("folder-sub-timing").selected_text(sub_timing_name(*v)).show_ui(ui, |ui| {
                                for (speed, name) in SUB_TIMINGS {
                                    ui.selectable_value(v, speed, name);
                                }
                            });
                        });
                        ui.end_row();
                    });
                    ui.weak(LANGUAGES_HINT);
                });
                ui.add_space(8.0);
                card(ui, "Playback", |ui| {
                    grid("folder-playback").show(ui, |ui| {
                        ui.label("Count as watched at");
                        custom_field(ui, &mut folder.watched_percent, global.watched_percent, |v| format!("{v:.0}%"), |ui, v| {
                            ui.add(egui::Slider::new(v, 50.0..=100.0).suffix("%").integer());
                        });
                        ui.end_row();
                        ui.label("Auto-play");
                        switch_field(ui, &mut folder.autoplay_next, global_autoplay);
                        ui.end_row();
                        ui.label("Even out volume");
                        switch_field(ui, &mut folder.level_volume, global.level_volume);
                        ui.end_row();
                        ui.label("Speed");
                        custom_field(ui, &mut folder.speed, 1.0, |v| format_speed(*v), |ui, v| {
                            egui::ComboBox::from_id_salt("folder-speed").selected_text(format_speed(*v)).show_ui(ui, |ui| {
                                for preset in SPEEDS {
                                    ui.selectable_value(v, preset, format_speed(preset));
                                }
                            });
                        });
                        ui.end_row();
                    });
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(if movie { "Reset video" } else { "Reset folder" }).on_hover_text("Follow the global settings again").clicked() {
                    folder = FolderSettings::default();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if gradient::button(ui, "Done").clicked() {
                        close = true;
                    }
                });
            });
        });
        if folder != saved {
            self.actions.push(Action::SetFolderSettings(video, folder));
        }
        if close || modal.should_close() {
            self.folder_dialog = None;
            self.store.save();
        }
    }
}

fn folder_name(folder: &Path) -> String {
    folder.file_name().map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn sub_timing_name(speed: f64) -> String {
    let name = SUB_TIMINGS.iter().find(|(s, _)| (s - speed).abs() < 1e-6).map_or("custom", |(_, name)| *name);
    name[..1].to_uppercase() + &name[1..]
}

/// A setting that follows the global one ("Default", which shows it) until
/// it is made "Custom" for the folder, shown with `edit`.
fn custom_field<T: Clone>(
    ui: &mut egui::Ui,
    value: &mut Option<T>,
    global: T,
    describe: impl Fn(&T) -> String,
    edit: impl FnOnce(&mut egui::Ui, &mut T),
) {
    ui.horizontal(|ui| {
        let mut custom = value.is_some();
        ui.selectable_value(&mut custom, false, "Default");
        ui.selectable_value(&mut custom, true, "Custom");
        if !custom {
            *value = None;
        } else if value.is_none() {
            *value = Some(global.clone());
        }
        ui.add_space(4.0);
        match value {
            Some(v) => edit(ui, v),
            None => {
                ui.weak(describe(&global));
            }
        }
    });
}

/// An on/off setting that follows the global one until it is set for the folder.
fn switch_field(ui: &mut egui::Ui, value: &mut Option<bool>, global: bool) {
    ui.horizontal(|ui| {
        ui.selectable_value(value, None, format!("Default ({})", if global { "on" } else { "off" }));
        ui.selectable_value(value, Some(true), "On");
        ui.selectable_value(value, Some(false), "Off");
    });
}
