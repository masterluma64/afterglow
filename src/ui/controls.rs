//! The top bar and the playback controls under the video.

use crate::*;

pub(crate) fn transport_button(icon: &str) -> egui::Button<'static> {
    egui::Button::new(icons::fill(icon).size(ICON_SIZE)).frame_when_inactive(false).min_size(vec2(34.0, 30.0))
}

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(icon).size(ICON_SIZE)).frame_when_inactive(false))
        .on_hover_text(tooltip)
}

impl App {
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if icon_button(ui, icons::SIDEBAR, "Show or hide the playlist (B)").clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
            ui.separator();
            if icon_button(ui, icons::FOLDER_PLUS, "Open folder: replace the playlist with its videos (O)").clicked() {
                self.actions.push(Action::OpenFolder);
            }
            if icon_button(ui, icons::FILE_VIDEO, "Add videos to the playlist (I)").clicked() {
                self.actions.push(Action::OpenVideos);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = egui::Button::new(RichText::new(icons::MORE).size(ICON_SIZE)).frame_when_inactive(false);
                let (response, _) = ui::menu_button(more).ui(ui, |ui| {
                    let recent = self.store.recent_folders.clone();
                    // Checked in the background while the menu is open.
                    self.recent.check(&recent, ui.ctx());
                    ui.add_enabled_ui(!recent.is_empty(), |ui| {
                        ui.menu_button(format!("{}  Recent folders", icons::RECENT), |ui| {
                            for folder in &recent {
                                let found = self.recent.found(folder);
                                let tip = if found { folder.display().to_string() } else { format!("{} (not found)", folder.display()) };
                                self.menu_item(ui, Action::Open(vec![folder.clone()])).on_hover_text(tip);
                            }
                            ui.separator();
                            self.menu_item(ui, Action::ClearRecentFolders);
                        });
                    });
                    self.menu_item(ui, Action::RefreshFolder).on_hover_text(
                        "Add new videos from the playlist's folder and drop missing ones. New videos are also picked up by themselves.",
                    );
                    let video = self.current_path().map(Path::to_path_buf).unwrap_or_default();
                    self.menu_item(ui, Action::OpenFolderSettings(video)).on_hover_text("Settings for the playing video's folder");
                    ui.separator();
                    self.app_items(ui);
                    ui.separator();
                    self.menu_item(ui, Action::ConfirmResetProgress);
                });
                response.on_hover_text("More");
                // The title takes the space left between the buttons.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if let Some(path) = self.current_path() {
                        ui.separator();
                        ui.add(egui::Label::new(RichText::new(self.video_name(path)).bold()).truncate());
                    }
                });
            });
        });
    }

    pub(crate) fn controls(&mut self, ui: &mut egui::Ui) {
        let Playback { position, duration, paused, muted, volume, .. } = self.playback;
        let position = self.scrub.unwrap_or(position);
        let has_prev = self.current.is_some_and(|i| i > 0);
        let has_next = self.current.is_some_and(|i| i + 1 < self.playlist.len());
        ui.horizontal(|ui| {
            if ui.add_enabled(has_prev, transport_button(icons::SKIP_BACK)).on_hover_text("Previous video (P)").clicked() {
                self.actions.push(Action::Prev);
            }
            let (icon, tip) = if self.loaded && !paused { (icons::PAUSE, "Pause (Space)") } else { (icons::PLAY, "Play (Space)") };
            if ui.add(transport_button(icon)).on_hover_text(tip).clicked() {
                self.actions.push(Action::TogglePause);
            }
            if ui.add_enabled(has_next, transport_button(icons::SKIP_FORWARD)).on_hover_text("Next video (N)").clicked() {
                self.actions.push(Action::Next);
            }
            ui.add_space(4.0);
            ui.monospace(format_time(if self.loaded { position } else { 0.0 }));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (fs_icon, fs_tip) = if self.fullscreen {
                    (icons::CORNERS_IN, "Exit fullscreen (F or Esc)")
                } else {
                    (icons::CORNERS_OUT, "Fullscreen (F)")
                };
                if icon_button(ui, fs_icon, fs_tip).clicked() {
                    self.actions.push(Action::ToggleFullscreen);
                }
                self.track_menu(ui, "sub");
                self.track_menu(ui, "audio");
                self.speed_menu(ui);

                let mut volume = volume;
                let slider = egui::Slider::new(&mut volume, 0.0..=150.0).show_value(false);
                if ui.add_sized([80.0, 18.0], slider).on_hover_text(format!("Volume {volume:.0}% (Up/Down)")).changed() {
                    self.actions.push(Action::SetVolume(volume));
                }
                let speaker = match () {
                    _ if muted || volume == 0.0 => icons::SPEAKER_X,
                    _ if volume < 50.0 => icons::SPEAKER_LOW,
                    _ => icons::SPEAKER_HIGH,
                };
                if icon_button(ui, speaker, if muted { "Unmute (M)" } else { "Mute (M)" }).clicked() {
                    self.actions.push(Action::ToggleMute);
                }
                ui.monospace(format_time(if self.loaded { duration } else { 0.0 }));
                self.seek_bar(ui, position);
            });
        });
    }

    pub(crate) fn seek_bar(&mut self, ui: &mut egui::Ui, position: f64) {
        let duration = self.playback.duration;
        let size = vec2(ui.available_width().max(40.0), 20.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let active = self.loaded && duration > 0.0;
        let frac_at = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;

        if active {
            // Dragging only moves the preview; the video jumps on release.
            if let Some(pos) = resp.interact_pointer_pos() {
                let to = frac_at(pos.x) * duration;
                if resp.dragged() {
                    self.scrub = Some(to);
                }
                if resp.drag_stopped() || resp.clicked() {
                    self.scrub = None;
                    self.actions.push(Action::Seek { to, exact: true });
                }
            }
            let pointer = if resp.dragged() { resp.interact_pointer_pos() } else { resp.hover_pos() };
            if let Some(pointer) = pointer {
                let at = frac_at(pointer.x) * duration;
                self.seek_preview(ui.ctx(), egui::pos2(pointer.x, rect.top() - 8.0), at, duration);
            }
        }

        let visuals = ui.visuals();
        let track = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
        if active {
            // The opening and ending, as a band around the track.
            for (_, start, end) in self.skips() {
                let x = |t: f64| track.left() + track.width() * (t / duration) as f32;
                let band = egui::Rect::from_x_y_ranges(x(start)..=x(end), track.y_range()).expand2(vec2(0.0, 3.0));
                ui.painter().rect_filled(band, 3.0, visuals.text_color().gamma_multiply(0.2));
            }
        }
        ui.painter().rect_filled(track, 2.0, visuals.widgets.inactive.bg_fill);
        if active {
            let frac = (position / duration).clamp(0.0, 1.0) as f32;
            let filled = egui::Rect::from_min_max(track.min, egui::pos2(track.left() + track.width() * frac, track.bottom()));
            ui.painter().rect_filled(filled, 2.0, visuals.selection.bg_fill);
            // Chapter starts show as small gaps in the track.
            for chapter in self.chapters.iter().filter(|c| c.time > 0.5 && c.time < duration) {
                let x = track.left() + track.width() * (chapter.time / duration) as f32;
                let gap = egui::Rect::from_center_size(egui::pos2(x, track.center().y), vec2(2.0, track.height() + 2.0));
                ui.painter().rect_filled(gap, 0.0, visuals.panel_fill);
            }
            if resp.hovered() || resp.dragged() {
                ui.painter().circle_filled(egui::pos2(filled.right(), track.center().y), 6.0, visuals.selection.bg_fill);
            }
        }
    }

    /// A popup above the seek bar at `anchor` with a preview frame, the time
    /// and the chapter at `at`.
    pub(crate) fn seek_preview(&mut self, ctx: &egui::Context, anchor: egui::Pos2, at: f64, duration: f64) {
        let text = match chapter_at(&self.chapters, at) {
            Some(i) => format!("{}  ·  {}", format_time(at), self.chapters[i].title),
            None => format_time(at),
        };
        let preview = self.preview(at, duration);
        egui::Area::new(egui::Id::unique("seek-preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(anchor)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .constrain(true)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if let Some(texture) = preview {
                            let size = vec2(thumbnails::SIZE[0] as f32, thumbnails::SIZE[1] as f32);
                            ui.image((texture, size / ctx.pixels_per_point().max(1.0)));
                        }
                        ui.label(text);
                    });
                });
            });
    }

    /// A preview frame of the playing video near `at`, once the previewer has
    /// started. The frame may still be from a moment ago.
    pub(crate) fn preview(&mut self, at: f64, duration: f64) -> Option<egui::TextureId> {
        let path = self.current_path()?.to_path_buf();
        let thumbnails = self.thumbnails.as_mut()?.as_mut().ok()?;
        // About 300 distinct previews along the video, at most one per second.
        thumbnails.request(&path, at, (duration / 300.0).max(1.0));
        thumbnails.texture(&path).map(TextureHandle::id)
    }

    pub(crate) fn track_menu(&mut self, ui: &mut egui::Ui, kind: &'static str) {
        let (icon, tip) = if kind == "sub" { (icons::SUBTITLES, "Subtitles (S to cycle)") } else { (icons::WAVEFORM, "Audio track (A to cycle)") };
        ui.add_enabled_ui(self.loaded, |ui| {
            let button = egui::Button::new(RichText::new(icon).size(ICON_SIZE)).frame_when_inactive(false);
            let (response, _) = ui::menu_button(button).ui(ui, |ui| {
                ui.label(RichText::new(if kind == "sub" { "Subtitles" } else { "Audio track" }).bold());
                self.track_items(ui, kind);
            });
            response.on_hover_text(tip);
        });
    }

    /// The choices in the subtitle or audio track menu.
    pub(crate) fn track_items(&mut self, ui: &mut egui::Ui, kind: &'static str) {
        let tracks: Vec<mpv::Track> = self.tracks(kind).cloned().collect();
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
        if tracks.is_empty() && kind != "sub" {
            ui.weak("No audio tracks");
        }
        if kind == "audio" {
            ui.separator();
            self.menu_item(ui, Action::ToggleLevelVolume).on_hover_text("Night mode: quiet dialogue gets louder and loud scenes quieter");
        }
        if kind == "sub" {
            ui.separator();
            self.menu_item(ui, Action::LoadSubtitle);
            ui.separator();
            ui.label(RichText::new("Timing").bold());
            ui.horizontal(|ui| {
                ui.label(format!("Delay {:+.1} s", self.playback.sub_delay));
                // Small buttons, named on hover as the actions are elsewhere.
                for (icon, action) in [(icons::MINUS, Action::AdjustSubDelay(-0.1)), (icons::PLUS, Action::AdjustSubDelay(0.1)), (icons::RESET, Action::SetSubDelay(0.0))] {
                    let item = self.item(&action);
                    let tip = if item.key.is_empty() { item.label.clone() } else { format!("{} ({})", item.label, item.key) };
                    if ui.add_enabled(item.enabled, egui::Button::new(icon).small()).on_hover_text(tip).clicked() {
                        self.actions.push(action);
                    }
                }
            });
            // Syncing by ear: when a line is heard, its subtitle is shown now.
            self.menu_item(ui, Action::SubStep(-1)).on_hover_text("For a line whose subtitle already went by");
            self.menu_item(ui, Action::SubStep(1)).on_hover_text("For a line whose subtitle hasn't shown yet");
            // For subtitles that drift further off as the video goes on.
            let sub_speed = self.playback.sub_speed;
            for (speed, label) in SUB_TIMINGS {
                let text = label[..1].to_uppercase() + &label[1..];
                let selected = (sub_speed - speed).abs() < 1e-6;
                let choice = ui.selectable_label(selected, text).on_hover_text("For subtitles that drift further off over time");
                if choice.clicked() && !selected {
                    self.actions.push(Action::SetSubSpeed(speed));
                }
            }
            self.menu_item(ui, Action::UseSubTimingForFolder)
                .on_hover_text("Videos in this folder start with this delay and timing, unless you set it for one of them");
        }
    }

    pub(crate) fn speed_menu(&mut self, ui: &mut egui::Ui) {
        let speed = self.playback.speed;
        let label = RichText::new(format_speed(speed)).monospace();
        let label = if speed == 1.0 { label.weak() } else { label.strong() };
        let button = egui::Button::new(label).frame_when_inactive(false);
        let (response, _) = ui::menu_button(button).ui(ui, |ui| {
            ui.label(RichText::new("Speed").bold());
            self.speed_items(ui);
        });
        response.on_hover_text("Playback speed ([ and ], Backspace to reset)");
    }

    pub(crate) fn speed_items(&mut self, ui: &mut egui::Ui) {
        let speed = self.playback.speed;
        for preset in SPEEDS {
            if ui.selectable_label((speed - preset).abs() < 0.001, format_speed(preset)).clicked() {
                self.actions.push(Action::SetSpeed(preset));
            }
        }
    }
}
