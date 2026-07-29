use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Palette {
    pub canvas: Color,
    pub surface: Color,
    pub elevated: Color,
    pub border: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub secondary: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub selected: Color,
    pub interactive: Color,
}

impl Palette {
    pub const fn dark() -> Self {
        Self {
            canvas: Color::Rgb(0x0B, 0x0F, 0x14),
            surface: Color::Rgb(0x11, 0x18, 0x27),
            elevated: Color::Rgb(0x17, 0x20, 0x33),
            border: Color::Rgb(0x26, 0x32, 0x44),
            text_primary: Color::Rgb(0xD7, 0xE3, 0xF4),
            text_muted: Color::Rgb(0x7F, 0x8E, 0xA3),
            accent: Color::Rgb(0x7D, 0xD3, 0xFC),
            secondary: Color::Rgb(0xC4, 0xB5, 0xFD),
            success: Color::Rgb(0x86, 0xEF, 0xAC),
            warning: Color::Rgb(0xFC, 0xD3, 0x4D),
            danger: Color::Rgb(0xFC, 0xA5, 0xA5),
            selected: Color::Rgb(0x7D, 0xD3, 0xFC),
            interactive: Color::Rgb(0x7D, 0xD3, 0xFC),
        }
    }

    pub const fn light() -> Self {
        Self {
            canvas: Color::Rgb(0xF8, 0xFA, 0xFC),
            surface: Color::Rgb(0xF1, 0xF5, 0xF9),
            elevated: Color::Rgb(0xE2, 0xE8, 0xF0),
            border: Color::Rgb(0x94, 0xA3, 0xB8),
            text_primary: Color::Rgb(0x0F, 0x17, 0x2A),
            text_muted: Color::Rgb(0x64, 0x74, 0x8B),
            accent: Color::Rgb(0x0E, 0x74, 0x9A),
            secondary: Color::Rgb(0x69, 0x3B, 0xAC),
            success: Color::Rgb(0x15, 0x9E, 0x4B),
            warning: Color::Rgb(0xB4, 0x53, 0x09),
            danger: Color::Rgb(0xDC, 0x26, 0x26),
            selected: Color::Rgb(0x0E, 0x74, 0x9A),
            interactive: Color::Rgb(0x0E, 0x74, 0x9A),
        }
    }

    pub const fn monochrome() -> Self {
        Self {
            canvas: Color::Rgb(0x00, 0x00, 0x00),
            surface: Color::Rgb(0x1A, 0x1A, 0x1A),
            elevated: Color::Rgb(0x2E, 0x2E, 0x2E),
            border: Color::Rgb(0x55, 0x55, 0x55),
            text_primary: Color::Rgb(0xFF, 0xFF, 0xFF),
            text_muted: Color::Rgb(0xAA, 0xAA, 0xAA),
            accent: Color::Rgb(0xCC, 0xCC, 0xCC),
            secondary: Color::Rgb(0xBB, 0xBB, 0xBB),
            success: Color::Rgb(0x99, 0x99, 0x99),
            warning: Color::Rgb(0xCC, 0xCC, 0xCC),
            danger: Color::Rgb(0xFF, 0xFF, 0xFF),
            selected: Color::Rgb(0xFF, 0xFF, 0xFF),
            interactive: Color::Rgb(0xCC, 0xCC, 0xCC),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub palette: Palette,
    pub colors_enabled: bool,
    pub unicode_enabled: bool,
}

impl Theme {
    pub fn detect() -> Self {
        let colors_enabled = std::env::var_os("NO_COLOR").is_none();
        let unicode_enabled = std::env::var("TERM").map_or(true, |term| term != "dumb");
        Self {
            palette: Palette::dark(),
            colors_enabled,
            unicode_enabled,
        }
    }

    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dumb_terminal_has_plain_text_fallback() {
        let theme = Theme {
            palette: Palette::dark(),
            colors_enabled: false,
            unicode_enabled: false,
        };
        assert!(!theme.colors_enabled);
        assert!(!theme.unicode_enabled);
    }

    #[test]
    fn dark_palette_has_expected_accent() {
        let palette = Palette::dark();
        assert_eq!(palette.accent, Color::Rgb(0x7D, 0xD3, 0xFC));
    }

    #[test]
    fn light_palette_has_expected_accent() {
        let palette = Palette::light();
        assert_eq!(palette.accent, Color::Rgb(0x0E, 0x74, 0x9A));
    }

    #[test]
    fn monochrome_palette_uses_grayscale() {
        let palette = Palette::monochrome();
        assert_eq!(palette.canvas, Color::Rgb(0x00, 0x00, 0x00));
    }
}
