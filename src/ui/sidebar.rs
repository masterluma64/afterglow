//! The playlist sidebar: docked, or sliding out over the video.

use crate::*;

/// A playlist entry with its watch progress. `selection` is every selected
/// video, in playlist order, when this one is among them. `still` is its
/// video's still, when stills are shown: `Some(None)` until it is taken.
/// What is picked from its right-click menu is added to `picked`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn playlist_row(
    ui: &mut egui::Ui,
    app: &App,
    picked: &mut Vec<Action>,
    index: usize,
    path: &Path,
    name: &str,
    playing: bool,
    selection: Option<&[PathBuf]>,
    still: Option<Option<egui::TextureId>>,
) -> egui::Response {
    let store = &app.store;
    let entry = store.get(path);
    // Painted under the row once its size is known.
    let background = ui.painter().add(egui::Shape::Noop);
    let in_progress = (!entry.watched && entry.fraction() > 0.0) || store.resume_position(path).is_some();
    let resp = match still {
        Some(still) => still_row(ui, &entry, name, playing, in_progress, still),
        None => {
            let dim = |text: RichText| ui::secondary(ui, playing, text);
            let text = if entry.watched { dim(RichText::new(format!("{}  {name}", icons::CHECK))) } else { RichText::new(name) };
            let length = if entry.duration > 0.0 { format_time(entry.duration) } else { String::new() };
            let button = egui::Button::selectable(playing, "")
                .left_text(text)
                .right_text(dim(RichText::new(length)))
                // One line per entry (cut off with …), so rows have a fixed height.
                .truncate()
                .sense(Sense::click_and_drag());
            // The playing row's fill is a gradient, painted under the button.
            let resp = ui
                .scope(|ui| {
                    ui.visuals_mut().selection.bg_fill = Color32::TRANSPARENT;
                    ui.add_sized([ui.available_width(), PLAYLIST_ROW_HEIGHT], button)
                })
                .inner;
            if playing {
                gradient::fill_slot(ui.painter(), background, resp.rect, 4.0, gradient::accent(ui.visuals()));
            }
            if in_progress {
                let r = resp.rect.shrink2(vec2(4.0, 1.0));
                let bar = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 2.0), egui::pos2(r.left() + r.width() * entry.fraction(), r.bottom()));
                let colors = if playing { [ui.visuals().selection.stroke.color; 2] } else { gradient::accent(ui.visuals()) };
                gradient::fill(ui.painter(), bar, 1.0, r, colors, gradient::Direction::Across);
            }
            resp
        }
    };
    if selection.is_some() {
        if playing {
            // Already accent-colored: an outline shows it is selected too.
            ui.painter().rect_stroke(resp.rect.shrink(1.0), 4.0, (1.5, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
        } else {
            let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.35);
            ui.painter().set(background, egui::Shape::rect_filled(resp.rect, 4.0, fill));
        }
    }
    let resp = resp.on_hover_text(format!("{}\n{}", path.display(), hover_text(&entry)));
    ui::context_menu(&resp, |ui| {
        if let Some(selection) = selection.filter(|s| s.len() > 1) {
            ui.label(RichText::new(format!("{} videos selected", selection.len())).bold());
            items::offer(ui, app, Action::SetWatchedMany(selection.to_vec(), true), picked);
            items::offer(ui, app, Action::SetWatchedMany(selection.to_vec(), false), picked);
            ui.separator();
            items::offer(ui, app, Action::RemoveMany(selection.to_vec()), picked).on_hover_text("The files are not touched");
            ui.separator();
            items::offer(ui, app, Action::ClearSelection, picked);
            return;
        }
        items::entry_items(ui, app, index, path, picked);
        ui.separator();
        subfolder_option(ui, app, picked);
    });
    resp
}

