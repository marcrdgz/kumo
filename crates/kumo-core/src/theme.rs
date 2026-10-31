//! Theme definitions: every color kumo renders, from the terminal emulator's
//! ANSI palette down to the chrome (status bar, sidebar, popups). The active
//! The active theme lives on the daemon's `App` and can be swapped live from the
//! status-bar Settings popup; switching re-applies the terminal defaults to
//! every existing pane.

use ratatui::style::Color as RColor;

use crate::color::{ColorRgb, parse_hex};

/// Shared neutral surface for text inputs in the built-in dark themes.
const INPUT_SURFACE: RColor = RColor::Rgb(0xc6, 0xca, 0xd2);

/// A complete color scheme: ANSI palette + terminal defaults + chrome colors.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Theme {
    pub name: &'static str,
    /// ANSI 16-color palette fed to the terminal emulator.
    pub palette: [ColorRgb; 16],
    /// Terminal default foreground/background/cursor.
    pub term_fg: ColorRgb,
    pub term_bg: ColorRgb,
    pub term_cursor: ColorRgb,
    /// Chrome colors (sidebars, status bar, chrome borders).
    pub fg: RColor,
    /// Primary accent: focused pane, selected session, MENU highlights.
    pub accent: RColor,
    /// Secondary accent: scrollbars and other structure.
    pub secondary: RColor,
    /// Surface of chrome panels.
    pub panel_sep: RColor,
    /// Muted text and idle borders.
    pub panel_muted: RColor,
    pub border_idle: RColor,
    pub green: RColor,
    pub orange: RColor,
    pub red: RColor,
    /// Light background of popup text inputs.
    pub input_bg: RColor,
}

/// Owned variant of `Theme` used for custom themes defined in `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedTheme {
    pub name: String,
    pub palette: [ColorRgb; 16],
    pub term_fg: ColorRgb,
    pub term_bg: ColorRgb,
    pub term_cursor: ColorRgb,
    pub fg: RColor,
    pub accent: RColor,
    pub secondary: RColor,
    pub panel_sep: RColor,
    pub panel_muted: RColor,
    pub border_idle: RColor,
    pub green: RColor,
    pub orange: RColor,
    pub red: RColor,
    pub input_bg: RColor,
}

impl From<Theme> for OwnedTheme {
    fn from(t: Theme) -> Self {
        Self {
            name: t.name.to_string(),
            palette: t.palette,
            term_fg: t.term_fg,
            term_bg: t.term_bg,
            term_cursor: t.term_cursor,
            fg: t.fg,
            accent: t.accent,
            secondary: t.secondary,
            panel_sep: t.panel_sep,
            panel_muted: t.panel_muted,
            border_idle: t.border_idle,
            green: t.green,
            orange: t.orange,
            red: t.red,
            input_bg: t.input_bg,
        }
    }
}

impl OwnedTheme {
    /// Light themes paint their own chrome background: the host terminal's
    /// default (`Reset`) is usually dark and would swallow their dark text.
    pub fn is_light(&self) -> bool {
        self.term_bg.relative_luminance() >= 0.5
    }

    /// Convert to a `Theme` with a leaked name. Leaks the string once per custom
    /// theme — negligible for the daemon's lifetime and keeps the existing
    /// `Theme`-based FFI unchanged.
    pub fn as_static(&self) -> Theme {
        Theme {
            name: Box::leak(self.name.clone().into_boxed_str()),
            palette: self.palette,
            term_fg: self.term_fg,
            term_bg: self.term_bg,
            term_cursor: self.term_cursor,
            fg: self.fg,
            accent: self.accent,
            secondary: self.secondary,
            panel_sep: self.panel_sep,
            panel_muted: self.panel_muted,
            border_idle: self.border_idle,
            green: self.green,
            orange: self.orange,
            red: self.red,
            input_bg: self.input_bg,
        }
    }
}

