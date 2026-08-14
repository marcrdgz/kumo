//! Theme definitions: every color kumo renders, from the terminal emulator's
//! ANSI palette down to the chrome (status bar, sidebar, popups). The active
//! theme lives on [`crate::app::App`] and can be swapped live from the
//! status-bar Settings popup; switching re-applies the terminal defaults to
//! every existing pane.

use ratatui::style::Color as RColor;

use crate::vt::ColorRgb;

/// A complete color scheme: ANSI palette + terminal defaults + chrome colors.
#[derive(Clone, Copy)]
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

/// All selectable themes, in Settings-popup order. The first entry is the
/// original kumo scheme; the rest are the Spider-Verse family.
pub const THEMES: [Theme; 5] = [
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
        input_bg: RColor::Rgb(0xcd, 0xd6, 0xf4),
    },
    Theme {
        name: "Spider-Verse",
        palette: [
            ColorRgb::new(0x0d, 0x0e, 0x15), // Black      (bg night)
            ColorRgb::new(0xff, 0x2a, 0x5f), // Red        (neon carmine)
            ColorRgb::new(0x2e, 0xe0, 0x6b), // Green      (neon green)
            ColorRgb::new(0xff, 0xd1, 0x66), // Yellow     (amber)
            ColorRgb::new(0x4a, 0x7b, 0xff), // Blue       (electric blue)
            ColorRgb::new(0xc7, 0x7d, 0xff), // Magenta    (neon purple)
            ColorRgb::new(0x00, 0xf0, 0xff), // Cyan       (cyber cyan)
            ColorRgb::new(0xe2, 0xe8, 0xf0), // White      (ash white)
            ColorRgb::new(0x16, 0x19, 0x23), // BrightBlack (surface)
            ColorRgb::new(0xff, 0x6b, 0x8a), // BrightRed
            ColorRgb::new(0x5b, 0xf0, 0xa0), // BrightGreen
            ColorRgb::new(0xff, 0xe0, 0x8a), // BrightYellow
            ColorRgb::new(0x7e, 0xa6, 0xff), // BrightBlue
            ColorRgb::new(0xd9, 0xaf, 0xff), // BrightMagenta
            ColorRgb::new(0x7d, 0xf6, 0xff), // BrightCyan
            ColorRgb::new(0xff, 0xff, 0xff), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xe2, 0xe8, 0xf0),
        term_bg: ColorRgb::new(0x0d, 0x0e, 0x15),
        term_cursor: ColorRgb::new(0x00, 0xf0, 0xff),
        fg: RColor::Rgb(0xe2, 0xe8, 0xf0),
        accent: RColor::Rgb(0xff, 0x2a, 0x5f),
        secondary: RColor::Rgb(0x00, 0xf0, 0xff),
        panel_sep: RColor::Rgb(0x16, 0x19, 0x23),
        panel_muted: RColor::Rgb(0x8a, 0x94, 0xad),
        border_idle: RColor::Rgb(0x8a, 0x94, 0xad),
        green: RColor::Rgb(0x2e, 0xe0, 0x6b),
        orange: RColor::Rgb(0xff, 0xb8, 0x4d),
        red: RColor::Rgb(0xff, 0x2a, 0x5f),
        input_bg: RColor::Rgb(0xe2, 0xe8, 0xf0),
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
        input_bg: RColor::Rgb(0xe6, 0xed, 0xf3),
    },
    Theme {
        name: "Toxic Arachnid",
        palette: [
            ColorRgb::new(0x09, 0x0a, 0x0f), // Black      (carbon)
            ColorRgb::new(0xff, 0x38, 0x60), // Red        (neon red)
            ColorRgb::new(0x00, 0xff, 0x88), // Green      (matrix)
            ColorRgb::new(0xff, 0xd1, 0x66), // Yellow     (amber)
            ColorRgb::new(0x3b, 0x82, 0xf6), // Blue       (blue)
            ColorRgb::new(0x9d, 0x4e, 0xdd), // Magenta    (neon purple)
            ColorRgb::new(0x00, 0xe5, 0xff), // Cyan       (cyan)
            ColorRgb::new(0xd1, 0xd5, 0xdb), // White      (light gray)
            ColorRgb::new(0x12, 0x15, 0x1e), // BrightBlack (slate)
            ColorRgb::new(0xff, 0x66, 0x80), // BrightRed
            ColorRgb::new(0x66, 0xff, 0xb8), // BrightGreen
            ColorRgb::new(0xff, 0xe0, 0x8a), // BrightYellow
            ColorRgb::new(0x7f, 0xb8, 0xff), // BrightBlue
            ColorRgb::new(0xb0, 0x6f, 0xe8), // BrightMagenta
            ColorRgb::new(0x66, 0xf0, 0xff), // BrightCyan
            ColorRgb::new(0xff, 0xff, 0xff), // BrightWhite
        ],
        term_fg: ColorRgb::new(0xd1, 0xd5, 0xdb),
        term_bg: ColorRgb::new(0x09, 0x0a, 0x0f),
        term_cursor: ColorRgb::new(0x00, 0xff, 0x88),
        fg: RColor::Rgb(0xd1, 0xd5, 0xdb),
        accent: RColor::Rgb(0x00, 0xff, 0x88),
        secondary: RColor::Rgb(0x9d, 0x4e, 0xdd),
        panel_sep: RColor::Rgb(0x12, 0x15, 0x1e),
        panel_muted: RColor::Rgb(0x71, 0x71, 0x7a),
        border_idle: RColor::Rgb(0x71, 0x71, 0x7a),
        green: RColor::Rgb(0x00, 0xff, 0x88),
        orange: RColor::Rgb(0xff, 0xb8, 0x4d),
        red: RColor::Rgb(0xff, 0x38, 0x60),
        input_bg: RColor::Rgb(0xd1, 0xd5, 0xdb),
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
        input_bg: RColor::Rgb(0xf8, 0xf9, 0xfa),
    },
];

/// Index of the theme applied on a fresh start.
pub const DEFAULT_THEME_IDX: usize = 1;
