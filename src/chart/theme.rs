//! Chart colors for the dark and light themes.

use eframe::egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color32,
    pub panel: Color32,
    pub grid: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub crosshair: Color32,
    pub label_bg: Color32,
    pub label_text: Color32,
    pub up: Color32,
    pub down: Color32,
    pub accent: Color32,
    pub warn: Color32,
    pub watermark: Color32,
}

pub const UP: Color32 = Color32::from_rgb(38, 166, 154);
pub const DOWN: Color32 = Color32::from_rgb(239, 83, 80);

/// TradingView's Pine Script colors (`color.green`, …), as the indicators use them.
pub mod pine {
    use eframe::egui::Color32;

    pub const GREEN: Color32 = Color32::from_rgb(0x4C, 0xAF, 0x50);
    pub const BLUE: Color32 = Color32::from_rgb(0x21, 0x96, 0xF3);
    pub const AQUA: Color32 = Color32::from_rgb(0x00, 0xBC, 0xD4);
    pub const RED: Color32 = Color32::from_rgb(0xFF, 0x52, 0x52);
    pub const ORANGE: Color32 = Color32::from_rgb(0xFF, 0x98, 0x00);
    pub const YELLOW: Color32 = Color32::from_rgb(0xFF, 0xEB, 0x3B);
    pub const BLACK: Color32 = Color32::from_rgb(0x36, 0x3A, 0x45);
    pub const SILVER: Color32 = Color32::from_rgb(0xB2, 0xB5, 0xBE);
    pub const TEAL: Color32 = Color32::from_rgb(0x00, 0x89, 0x7B);
    pub const GRAY: Color32 = Color32::from_rgb(0x78, 0x7B, 0x86);
    /// The RSI line of TradingView's built-in RSI.
    pub const RSI: Color32 = Color32::from_rgb(0x7E, 0x57, 0xC2);

    /// `color.new(c, transp)`: transparency in percent. Premultiplied in gamma
    /// space, so blending gives what TradingView shows (`bg * (1 - a) + c * a`
    /// on sRGB values). `Color32::from_rgba_unmultiplied` premultiplies in
    /// linear space, which makes a 90 %-transparent color look about twice as
    /// strong on a dark background.
    pub fn transp(c: Color32, transp: u8) -> Color32 {
        let a = 1.0 - f32::from(transp.min(100)) / 100.0;
        let m = |v: u8| (f32::from(v) * a).round() as u8;
        Color32::from_rgba_premultiplied(m(c.r()), m(c.g()), m(c.b()), (255.0 * a).round() as u8)
    }
}

impl Theme {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                bg: Color32::from_rgb(19, 23, 34),
                panel: Color32::from_rgb(24, 28, 39),
                grid: Color32::from_rgb(32, 36, 48),
                border: Color32::from_rgb(54, 58, 69),
                text: Color32::from_rgb(209, 212, 220),
                muted: Color32::from_rgb(134, 138, 150),
                crosshair: Color32::from_rgb(120, 123, 134),
                label_bg: Color32::from_rgb(54, 58, 69),
                label_text: Color32::from_rgb(230, 232, 238),
                up: UP,
                down: DOWN,
                accent: Color32::from_rgb(41, 98, 255),
                warn: Color32::from_rgb(242, 190, 80),
                watermark: Color32::from_rgba_unmultiplied(209, 212, 220, 6),
            }
        } else {
            Self {
                bg: Color32::WHITE,
                panel: Color32::from_rgb(248, 249, 253),
                grid: Color32::from_rgb(240, 243, 250),
                border: Color32::from_rgb(224, 227, 235),
                text: Color32::from_rgb(19, 23, 34),
                muted: Color32::from_rgb(106, 109, 120),
                crosshair: Color32::from_rgb(149, 152, 161),
                label_bg: Color32::from_rgb(19, 23, 34),
                label_text: Color32::WHITE,
                up: UP,
                down: DOWN,
                accent: Color32::from_rgb(41, 98, 255),
                warn: Color32::from_rgb(196, 120, 0),
                watermark: Color32::from_rgba_unmultiplied(19, 23, 34, 16),
            }
        }
    }
}
