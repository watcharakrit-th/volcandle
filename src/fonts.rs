//! Fonts: egui's bundled fonts plus, when one can be found, a system font with
//! Thai glyphs (for Thai file names, symbols and column headers).

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Tried in order; `VOLCANDLE_FONT` overrides them all.
const THAI_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/tlwg/Loma.ttf",
    "/usr/share/fonts/truetype/tlwg/Garuda.ttf",
    "/usr/share/fonts/opentype/tlwg/Loma.otf",
    "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansThai-Regular.ttf",
    "C:\\Windows\\Fonts\\LeelawUI.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    "/System/Library/Fonts/Supplemental/Tahoma.ttf",
    "/Library/Fonts/Tahoma.ttf",
];

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // Arrows and some symbols only exist in the bundled monospace font.
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.push("Hack".to_owned());
    }
    let custom = std::env::var("VOLCANDLE_FONT").ok();
    let candidates = custom
        .iter()
        .map(String::as_str)
        .chain(THAI_FONTS.iter().copied());
    if let Some(bytes) = candidates.filter_map(|p| std::fs::read(p).ok()).next() {
        fonts
            .font_data
            .insert("thai".to_owned(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push("thai".to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}