/// All selectable built-in themes, in Settings-popup order. Custom (if defined
/// in `~/.config/kumo/config.toml`) appears last as an extra entry.
pub const THEMES: [Theme; 11] = [
    Theme {
        name: "Catppuccin Mocha",
        palette: [
            ColorRgb::new(0x45, 0x47, 0x5a), // Black
            ColorRgb::new(0xf3, 0x8b, 0xa8), // Red
            ColorRgb::new(0xa6, 0xe3, 0xa1), // Green
            ColorRgb::new(0xf9, 0xe2, 0xaf), // Yellow
            ColorRgb::new(0x89, 0xb4, 0xfa), // Blue
            ColorRgb::new(0xf5, 0xc2, 0xe7), // Magenta
            ColorRgb::new(0x94, 0xe2, 0xd5), // Cyan
            ColorRgb::new(0xba, 0xc2, 0xde), // White
            ColorRgb::new(0x58, 0x5b, 0x70), // BrightBlack
            ColorRgb::new(0xf3, 0x8b, 0xa8), // BrightRed
            ColorRgb::new(0xa6, 0xe3, 0xa1), // BrightGreen
            ColorRgb::new(0xf9, 0xe2, 0xaf), // BrightYellow
            ColorRgb::new(0x89, 0xb4, 0xfa), // BrightBlue
            ColorRgb::new(0xf5, 0xc2, 0xe7), // BrightMagenta
            ColorRgb::new(0x94, 0xe2, 0xd5), // BrightCyan
            ColorRgb::new(0xcd, 0xd6, 0xf4), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xcd, 0xd6, 0xf4),
        term_bg: ColorRgb::new(0x1e, 0x1e, 0x2e),
        term_cursor: ColorRgb::new(0xb4, 0xbe, 0xfe),
        fg: RColor::Rgb(0xcd, 0xd6, 0xf4),
        accent: RColor::Rgb(0x5e, 0x9e, 0xff),
        secondary: RColor::Rgb(0x89, 0xb4, 0xfa),
        panel_sep: RColor::Rgb(0x17, 0x18, 0x26),
        panel_muted: RColor::Rgb(0x6c, 0x70, 0x86),
        border_idle: RColor::Rgb(0x6c, 0x70, 0x86),
        green: RColor::Rgb(0xa6, 0xe3, 0xa1),
        orange: RColor::Rgb(0xfa, 0xb3, 0x87),
        red: RColor::Rgb(0xf3, 0x8b, 0xa8),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Spider-Verse",
        palette: [
            ColorRgb::new(0x16, 0x16, 0x22), // Black       (card surface)
            ColorRgb::new(0xef, 0x39, 0x45), // Red         (spider red)
            ColorRgb::new(0x2e, 0xe0, 0x6b), // Green       (neon green)
            ColorRgb::new(0xff, 0xd1, 0x66), // Yellow      (amber)
            ColorRgb::new(0x4a, 0x7b, 0xff), // Blue        (electric blue)
            ColorRgb::new(0xf5, 0x47, 0xaa), // Magenta     (Gwen magenta)
            ColorRgb::new(0x0a, 0xba, 0xff), // Cyan        (electric cyan)
            ColorRgb::new(0xed, 0xed, 0xf3), // White       (comic ink highlight)
            ColorRgb::new(0x28, 0x28, 0x40), // BrightBlack (surface ramp)
            ColorRgb::new(0xf3, 0x68, 0x71), // BrightRed
            ColorRgb::new(0x5b, 0xf0, 0xa0), // BrightGreen
            ColorRgb::new(0xff, 0xe0, 0x8a), // BrightYellow
            ColorRgb::new(0x7e, 0xa6, 0xff), // BrightBlue
            ColorRgb::new(0xf8, 0x77, 0xc0), // BrightMagenta
            ColorRgb::new(0x3d, 0xc8, 0xff), // BrightCyan
            ColorRgb::new(0xff, 0xff, 0xff), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xed, 0xed, 0xf3),
        term_bg: ColorRgb::new(0x0d, 0x0d, 0x17),
        term_cursor: ColorRgb::new(0x0a, 0xba, 0xff),
        fg: RColor::Rgb(0xed, 0xed, 0xf3),
        accent: RColor::Rgb(0xef, 0x39, 0x45),
        secondary: RColor::Rgb(0x0a, 0xba, 0xff),
        panel_sep: RColor::Rgb(0x16, 0x16, 0x22),
        panel_muted: RColor::Rgb(0xa4, 0xa4, 0xb7),
        border_idle: RColor::Rgb(0xa4, 0xa4, 0xb7),
        green: RColor::Rgb(0x2e, 0xe0, 0x6b),
        orange: RColor::Rgb(0xff, 0xb8, 0x4d),
        red: RColor::Rgb(0xef, 0x39, 0x45),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Cyber Spider",
        palette: [
            ColorRgb::new(0x0a, 0x0d, 0x14), // Black      (abyss)
            ColorRgb::new(0xff, 0x54, 0x70), // Red        (neon red)
            ColorRgb::new(0x4a, 0xde, 0x80), // Green      (spring green)
            ColorRgb::new(0xfb, 0xbf, 0x24), // Yellow     (amber)
            ColorRgb::new(0x38, 0xbd, 0xf8), // Blue       (electric blue)
            ColorRgb::new(0xa7, 0x8b, 0xfa), // Magenta    (violet)
            ColorRgb::new(0x22, 0xd3, 0xee), // Cyan       (cyan)
            ColorRgb::new(0xe6, 0xed, 0xf3), // White      (technical)
            ColorRgb::new(0x14, 0x1a, 0x29), // BrightBlack (surface)
            ColorRgb::new(0xff, 0x8f, 0xa3), // BrightRed
            ColorRgb::new(0x6e, 0xe7, 0xb7), // BrightGreen
            ColorRgb::new(0xfc, 0xd3, 0x4d), // BrightYellow
            ColorRgb::new(0x7d, 0xd3, 0xfc), // BrightBlue
            ColorRgb::new(0xc4, 0xb5, 0xfd), // BrightMagenta
            ColorRgb::new(0x67, 0xe8, 0xf9), // BrightCyan
            ColorRgb::new(0xff, 0xff, 0xff), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xe6, 0xed, 0xf3),
        term_bg: ColorRgb::new(0x0a, 0x0d, 0x14),
        term_cursor: ColorRgb::new(0x38, 0xbd, 0xf8),
        fg: RColor::Rgb(0xe6, 0xed, 0xf3),
        accent: RColor::Rgb(0x38, 0xbd, 0xf8),
        secondary: RColor::Rgb(0xf9, 0x73, 0x16),
        panel_sep: RColor::Rgb(0x14, 0x1a, 0x29),
        panel_muted: RColor::Rgb(0x64, 0x74, 0x8b),
        border_idle: RColor::Rgb(0x64, 0x74, 0x8b),
        green: RColor::Rgb(0x4a, 0xde, 0x80),
        orange: RColor::Rgb(0xf9, 0x73, 0x16),
        red: RColor::Rgb(0xff, 0x54, 0x70),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Silk & Steel",
        palette: [
            ColorRgb::new(0x12, 0x12, 0x12), // Black      (matte)
            ColorRgb::new(0xe5, 0x48, 0x4d), // Red        (dusty red)
            ColorRgb::new(0x47, 0xc0, 0x87), // Green      (sage green)
            ColorRgb::new(0xff, 0xb7, 0x03), // Yellow     (warm amber)
            ColorRgb::new(0x5d, 0x8a, 0xa8), // Blue       (steel blue)
            ColorRgb::new(0xa5, 0x7f, 0xb8), // Magenta    (dusty purple)
            ColorRgb::new(0x5f, 0xbf, 0xc0), // Cyan       (steel cyan)
            ColorRgb::new(0xf8, 0xf9, 0xfa), // White      (silk)
            ColorRgb::new(0x1e, 0x1e, 0x1e), // BrightBlack (studio)
            ColorRgb::new(0xff, 0x7a, 0x7a), // BrightRed
            ColorRgb::new(0x6f, 0xd9, 0xa0), // BrightGreen
            ColorRgb::new(0xff, 0xc5, 0x3d), // BrightYellow
            ColorRgb::new(0x82, 0xaa, 0xcb), // BrightBlue
            ColorRgb::new(0xc3, 0xa5, 0xd6), // BrightMagenta
            ColorRgb::new(0x8f, 0xd6, 0xd6), // BrightCyan
            ColorRgb::new(0xff, 0xff, 0xff), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xf8, 0xf9, 0xfa),
        term_bg: ColorRgb::new(0x12, 0x12, 0x12),
        term_cursor: ColorRgb::new(0xff, 0xb7, 0x03),
        fg: RColor::Rgb(0xf8, 0xf9, 0xfa),
        accent: RColor::Rgb(0xff, 0xb7, 0x03),
        secondary: RColor::Rgb(0x70, 0x80, 0x90),
        panel_sep: RColor::Rgb(0x1e, 0x1e, 0x1e),
        panel_muted: RColor::Rgb(0x8e, 0x8e, 0x93),
        border_idle: RColor::Rgb(0x8e, 0x8e, 0x93),
        green: RColor::Rgb(0x47, 0xc0, 0x87),
        orange: RColor::Rgb(0xff, 0xb7, 0x03),
        red: RColor::Rgb(0xe5, 0x48, 0x4d),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Gruvbox Dark",
        palette: [
            ColorRgb::new(0x28, 0x28, 0x28), // Black      (bg)
            ColorRgb::new(0xcc, 0x24, 0x1d), // Red
            ColorRgb::new(0x98, 0x97, 0x1a), // Green
            ColorRgb::new(0xd7, 0x99, 0x21), // Yellow
            ColorRgb::new(0x45, 0x85, 0x88), // Blue
            ColorRgb::new(0xb1, 0x62, 0x86), // Magenta
            ColorRgb::new(0x68, 0x9d, 0x6a), // Cyan
            ColorRgb::new(0xa8, 0x99, 0x84), // White
            ColorRgb::new(0x92, 0x83, 0x74), // BrightBlack
            ColorRgb::new(0xfb, 0x49, 0x34), // BrightRed
            ColorRgb::new(0xb8, 0xbb, 0x26), // BrightGreen
            ColorRgb::new(0xfa, 0xbd, 0x2f), // BrightYellow
            ColorRgb::new(0x83, 0xa5, 0x98), // BrightBlue
            ColorRgb::new(0xd3, 0x86, 0x9b), // BrightMagenta
            ColorRgb::new(0x8e, 0xc0, 0x7c), // BrightCyan
            ColorRgb::new(0xeb, 0xdb, 0xb2), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xeb, 0xdb, 0xb2),
        term_bg: ColorRgb::new(0x28, 0x28, 0x28),
        term_cursor: ColorRgb::new(0xfe, 0x80, 0x19),
        fg: RColor::Rgb(0xeb, 0xdb, 0xb2),
        accent: RColor::Rgb(0xfe, 0x80, 0x19),
        secondary: RColor::Rgb(0x8e, 0xc0, 0x7c),
        panel_sep: RColor::Rgb(0x3c, 0x38, 0x36),
        panel_muted: RColor::Rgb(0x92, 0x83, 0x74),
        border_idle: RColor::Rgb(0x92, 0x83, 0x74),
        green: RColor::Rgb(0xb8, 0xbb, 0x26),
        orange: RColor::Rgb(0xfe, 0x80, 0x19),
        red: RColor::Rgb(0xfb, 0x49, 0x34),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Dracula",
        palette: [
            ColorRgb::new(0x21, 0x22, 0x2c), // Black
            ColorRgb::new(0xff, 0x55, 0x55), // Red
            ColorRgb::new(0x50, 0xfa, 0x7b), // Green
            ColorRgb::new(0xf1, 0xfa, 0x8c), // Yellow
            ColorRgb::new(0xbd, 0x93, 0xf9), // Blue
            ColorRgb::new(0xff, 0x79, 0xc6), // Magenta
            ColorRgb::new(0x8b, 0xe9, 0xfd), // Cyan
            ColorRgb::new(0xf8, 0xf8, 0xf2), // White
            ColorRgb::new(0x62, 0x72, 0xa4), // BrightBlack
            ColorRgb::new(0xff, 0x6e, 0x67), // BrightRed
            ColorRgb::new(0x5a, 0xf7, 0x8e), // BrightGreen
            ColorRgb::new(0xf4, 0xf9, 0x9d), // BrightYellow
            ColorRgb::new(0xca, 0xa9, 0xfa), // BrightBlue
            ColorRgb::new(0xff, 0x92, 0xd0), // BrightMagenta
            ColorRgb::new(0x9a, 0xed, 0xfe), // BrightCyan
            ColorRgb::new(0xff, 0xff, 0xff), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xf8, 0xf8, 0xf2),
        term_bg: ColorRgb::new(0x28, 0x2a, 0x36),
        term_cursor: ColorRgb::new(0xbd, 0x93, 0xf9),
        fg: RColor::Rgb(0xf8, 0xf8, 0xf2),
        accent: RColor::Rgb(0xbd, 0x93, 0xf9),
        secondary: RColor::Rgb(0xff, 0x79, 0xc6),
        panel_sep: RColor::Rgb(0x21, 0x22, 0x2c),
        panel_muted: RColor::Rgb(0x62, 0x72, 0xa4),
        border_idle: RColor::Rgb(0x62, 0x72, 0xa4),
        green: RColor::Rgb(0x50, 0xfa, 0x7b),
        orange: RColor::Rgb(0xff, 0xb8, 0x6c),
        red: RColor::Rgb(0xff, 0x55, 0x55),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Tokyo Night",
        palette: [
            ColorRgb::new(0x41, 0x48, 0x68), // Black
            ColorRgb::new(0xf7, 0x76, 0x8e), // Red
            ColorRgb::new(0x9e, 0xce, 0x6a), // Green
            ColorRgb::new(0xe0, 0xaf, 0x68), // Yellow
            ColorRgb::new(0x7a, 0xa2, 0xf7), // Blue
            ColorRgb::new(0xbb, 0x9a, 0xf7), // Magenta
            ColorRgb::new(0x7d, 0xcf, 0xff), // Cyan
            ColorRgb::new(0xc0, 0xca, 0xf5), // White
            ColorRgb::new(0x41, 0x48, 0x68), // BrightBlack (same as dark, historical)
            ColorRgb::new(0xf7, 0x76, 0x8e), // BrightRed
            ColorRgb::new(0x9e, 0xce, 0x6a), // BrightGreen
            ColorRgb::new(0xe0, 0xaf, 0x68), // BrightYellow
            ColorRgb::new(0x7a, 0xa2, 0xf7), // BrightBlue
            ColorRgb::new(0xbb, 0x9a, 0xf7), // BrightMagenta
            ColorRgb::new(0x7d, 0xcf, 0xff), // BrightCyan
            ColorRgb::new(0xc0, 0xca, 0xf5), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xc0, 0xca, 0xf5),
        term_bg: ColorRgb::new(0x24, 0x28, 0x3b),
        term_cursor: ColorRgb::new(0x7a, 0xa2, 0xf7),
        fg: RColor::Rgb(0xc0, 0xca, 0xf5),
        accent: RColor::Rgb(0x7a, 0xa2, 0xf7),
        secondary: RColor::Rgb(0xbb, 0x9a, 0xf7),
        panel_sep: RColor::Rgb(0x16, 0x1a, 0x2e),
        panel_muted: RColor::Rgb(0x56, 0x5f, 0x89),
        border_idle: RColor::Rgb(0x56, 0x5f, 0x89),
        green: RColor::Rgb(0x9e, 0xce, 0x6a),
        orange: RColor::Rgb(0xe0, 0xaf, 0x68),
        red: RColor::Rgb(0xf7, 0x76, 0x8e),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Nord",
        palette: [
            ColorRgb::new(0x3b, 0x42, 0x52), // Black
            ColorRgb::new(0xbf, 0x61, 0x6a), // Red
            ColorRgb::new(0xa3, 0xbe, 0x8c), // Green
            ColorRgb::new(0xeb, 0xcb, 0x8b), // Yellow
            ColorRgb::new(0x81, 0xa1, 0xc1), // Blue
            ColorRgb::new(0xb4, 0x8e, 0xad), // Magenta
            ColorRgb::new(0x88, 0xc0, 0xd0), // Cyan
            ColorRgb::new(0xe5, 0xe9, 0xf0), // White
            ColorRgb::new(0x4c, 0x56, 0x6a), // BrightBlack
            ColorRgb::new(0xbf, 0x61, 0x6a), // BrightRed
            ColorRgb::new(0xa3, 0xbe, 0x8c), // BrightGreen
            ColorRgb::new(0xeb, 0xcb, 0x8b), // BrightYellow
            ColorRgb::new(0x81, 0xa1, 0xc1), // BrightBlue
            ColorRgb::new(0xb4, 0x8e, 0xad), // BrightMagenta
            ColorRgb::new(0x8f, 0xbc, 0xbb), // BrightCyan
            ColorRgb::new(0xec, 0xef, 0xf4), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xd8, 0xde, 0xe9),
        term_bg: ColorRgb::new(0x2e, 0x34, 0x40),
        term_cursor: ColorRgb::new(0xd8, 0xde, 0xe9),
        fg: RColor::Rgb(0xd8, 0xde, 0xe9),
        accent: RColor::Rgb(0x88, 0xc0, 0xd0),
        secondary: RColor::Rgb(0x81, 0xa1, 0xc1),
        panel_sep: RColor::Rgb(0x24, 0x29, 0x33),
        panel_muted: RColor::Rgb(0x61, 0x6e, 0x88),
        border_idle: RColor::Rgb(0x61, 0x6e, 0x88),
        green: RColor::Rgb(0xa3, 0xbe, 0x8c),
        orange: RColor::Rgb(0xd0, 0x87, 0x70),
        red: RColor::Rgb(0xbf, 0x61, 0x6a),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "One Dark",
        palette: [
            ColorRgb::new(0x3f, 0x44, 0x51), // Black
            ColorRgb::new(0xe0, 0x6c, 0x75), // Red
            ColorRgb::new(0x98, 0xc3, 0x79), // Green
            ColorRgb::new(0xe5, 0xc0, 0x7b), // Yellow
            ColorRgb::new(0x61, 0xaf, 0xef), // Blue
            ColorRgb::new(0xc6, 0x78, 0xdd), // Magenta
            ColorRgb::new(0x56, 0xb6, 0xc2), // Cyan
            ColorRgb::new(0xab, 0xb2, 0xbf), // White
            ColorRgb::new(0x5c, 0x63, 0x70), // BrightBlack
            ColorRgb::new(0xe0, 0x6c, 0x75), // BrightRed
            ColorRgb::new(0x98, 0xc3, 0x79), // BrightGreen
            ColorRgb::new(0xe5, 0xc0, 0x7b), // BrightYellow
            ColorRgb::new(0x61, 0xaf, 0xef), // BrightBlue
            ColorRgb::new(0xc6, 0x78, 0xdd), // BrightMagenta
            ColorRgb::new(0x56, 0xb6, 0xc2), // BrightCyan
            ColorRgb::new(0xd7, 0xda, 0xe0), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xab, 0xb2, 0xbf),
        term_bg: ColorRgb::new(0x28, 0x2c, 0x34),
        term_cursor: ColorRgb::new(0x52, 0x8b, 0xff),
        fg: RColor::Rgb(0xab, 0xb2, 0xbf),
        accent: RColor::Rgb(0x61, 0xaf, 0xef),
        secondary: RColor::Rgb(0xc6, 0x78, 0xdd),
        panel_sep: RColor::Rgb(0x21, 0x25, 0x2b),
        panel_muted: RColor::Rgb(0x5c, 0x63, 0x70),
        border_idle: RColor::Rgb(0x5c, 0x63, 0x70),
        green: RColor::Rgb(0x98, 0xc3, 0x79),
        orange: RColor::Rgb(0xd1, 0x9a, 0x66),
        red: RColor::Rgb(0xe0, 0x6c, 0x75),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Rose Pine",
        palette: [
            ColorRgb::new(0x26, 0x23, 0x3a), // Black
            ColorRgb::new(0xeb, 0x6f, 0x92), // Red
            ColorRgb::new(0x31, 0x74, 0x8f), // Green
            ColorRgb::new(0xf6, 0xc1, 0x77), // Yellow
            ColorRgb::new(0x9c, 0xcf, 0xd8), // Blue
            ColorRgb::new(0xc4, 0xa7, 0xe7), // Magenta
            ColorRgb::new(0xeb, 0xbc, 0xba), // Cyan
            ColorRgb::new(0xe0, 0xde, 0xf4), // White
            ColorRgb::new(0x6e, 0x6a, 0x86), // BrightBlack
            ColorRgb::new(0xeb, 0x6f, 0x92), // BrightRed
            ColorRgb::new(0x31, 0x74, 0x8f), // BrightGreen
            ColorRgb::new(0xf6, 0xc1, 0x77), // BrightYellow
            ColorRgb::new(0x9c, 0xcf, 0xd8), // BrightBlue
            ColorRgb::new(0xc4, 0xa7, 0xe7), // BrightMagenta
            ColorRgb::new(0xeb, 0xbc, 0xba), // BrightCyan
            ColorRgb::new(0xe0, 0xde, 0xf4), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xe0, 0xde, 0xf4),
        term_bg: ColorRgb::new(0x19, 0x17, 0x24),
        term_cursor: ColorRgb::new(0xe0, 0xde, 0xf4),
        fg: RColor::Rgb(0xe0, 0xde, 0xf4),
        accent: RColor::Rgb(0xc4, 0xa7, 0xe7),
        secondary: RColor::Rgb(0xeb, 0xbc, 0xba),
        panel_sep: RColor::Rgb(0x1f, 0x1d, 0x2e),
        panel_muted: RColor::Rgb(0x6e, 0x6a, 0x86),
        border_idle: RColor::Rgb(0x6e, 0x6a, 0x86),
        green: RColor::Rgb(0x9c, 0xcf, 0xd8),
        orange: RColor::Rgb(0xf6, 0xc1, 0x77),
        red: RColor::Rgb(0xeb, 0x6f, 0x92),
        input_bg: INPUT_SURFACE,
    },
    Theme {
        name: "Catppuccin Latte",
        palette: [
            ColorRgb::new(0x5c, 0x5f, 0x77), // Black
            ColorRgb::new(0xd2, 0x0f, 0x39), // Red
            ColorRgb::new(0x40, 0xa0, 0x2b), // Green
            ColorRgb::new(0xdf, 0x8e, 0x1d), // Yellow
            ColorRgb::new(0x1e, 0x66, 0xf5), // Blue
            ColorRgb::new(0xea, 0x76, 0xcb), // Magenta
            ColorRgb::new(0x17, 0x92, 0x99), // Cyan
            ColorRgb::new(0xac, 0xb0, 0xbe), // White
            ColorRgb::new(0x6c, 0x6f, 0x85), // BrightBlack
            ColorRgb::new(0xd2, 0x0f, 0x39), // BrightRed
            ColorRgb::new(0x40, 0xa0, 0x2b), // BrightGreen
            ColorRgb::new(0xdf, 0x8e, 0x1d), // BrightYellow
            ColorRgb::new(0x1e, 0x66, 0xf5), // BrightBlue
            ColorRgb::new(0xea, 0x76, 0xcb), // BrightMagenta
            ColorRgb::new(0x17, 0x92, 0x99), // BrightCyan
            ColorRgb::new(0xbc, 0xc0, 0xcc), // BrightWhite
        ],
        term_fg: ColorRgb::new(0x4c, 0x4f, 0x69),
        term_bg: ColorRgb::new(0xef, 0xf1, 0xf5),
        term_cursor: ColorRgb::new(0xdc, 0x8a, 0x78),
        fg: RColor::Rgb(0x4c, 0x4f, 0x69),
        accent: RColor::Rgb(0x1e, 0x66, 0xf5),
        secondary: RColor::Rgb(0x72, 0x87, 0xfd),
        panel_sep: RColor::Rgb(0xe6, 0xe9, 0xef),
        panel_muted: RColor::Rgb(0x6c, 0x6f, 0x85),
        border_idle: RColor::Rgb(0x8c, 0x8f, 0xa1),
        green: RColor::Rgb(0x40, 0xa0, 0x2b),
        orange: RColor::Rgb(0xfe, 0x64, 0x0b),
        red: RColor::Rgb(0xd2, 0x0f, 0x39),
        input_bg: RColor::Rgb(0xcc, 0xd0, 0xda),
    },
];

