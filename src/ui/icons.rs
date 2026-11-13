//! Fonts embedded and registered with egui: the Phosphor icon font
//! (<https://phosphoricons.com>), whose regular icons render inline in normal
//! text (use [`fill`] for the solid variant), and Ubuntu Medium for bold text
//! (see [`Bold`]).

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
pub const RESET: &str = "\u{E038}";
pub const MORE: &str = "\u{E1FE}";
pub const WARNING: &str = "\u{E4E0}";
pub const SEARCH: &str = "\u{E30C}";
pub const TIMER: &str = "\u{E492}";
pub const REFRESH: &str = "\u{E036}";
pub const RECENT: &str = "\u{E1A0}";
pub const FOLDER: &str = "\u{E24A}";
pub const KEYBOARD: &str = "\u{E2D8}";
pub const GEAR: &str = "\u{E272}";
pub const PIN: &str = "\u{E3E2}";
pub const GAUGE: &str = "\u{E628}";
pub const CHART: &str = "\u{E150}";
pub const REWIND: &str = "\u{E6A8}";
pub const REPEAT: &str = "\u{E3F6}";
pub const EYE_SLASH: &str = "\u{E224}";
pub const BROOM: &str = "\u{EC54}";
pub const SELECTION_SLASH: &str = "\u{E69E}";
pub const FILE_PLUS: &str = "\u{E236}";
pub const LIST_NUMBERS: &str = "\u{E2F6}";
pub const COMMAND: &str = "\u{E1C4}";
pub const FILM_STRIP: &str = "\u{E792}";
pub const MOON: &str = "\u{E330}";
pub const FAST_FORWARD: &str = "\u{E6A6}";
pub const SLIDERS: &str = "\u{E434}";
pub const FLAG: &str = "\u{E244}";
pub const FLAG_CHECKERED: &str = "\u{EA38}";
pub const FILM_SLATE: &str = "\u{E8C2}";
pub const MONITOR_PLAY: &str = "\u{E58C}";
pub const CAMERA: &str = "\u{E10E}";
pub const IMAGES: &str = "\u{E836}";
pub const STACK: &str = "\u{E466}";
pub const PICTURE_IN_PICTURE: &str = "\u{E64C}";
pub const HOUSE: &str = "\u{E2C2}";
pub const DOWNLOAD: &str = "\u{E20A}";
pub const SPARKLE: &str = "\u{E6A2}";
// The text font has no arrows; shortcut texts use these code points inline.
pub const ARROW_LEFT: &str = "\u{E058}";
pub const ARROW_RIGHT: &str = "\u{E06C}";
#[cfg(test)]
pub const ARROW_UP: &str = "\u{E08E}";
#[cfg(test)]
pub const ARROW_DOWN: &str = "\u{E03E}";

pub(crate) const FILL_FAMILY: &str = "phosphor-fill";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Phosphor.ttf"))),
    );
    fonts.font_data.insert(
        FILL_FAMILY.into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Phosphor-Fill.ttf"))),
    );
    // Monospace text (key names, times) can contain icons too.
    fonts.families.entry(FontFamily::Monospace).or_default().push("phosphor".into());
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.push("phosphor".into());
    let mut fill = vec![FILL_FAMILY.to_string()];
    fill.extend(proportional.iter().cloned());
    let mut bold = vec![BOLD_FAMILY.to_string()];
    bold.extend(proportional.iter().cloned());
    fonts.families.insert(FontFamily::Name(FILL_FAMILY.into()), fill);
    // The same typeface as the text (egui's Ubuntu Light), in a heavier weight.
    fonts.font_data.insert(
        BOLD_FAMILY.into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Ubuntu-Medium.ttf"))),
    );
    fonts.families.insert(FontFamily::Name(BOLD_FAMILY.into()), bold);
    ctx.set_fonts(fonts);
}

pub(crate) const BOLD_FAMILY: &str = "bold";

/// Bold text, for titles and the values that matter; the text size stays the
/// same everywhere.
pub trait Bold {
    fn bold(self) -> RichText;
}

impl Bold for RichText {
    fn bold(self) -> RichText {
        self.family(FontFamily::Name(BOLD_FAMILY.into())).strong()
    }
}

/// The solid variant of an icon.
pub fn fill(icon: &str) -> RichText {
    RichText::new(icon).family(FontFamily::Name(FILL_FAMILY.into()))
}
