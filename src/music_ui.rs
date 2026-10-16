//! The music view: album grid, album pages, songs table and now playing.

use std::path::PathBuf;

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, TextureHandle, vec2};
use egui_extras::{Column, TableBuilder};

use crate::music::{Album, MusicLibrary, Track, Visualizer};
use crate::{Action, App, Item, Mode, display_name, format_time, icons};

const CARD_W: f32 = 168.0;
const COVER: f32 = 152.0;
const CARD_H: f32 = COVER + 48.0;
const GAP: f32 = 14.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MusicView {
    #[default]
    Albums,
    Songs,
    NowPlaying,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Artist,
    Title,
    Year,
    Added,
}

impl AlbumSort {
    const ALL: [AlbumSort; 4] = [Self::Artist, Self::Title, Self::Year, Self::Added];

    fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Title => "Album",
            Self::Year => "Year",
            Self::Added => "Recently added",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SongColumn {
    Title,
    #[default]
    Artist,
    Album,
    Year,
    Duration,
}

#[derive(Default)]
pub struct MusicUi {
    pub view: MusicView,
    pub album_sort: AlbumSort,
    pub song_sort: SongColumn,
    pub song_desc: bool,
    pub search: String,
    pub artist: Option<String>,
    /// Key of the album whose page is open.
    pub open_album: Option<String>,
}

struct Filter {
    search: String,
    artist: Option<String>,
}

impl Filter {
    fn new(ui: &MusicUi) -> Self {
        Self { search: ui.search.trim().to_lowercase(), artist: ui.artist.clone() }
    }

    fn text(&self, s: &str) -> bool {
        self.search.is_empty() || s.to_lowercase().contains(&self.search)
    }

    fn track(&self, t: &Track) -> bool {
        self.artist.as_ref().is_none_or(|a| t.artist == *a || t.album_artist == *a)
            && (self.text(&t.title) || self.text(&t.artist) || self.text(&t.album))
    }

    fn album(&self, lib: &MusicLibrary, album: &Album) -> bool {
        let tracks = || album.tracks.iter().map(|&t| &lib.tracks[t]);
        self.artist.as_ref().is_none_or(|a| album.artist == *a || tracks().any(|t| t.artist == *a))
            && (self.text(&album.title) || self.text(&album.artist) || tracks().any(|t| self.text(&t.title)))
    }
}

impl App {
    fn current_track(&self) -> Option<(PathBuf, Option<&Track>)> {
        match &self.current {
            Some(Item::Track(path)) => Some((path.clone(), self.music.track_index(path).map(|i| &self.music.tracks[i]))),
            _ => None,
        }
    }

    /// View switcher and sorting, shown in the top bar in music mode.
    pub(crate) fn music_toolbar(&mut self, ui: &mut egui::Ui) {
        let views = [
            (MusicView::Albums, icons::GRID, "Albums"),
            (MusicView::Songs, icons::LIST, "Songs"),
            (MusicView::NowPlaying, icons::WAVEFORM, "Now playing"),
        ];
        for (view, icon, label) in views {
            if ui.selectable_label(self.music_ui.view == view, format!("{icon}  {label}")).clicked() {
                if view == MusicView::Albums && self.music_ui.view == MusicView::Albums {
                    self.music_ui.open_album = None;
                }
                self.music_ui.view = view;
            }
        }
        if self.music_ui.view == MusicView::Albums && self.music_ui.open_album.is_none() {
            ui.separator();
            egui::ComboBox::from_id_salt("album-sort")
                .selected_text(format!("Sort: {}", self.music_ui.album_sort.label()))
                .show_ui(ui, |ui| {
                    for sort in AlbumSort::ALL {
                        ui.selectable_value(&mut self.music_ui.album_sort, sort, sort.label());
                    }
                });
        }
    }

