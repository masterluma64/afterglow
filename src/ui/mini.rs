//! The mini player: a small borderless window kept on top of the others,
//! showing just the video, with its controls over it while the pointer is
//! on it. Dragging the video moves it; dragging its edges resizes it.

use crate::*;

/// The mini player's size the first time, a 16:9 video.
const MINI_SIZE: egui::Vec2 = vec2(480.0, 270.0);
const MINI_MIN_SIZE: egui::Vec2 = vec2(320.0, 180.0);
/// The gap between the mini player and the edges of the screen.
const MINI_MARGIN: f32 = 24.0;
/// How close to the window's edge dragging resizes it, in points.
const RESIZE_EDGE: f32 = 6.0;

/// How the window was before the mini player, to put it back.
pub(crate) struct Restore {
    outer: Option<egui::Pos2>,
    inner: Option<egui::Vec2>,
    maximized: bool,
    fullscreen: bool,
    show_sidebar: bool,
}

impl App {
    pub(crate) fn mini(&self) -> bool {
        self.mini.is_some()
    }

    /// Whether the controls float over the video, rather than sitting in a
    /// bar under it.
    pub(crate) fn controls_overlay(&self) -> bool {
        self.fullscreen || self.mini()
    }

    pub(crate) fn enter_mini(&mut self, ctx: &egui::Context) {
        if self.mini() {
            return;
        }
        let (outer, inner, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect.map(|r| r.min), v.inner_rect.map(|r| r.size()), v.maximized.unwrap_or(false))
        });
        self.mini = Some(Restore { outer, inner, maximized, fullscreen: self.fullscreen, show_sidebar: self.show_sidebar });
        if self.fullscreen {
            self.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
        }
        let (pos, size) = self.mini_placement(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MINI_MIN_SIZE));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if let Some(pos) = pos {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        self.peek = None;
        self.last_activity = Instant::now();
    }

    /// Puts the window back the way it was before the mini player.
    pub(crate) fn exit_mini(&mut self, ctx: &egui::Context) {
        let Some(restore) = self.mini.take() else { return };
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MIN_WINDOW_SIZE.into()));
        if let Some(inner) = restore.inner {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(inner));
        }
        if let Some(outer) = restore.outer {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(outer));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(window_level(self.store.on_top)));
        if restore.maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        if restore.fullscreen {
            self.fullscreen = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        self.show_sidebar = restore.show_sidebar;
        self.last_activity = Instant::now();
    }

    /// Where the mini player goes and how big it is: where it was last time,
    /// or else in the bottom-right corner of the screen the window is on.
    fn mini_placement(&self, ctx: &egui::Context) -> (Option<egui::Pos2>, egui::Vec2) {
        if let Some(last) = self.store.mini_window.filter(screen::title_bar_visible) {
            return (Some(egui::pos2(last.x, last.y)), vec2(last.width, last.height));
        }
        let scale = ctx.pixels_per_point();
        let corner = self.hwnd.and_then(screen::work_area).map(|[_, _, right, bottom]| {
            egui::pos2(right as f32 / scale, bottom as f32 / scale) - MINI_SIZE - vec2(MINI_MARGIN, MINI_MARGIN)
        });
        (corner, MINI_SIZE)
    }

    /// Lets the borderless mini player be resized by dragging its edges,
    /// showing a resize cursor near them. Call after the window's contents,
    /// so the cursor wins over theirs.
    pub(crate) fn mini_resize(&self, ctx: &egui::Context, window: egui::Rect) {
        let Some(pointer) = ctx.pointer_hover_pos() else { return };
        let near = |a: f32, b: f32| (a - b).abs() <= RESIZE_EDGE;
        let (left, right) = (near(pointer.x, window.left()), near(pointer.x, window.right()));
        let (top, bottom) = (near(pointer.y, window.top()), near(pointer.y, window.bottom()));
        use egui::{CursorIcon as C, ResizeDirection as D};
        let (direction, cursor) = match (left, right, top, bottom) {
            (true, _, true, _) => (D::NorthWest, C::ResizeNwSe),
            (_, true, _, true) => (D::SouthEast, C::ResizeNwSe),
            (_, true, true, _) => (D::NorthEast, C::ResizeNeSw),
            (true, _, _, true) => (D::SouthWest, C::ResizeNeSw),
            (true, ..) => (D::West, C::ResizeHorizontal),
            (_, true, ..) => (D::East, C::ResizeHorizontal),
            (_, _, true, _) => (D::North, C::ResizeVertical),
            (.., true) => (D::South, C::ResizeVertical),
            _ => return,
        };
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }

    /// Keeps where the mini player was put, to open it there next time.
    pub(crate) fn remember_mini(&mut self, ctx: &egui::Context) {
        let (outer, inner, maximized, scale) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized, i.pixels_per_point)
        });
        // Dragged against the top of the screen, Windows maximizes it, which
        // no mini player wants: put it back.
        if maximized == Some(true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            return;
        }
        let (Some(outer), Some(inner)) = (outer, inner) else { return };
        let window = progress::WindowPlacement {
            x: outer.min.x,
            y: outer.min.y,
            width: inner.width(),
            height: inner.height(),
            maximized: false,
            scale,
        };
        self.store.set_mini_window(window);
    }
}
