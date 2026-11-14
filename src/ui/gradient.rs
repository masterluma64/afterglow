//! Gradients in the accent color, running from the accent to a warmer,
//! lighter glow, like the sunset in the app icon.

use eframe::egui::{self, Color32, CornerRadius, Rect, ecolor::Hsva, epaint};

/// How far the glow's hue is turned from the accent's: towards red, orange
/// and yellow, the way a sky goes from violet to the horizon at sunset.
const GLOW_HUE: f32 = 40.0 / 360.0;
/// How much brighter the glow is than the accent.
const GLOW_VALUE: f32 = 1.15;

/// Which way a gradient runs.
#[derive(Clone, Copy)]
pub(crate) enum Direction {
    /// From the left (the accent) to the right (the glow).
    Across,
    /// From the bottom (the accent) up to the top (the glow).
    Up,
}

/// The two ends of a gradient in `accent`: the accent and its glow.
pub(crate) fn glow(accent: Color32) -> [Color32; 2] {
    let mut warm = Hsva::from(accent);
    warm.h = (warm.h + GLOW_HUE).fract();
    warm.v = (warm.v * GLOW_VALUE).min(1.0);
    [accent, Color32::from(warm)]
}

/// The gradient in the visuals' accent color.
pub(crate) fn accent(visuals: &egui::Visuals) -> [Color32; 2] {
    glow(visuals.selection.bg_fill)
}

/// The color `t` (0 to 1) of the way along a gradient.
pub(crate) fn at(colors: [Color32; 2], t: f32) -> Color32 {
    colors[0].lerp_to_gamma(colors[1], t.clamp(0.0, 1.0))
}

/// Fills `rect`, with rounded corners, with a gradient that runs over
/// `span`: usually `rect` itself, but for a bar filled part of the way it is
/// the whole bar, so each point of it keeps its color as it fills up.
pub(crate) fn fill(painter: &egui::Painter, rect: Rect, corner_radius: impl Into<CornerRadius>, span: Rect, colors: [Color32; 2], direction: Direction) {
    if let Some(shape) = shape(painter.ctx(), rect, corner_radius.into(), span, colors, direction) {
        painter.add(shape);
    }
}

/// Fills `rect` with a gradient across it, in a place kept earlier with
/// `painter.add(Shape::Noop)`, so it goes under what was painted since.
pub(crate) fn fill_slot(painter: &egui::Painter, slot: egui::layers::ShapeIdx, rect: Rect, corner_radius: impl Into<CornerRadius>, colors: [Color32; 2]) {
    if let Some(shape) = shape(painter.ctx(), rect, corner_radius.into(), rect, colors, Direction::Across) {
        painter.set(slot, shape);
    }
}

/// The shape `fill` paints.
fn shape(ctx: &egui::Context, rect: Rect, corner_radius: CornerRadius, span: Rect, colors: [Color32; 2], direction: Direction) -> Option<egui::Shape> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    // The rounded rectangle as egui would draw it in white, recolored point
    // by point; its anti-aliased edge stays faded.
    let options = ctx.tessellation_options(|o| *o);
    let mut tessellator = epaint::Tessellator::new(ctx.pixels_per_point(), options, [1, 1], Vec::new());
    let mut mesh = epaint::Mesh::default();
    tessellator.tessellate_rect(&epaint::RectShape::filled(rect, corner_radius, Color32::WHITE), &mut mesh);
    for vertex in &mut mesh.vertices {
        let t = match direction {
            Direction::Across => (vertex.pos.x - span.left()) / span.width().max(1.0),
            Direction::Up => (span.bottom() - vertex.pos.y) / span.height().max(1.0),
        };
        let coverage = f32::from(vertex.color.a()) / 255.0;
        vertex.color = at(colors, t).gamma_multiply(coverage);
    }
    Some(egui::Shape::mesh(mesh))
}

/// A glow rising from the bottom of `rect`, for the dark video area while
/// nothing plays: an afterglow over the horizon.
pub(crate) fn horizon(painter: &egui::Painter, rect: Rect, accent: Color32) {
    let [_, warm] = glow(accent);
    let glow = Rect::from_min_max(egui::pos2(rect.left(), rect.top() + rect.height() * 0.4), rect.max);
    fill(painter, glow, 0, glow, [warm.gamma_multiply(0.14), Color32::TRANSPARENT], Direction::Up);
}

/// A button in a gradient of the accent color, for the main button of a
/// dialog. Its text takes the color that reads on the accent.
pub(crate) fn button(ui: &mut egui::Ui, text: impl Into<egui::RichText>) -> egui::Response {
    let selection = ui.visuals().selection;
    // Painted under the button once its size is known.
    let background = ui.painter().add(egui::Shape::Noop);
    let resp = ui.add(egui::Button::new(text.into().color(selection.stroke.color)).fill(Color32::TRANSPARENT));
    let colors = accent(ui.visuals());
    // Lighter on hover, as other buttons get.
    let colors = if resp.hovered() { colors.map(|c| c.gamma_multiply(1.2)) } else { colors };
    fill_slot(ui.painter(), background, resp.rect, ui.visuals().widgets.inactive.corner_radius, colors);
    resp
}

/// A selectable button (`egui::Button::selectable(selected, …)`) that is
/// filled with the accent gradient while selected, like the main buttons,
/// in place of egui's flat accent.
pub(crate) fn add_selectable(ui: &mut egui::Ui, selected: bool, button: egui::Button<'_>) -> egui::Response {
    // Painted under the button once its size is known.
    let background = ui.painter().add(egui::Shape::Noop);
    let resp = ui
        .scope(|ui| {
            ui.visuals_mut().selection.bg_fill = Color32::TRANSPARENT;
            ui.add(button)
        })
        .inner;
    if selected {
        let colors = accent(ui.visuals());
        let colors = if resp.hovered() { colors.map(|c| c.gamma_multiply(1.2)) } else { colors };
        fill_slot(ui.painter(), background, resp.rect, ui.visuals().widgets.inactive.corner_radius, colors);
    }
    resp
}

/// `ui.selectable_label`, filled with the accent gradient while selected.
pub(crate) fn selectable(ui: &mut egui::Ui, selected: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    add_selectable(ui, selected, egui::Button::selectable(selected, text))
}

/// `ui.selectable_value`, filled with the accent gradient while selected.
pub(crate) fn selectable_value<T: PartialEq>(ui: &mut egui::Ui, current: &mut T, value: T, text: impl Into<egui::WidgetText>) -> egui::Response {
    let mut resp = selectable(ui, *current == value, text);
    if resp.clicked() && *current != value {
        *current = value;
        resp.mark_changed();
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glows_warmer_than_the_accent() {
        // Purple goes towards pink, as in the icon's sunset.
        let [start, end] = glow(Color32::from_rgb(140, 95, 225));
        assert_eq!(start, Color32::from_rgb(140, 95, 225));
        let warmth = |c: Color32| f32::from(c.r()) / f32::from(c.b());
        assert!(warmth(end) > warmth(start) * 1.2, "{end:?}");
        assert_eq!(at([start, end], 0.0), start);
        assert_eq!(at([start, end], 1.0), end);
    }
}
