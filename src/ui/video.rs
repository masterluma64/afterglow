//! The video area: the frame, notices over it, and what shows before a video plays.

use crate::*;

/// How long the Skip button stays up while the controls are hidden.
const SKIP_BUTTON_TIME: f64 = 8.0;

/// The size of the Skip button, and of the buttons over the video like it.
const SKIP_BUTTON_PADDING: egui::Vec2 = vec2(26.0, 15.0);

/// The dark look of everything drawn over the video, whatever the theme.
pub(crate) fn video_visuals(accent: Option<[u8; 3]>) -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.selection = accent_selection(accent, true);
    visuals.error_fg_color = error_color(true);
    visuals
}

impl App {
    pub(crate) fn video_area(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        // The borderless mini player is moved by dragging the video.
        let sense = if self.mini() { Sense::click_and_drag() } else { Sense::click() };
        let resp = ui.interact(rect, egui::Id::unique("video"), sense);
        if resp.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if self.loaded && self.home {
            // The home screen over the paused video, which shows dimly through.
            self.paint_frame(ui, rect);
            ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(225));
            self.home_screen(ui, rect);
        } else if self.loaded {
            self.paint_frame(ui, rect);
            self.skip_button(ui, rect);
            if resp.double_clicked() {
                self.actions.push(if self.mini() { Action::ToggleMiniPlayer } else { Action::ToggleFullscreen });
            }
            if resp.middle_clicked() {
                self.actions.push(Action::TogglePause);
            }
            // Scrolling changes the volume, 5% per wheel notch.
            if resp.hovered() {
                self.scroll_volume += ui.input(|i| {
                    let wheel = |e: &egui::Event| match e {
                        egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => delta.y,
                        egui::Event::MouseWheel { delta, .. } => delta.y * SCROLL_NOTCH,
                        _ => 0.0,
                    };
                    i.raw.events.iter().map(wheel).sum::<f32>()
                });
                let notches = (self.scroll_volume / SCROLL_NOTCH).trunc();
                if notches != 0.0 {
                    self.scroll_volume -= notches * SCROLL_NOTCH;
                    let volume = (self.playback.volume + 5.0 * f64::from(notches)).clamp(0.0, 150.0);
                    self.actions.push(Action::SetVolume(volume));
                }
            }
        } else {
            self.placeholder(ui, rect);
        }
        if self.mpv.is_ok() {
            ui::context_menu(&resp, |ui| self.video_menu(ui));
        }
        self.paint_notice(ui, rect);
    }

    /// Paints the video into `rect`; mpv fits the video into it itself.
    pub(crate) fn paint_frame(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if let Ok(mpv) = &self.mpv
            && let Some(callback) = mpv.paint_callback(rect)
        {
            ui.painter().add(callback);
        }
    }

    /// The Skip button, in the bottom-right corner of the video while an
    /// opening or ending plays. While the controls are hidden, it only shows
    /// for the first seconds of it.
    fn skip_button(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Some((skip, start, _)) = self.skip_now() else { return };
        let idle = self.controls_idle();
        if idle && self.playback.position - start > SKIP_BUTTON_TIME {
            return;
        }
        // Above the controls, where they float over the video.
        let bottom = match self.controls_rect {
            Some(controls) if self.controls_overlay() => controls.top(),
            _ => rect.bottom(),
        };
        let label = if skip == progress::Skip::Opening { "Skip opening" } else { "Skip ending" };
        let accent = self.store.settings.accent;
        egui::Area::new(egui::Id::unique("skip"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::pos2(rect.right() - 24.0, bottom - 24.0))
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .show(ui.ctx(), |ui| {
                ui.style_mut().visuals = video_visuals(accent);
                ui.spacing_mut().button_padding = SKIP_BUTTON_PADDING;
                // Near the window's edge the area is narrow; keep it on one line.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                if ui.add(skip_button(ui, format!("{}  {label}", icons::FAST_FORWARD))).on_hover_text("Enter").clicked() {
                    self.actions.push(Action::Skip);
                }
            });
    }

    /// What the white button over the video does while nothing plays: play
    /// up next, play the next video or open something new, or continue.
    pub(crate) fn main_button(&self) -> Option<Action> {
        if self.mpv.is_err() || self.current.is_some() {
            return None;
        }
        if let Some(up_next) = &self.up_next {
            return Some(Action::PlayFromStart(up_next.index));
        }
        if let Some(index) = self.end_screen_index() {
            return Some(if index + 1 < self.playlist.len() {
                Action::PlayFromStart(index + 1)
            } else if self.single_video() {
                Action::OpenVideo
            } else {
                Action::OpenFolder
            });
        }
        self.continue_target().map(Action::Play)
    }

    /// The video the end screen is for, if it shows.
    fn end_screen_index(&self) -> Option<usize> {
        // A watched video opened by itself (restored on startup, say) has
        // nothing in the hidden playlist to pick, so it gets the end screen too.
        let watched_single = (self.single_video() && self.continue_target().is_none()).then_some(0);
        self.ended.filter(|&i| i < self.playlist.len()).or(watched_single)
    }

    /// The current notice, as a pill in the top-left corner of the video.
    pub(crate) fn paint_notice(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let Some(notice) = &self.notice else { return };
        let now = Instant::now();
        if now >= notice.until {
            self.notice = None;
            return;
        }
        // Fade out over the last 300 ms.
        let left = (notice.until - now).as_secs_f32();
        let alpha = (left / 0.3).min(1.0);
        let color = if notice.warning { Color32::from_rgb(255, 196, 92) } else { Color32::WHITE };
        pill(ui, rect, notice.text.clone(), color, alpha);
        ui.ctx().request_repaint_after(Duration::from_millis(30).min(notice.until - now));
    }

    pub(crate) fn placeholder(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)));
        // The video area is black in either theme.
        child.style_mut().visuals = video_visuals(self.store.settings.accent);
        child.add_space(rect.height() * 0.36);
        if let Err(e) = &self.mpv {
            child.label(RichText::new(format!("{}  Afterglow can't play videos", icons::WARNING)).bold());
            child.add_space(6.0);
            child.colored_label(child.visuals().error_fg_color, e);
            child.add_space(6.0);
            child.label(format!(
                "It needs {} from an mpv build (the \"mpv-dev-x86_64\" archive), placed next to afterglow.exe.",
                mpv::DLL_NAME
            ));
            child.add_space(6.0);
            child.hyperlink_to("Download libmpv", mpv::DOWNLOAD_URL);
            if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
                && child.button("Open Afterglow's folder").clicked()
            {
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
            child.weak("Then restart Afterglow.");
            return;
        }
        // Opening a video: just black until it shows.
        if self.current.is_some() {
            return;
        }
        if let Some(up_next) = &self.up_next {
            let (index, left) = (up_next.index, up_next.at.saturating_duration_since(Instant::now()));
            child.label(RichText::new("Up next").weak());
            child.add_space(4.0);
            child.label(RichText::new(self.video_name(&self.playlist[index])).bold());
            child.add_space(4.0);
            child.weak(format!("Starts in {} s", left.as_secs_f32().ceil() as u32));
            child.add_space(6.0);
            // The countdown, filling up until the video starts.
            let gone = 1.0 - left.as_secs_f32() / AUTOPLAY_DELAY.as_secs_f32();
            child.allocate_ui(vec2(220.0, 4.0), |ui| dialogs::meter(ui, gone));
            child.add_space(12.0);
            // Centred under the text by the buttons' width on the last frame.
            let id = egui::Id::unique("up-next-buttons");
            let width = child.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
            child.horizontal(|ui| {
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                let start = ui.cursor().left();
                ui.spacing_mut().button_padding = SKIP_BUTTON_PADDING;
                let play = skip_button(ui, format!("{}  Play now", icons::PLAY));
                if ui.add(play).on_hover_text("Enter").clicked() {
                    self.actions.push(Action::PlayFromStart(index));
                }
                if ui.add(egui::Button::new("Cancel").corner_radius(8.0)).on_hover_text("Esc").clicked() {
                    self.actions.push(Action::CancelUpNext);
                }
                let measured = ui.min_rect().right() - start;
                if (measured - width).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(id, measured));
                    ui.ctx().request_repaint();
                }
            });
            child.ctx().request_repaint_after(Duration::from_millis(200).min(left));
            return;
        }
        if let Some(index) = self.end_screen_index() {
            self.end_screen(&mut child, index);
            return;
        }
        self.home_screen(ui, rect);
    }

    /// What shows once a video played to its end and nothing starts after it:
    /// the next video to play, or for the last one (or a video opened by
    /// itself), something new to open; and watching it again.
    fn end_screen(&mut self, ui: &mut egui::Ui, index: usize) {
        let next = (index + 1 < self.playlist.len()).then_some(index + 1);
        ui.label(RichText::new(format!("{}  Watched", icons::CHECK)).weak());
        ui.add_space(4.0);
        ui.label(RichText::new(self.video_name(&self.playlist[index])).bold());
        ui.add_space(4.0);
        match next {
            Some(next) => ui.weak(format!("Next: {}", self.video_name(&self.playlist[next]))),
            None if self.single_video() => ui.weak("Open another video, or watch it again."),
            None => ui.weak("That was the last video in the playlist."),
        };
        ui.add_space(12.0);
        // Centred under the text by the buttons' width on the last frame.
        let id = egui::Id::unique("end-screen-buttons");
        let width = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
            let start = ui.cursor().left();
            ui.spacing_mut().button_padding = SKIP_BUTTON_PADDING;
            if let Some(next) = next {
                let play = skip_button(ui, format!("{}  Play next", icons::PLAY));
                if ui.add(play).on_hover_text("Enter").clicked() {
                    self.actions.push(Action::PlayFromStart(next));
                }
            } else {
                let open = if self.single_video() { Action::OpenVideo } else { Action::OpenFolder };
                let item = self.item(&open);
                let button = skip_button(ui, format!("{}  {}", item.icon, item.label));
                if ui.add(button).on_hover_text(format!("Enter or {}", item.key)).clicked() {
                    self.actions.push(open);
                }
            }
            let again = self.item(&Action::PlayOver(index));
            if ui.add(egui::Button::new(format!("{}  {}", again.icon, again.label)).corner_radius(8.0)).clicked() {
                self.actions.push(Action::PlayOver(index));
            }
            let measured = ui.min_rect().right() - start;
            if (measured - width).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(id, measured));
                ui.ctx().request_repaint();
            }
        });
    }

    pub(crate) fn drop_overlay(&self, ctx: &egui::Context, rect: egui::Rect) {
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
            "Drop a folder to play it, or videos to add them to the playlist",
            egui::TextStyle::Body.resolve(&ctx.global_style()),
            Color32::WHITE,
        );
    }
}

/// The button Enter presses: white, so it stands out on any video, edged in
/// the accent color; bold (the text keeps the one font size) to be noticed.
fn skip_button(ui: &egui::Ui, text: String) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(Color32::from_gray(20)).bold())
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(3.0, ui.visuals().selection.bg_fill))
        .corner_radius(8.0)
}

/// Text in a dark pill in the top-left corner of the video `rect`, faded by
/// `alpha`.
fn pill(ui: &egui::Ui, rect: egui::Rect, text: String, color: Color32, alpha: f32) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout(text, font, color.gamma_multiply(alpha), (rect.width() - 64.0).max(100.0));
    let pos = egui::pos2(rect.left() + 20.0, rect.top() + 18.0);
    let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(vec2(12.0, 7.0));
    ui.painter().rect_filled(bg, 8.0, Color32::from_black_alpha((170.0 * alpha) as u8));
    ui.painter().galley(pos, galley, Color32::WHITE);
}