    pub(crate) fn music_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let search = egui::TextEdit::singleline(&mut self.music_ui.search)
            .hint_text(format!("{}  Search music", icons::SEARCH))
            .desired_width(f32::INFINITY);
        if ui.add(search).changed() && self.music_ui.view == MusicView::NowPlaying {
            self.music_ui.view = MusicView::Albums;
        }
        if self.music_scan.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Scanning library…");
            });
        }
        if self.store.music_folders.is_empty() {
            ui.add_space(12.0);
            ui.weak("Add a music folder to build your library. Subfolders are scanned too.");
            return;
        }

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            section_label(ui, "Artists");
            let mut picked = None;
            let all = egui::Button::selectable(self.music_ui.artist.is_none(), "")
                .left_text("All artists")
                .right_text(RichText::new(self.music.tracks.len().to_string()).weak());
            if ui.add_sized([ui.available_width(), 22.0], all).clicked() {
                picked = Some(None);
            }
            for (name, count) in &self.music.artists {
                let selected = self.music_ui.artist.as_deref() == Some(name.as_str());
                let button = egui::Button::selectable(selected, "")
                    .left_text(name.as_str())
                    .right_text(RichText::new(count.to_string()).weak());
                if ui.add_sized([ui.available_width(), 22.0], button).clicked() {
                    picked = Some(if selected { None } else { Some(name.clone()) });
                }
            }
            if let Some(artist) = picked {
                self.music_ui.artist = artist;
                self.music_ui.open_album = None;
                if self.music_ui.view == MusicView::NowPlaying {
                    self.music_ui.view = MusicView::Albums;
                }
            }

            ui.add_space(8.0);
            section_label(ui, "Folders");
            for (i, folder) in self.store.music_folders.iter().enumerate() {
                let resp = ui
                    .add(egui::Label::new(RichText::new(display_name(folder)).weak()).truncate().sense(Sense::click()))
                    .on_hover_text(folder.display().to_string());
                resp.context_menu(|ui| {
                    if ui.button(format!("{}  Rescan", icons::REFRESH)).clicked() {
                        self.actions.push(Action::RescanMusic);
                    }
                    if ui.button(format!("{}  Remove folder", icons::TRASH)).clicked() {
                        self.actions.push(Action::RemoveMusicFolder(i));
                    }
                });
            }
        });
    }

    pub(crate) fn music_browser(&mut self, ui: &mut egui::Ui) {
        match self.music_ui.view {
            MusicView::NowPlaying => self.now_playing(ui),
            MusicView::Songs => {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| self.songs_table(ui));
            }
            MusicView::Albums => {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                    match self.music_ui.open_album.clone() {
                        Some(key) => self.album_page(ui, &key),
                        None => self.albums_grid(ui),
                    }
                });
            }
        }
    }

    fn albums_grid(&mut self, ui: &mut egui::Ui) {
        if self.music.albums.is_empty() {
            ui.weak(if self.music_scan.is_some() { "Scanning…" } else { "No music yet." });
            return;
        }
        let filter = Filter::new(&self.music_ui);
        let lib = &self.music;
        let mut list: Vec<usize> = (0..lib.albums.len()).filter(|&i| filter.album(lib, &lib.albums[i])).collect();
        match self.music_ui.album_sort {
            AlbumSort::Artist => {}
            AlbumSort::Title => list.sort_by_key(|&i| lib.albums[i].title.to_lowercase()),
            AlbumSort::Year => list.sort_by_key(|&i| std::cmp::Reverse(lib.albums[i].year.unwrap_or(0))),
            AlbumSort::Added => list.sort_by_key(|&i| std::cmp::Reverse(lib.albums[i].added)),
        }
        if list.is_empty() {
            ui.weak("No albums match.");
            return;
        }

        let cols = (((ui.available_width() + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
        let rows = list.len().div_ceil(cols);
        ui.spacing_mut().item_spacing.y = GAP;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, CARD_H, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for &index in list.iter().skip(row * cols).take(cols) {
                        self.album_card(ui, index);
                    }
                });
            }
        });
    }

    fn album_card(&mut self, ui: &mut egui::Ui, index: usize) {
        let album = &self.music.albums[index];
        let first = &self.music.tracks[album.tracks[0]].path;
        let (rect, resp) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click());
        let cover_rect = Rect::from_min_size(rect.min + vec2((CARD_W - COVER) / 2.0, 0.0), vec2(COVER, COVER));
        paint_cover(ui, cover_rect, self.covers.thumb(&album.key, first), 6.0);
        if resp.hovered() {
            ui.painter().rect_filled(cover_rect, 6.0, Color32::from_black_alpha(90));
            ui.painter().circle_filled(cover_rect.center(), 22.0, Color32::from_white_alpha(230));
            ui.painter().text(
                cover_rect.center() + vec2(1.5, 0.0),
                egui::Align2::CENTER_CENTER,
                icons::PLAY,
                egui::FontId::new(20.0, egui::FontFamily::Name("phosphor-fill".into())),
                Color32::BLACK,
            );
        }
        let text_rect = Rect::from_min_size(cover_rect.left_bottom() + vec2(0.0, 6.0), vec2(COVER, 40.0));
        let mut text_ui = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
        text_ui.spacing_mut().item_spacing.y = 2.0;
        text_ui.add(egui::Label::new(RichText::new(&album.title).strong()).truncate());
        let sub = match album.year {
            Some(y) => format!("{} · {y}", album.artist),
            None => album.artist.clone(),
        };
        text_ui.add(egui::Label::new(RichText::new(sub).weak().small()).truncate());

        // Clicking the play badge plays the album; elsewhere opens it.
        let on_badge = resp.interact_pointer_pos().is_some_and(|p| p.distance(cover_rect.center()) < 24.0);
        if resp.clicked() {
            if on_badge {
                let queue = album.tracks.iter().map(|&t| self.music.tracks[t].path.clone()).collect();
                self.actions.push(Action::PlayTracks { queue, start: 0 });
            } else {
                self.music_ui.open_album = Some(album.key.clone());
            }
        }
    }

    fn album_page(&mut self, ui: &mut egui::Ui, key: &str) {
        let Some(album) = self.music.album(key) else {
            self.music_ui.open_album = None;
            return;
        };
        let first = self.music.tracks[album.tracks[0]].path.clone();
        let queue: Vec<PathBuf> = album.tracks.iter().map(|&t| self.music.tracks[t].path.clone()).collect();
        let playing = self.current_track().map(|(p, _)| p);

        if ui.add(egui::Button::new(format!("{}  Albums", icons::CARET_LEFT)).frame_when_inactive(false)).clicked() {
            self.music_ui.open_album = None;
        }
        ui.add_space(8.0);
        let cover = self.covers.large(key, &first);
        let album = self.music.album(key).unwrap();
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(200.0, 200.0), Sense::hover());
            paint_cover(ui, rect, cover.as_ref(), 8.0);
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.add_space(70.0);
                ui.label(RichText::new(&album.title).size(26.0).strong());
                if ui.add(egui::Label::new(RichText::new(&album.artist).size(16.0)).sense(Sense::click())).on_hover_text("Show this artist").clicked() {
                    self.music_ui.artist = Some(album.artist.clone());
                    self.music_ui.open_album = None;
                }
                let mut details = Vec::new();
                if let Some(y) = album.year {
                    details.push(y.to_string());
                }
                details.push(format!("{} tracks", album.tracks.len()));
                details.push(format!("{} min", (album.duration(&self.music) / 60.0).round()));
                ui.weak(details.join(" · "));
                ui.add_space(8.0);
                if ui.button(RichText::new(format!("{}  Play", icons::PLAY)).size(15.0)).clicked() {
                    self.actions.push(Action::PlayTracks { queue: queue.clone(), start: 0 });
                }
            });
        });
        ui.add_space(12.0);
        ui.separator();

        let multi_disc = album.tracks.iter().any(|&t| self.music.tracks[t].disc.unwrap_or(1) > 1);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (n, &t) in album.tracks.iter().enumerate() {
                let track = &self.music.tracks[t];
                let number = match (multi_disc, track.disc, track.track) {
                    (true, Some(d), Some(t)) => format!("{d}-{t}"),
                    (_, _, Some(t)) => t.to_string(),
                    _ => (n + 1).to_string(),
                };
                let is_playing = playing.as_ref() == Some(&track.path);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                let visuals = ui.visuals();
                if is_playing {
                    ui.painter().rect_filled(rect, 4.0, visuals.selection.bg_fill.gamma_multiply(0.35));
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect.shrink2(vec2(8.0, 0.0)));
                let y = rect.center().y;
                let font = egui::FontId::proportional(14.0);
                let weak = visuals.weak_text_color();
                let strong = visuals.strong_text_color();
                let num = if is_playing { icons::SPEAKER_HIGH.to_string() } else { number };
                painter.text(egui::pos2(rect.left() + 36.0, y), egui::Align2::RIGHT_CENTER, num, font.clone(), weak);
                let mut title = track.title.clone();
                if track.artist != album.artist {
                    title = format!("{title}  ·  {}", track.artist);
                }
                painter.text(egui::pos2(rect.left() + 52.0, y), egui::Align2::LEFT_CENTER, title, font.clone(), strong);
                painter.text(egui::pos2(rect.right() - 12.0, y), egui::Align2::RIGHT_CENTER, format_time(track.duration), font, weak);
                if resp.clicked() {
                    self.actions.push(Action::PlayTracks { queue: queue.clone(), start: n });
                }
            }
        });
    }

    fn songs_table(&mut self, ui: &mut egui::Ui) {
        let filter = Filter::new(&self.music_ui);
        let lib = &self.music;
        let mut list: Vec<usize> = (0..lib.tracks.len()).filter(|&i| filter.track(&lib.tracks[i])).collect();
        let column = self.music_ui.song_sort;
        list.sort_by(|&a, &b| {
            let (a, b) = (&lib.tracks[a], &lib.tracks[b]);
            match column {
                SongColumn::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                SongColumn::Artist => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
                SongColumn::Album => a.album.to_lowercase().cmp(&b.album.to_lowercase()),
                SongColumn::Year => a.year.cmp(&b.year),
                SongColumn::Duration => a.duration.total_cmp(&b.duration),
            }
        });
        if self.music_ui.song_desc {
            list.reverse();
        }
        if list.is_empty() {
            ui.weak(if lib.tracks.is_empty() { "No music yet." } else { "No songs match." });
            return;
        }

        let playing = self.current_track().map(|(p, _)| p);
        let mut sort_by = None;
        let columns = [
            (SongColumn::Title, "Title"),
            (SongColumn::Artist, "Artist"),
            (SongColumn::Album, "Album"),
            (SongColumn::Year, "Year"),
            (SongColumn::Duration, "Time"),
        ];
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::initial(200.0).at_least(80.0).clip(true))
            .column(Column::initial(220.0).at_least(80.0).clip(true))
            .column(Column::exact(52.0))
            .column(Column::exact(60.0))
            .header(26.0, |mut header| {
                for (col, label) in columns {
                    header.col(|ui| {
                        let arrow = match (column == col, self.music_ui.song_desc) {
                            (true, false) => icons::CARET_UP,
                            (true, true) => icons::CARET_DOWN,
                            _ => "",
                        };
                        let text = RichText::new(format!("{label} {arrow}")).strong();
                        if ui.add(egui::Button::new(text).frame(false)).clicked() {
                            sort_by = Some(col);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(26.0, list.len(), |mut row| {
                    let n = row.index();
                    let t = &lib.tracks[list[n]];
                    row.set_selected(playing.as_ref() == Some(&t.path));
                    row.col(|ui| {
                        ui.add(egui::Label::new(&t.title).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&t.artist).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(&t.album).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.weak(t.year.map(|y| y.to_string()).unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.weak(format_time(t.duration));
                    });
                    if row.response().clicked() {
                        let queue = list.iter().map(|&i| lib.tracks[i].path.clone()).collect();
                        self.actions.push(Action::PlayTracks { queue, start: n });
                    }
                });
            });

        if let Some(col) = sort_by {
            if col == self.music_ui.song_sort {
                self.music_ui.song_desc = !self.music_ui.song_desc;
            } else {
                self.music_ui.song_sort = col;
                self.music_ui.song_desc = false;
            }
        }
    }

    /// Full-size now-playing view: visualizer (or cover) with track info.
    pub(crate) fn now_playing(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        let Some((path, track)) = self.current_track() else {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)));
            child.add_space(rect.height() * 0.4);
            child.label(RichText::new(icons::VINYL).size(44.0).weak());
            child.weak("Nothing is playing. Pick an album or song.");
            return;
        };
        let (title, artist, album, key) = match track {
            Some(t) => (t.title.clone(), t.artist.clone(), t.album.clone(), self.music.album_of(t).map(|a| a.key.clone())),
            None => (display_name(&path), String::new(), String::new(), None),
        };
        let key = key.unwrap_or_else(|| path.display().to_string());
        let cover = self.covers.large(&key, &path);

        let visualizer = self.store.visualizer;
        let info_h = 128.0;
        match visualizer.preset().filter(|_| self.viz_ready) {
            Some(preset) => {
                crate::viz::paint(ui, rect, &self.features, preset, self.started.elapsed().as_secs_f32());
                self.viz_visible = true;
            }
            None => {
                let area = Rect::from_min_max(rect.min, rect.max - vec2(0.0, info_h));
                let side = (area.width().min(area.height()) * 0.7).min(520.0);
                paint_cover(ui, Rect::from_center_size(area.center(), vec2(side, side)), cover.as_ref(), 10.0);
            }
        }

        // Bottom band with the cover, track info and visualizer picker,
        // fading in from transparent so the visuals show through.
        let band = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - info_h - 40.0), rect.max);
        let mut shade = egui::Mesh::default();
        let (clear, dark) = (Color32::TRANSPARENT, Color32::from_black_alpha(200));
        shade.colored_vertex(band.left_top(), clear);
        shade.colored_vertex(band.right_top(), clear);
        shade.colored_vertex(band.right_bottom(), dark);
        shade.colored_vertex(band.left_bottom(), dark);
        shade.add_triangle(0, 1, 2);
        shade.add_triangle(0, 2, 3);
        ui.painter().add(shade);
        let band = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - info_h), rect.max);
        let thumb = Rect::from_min_size(band.min + vec2(24.0, 22.0), vec2(84.0, 84.0));
        paint_cover(ui, thumb, cover.as_ref(), 6.0);
        let mut info = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(egui::pos2(thumb.right() + 18.0, thumb.top() + 14.0), band.max - vec2(24.0, 0.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        info.add(egui::Label::new(RichText::new(title).size(22.0).strong().color(Color32::WHITE)).truncate());
        let sub = [artist, album].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  ·  ");
        info.add(egui::Label::new(RichText::new(sub).size(15.0).color(Color32::from_gray(190))).truncate());

        let picker_rect = Rect::from_min_max(egui::pos2(band.right() - 640.0, band.bottom() - 44.0), band.max - vec2(24.0, 12.0));
        let mut picker = ui.new_child(egui::UiBuilder::new().max_rect(picker_rect).layout(Layout::right_to_left(Align::Center)));
        if self.viz_ready {
            let auto = picker
                .selectable_label(self.store.viz_auto, format!("{}  Auto", icons::REFRESH))
                .on_hover_text("Switch presets by themselves, on the beat");
            if auto.clicked() {
                self.actions.push(Action::ToggleVizAuto);
            }
            picker.separator();
        }
        let choices = Visualizer::PRESETS.into_iter().filter(|_| self.viz_ready).chain([Visualizer::Off]);
        for vis in choices.collect::<Vec<_>>().into_iter().rev() {
            let selected = visualizer.preset() == vis.preset();
            if picker.selectable_label(selected, vis.label()).on_hover_text("Visualizer (V to cycle)").clicked() {
                self.actions.push(Action::SetVisualizer(vis));
            }
        }
    }

    /// Small cover and title of the playing track, for the controls bar.
    pub(crate) fn now_playing_chip(&mut self, ui: &mut egui::Ui) {
        let Some((path, track)) = self.current_track() else { return };
        let (title, artist, key) = match track {
            Some(t) => (t.title.clone(), t.artist.clone(), self.music.album_of(t).map(|a| a.key.clone())),
            None => (display_name(&path), String::new(), None),
        };
        let key = key.unwrap_or_else(|| path.display().to_string());
        let (rect, resp) = ui.allocate_exact_size(vec2(230.0, 36.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let cover_rect = Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(32.0, 32.0));
        paint_cover(ui, cover_rect, self.covers.thumb(&key, &path), 3.0);
        let painter = ui.painter_at(rect.shrink(2.0));
        let x = cover_rect.right() + 8.0;
        let visuals = ui.visuals();
        painter.text(egui::pos2(x, rect.top() + 10.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(13.5), visuals.strong_text_color());
        painter.text(egui::pos2(x, rect.bottom() - 10.0), egui::Align2::LEFT_CENTER, artist, egui::FontId::proportional(12.0), visuals.weak_text_color());
        if resp.on_hover_text("Show now playing").clicked() {
            self.actions.push(Action::SetMode(Mode::Music));
            self.music_ui.view = MusicView::NowPlaying;
        }
    }
}

/// Draws a cover cropped to a square, or a placeholder when there is none.
fn paint_cover(ui: &egui::Ui, rect: Rect, texture: Option<&TextureHandle>, radius: f32) {
    match texture {
        Some(tex) => {
            let [w, h] = tex.size().map(|v| v as f32);
            let (du, dv) = if w > h { ((1.0 - h / w) / 2.0, 0.0) } else { (0.0, (1.0 - w / h) / 2.0) };
            let uv = Rect::from_min_max(egui::pos2(du, dv), egui::pos2(1.0 - du, 1.0 - dv));
            egui::Image::from_texture(tex).uv(uv).corner_radius(radius).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, radius, ui.visuals().faint_bg_color.gamma_multiply(2.0));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::MUSIC_NOTES,
                egui::FontId::proportional(rect.width() * 0.3),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text.to_uppercase()).small().weak());
}
