//! Rendering helpers: assemble the daemon's `WireCell` grid and turn each row
//! into a GPUI `StyledText` with per-cell colors/styles.

use std::ops::Range;

use gpui::{FontStyle, FontWeight, HighlightStyle, Hsla, SharedString, StyledText, TextStyle, UnderlineStyle, px};

use kumo_protocol::{FrameMsg, WireCell};

/// A retained cell grid assembled from `FrameMsg` row patches (the daemon sends
/// full frames on attach/resize, then only dirty rows).
#[derive(Default)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<WireCell>>,
}

impl Grid {
    pub fn rows(&self) -> u16 {
        self.rows as u16
    }

    pub fn row(&self, row: u16) -> Option<&[WireCell]> {
        self.cells.get(row as usize).map(|r| r.as_slice())
    }

    pub fn apply(&mut self, frame: &FrameMsg) {
        if frame.full {
            self.cols = frame.cols as usize;
            self.rows = frame.rows as usize;
            self.cells = vec![vec![blank_cell(); self.cols]; self.rows];
        } else if self.rows != frame.rows as usize || self.cols != frame.cols as usize {
            return;
        }
        for patch in &frame.rows_dirty {
            let Some(row) = self.cells.get_mut(patch.row as usize) else { continue };
            for (x, cell) in patch.cells.iter().enumerate() {
                if let Some(slot) = row.get_mut(x) {
                    *slot = cell.clone();
                }
            }
        }
    }
}

fn blank_cell() -> WireCell {
    WireCell {
        text: String::new(),
        fg: None,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
        inverse: false,
        faint: false,
        cell_width: 1,
    }
}

/// 0xRRGGBB -> opaque `Hsla`.
fn color(hex: u32) -> Hsla {
    gpui::rgba((hex << 8) | 0xff).into()
}

/// Build the `StyledText` for one row. Blank cells become spaces so the row
/// keeps its full width; continuation cells after a wide grapheme are skipped.
pub fn row_styled_text(cells: &[WireCell], base: &TextStyle) -> StyledText {
    let mut text = String::with_capacity(cells.len());
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for cell in cells {
        if cell.cell_width == 0 {
            continue;
        }
        let start = text.len();
        let symbol = if cell.text.is_empty() { " " } else { cell.text.as_str() };
        text.push_str(symbol);
        let end = text.len();
        if end == start {
            continue;
        }
        let styled = cell.bold
            || cell.italic
            || cell.underline
            || cell.inverse
            || cell.faint
            || cell.fg.is_some()
            || cell.bg.is_some();
        if !styled {
            continue;
        }
        let mut fg = cell.fg.map(color);
        let mut bg = cell.bg.map(color);
        if cell.inverse {
            std::mem::swap(&mut fg, &mut bg);
            if bg.is_none() {
                bg = Some(base.color);
            }
        }
        let mut style = HighlightStyle {
            color: fg,
            font_weight: None,
            font_style: None,
            background_color: bg,
            underline: None,
            strikethrough: None,
            fade_out: None,
        };
        if cell.bold {
            style.font_weight = Some(FontWeight::BOLD);
        }
        if cell.italic {
            style.font_style = Some(FontStyle::Italic);
        }
        if cell.underline {
            style.underline = Some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false });
        }
        if cell.faint {
            style.fade_out = Some(0.55);
        }
        highlights.push((start..end, style));
    }
    StyledText::new(text).with_default_highlights(base, highlights)
}

/// The app-wide base text style: monospace, used for every grid row.
pub fn base_text_style(window: &mut gpui::Window) -> TextStyle {
    let mut style = window.text_style();
    style.font_family = SharedString::from("Menlo");
    style.color = hsla_from_hex(0xdddddd);
    style
}

/// A dimmer variant of the base style for sidebar/secondary text.
pub fn dim_text_style(window: &mut gpui::Window) -> TextStyle {
    let mut style = base_text_style(window);
    style.color = hsla_from_hex(0x888888);
    style
}

fn hsla_from_hex(hex: u32) -> Hsla {
    gpui::rgba((hex << 8) | 0xff).into()
}

pub fn cell_size(window: &mut gpui::Window, style: &TextStyle) -> (f32, f32) {
    let font = style.font();
    let rem = window.rem_size();
    let font_size = style.font_size.to_pixels(rem);
    let probe = window.text_system().shape_line(
        SharedString::from("X"),
        font_size,
        &[gpui::TextRun { len: 1, font, color: style.color, background_color: None, underline: None, strikethrough: None }],
        None,
    );
    let w = f32::from(probe.width).max(1.0);
    let h = f32::from(style.line_height_in_pixels(rem)).max(1.0);
    (w, h)
}
