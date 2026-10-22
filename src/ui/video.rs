//! The video area: the frame, notices over it, and what shows before a video plays.

use crate::*;

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
        let resp = ui.interact(rect, egui::Id::unique("video"), Sense::click());
        if self.loaded {
            self.paint_frame(ui, rect);
            if resp.double_clicked() {
                self.actions.push(Action::ToggleFullscreen);
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
        let galley = ui.painter().layout(
            notice.text.clone(),
            egui::FontId::proportional(16.0),
            color.gamma_multiply(alpha),
            (rect.width() - 64.0).max(100.0),
        );
        let pos = rect.left_top() + vec2(20.0, 18.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(vec2(12.0, 7.0));
        ui.painter().rect_filled(bg, 8.0, Color32::from_black_alpha((170.0 * alpha) as u8));
        ui.painter().galley(pos, galley, Color32::WHITE);
        ui.ctx().request_repaint_after(Duration::from_millis(30).min(notice.until - now));
    }

    pub(crate) fn placeholder(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)));
        // The video area is black in either theme.
        child.style_mut().visuals = video_visuals(self.store.settings.accent);
        child.add_space(rect.height() * 0.36);
        if let Err(e) = &self.mpv {
            child.label(RichText::new(format!("{}  Afterglow can't play videos", icons::WARNING)).size(18.0));
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
            child.label(RichText::new(display_name(&self.playlist[index])).size(20.0).strong());
            child.add_space(4.0);
            child.weak(format!("Starts in {} s", left.as_secs_f32().ceil() as u32));
            child.add_space(6.0);
            // The countdown, filling up until the video starts.
            let gone = 1.0 - left.as_secs_f32() / AUTOPLAY_DELAY.as_secs_f32();
            child.allocate_ui(vec2(220.0, 4.0), |ui| dialogs::meter(ui, gone));
            child.add_space(12.0);
            child.horizontal(|ui| {
                ui.spacing_mut().button_padding = vec2(14.0, 6.0);
                // Centre the two buttons under the text.
                ui.add_space((ui.available_width() - 190.0).max(0.0) / 2.0);
                let play = dialogs::primary_button(ui, format!("{}  Play now", icons::PLAY));
                if ui.add(play).on_hover_text("Space").clicked() {
                    self.actions.push(Action::PlayFromStart(index));
                }
                if ui.button("Cancel").on_hover_text("Esc").clicked() {
                    self.actions.push(Action::CancelUpNext);
                }
            });
            child.ctx().request_repaint_after(Duration::from_millis(200).min(left));
            return;
        }
        if let Some(index) = self.continue_target() {
            let path = &self.playlist[index];
            let resume = self.store.resume_position(path);
            let verb = if resume.is_some() { "Continue" } else { "Play" };
            let text = RichText::new(format!("{}   {verb}: {}", icons::PLAY, display_name(path))).size(18.0);
            child.spacing_mut().button_padding = vec2(18.0, 9.0);
            if child.add(dialogs::primary_button(&child, text)).on_hover_text("Space").clicked() {
                self.actions.push(Action::Play(index));
            }
            if let Some(pos) = resume {
                child.add_space(4.0);
                child.weak(format!("from {}", format_time(pos)));
            }
            return;
        }

        let (title, hint) = if self.playlist.is_empty() {
            ("Drop a folder or videos here", "A folder plays all of its videos in order.")
        } else {
            ("Everything in the playlist is watched", "Pick a video to watch it again, or open something new.")
        };
        child.label(RichText::new(icons::FOLDER_PLUS).size(44.0).weak());
        child.add_space(6.0);
        child.label(RichText::new(title).size(18.0));
        child.weak(hint);
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
            egui::FontId::proportional(20.0),
            Color32::WHITE,
        );
    }
}
