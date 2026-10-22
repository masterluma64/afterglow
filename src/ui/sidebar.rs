//! The playlist sidebar: docked, or sliding out over the video.

use crate::*;

/// A playlist entry with its watch progress. `selection` is every selected
/// video, in playlist order, when this one is among them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn playlist_row(
    ui: &mut egui::Ui,
    store: &Store,
    actions: &mut Vec<Action>,
    index: usize,
    path: &Path,
    name: &str,
    playing: bool,
    selection: Option<&[PathBuf]>,
) -> egui::Response {
    let entry = store.get(path);
    // Painted under the row once its size is known.
    let background = ui.painter().add(egui::Shape::Noop);
    // Dimmed text is hard to read on the accent-colored background of the
    // playing row, so there it uses the selection's text color.
    let dim = |text: RichText| if playing { text.color(ui.visuals().selection.stroke.color) } else { text.weak() };
    let text = if entry.watched { dim(RichText::new(format!("{}  {name}", icons::CHECK))) } else { RichText::new(name) };
    let length = if entry.duration > 0.0 { format_time(entry.duration) } else { String::new() };
    let button = egui::Button::selectable(playing, "")
        .left_text(text)
        .right_text(dim(RichText::new(length)))
        // One line per entry (cut off with …), so rows have a fixed height.
        .truncate()
        .sense(Sense::click_and_drag());
    let resp = ui.add_sized([ui.available_width(), PLAYLIST_ROW_HEIGHT], button);
    if selection.is_some() {
        if playing {
            // Already accent-colored: an outline shows it is selected too.
            ui.painter().rect_stroke(resp.rect.shrink(1.0), 4.0, (1.5, ui.visuals().selection.stroke.color), egui::StrokeKind::Inside);
        } else {
            let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.35);
            ui.painter().set(background, egui::Shape::rect_filled(resp.rect, 4.0, fill));
        }
    }
    if (!entry.watched && entry.fraction() > 0.0) || store.resume_position(path).is_some() {
        let r = resp.rect.shrink2(vec2(4.0, 1.0));
        let end = egui::pos2(r.left() + r.width() * entry.fraction(), r.bottom());
        ui.painter().line_segment([r.left_bottom(), end], (2.0, ui.visuals().selection.bg_fill));
    }
    let resp = resp.on_hover_text(format!("{}\n{}", path.display(), hover_text(&entry)));
    ui::context_menu(&resp, |ui| {
        if let Some(selection) = selection.filter(|s| s.len() > 1) {
            let videos = format!("{} videos", selection.len());
            ui.label(RichText::new(format!("{videos} selected")).small().weak());
            if ui.button(format!("{}  Mark as watched", icons::CHECK)).clicked() {
                actions.push(Action::SetWatchedMany(selection.to_vec(), true));
            }
            if ui.button(format!("{}  Mark as unwatched", icons::RESET)).on_hover_text("Reset their progress to the start").clicked() {
                actions.push(Action::SetWatchedMany(selection.to_vec(), false));
            }
            ui.separator();
            let remove = egui::Button::new(format!("{}  Remove from playlist", icons::X)).shortcut_text("Delete");
            if ui.add(remove).on_hover_text("The files are not touched").clicked() {
                actions.push(Action::RemoveMany(selection.to_vec()));
            }
            ui.separator();
            if ui.add(egui::Button::new("Clear the selection").shortcut_text("Esc")).clicked() {
                actions.push(Action::ClearSelection);
            }
            return;
        }
        if ui.button(format!("{}  Play from start", icons::PLAY)).clicked() {
            actions.push(Action::SetWatched(path.to_path_buf(), false));
            actions.push(Action::Play(index));
        }
        if !entry.watched && ui.button(format!("{}  Mark as watched", icons::CHECK)).clicked() {
            actions.push(Action::SetWatched(path.to_path_buf(), true));
        }
        if index > 0 && ui.button(format!("{}  Mark all above as watched", icons::CHECKS)).clicked() {
            actions.push(Action::MarkWatchedBefore(index));
        }
        // Also offered for half-watched videos: resets progress without playing.
        if entry.watched || entry.position > 0.0 {
            let reset = ui.button(format!("{}  Mark as unwatched", icons::RESET)).on_hover_text("Reset its progress to the start");
            if reset.clicked() {
                actions.push(Action::SetWatched(path.to_path_buf(), false));
            }
        }
        ui.separator();
        if ui.button(format!("{}  Remove from playlist", icons::X)).on_hover_text("The file is not touched").clicked() {
            actions.push(Action::Remove(index));
        }
        ui.separator();
        subfolder_option(ui, store.include_subfolders, actions);
    });
    resp
}

