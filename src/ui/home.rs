//! The home screen: what there is to continue watching, as cards with a
//! still of the video that is next. It shows while nothing plays, and over
//! the video from the house button.

use crate::*;

/// How long the cards are kept before they are worked out again.
const REFRESH: Duration = Duration::from_secs(2);
const CARD_WIDTH: f32 = 220.0;
const CARD_GAP: f32 = 16.0;

/// Something to continue.
#[derive(Clone)]
pub(crate) enum Target {
    /// The video to play next in the playlist as it is.
    Playlist(usize),
    /// A folder opened before, and the video to go on with in it.
    Folder { folder: PathBuf, video: PathBuf },
    /// A video opened by itself, like a movie, and not finished.
    Movie(PathBuf),
}

/// A card on the home screen.
#[derive(Clone)]
pub(crate) struct Card {
    target: Target,
    /// The show or movie.
    title: String,
    /// What plays: the next video, and how much of it is left.
    detail: String,
    /// How many videos are watched, of how many.
    count: Option<(usize, usize)>,
    /// The video whose still is shown.
    video: PathBuf,
    /// How far into that video playback is, 0 to 1.
    fraction: f32,
}

impl App {
    /// The cards: the playlist first, then the recent folders and the
    /// unfinished videos opened by themselves, most recent first. Folders and
    /// videos that are all watched are left out.
    pub(crate) fn home_cards(&mut self) -> Vec<Card> {
        if let Some((at, cards)) = &self.home_cards
            && at.elapsed() < REFRESH
        {
            return cards.clone();
        }
        let mut cards = Vec::new();
        let playlist_folder = self.store.folder.clone();
        if let Some(index) = self.continue_target() {
            let video = self.playlist[index].clone();
            let title = match &playlist_folder {
                Some(folder) => display_name(folder),
                None if self.single_video() => self.video_name(&video),
                None => "Playlist".into(),
            };
            let watched = self.playlist.iter().filter(|p| self.store.get(p).watched).count();
            let count = (!self.single_video()).then_some((watched, self.playlist.len()));
            cards.push(self.card(Target::Playlist(index), title, video, count));
        }
        for folder in self.store.recent_folders.clone() {
            if Some(&folder) == playlist_folder.as_ref() || !self.recent.found(&folder) {
                continue;
            }
            if let Some((video, watched, total)) = self.store.continue_in(&folder) {
                let target = Target::Folder { folder: folder.clone(), video: video.clone() };
                cards.push(self.card(target, display_name(&folder), video, Some((watched, total))));
            }
        }
        for movie in self.store.recent_videos.clone() {
            let playing = self.single_video() && self.playlist.first() == Some(&movie);
            if playing || !self.recent.found(&movie) || self.store.resume_position(&movie).is_none() {
                continue;
            }
            let title = self.store.title(&movie).map_or_else(|| display_name(&movie), str::to_string);
            cards.push(self.card(Target::Movie(movie.clone()), title, movie, None));
        }
        self.home_cards = Some((Instant::now(), cards.clone()));
        cards
    }

    fn card(&self, target: Target, title: String, video: PathBuf, count: Option<(usize, usize)>) -> Card {
        let entry = self.store.get(&video);
        let resume = self.store.resume_position(&video);
        let left = match resume {
            Some(position) if entry.duration > 0.0 => format!("{} left", ui::sidebar::format_total(entry.duration - position)),
            _ if entry.duration > 0.0 => format_time(entry.duration),
            _ => String::new(),
        };
        let name = if matches!(target, Target::Movie(_)) || count.is_none() { String::new() } else { self.store.title(&video).map_or_else(|| display_name(&video), str::to_string) };
        let detail = match (name.is_empty(), left.is_empty()) {
            (true, _) => left,
            (false, true) => name,
            (false, false) => format!("{name}  ·  {left}"),
        };
        let fraction = if resume.is_some() { entry.fraction() } else { 0.0 };
        Card { target, title, detail, count, video, fraction }
    }

    /// The action a card carries out.
    fn card_action(target: &Target) -> Action {
        match target {
            Target::Playlist(index) => Action::Play(*index),
            Target::Folder { folder, video } => Action::ContinueFolder { folder: folder.clone(), video: video.clone() },
            Target::Movie(movie) => Action::OpenSingle(movie.clone()),
        }
    }