/// A playlist entry with a still of its video (`None` until it is taken),
/// its progress along the bottom of the still, and its name above how much
/// of it is left.
fn still_row(ui: &mut egui::Ui, entry: &progress::Entry, name: &str, playing: bool, in_progress: bool, still: Option<egui::TextureId>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), STILL_ROW_HEIGHT), Sense::click_and_drag());
    let visuals = ui.visuals();
    let painter = ui.painter();
    if playing {
        gradient::fill(painter, rect, 4.0, rect, gradient::accent(visuals), gradient::Direction::Across);
    } else if resp.hovered() || resp.dragged() {
        painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    // 16:9, as tall as the row leaves room for.
    let height = rect.height() - 8.0;
    let frame = egui::Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(height * 16.0 / 9.0, height));
    match still {
        // A watched video's still is dimmed, like its name.
        Some(id) => {
            let tint = if entry.watched && !playing { Color32::from_gray(110) } else { Color32::WHITE };
            egui::Image::new((id, frame.size())).corner_radius(3).tint(tint).paint_at(ui, frame);
        }
        None => {
            painter.rect_filled(frame, 3.0, Color32::from_black_alpha(if visuals.dark_mode { 90 } else { 30 }));
        }
    }
    if in_progress {
        let bar = egui::Rect::from_min_max(egui::pos2(frame.left(), frame.bottom() - 3.0), frame.max);
        painter.rect_filled(bar, egui::CornerRadius { sw: 3, se: 3, ..Default::default() }, Color32::from_black_alpha(160));
        let filled = egui::Rect::from_min_size(bar.min, vec2(bar.width() * entry.fraction(), bar.height()));
        let colors = if playing { [visuals.selection.stroke.color; 2] } else { gradient::accent(visuals) };
        gradient::fill(painter, filled, egui::CornerRadius { sw: 3, ..Default::default() }, bar, colors, gradient::Direction::Across);
    }

    let (strong, weak) = if playing {
        (visuals.selection.stroke.color, visuals.selection.stroke.color.gamma_multiply(0.8))
    } else {
        (visuals.text_color(), visuals.weak_text_color())
    };
    let detail = match (entry.watched, entry.duration > 0.0) {
        (true, true) => format!("{}  Watched · {}", icons::CHECK, format_time(entry.duration)),
        (true, false) => format!("{}  Watched", icons::CHECK),
        (false, true) if in_progress => format!("{} left", format_total(entry.duration - entry.position)),
        (false, true) => format_time(entry.duration),
        (false, false) => String::new(),
    };
    let left = frame.right() + 8.0;
    let width = (rect.right() - 6.0 - left).max(10.0);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let title = ui::one_line(ui, name.to_string(), font.clone(), if entry.watched && !playing { weak } else { strong }, width);
    let detail = ui::one_line(ui, detail, font, weak, width);
    let gap = 2.0;
    let top = rect.center().y - (title.size().y + gap + detail.size().y) / 2.0;
    let painter = ui.painter();
    painter.galley(egui::pos2(left, top), title.clone(), strong);
    painter.galley(egui::pos2(left, top + title.size().y + gap), detail, weak);
    resp
}

/// A playlist entry's display name, and its season.
pub(crate) struct RowName {
    /// With the season it is from, when it isn't directly in the opened folder.
    pub(crate) name: String,
    /// Without the season.
    pub(crate) short: String,
    /// `name` in lowercase, for the search.
    pub(crate) lower: String,
    /// An index into `App::seasons`, if the playlist has seasons and this
    /// video is in the opened folder.
    pub(crate) season: Option<usize>,
}

/// Which season the playlist shows, when it is a show with a folder per season.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum SeasonPick {
    /// The playing video's, following it into the next season.
    Playing,
    All,
    /// This one (its folder, relative to the opened folder).
    Only(PathBuf),
}

/// A season's name: its folder ("Season 2"), or for one deeper in, its
/// folders ("Season 2 / Extras"). Videos directly in the show's `folder`
/// go by its name.
pub(crate) fn season_name(folder: &Path, season: &Path) -> String {
    if season.as_os_str().is_empty() {
        return display_name(folder);
    }
    season.to_string_lossy().replace(std::path::MAIN_SEPARATOR, " / ")
}