/// The "Include subfolders" switch in the playlist's right-click menus.
pub(crate) fn subfolder_option(ui: &mut egui::Ui, include: bool, actions: &mut Vec<Action>) {
    let mut include = include;
    let tip = "Also play videos in subfolders when opening a folder (rescans the opened folder now)";
    if ui.checkbox(&mut include, "Include subfolders").on_hover_text(tip).changed() {
        actions.push(Action::SetIncludeSubfolders(include));
        ui.close();
    }
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
    /// Whether the playlist sits beside the video. Unpinned (and in fullscreen)
    /// it slides out over the video instead.
    pub(crate) fn sidebar_docked(&self) -> bool {
        self.store.sidebar_pinned && !self.fullscreen
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
        if self.peek.is_none() && at_edge && !ctx.input(|i| i.pointer.any_down()) {
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
        let include = self.store.include_subfolders;
        ui::context_menu(&background, |ui| subfolder_option(ui, include, &mut self.actions));
    }

    pub(crate) fn sidebar_contents(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("PLAYLIST").small().weak());
            let count = self.playlist.len();
            if count > 0 {
                ui.label(RichText::new(format!("{count} video{}", if count == 1 { "" } else { "s" })).small().weak());
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
                    if ui.add(clear).on_hover_text("Clear the playlist (files are not touched)").clicked() {
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
        // Indices of the entries to list, filtered by the search.
        let visible: Vec<usize> =
            (0..self.playlist.len()).filter(|&i| query.is_empty() || self.names[i].1.contains(&query)).collect();
        self.search_box(ui, &visible);
        if visible.is_empty() {
            ui.add_space(4.0);
            ui.weak(format!("No videos match \"{}\".", self.search.trim()));
            return;
        }
        let picked = (!query.is_empty()).then(|| visible.get(self.search_pick).copied()).flatten();
        let mut scroll = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(pick) = self.scroll_to_pick.take() {
            // Keep the picked result in the middle of the list.
            let row = PLAYLIST_ROW_HEIGHT + ui.spacing().item_spacing.y;
            let offset = pick as f32 * row - (ui.available_height() - PLAYLIST_ROW_HEIGHT) / 2.0;
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        let selection = self.selected_paths();
        if selection.len() != self.selected.len() {
            // Some selected videos left the playlist.
            self.selected = selection.iter().cloned().collect();
        }
        // Only the rows in view are laid out.
        scroll.show_rows(ui, PLAYLIST_ROW_HEIGHT, visible.len(), |ui, range| {
            let mut rows = Vec::new();
            for &index in &visible[range] {
                let path = &self.playlist[index];
                let name = &self.names[index].0;
                let selected = self.selected.contains(path).then_some(selection.as_slice());
                let resp = playlist_row(ui, &self.store, &mut self.actions, index, path, name, self.current == Some(index), selected);
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
        self.names = self
            .playlist
            .iter()
            .map(|path| {
                // Videos from a subfolder of the opened folder show where they're from.
                let subfolder = folder.and_then(|f| path.parent()?.strip_prefix(f).ok());
                let name = match subfolder {
                    Some(sub) if !sub.as_os_str().is_empty() => {
                        format!("{} / {}", sub.to_string_lossy().replace(std::path::MAIN_SEPARATOR, " / "), display_name(path))
                    }
                    _ => display_name(path),
                };
                let lower = name.to_lowercase();
                (name, lower)
            })
            .collect();
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
            let width = ui.available_width() - if self.search.is_empty() { 0.0 } else { 24.0 };
            let edit = egui::TextEdit::singleline(&mut self.search)
                .margin(TEXT_MARGIN)
                .id(id)
                .hint_text(format!("{}  Search the playlist (/)", icons::SEARCH))
                .desired_width(width);
            let resp = ui.add(edit);
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
        ui.painter().rect_filled(filled, 2.0, ui.visuals().selection.bg_fill);
        ui.add_space(2.0);
    }
}
