//! Phosphor icon font (https://phosphoricons.com), embedded and registered
//! with egui. Regular icons render inline in normal text; use [`fill`] for
//! the solid variant.

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, RichText};

pub const PLAY: &str = "\u{E3D0}";
pub const PAUSE: &str = "\u{E39E}";
pub const SKIP_BACK: &str = "\u{E5A4}";
pub const SKIP_FORWARD: &str = "\u{E5A6}";
pub const CORNERS_OUT: &str = "\u{E1D0}";
pub const CORNERS_IN: &str = "\u{E1CE}";
pub const SPEAKER_HIGH: &str = "\u{E44A}";
pub const SPEAKER_LOW: &str = "\u{E44C}";
pub const SPEAKER_X: &str = "\u{E45C}";
pub const SUBTITLES: &str = "\u{E1A8}";
pub const WAVEFORM: &str = "\u{E802}";
pub const SIDEBAR: &str = "\u{EC24}";
pub const FOLDER_PLUS: &str = "\u{E25E}";
pub const FILE_VIDEO: &str = "\u{EA22}";
pub const CHECK: &str = "\u{E182}";
pub const PLUS: &str = "\u{E3D4}";
pub const MINUS: &str = "\u{E32A}";
pub const X: &str = "\u{E4F6}";
pub const TRASH: &str = "\u{E4A6}";

const FILL_FAMILY: &str = "phosphor-fill";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Phosphor.ttf"))),
    );
    fonts.font_data.insert(
        FILL_FAMILY.into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Phosphor-Fill.ttf"))),
    );
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.push("phosphor".into());
    let mut fill = vec![FILL_FAMILY.to_string()];
    fill.extend(proportional.iter().cloned());
    fonts.families.insert(FontFamily::Name(FILL_FAMILY.into()), fill);
    ctx.set_fonts(fonts);
}

/// The solid variant of an icon.
pub fn fill(icon: &str) -> RichText {
    RichText::new(icon).family(FontFamily::Name(FILL_FAMILY.into()))
}