/// The "Include subfolders" switch in the playlist's right-click menus.
pub(crate) fn subfolder_option(ui: &mut egui::Ui, app: &App, picked: &mut Vec<Action>) {
    let tip = "Also play videos in subfolders when opening a folder (rescans the opened folder now)";
    items::offer(ui, app, Action::SetIncludeSubfolders(!app.store.include_subfolders), picked).on_hover_text(tip);
}

pub(crate) fn hover_text(entry: &progress::Entry) -> String {
    if entry.watched {
        "Watched · right-click for options".into()
    } else if entry.position > 0.0 {
        format!("{} / {} · right-click for options", format_time(entry.position), format_time(entry.duration))
    } else {
        "Not started · right-click for options".into()
    }
}

/// A running time for totals: "4 h 12 min", "42 min", "30 s".
pub(crate) fn format_total(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    match (s / 3600, (s / 60) % 60) {
        (0, 0) => format!("{s} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

impl App {
    /// Height of a playlist entry, without the spacing between entries.
    pub(crate) fn row_height(&self) -> f32 {
        if self.store.settings.playlist_stills { STILL_ROW_HEIGHT } else { PLAYLIST_ROW_HEIGHT }
    }

    /// Whether the playlist sits beside the video. Unpinned (and in fullscreen)
    /// it slides out over the video instead.
    pub(crate) fn sidebar_docked(&self) -> bool {
        self.store.sidebar_pinned && !self.controls_overlay()
    }

    /// The unpinned playlist: it slides out over the video when the pointer
    /// reaches its edge of the video (or from the keyboard) and hides once the
    /// pointer leaves it, unless it's in use.
    pub(crate) fn floating_sidebar(&mut self, ctx: &egui::Context, video: egui::Rect) {
        let pointer = ctx.pointer_hover_pos();
        let right = self.store.settings.sidebar_side == progress::Side::Right;
        let at_edge = pointer.is_some_and(|p| {
            video.contains(p) && if right { p.x > video.right() - EDGE_WIDTH } else { p.x < video.left() + EDGE_WIDTH }
        });
        // Not for a single video, whose playlist only comes out with B, nor
        // in the mini player, whose edges resize it.
        if self.peek.is_none() && at_edge && !self.single_video() && !self.mini() && !ctx.input(|i| i.pointer.any_down()) {
            self.peek = Some(Peek { entered: true, left_at: None });
        }
        if self.peek.is_none() {
            return;
        }
        let size = vec2(self.sidebar_width, video.height());
        let (pos, pivot) = if right { (video.right_top(), egui::Align2::RIGHT_TOP) } else { (video.left_top(), egui::Align2::LEFT_TOP) };
        let id = egui::Id::unique("floating-sidebar");
        // A new area is normally laid out invisibly on its first frame to
        // measure it, which flashes the video through when unpinning. Its size
        // is known, so skip that.
        if egui::AreaState::load(ctx, id).and_then(|s| s.size) != Some(size) {
            let state = egui::AreaState { pivot_pos: Some(pos), pivot, size: Some(size), interactable: true, last_became_visible_at: None };
            ctx.memory_mut(|m| m.areas_mut().set_state(egui::LayerId::new(egui::Order::Middle, id), state));
        }
        let area = egui::Area::new(id)
            .fade_in(false)
            .fixed_pos(pos)
            .pivot(pivot)
            .constrain(false)
            .show(ctx, |ui| {
                let frame = egui::Frame::side_top_panel(ui.style()).shadow(ui.visuals().popup_shadow);
                let inner = size - frame.total_margin().sum();
                frame.show(ui, |ui| {
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    self.sidebar(ui);
                });
            });
        let inside = pointer.is_some_and(|p| area.response.rect.expand(12.0).contains(p));
        if inside {
            // Keeps the cursor from hiding while it rests on the playlist.
            self.last_activity = Instant::now();
        }
        let searching = ctx.memory(|m| m.has_focus(egui::Id::unique("playlist-search")));
        let busy = searching || self.drag_row.is_some() || egui::Popup::is_any_open(ctx);
        let Some(peek) = &mut self.peek else { return };
        if inside || busy {
            peek.entered |= inside;
            peek.left_at = None;
        } else if peek.entered {
            let left_at = *peek.left_at.get_or_insert_with(Instant::now);
            if left_at.elapsed() >= PEEK_HIDE_DELAY {
                self.peek = None;
            } else {
                ctx.request_repaint_after(PEEK_HIDE_DELAY);
            }
        }
    }

    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui) {
        // Right-clicking where there are no videos (registered first, so the
        // rows and buttons on top take precedence).
        let background = ui.interact(ui.max_rect(), egui::Id::unique("playlist-area"), Sense::click());
        // The same scope docked or slid out, so the list keeps its scroll position.
        ui.scope_builder(egui::UiBuilder::new().scope_id(egui::Id::unique("playlist")), |ui| self.sidebar_contents(ui));
        // The playlist's own actions, matching the buttons in its header.
        ui::context_menu(&background, |ui| {
            self.menu_item(ui, Action::OpenFolder);
            self.menu_item(ui, Action::OpenVideo);
            self.menu_item(ui, Action::OpenVideos);
            ui.separator();
            self.menu_item(ui, Action::RefreshFolder);
            self.menu_item(ui, Action::FocusSearch);
            self.menu_item(ui, Action::ClearPlaylist);
            ui.separator();
            let mut picked = Vec::new();
            subfolder_option(ui, self, &mut picked);
            self.actions.append(&mut picked);
            self.menu_item(ui, Action::TogglePin);
        });
    }

    pub(crate) fn sidebar_contents(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Playlist").bold());
            let count = self.playlist.len();
            if count > 0 {
                ui.label(RichText::new(format!("{count} video{}", if count == 1 { "" } else { "s" })).weak());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pinned = self.store.sidebar_pinned;
                let (pin, tip) = if pinned {
                    (icons::fill(icons::PIN), "Unpin: hide the playlist until you point at the edge of the video")
                } else {
                    (RichText::new(icons::PIN), "Pin the playlist beside the video")
                };
                if ui.add(egui::Button::new(pin.size(14.0)).frame_when_inactive(false)).on_hover_text(tip).clicked() {
                    self.actions.push(Action::TogglePin);
                }
                if count > 0 {
                    let clear = egui::Button::new(RichText::new(icons::TRASH).size(14.0)).frame_when_inactive(false);
                    if ui.add(clear).on_hover_text("Clear playlist (files are not touched)").clicked() {
                        self.actions.push(Action::ClearPlaylist);
                    }
                }
            });
        });
        if !self.playlist.is_empty() {
            self.playlist_summary(ui);
        }
        ui.separator();

        if self.playlist.is_empty() {
            ui.add_space(4.0);
            ui.weak("Empty. Open a folder to play its videos, or add videos one by one.");
            ui.add_space(4.0);
            ui.weak("You can also drop them onto the window.");
            return;
        }
        let query = self.search.trim().to_lowercase();
        self.refresh_names();
        // The search looks through every season.
        let season = if query.is_empty() { self.shown_season() } else { None };
        // Indices of the entries to list, filtered by the search or season.
        let visible: Vec<usize> = (0..self.playlist.len())
            .filter(|&i| query.is_empty() || self.names[i].lower.contains(&query))
            .filter(|&i| season.is_none() || self.names[i].season == season)
            .collect();
        self.search_box(ui, &visible);
        if query.is_empty() && !self.seasons.is_empty() {
            self.season_picker(ui, season);
        }
        if visible.is_empty() {
            ui.add_space(4.0);
            ui.weak(format!("No videos match \"{}\".", self.search.trim()));
            return;
        }
        let picked = (!query.is_empty()).then(|| visible.get(self.search_pick).copied()).flatten();
        let mut scroll = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(pick) = self.scroll_to_pick.take() {
            // Keep the picked result in the middle of the list.
            let row = self.row_height() + ui.spacing().item_spacing.y;
            let offset = pick as f32 * row - (ui.available_height() - self.row_height()) / 2.0;
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        let selection = self.selected_paths();
        if selection.len() != self.selected.len() {
            // Some selected videos left the playlist.
            self.selected = selection.iter().cloned().collect();
        }
        // Only the rows in view are laid out.
        let (row_height, stills) = (self.row_height(), self.store.settings.playlist_stills);
        scroll.show_rows(ui, row_height, visible.len(), |ui, range| {
            let mut rows = Vec::new();
            for &index in &visible[range] {
                let path = &self.playlist[index];
                // Within a season, the season's name needn't be on every row.
                let row = &self.names[index];
                let name = if season.is_some() { &row.short } else { &row.name };
                let selected = self.selected.contains(path).then_some(selection.as_slice());
                let still = stills.then(|| self.stills.get(path));
                let mut chosen = Vec::new();
                let resp = playlist_row(ui, self, &mut chosen, index, path, name, self.current == Some(index), selected, still);
                self.actions.append(&mut chosen);
                if resp.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    self.click_row(&visible, index, modifiers);
                }
                if picked == Some(index) {
                    // The search result Enter plays.
                    ui.painter().rect_stroke(resp.rect, 4.0, (1.5, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
                }
                if resp.drag_started() {
                    self.drag_row = Some(index);
                }
                rows.push((index, resp.rect));
            }
            self.drag_rows(ui, &rows);
        });
    }

    /// A click on a playlist row plays it; with Ctrl it is added to (or taken
    /// out of) the selection, and Shift selects every row from the last one
    /// clicked. `visible` is the rows shown, so a search limits the range.
    pub(crate) fn click_row(&mut self, visible: &[usize], index: usize, modifiers: egui::Modifiers) {
        let path = self.playlist[index].clone();
        if modifiers.shift {
            let at = |p: &PathBuf| visible.iter().position(|&i| self.playlist[i] == *p);
            let here = at(&path).unwrap_or(0);
            let from = self.select_anchor.as_ref().and_then(at).unwrap_or(here);
            let range: Vec<PathBuf> = visible[from.min(here)..=from.max(here)].iter().map(|&i| self.playlist[i].clone()).collect();
            if !modifiers.command {
                self.selected.clear();
            }
            self.selected.extend(range);
        } else if modifiers.command {
            if !self.selected.remove(&path) {
                self.selected.insert(path.clone());
            }
            self.select_anchor = Some(path);
        } else {
            self.selected.clear();
            self.select_anchor = Some(path);
            self.actions.push(Action::Play(index));
        }
    }

    /// The selected videos, in playlist order.
    pub(crate) fn selected_paths(&self) -> Vec<PathBuf> {
        if self.selected.is_empty() {
            return Vec::new();
        }
        self.playlist.iter().filter(|p| self.selected.contains(*p)).cloned().collect()
    }

    /// Rebuilds the cached display names (and their lowercase forms, for the
    /// search) after the playlist changed.
    pub(crate) fn refresh_names(&mut self) {
        if self.names.len() == self.playlist.len() {
            return;
        }
        let folder = self.store.folder.as_deref();
        // How many videos have each title, to keep shared ones out (see `video_name`).
        let mut titles: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for title in self.playlist.iter().filter_map(|p| self.store.title(p)) {
            *titles.entry(title).or_default() += 1;
        }
        let mut seasons: Vec<PathBuf> = Vec::new();
        let names = self
            .playlist
            .iter()
            .map(|path| {
                let short = match self.store.title(path) {
                    Some(title) if titles[title] == 1 => title.to_string(),
                    _ => display_name(path),
                };
                // Videos from a subfolder of the opened folder show where they're from.
                let subfolder = folder.and_then(|f| library::season_of(f, path));
                let name = match (folder, subfolder) {
                    (Some(folder), Some(sub)) if !sub.as_os_str().is_empty() => format!("{} / {short}", season_name(folder, sub)),
                    _ => short.clone(),
                };
                let season = subfolder.map(|sub| match seasons.iter().position(|s| s == sub) {
                    Some(i) => i,
                    None => {
                        seasons.push(sub.to_path_buf());
                        seasons.len() - 1
                    }
                });
                let lower = name.to_lowercase();
                RowName { name, short, lower, season }
            })
            .collect();
        self.names = names;
        // Only a show with more than one season gets to pick one.
        if seasons.len() < 2 {
            seasons.clear();
            for row in &mut self.names {
                row.season = None;
            }
        }
        self.seasons = seasons;
    }

    /// The season the playlist shows (an index into `seasons`), or `None`
    /// for all of them.
    pub(crate) fn shown_season(&self) -> Option<usize> {
        if self.seasons.is_empty() {
            return None;
        }
        let of = |index: usize| self.names.get(index).and_then(|row| row.season);
        match &self.season_pick {
            SeasonPick::All => None,
            SeasonPick::Only(season) => self.seasons.iter().position(|s| s == season),
            // The playing video's, or else the one to carry on with.
            SeasonPick::Playing => self.current.or_else(|| self.continue_target()).and_then(of).or(Some(0)),
        }
    }

    /// Picks the season the playlist shows, with how much of each is watched.
    fn season_picker(&mut self, ui: &mut egui::Ui, shown: Option<usize>) {
        let playing = self.current.and_then(|i| self.names.get(i)?.season);
        let folder = self.store.folder.clone().unwrap_or_default();
        let label = |i: Option<usize>| match i {
            Some(i) => season_name(&folder, &self.seasons[i]),
            None => "All seasons".to_string(),
        };
        let watched = |season: usize| {
            let rows = self.names.iter().zip(&self.playlist).filter(|(row, _)| row.season == Some(season));
            let (count, done) = rows.fold((0, 0), |(n, w), (_, p)| (n + 1, w + usize::from(self.store.get(p).watched)));
            format!("{done} of {count} watched")
        };
        let mut pick = None;
        egui::ComboBox::from_id_salt("season-picker")
            .width(ui.available_width())
            .selected_text(label(shown))
            .show_ui(ui, |ui| {
                for i in (0..self.seasons.len()).map(Some).chain([None]) {
                    let text = match i {
                        Some(i) if playing == Some(i) => format!("{}  {}", label(Some(i)), icons::PLAY),
                        _ => label(i),
                    };
                    let hover = i.map(watched).unwrap_or_else(|| format!("Every video, {} seasons", self.seasons.len()));
                    if gradient::selectable(ui, shown == i, text).on_hover_text(hover).clicked() {
                        pick = Some(i);
                    }
                }
            });
        if let Some(i) = pick {
            // The playing video's season goes along when the next season starts.
            self.season_pick = match i {
                None => SeasonPick::All,
                Some(i) if playing.is_none_or(|p| p == i) => SeasonPick::Playing,
                Some(i) => SeasonPick::Only(self.seasons[i].clone()),
            };
        }
        ui.add_space(2.0);
    }

    /// The name a video is shown by: the title stored in its file, unless it
    /// has none or another video in the playlist has the same one (as when a
    /// show puts its own name in every file), else its file name.
    pub(crate) fn video_name(&self, path: &Path) -> String {
        match self.store.title(path) {
            Some(title) if !self.playlist.iter().any(|p| p != path && self.store.title(p) == Some(title)) => title.to_string(),
            _ => display_name(path),
        }
    }

    /// The search box: / focuses it, ↑/↓ pick a result, Enter plays it and Esc
    /// clears the search.
    pub(crate) fn search_box(&mut self, ui: &mut egui::Ui, results: &[usize]) {
        let id = egui::Id::unique("playlist-search");
        if ui.memory(|m| m.has_focus(id)) {
            let (down, up) = ui.input_mut(|i| {
                (i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::ArrowUp))
            });
            let last = results.len().saturating_sub(1);
            if down {
                self.search_pick = (self.search_pick + 1).min(last);
            }
            if up {
                self.search_pick = self.search_pick.saturating_sub(1);
            }
            if down || up {
                self.scroll_to_pick = Some(self.search_pick);
            }
        }
        ui.horizontal(|ui| {
            // Room for the clear button once there is text: its icon, padding and gap.
            let spacing = ui.spacing();
            let clear_button = ICON_SIZE + 2.0 * spacing.button_padding.x + spacing.item_spacing.x;
            let width = ui.available_width() - if self.search.is_empty() { 0.0 } else { clear_button };
            let edit = egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search playlist (/)", icons::SEARCH))
                .desired_width(width);
            let resp = ui::text_field(ui, edit, id, TEXT_MARGIN);
            if resp.changed() {
                self.search_pick = 0;
                self.scroll_to_pick = Some(0);
            }
            if std::mem::take(&mut self.focus_search) {
                resp.request_focus();
            }
            // egui has already dropped focus by the time these show up.
            if resp.lost_focus() {
                let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if escape {
                    self.search.clear();
                }
                if enter
                    && !self.search.trim().is_empty()
                    && let Some(index) = results.get(self.search_pick)
                {
                    self.actions.push(Action::Play(*index));
                    self.search.clear();
                    self.peek = None;
                }
            }
            if !self.search.is_empty() && icon_button(ui, icons::X, "Clear the search (Esc)").clicked() {
                self.search.clear();
            }
        });
        ui.add_space(2.0);
    }

    /// While a row is dragged, shows where it would land, and moves it there
    /// when released.
    pub(crate) fn drag_rows(&mut self, ui: &mut egui::Ui, rows: &[(usize, egui::Rect)]) {
        let Some(from) = self.drag_row else { return };
        let (Some(pointer), Some(&(last, last_rect))) = (ui.ctx().pointer_interact_pos(), rows.last()) else {
            self.drag_row = None;
            return;
        };
        // Land before the first row whose middle is below the pointer.
        let (to, y) = match rows.iter().find(|(_, r)| pointer.y < r.center().y) {
            Some(&(index, rect)) => (index, rect.top()),
            None => (last + 1, last_rect.bottom()),
        };
        let unchanged = to == from || to == from + 1;
        if !unchanged {
            let x = last_rect.x_range();
            ui.painter().hline(x, y, (2.0, ui.visuals().selection.bg_fill));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        // Scroll when dragging near the top or bottom edge.
        let clip = ui.clip_rect();
        if pointer.y < clip.top() + 24.0 {
            ui.scroll_with_delta(vec2(0.0, 6.0));
        } else if pointer.y > clip.bottom() - 24.0 {
            ui.scroll_with_delta(vec2(0.0, -6.0));
        }
        if !ui.input(|i| i.pointer.any_down()) {
            self.drag_row = None;
            if !unchanged {
                self.actions.push(Action::Move { from, to });
            }
        }
    }

    /// Total and watched running time of the playlist, with a progress bar.
    pub(crate) fn playlist_summary(&mut self, ui: &mut egui::Ui) {
        // Adding up the whole playlist every frame adds up; once a second is plenty.
        let fresh = self.summary.is_some_and(|(at, ..)| at.elapsed() < Duration::from_secs(1));
        if !fresh {
            let (mut total, mut watched, mut unknown) = (0.0, 0.0, 0);
            for path in &self.playlist {
                let entry = self.store.get(path);
                if entry.duration <= 0.0 {
                    unknown += 1;
                    continue;
                }
                total += entry.duration;
                watched += if entry.watched { entry.duration } else { entry.position.clamp(0.0, entry.duration) };
            }
            self.summary = Some((Instant::now(), total, watched, unknown));
        }
        let Some((_, total, watched, unknown)) = self.summary else { return };
        let measuring = unknown > 0 && !self.durations.pending.is_empty();
        let mut text = format!("{} of {} watched", format_total(watched), format_total(total));
        if measuring {
            text.push_str("  ·  measuring…");
        } else if unknown > 0 {
            text.push_str(&format!("  ·  {unknown} unknown"));
        }
        let tip = if unknown > 0 && !measuring {
            format!("The length of {unknown} video(s) couldn't be read; they aren't counted.")
        } else {
            format!("{} left to watch", format_total(total - watched))
        };
        ui.label(RichText::new(text).weak()).on_hover_text(tip);
        let fraction = if total > 0.0 { (watched / total) as f32 } else { 0.0 };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.inactive.bg_fill);
        let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
        gradient::fill(ui.painter(), filled, 2.0, rect, gradient::accent(ui.visuals()), gradient::Direction::Across);
        ui.add_space(2.0);
    }
}