    /// The home screen in `rect`: the cards, centered, and under them what
    /// else can be opened. With no cards, an invitation to open something.
    pub(crate) fn home_screen(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let cards = self.home_cards();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(24.0)).layout(Layout::top_down(Align::Center)));
        let ui = &mut child;
        ui.style_mut().visuals = video_visuals(self.store.settings.accent);
        if cards.is_empty() {
            ui.add_space(rect.height() * 0.36 - 24.0);
            let (title, hint) = if self.playlist.is_empty() {
                ("Drop a folder or videos here", "A folder plays all of its videos in order.")
            } else {
                ("Everything in the playlist is watched", "Pick a video to watch it again, or open something new.")
            };
            ui.label(RichText::new(icons::FOLDER_PLUS).size(44.0).weak());
            ui.add_space(6.0);
            ui.label(RichText::new(title).bold());
            ui.weak(hint);
            if self.home {
                ui.add_space(12.0);
                self.home_buttons(ui);
            }
            return;
        }
        let width = ui.available_width();
        let columns = (((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).clamp(1, cards.len());
        let grid_width = columns as f32 * (CARD_WIDTH + CARD_GAP) - CARD_GAP;
        let card_height = card_height(ui);
        let rows = cards.len().div_ceil(columns);
        let content = rows as f32 * (card_height + CARD_GAP) + 80.0;
        // Centered in the height too, when it fits.
        ui.add_space(((ui.available_height() - content) / 2.0).max(0.0));
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            // Over the first card.
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - grid_width) / 2.0).max(0.0));
                ui.label(RichText::new("Continue watching").bold());
            });
            ui.add_space(10.0);
            for row in cards.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - grid_width) / 2.0).max(0.0));
                    ui.spacing_mut().item_spacing.x = CARD_GAP;
                    for card in row {
                        let still = self.stills.get(&card.video);
                        let resp = card_ui(ui, card, still);
                        let resp = resp.on_hover_text(match &card.target {
                            Target::Folder { folder, .. } => folder.display().to_string(),
                            _ => card.video.display().to_string(),
                        });
                        if resp.clicked() {
                            self.home = false;
                            // The paused video it is about plays on.
                            let action = match card.target {
                                Target::Playlist(index) if self.loaded && self.current == Some(index) => Action::SetPaused(false),
                                _ => Self::card_action(&card.target),
                            };
                            self.actions.push(action);
                        }
                        if let Target::Folder { folder, .. } | Target::Movie(folder) = &card.target {
                            let folder = folder.clone();
                            ui::context_menu(&resp, |ui| {
                                if ui.button(format!("{}  Remove from Continue watching", icons::X)).clicked() {
                                    self.actions.push(Action::ForgetRecent(folder));
                                }
                            });
                        }
                    }
                });
                ui.add_space(CARD_GAP);
            }
            self.home_buttons(ui);
        });
    }

    /// What else can be opened, under the cards.
    fn home_buttons(&mut self, ui: &mut egui::Ui) {
        // Centered by the buttons' width on the last frame.
        let id = egui::Id::unique("home-buttons");
        let width = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
            let start = ui.cursor().left();
            for action in [Action::OpenFolder, Action::OpenVideo] {
                let item = self.item(&action);
                let button = egui::Button::new(format!("{}  {}", item.icon, item.label)).corner_radius(8.0);
                if ui.add(button).on_hover_text(item.key).clicked() {
                    self.home = false;
                    self.actions.push(action);
                }
            }
            let measured = ui.min_rect().right() - start;
            if (measured - width).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(id, measured));
                ui.ctx().request_repaint();
            }
        });
    }
}

/// A card's height: the still, then three lines of text.
fn card_height(ui: &egui::Ui) -> f32 {
    let line = ui.text_style_height(&egui::TextStyle::Body);
    CARD_WIDTH * 9.0 / 16.0 + 8.0 + 3.0 * (line + 2.0)
}

/// A card: the still (`None` until it is taken) with the progress along its
/// bottom, then the title, what plays and how much of it is watched.
fn card_ui(ui: &mut egui::Ui, card: &Card, still: Option<egui::TextureId>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(CARD_WIDTH, card_height(ui)), Sense::click());
    let visuals = ui.visuals();
    let accent = visuals.selection.bg_fill;
    let frame = egui::Rect::from_min_size(rect.min, vec2(CARD_WIDTH, CARD_WIDTH * 9.0 / 16.0));
    let painter = ui.painter();
    match still {
        Some(id) => egui::Image::new((id, frame.size())).corner_radius(8).paint_at(ui, frame),
        None => {
            painter.rect_filled(frame, 8.0, Color32::from_gray(28));
        }
    }
    if card.fraction > 0.0 {
        let bar = egui::Rect::from_min_max(egui::pos2(frame.left(), frame.bottom() - 4.0), frame.max);
        let round = egui::CornerRadius { sw: 8, se: 8, ..Default::default() };
        painter.rect_filled(bar, round, Color32::from_black_alpha(170));
        let filled = egui::Rect::from_min_size(bar.min, vec2(bar.width() * card.fraction, bar.height()));
        let round = egui::CornerRadius { sw: 8, ..Default::default() };
        gradient::fill(painter, filled, round, bar, gradient::accent(visuals), gradient::Direction::Across);
    }
    if resp.hovered() {
        painter.rect_stroke(frame, 8.0, (2.0, accent), egui::StrokeKind::Outside);
        painter.circle_filled(frame.center(), 22.0, Color32::from_black_alpha(170));
        let font = egui::FontId::new(20.0, egui::FontFamily::Name(icons::FILL_FAMILY.into()));
        painter.text(frame.center() + vec2(1.0, 0.0), egui::Align2::CENTER_CENTER, icons::PLAY, font, Color32::WHITE);
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let bold = egui::FontId::new(font.size, egui::FontFamily::Name(icons::BOLD_FAMILY.into()));
    let mut y = frame.bottom() + 8.0;
    let mut line = |text: String, font: egui::FontId, color: Color32| {
        if text.is_empty() {
            return;
        }
        let galley = ui::one_line(ui, text, font, color, CARD_WIDTH);
        let height = galley.size().y;
        ui.painter().galley(egui::pos2(rect.left(), y), galley, color);
        y += height + 2.0;
    };
    line(card.title.clone(), bold, visuals.strong_text_color());
    line(card.detail.clone(), font.clone(), visuals.text_color());
    if let Some((watched, total)) = card.count {
        line(format!("{watched} of {total} watched"), font, visuals.weak_text_color());
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
