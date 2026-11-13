//! The window's contents: controls, sidebar, menus, dialogs and the video area.

pub(crate) mod app_icon;
pub(crate) mod commands;
pub(crate) mod controls;
pub(crate) mod dialogs;
pub(crate) mod folder;
pub(crate) mod home;
pub(crate) mod icons;
pub(crate) mod items;
pub(crate) mod mini;
pub(crate) mod sidebar;
pub(crate) mod video;

use eframe::egui;

/// egui's menu style, with roomier items than its default.
fn menu_style(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
}

/// A button that opens a menu in our style; its submenus follow it.
pub(crate) fn menu_button(button: egui::Button<'_>) -> egui::containers::menu::MenuButton<'_> {
    egui::containers::menu::MenuButton::from_button(button).config(egui::containers::menu::MenuConfig::new().style(menu_style))
}

/// A right-click menu in our style.
pub(crate) fn context_menu(response: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Popup::context_menu(response).style(menu_style).show(|ui| fit_to_window(ui, None, add_contents));
}

/// A menu's contents under its `title`, if it has one, scrolling once they
/// are taller than the window, so in a small window the last items can still
/// be reached. The title stays put.
pub(crate) fn fit_to_window(ui: &mut egui::Ui, title: Option<&str>, add_contents: impl FnOnce(&mut egui::Ui)) {
    // Room for the menu's frame and a gap to the window's edges.
    let max_height = (ui.ctx().content_rect().height() - 40.0).max(80.0);
    // A menu starts out at egui's default area size (400 points high) and
    // would keep scrolling at that height; let it grow to the window's. This
    // puts egui's cursor back at the menu's top, so it comes before the title.
    ui.set_max_height(max_height);
    let top = ui.cursor().top();
    if let Some(title) = title {
        ui.label(icons::Bold::bold(egui::RichText::new(title)));
    }
    let above = ui.cursor().top() - top;
    egui::ScrollArea::vertical().max_height((max_height - above).max(40.0)).show(ui, add_contents);
}

/// Secondary text, dimmed, except on a selected (accent-colored) background:
/// dimmed text is hard to read there, so it takes the selection's text color.
pub(crate) fn secondary(ui: &egui::Ui, on_accent: bool, text: impl Into<egui::RichText>) -> egui::RichText {
    let text = text.into();
    if on_accent { text.color(ui.visuals().selection.stroke.color) } else { text.weak() }
}

/// Adds a text field, outlined in the accent color while it has focus. egui
/// would outline it in the selection's text color, which is black or white
/// and doesn't follow the accent (and can't be changed without also changing
/// the color of selected text). `margin` is the padding around the text.
pub(crate) fn text_field(ui: &mut egui::Ui, edit: egui::TextEdit<'_>, id: egui::Id, margin: egui::Margin) -> egui::Response {
    let visuals = ui.visuals();
    let focused = ui.memory(|m| m.has_focus(id));
    // Always a 1 px outline, invisible without focus, so nothing shifts.
    let outline = if focused { visuals.selection.bg_fill } else { egui::Color32::TRANSPARENT };
    let frame = egui::Frame::new()
        .fill(visuals.text_edit_bg_color())
        .corner_radius(visuals.widgets.inactive.corner_radius)
        .stroke((1.0, outline))
        .inner_margin(margin - egui::Margin::same(1));
    ui.add(edit.id(id).frame(frame))
}

/// Text laid out on one line of at most `width`, cut off with … when longer,
/// for painting where a label can't go.
pub(crate) fn one_line(ui: &egui::Ui, text: String, font: egui::FontId, color: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text, egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    ui.fonts_mut(|f| f.layout_job(job))
}

/// A submenu whose contents scroll once they are taller than the window
/// (see `fit_to_window`).
pub(crate) fn submenu(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.menu_button(label, |ui| fit_to_window(ui, None, add_contents));
}
