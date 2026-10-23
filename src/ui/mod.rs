//! The window's contents: controls, sidebar, menus, dialogs and the video area.

pub(crate) mod app_icon;
pub(crate) mod commands;
pub(crate) mod controls;
pub(crate) mod dialogs;
pub(crate) mod icons;
pub(crate) mod items;
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
    egui::Popup::context_menu(response).style(menu_style).show(add_contents);
}

/// Secondary text, dimmed, except on a selected (accent-colored) background:
/// dimmed text is hard to read there, so it takes the selection's text color.
pub(crate) fn secondary(ui: &egui::Ui, on_accent: bool, text: impl Into<egui::RichText>) -> egui::RichText {
    let text = text.into();
    if on_accent { text.color(ui.visuals().selection.stroke.color) } else { text.weak() }
}