/// Index of the theme applied on a fresh start.
pub const DEFAULT_THEME_IDX: usize = 1;

/// Resolve a theme's chrome color reference (accent/fg) to 0xRRGGBB — RGB
/// directly, or through the theme's ANSI palette for indexed colors. Used by
/// clients that derive their own chrome from a shared theme.
pub fn chrome_hex(color: RColor, theme: &Theme) -> Option<u32> {
    match color {
        RColor::Rgb(r, g, b) => Some(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        RColor::Indexed(i) => theme
            .palette
            .get(i as usize)
            .map(|c| ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32),
        _ => None,
    }
}

pub fn chrome_hex_owned(color: RColor, theme: &OwnedTheme) -> Option<u32> {
    match color {
        RColor::Rgb(r, g, b) => Some(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        RColor::Indexed(i) => theme
            .palette
            .get(i as usize)
            .map(|c| ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32),
        _ => None,
    }
}

/// Parse a hex string into an `RColor::Rgb`. Accepts `#rrggbb` / `rrggbb` / `#rgb`.
pub fn parse_rcolor(s: &str) -> Option<RColor> {
    parse_hex(s).map(|c| RColor::Rgb(c.r, c.g, c.b))
}

/// Normalize a theme name for lookup: lowercase, trim, hyphens/spaces equivalent.
pub fn normalize_name(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

/// Retired built-in themes (normalized names) and their closest successor, so
/// configs that still select them keep a similar look instead of the default.
const RETIRED_THEMES: [(&str, &str); 1] = [("toxic-arachnid", "cyber-spider")];

/// Find the index of a built-in theme by name (case-insensitive, hyphen/space tolerant).
/// Supports short aliases: "gruvbox" matches "Gruvbox Dark", "catppuccin" matches
/// "Catppuccin Mocha", etc. via prefix matching.
pub fn builtin_index_by_name(name: &str) -> Option<usize> {
    let needle = normalize_name(name);
    let needle = RETIRED_THEMES
        .iter()
        .find(|(retired, _)| *retired == needle)
        .map_or(needle, |(_, successor)| successor.to_string());
    // Exact match first
    if let Some(idx) = THEMES.iter().position(|t| normalize_name(t.name) == needle) {
        return Some(idx);
    }
    // Prefix / alias match: "gruvbox" -> "gruvbox-dark", "tokyo" -> "tokyo-night"
    THEMES.iter().position(|t| {
        let n = normalize_name(t.name);
        n.starts_with(&needle) || needle.starts_with(&n)
    })
}

/// Build the full list of `OwnedTheme`s including the optional custom theme at the end.
pub fn all_themes(custom: Option<OwnedTheme>) -> Vec<OwnedTheme> {
    let mut v: Vec<OwnedTheme> = THEMES.iter().copied().map(OwnedTheme::from).collect();
    if let Some(c) = custom {
        v.push(c);
    }
    v
}

/// Select one theme without allocating the full picker list. The returned
/// index is clamped exactly as the picker list would be indexed.
pub fn selected_theme(index: usize, custom: Option<&OwnedTheme>) -> (usize, OwnedTheme) {
    let last = THEMES.len() + usize::from(custom.is_some()) - 1;
    let index = index.min(last);
    let theme = if index == THEMES.len() {
        custom.expect("custom index requires a custom theme").clone()
    } else {
        OwnedTheme::from(THEMES[index])
    };
    (index, theme)
}

/// Resolve the initial theme index from config. `selected` is the `theme = "..."` value.
/// Returns the fallback `DEFAULT_THEME_IDX` when the name is unknown.
pub fn resolve_theme_idx(selected: Option<&str>, custom: Option<&OwnedTheme>) -> usize {
    if let Some(name) = selected {
        // "custom" (any case) selects the dynamic slot when present.
        if normalize_name(name) == "custom" {
            if custom.is_some() {
                return THEMES.len();
            }
            return DEFAULT_THEME_IDX;
        }
        if let Some(idx) = builtin_index_by_name(name) {
            return idx;
        }
        // numeric idx as string ("0", "3")
        if let Ok(n) = name.trim().parse::<usize>() {
            if n < THEMES.len() {
                return n;
            }
        }
        log::warn!("kumo: unknown theme {name:?}; using default");
    }
    DEFAULT_THEME_IDX
}

fn rcolor_to_triplet(c: RColor, palette: &[ColorRgb; 16]) -> [u8; 3] {
    match c {
        RColor::Rgb(r, g, b) => [r, g, b],
        RColor::Indexed(i) => palette
            .get(i as usize)
            .map(|cc| [cc.r, cc.g, cc.b])
            .unwrap_or([0, 0, 0]),
        _ => [0, 0, 0],
    }
}
fn triplet_to_rcolor(t: [u8; 3]) -> RColor {
    RColor::Rgb(t[0], t[1], t[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_theme_matches_picker_order_and_fallbacks() {
        let mut custom = OwnedTheme::from(THEMES[0]);
        custom.name = "Custom".to_string();

        let (built_in_idx, built_in) = selected_theme(1, Some(&custom));
        assert_eq!(built_in_idx, 1);
        assert_eq!(built_in.name, THEMES[1].name);

        let (custom_idx, selected_custom) = selected_theme(THEMES.len(), Some(&custom));
        assert_eq!(custom_idx, THEMES.len());
        assert_eq!(selected_custom, custom);

        let (fallback_idx, fallback) = selected_theme(usize::MAX, None);
        assert_eq!(fallback_idx, THEMES.len() - 1);
        assert_eq!(fallback.name, THEMES[THEMES.len() - 1].name);
    }

    #[test]
    fn theme_names_resolve_including_retired_ones() {
        let name_of = |query: &str| builtin_index_by_name(query).map(|idx| THEMES[idx].name);
        assert_eq!(name_of("nord"), Some("Nord"));
        assert_eq!(name_of("One Dark"), Some("One Dark"));
        assert_eq!(name_of("rose_pine"), Some("Rose Pine"));
        assert_eq!(name_of("Toxic Arachnid"), Some("Cyber Spider"));
        assert_eq!(name_of("catppuccin-latte"), Some("Catppuccin Latte"));
        assert_eq!(name_of("catppuccin"), Some("Catppuccin Mocha"), "the bare family name keeps the dark default");
    }

    #[test]
    fn only_latte_is_a_light_builtin() {
        let light: Vec<_> = all_themes(None).into_iter().filter(OwnedTheme::is_light).map(|t| t.name).collect();
        assert_eq!(light, ["Catppuccin Latte"]);
    }
}

/// Convert an `OwnedTheme` to the wire representation.
pub fn owned_to_wire(t: &OwnedTheme) -> kumo_protocol::WireTheme {
    kumo_protocol::WireTheme {
        name: t.name.clone(),
        palette: t.palette.map(|c| [c.r, c.g, c.b]),
        term_fg: [t.term_fg.r, t.term_fg.g, t.term_fg.b],
        term_bg: [t.term_bg.r, t.term_bg.g, t.term_bg.b],
        term_cursor: [t.term_cursor.r, t.term_cursor.g, t.term_cursor.b],
        fg: rcolor_to_triplet(t.fg, &t.palette),
        accent: rcolor_to_triplet(t.accent, &t.palette),
        secondary: rcolor_to_triplet(t.secondary, &t.palette),
        panel_sep: rcolor_to_triplet(t.panel_sep, &t.palette),
        panel_muted: rcolor_to_triplet(t.panel_muted, &t.palette),
        border_idle: rcolor_to_triplet(t.border_idle, &t.palette),
        green: rcolor_to_triplet(t.green, &t.palette),
        orange: rcolor_to_triplet(t.orange, &t.palette),
        red: rcolor_to_triplet(t.red, &t.palette),
        input_bg: rcolor_to_triplet(t.input_bg, &t.palette),
    }
}

/// Convert a `WireTheme` back to an `OwnedTheme`.
pub fn wire_to_owned(w: kumo_protocol::WireTheme) -> OwnedTheme {
    let palette: [ColorRgb; 16] = w.palette.map(|t| ColorRgb::new(t[0], t[1], t[2]));
    OwnedTheme {
        name: w.name,
        palette,
        term_fg: ColorRgb::new(w.term_fg[0], w.term_fg[1], w.term_fg[2]),
        term_bg: ColorRgb::new(w.term_bg[0], w.term_bg[1], w.term_bg[2]),
        term_cursor: ColorRgb::new(w.term_cursor[0], w.term_cursor[1], w.term_cursor[2]),
        fg: triplet_to_rcolor(w.fg),
        accent: triplet_to_rcolor(w.accent),
        secondary: triplet_to_rcolor(w.secondary),
        panel_sep: triplet_to_rcolor(w.panel_sep),
        panel_muted: triplet_to_rcolor(w.panel_muted),
        border_idle: triplet_to_rcolor(w.border_idle),
        green: triplet_to_rcolor(w.green),
        orange: triplet_to_rcolor(w.orange),
        red: triplet_to_rcolor(w.red),
        input_bg: triplet_to_rcolor(w.input_bg),
    }
}

/// Agent-status chrome colors (Tailwind palette), fixed across themes so the
/// status dots of every kumo surface read identically:
/// working blue, blocked amber, idle grey, done green, unknown violet.
pub const AGENT_WORKING: (u8, u8, u8) = (0x3b, 0x82, 0xf6);
pub const AGENT_BLOCKED: (u8, u8, u8) = (0xf5, 0x9e, 0x0b);
pub const AGENT_IDLE: (u8, u8, u8) = (0x6b, 0x72, 0x80);
pub const AGENT_DONE: (u8, u8, u8) = (0x10, 0xb9, 0x81);
pub const AGENT_UNKNOWN: (u8, u8, u8) = (0x8b, 0x5c, 0xf6);

/// The fixed RGB color of an agent status. Shared by the TUI chrome (sidebar,
/// status bar) and the CLI so every surface uses the same palette.
pub fn agent_status_color(status: kumo_protocol::AgentStatus) -> (u8, u8, u8) {
    match status {
        kumo_protocol::AgentStatus::Working => AGENT_WORKING,
        kumo_protocol::AgentStatus::Blocked => AGENT_BLOCKED,
        kumo_protocol::AgentStatus::Idle => AGENT_IDLE,
        kumo_protocol::AgentStatus::Done => AGENT_DONE,
        kumo_protocol::AgentStatus::Unknown => AGENT_UNKNOWN,
    }
}
