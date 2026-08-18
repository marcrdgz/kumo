//! Daemon-side serialization of a rendered pane grid into wire `PaneFrame`s
//! (`kumo_protocol`).
//!
//! The daemon renders ONLY per-pane terminal content (never chrome/borders);
//! this module owns cell serialization (colors resolved against the active
//! theme palette, wide-char continuation cells, row diffs) plus the per-row
//! link ranges and the scrollback state clients need to draw scrollbars and
//! underline links. Kept here — not in `kumo_protocol` — because it depends on
//! `ratatui` and `kumo_core::color::ColorRgb`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use kumo_protocol::{PaneFrame, RowPatch, ScrollState, WireCell};

use kumo_core::color::ColorRgb;

/// Pack a ratatui `Color` into the wire's 0xRRGGBB form. Named ANSI colors map
/// to the active theme's palette (the ANSI 0-15 entries), so cells styled with
/// named colors keep a real value on the client. `Reset` (and anything unnamed)
/// stays `None`.
fn color_to_wire(color: ratatui::style::Color, palette: &[ColorRgb; 16]) -> Option<u32> {
    use ratatui::style::Color;
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Black => (palette[0].r, palette[0].g, palette[0].b),
        Color::Red => (palette[1].r, palette[1].g, palette[1].b),
        Color::Green => (palette[2].r, palette[2].g, palette[2].b),
        Color::Yellow => (palette[3].r, palette[3].g, palette[3].b),
        Color::Blue => (palette[4].r, palette[4].g, palette[4].b),
        Color::Magenta => (palette[5].r, palette[5].g, palette[5].b),
        Color::Cyan => (palette[6].r, palette[6].g, palette[6].b),
        Color::White => (palette[7].r, palette[7].g, palette[7].b),
        Color::Gray => (palette[8].r, palette[8].g, palette[8].b),
        Color::DarkGray => (palette[8].r, palette[8].g, palette[8].b),
        Color::LightRed => (palette[9].r, palette[9].g, palette[9].b),
        Color::LightGreen => (palette[10].r, palette[10].g, palette[10].b),
        Color::LightYellow => (palette[11].r, palette[11].g, palette[11].b),
        Color::LightBlue => (palette[12].r, palette[12].g, palette[12].b),
        Color::LightMagenta => (palette[13].r, palette[13].g, palette[13].b),
        Color::LightCyan => (palette[14].r, palette[14].g, palette[14].b),
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return None,
    };
    Some(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

/// The cells of one row, in column order. A cell that follows a wide grapheme
/// (a CJK char or emoji occupying two columns) is a continuation cell; it is
/// forced to `cell_width = 0` so the client skips it instead of overwriting
/// the wide char's right half.
pub(crate) fn row_cells(buf: &Buffer, row: u16, cols: u16, palette: &[ColorRgb; 16]) -> Vec<WireCell> {
    let s = row as usize * cols as usize;
    let e = s + cols as usize;
    let cells: Vec<WireCell> = buf.content[s..e].iter().map(|c| cell_from_ratatui(c, palette)).collect();
    let mut out = Vec::with_capacity(cells.len());
    let mut prev_wide = false;
    for mut cell in cells {
        if prev_wide {
            cell.cell_width = 0;
            out.push(cell);
            prev_wide = false;
        } else {
            prev_wide = cell.cell_width == 2;
            out.push(cell);
        }
    }
    out
}

/// Whether any cell in `row` differs between the two buffers.
fn row_changed(buf: &Buffer, last: &Buffer, row: u16, cols: u16) -> bool {
    let s = row as usize * cols as usize;
    let e = s + cols as usize;
    // Compare only visible attributes (symbol, fg, bg, modifier), ignoring
    // `diff_option`. The width fix introduces `ForcedWidth(2)`/`Skip` on
    // emoji cells, which would otherwise cause `row_changed` to report
    // "changed" even when the visible content is identical — or, worse,
    // miss actual visible changes when the diff_options happen to match.
    buf.content[s..e]
        .iter()
        .zip(last.content[s..e].iter())
        .any(|(a, b)| {
            a.symbol() != b.symbol()
                || a.fg != b.fg
                || a.bg != b.bg
                || a.modifier != b.modifier
        })
}

/// Serialize one `ratatui` cell into a wire cell.
fn cell_from_ratatui(cell: &ratatui::buffer::Cell, palette: &[ColorRgb; 16]) -> WireCell {
    use ratatui::buffer::CellWidth;
    use ratatui::style::Modifier;
    let fg = color_to_wire(cell.fg, palette);
    let bg = color_to_wire(cell.bg, palette);
    let m = cell.modifier;
    let cell_width = if cell.diff_option == ratatui::buffer::CellDiffOption::Skip {
        0
    } else {
        cell.cell_width()
    };
    WireCell {
        text: cell.symbol().to_string(),
        fg,
        bg,
        bold: m.contains(Modifier::BOLD),
        italic: m.contains(Modifier::ITALIC),
        underline: m.contains(Modifier::UNDERLINED),
        inverse: m.contains(Modifier::REVERSED),
        faint: m.contains(Modifier::DIM),
        cell_width,
    }
}

/// Build a per-pane frame from the pane's retained render buffer, with either a
/// full frame (first subscribe / resize) or a diff against the previously sent
/// buffer. `pane` supplies the per-row link ranges (OSC 8 + plain-text URLs)
/// for the rows actually shipped; `scroll` is the pane's scrollback state.
pub(crate) fn pane_frame(
    pane_id: u64,
    buf: &Buffer,
    last: Option<&Buffer>,
    cursor: Option<(u16, u16)>,
    palette: &[ColorRgb; 16],
    pane: Option<&crate::daemon::pane::Pane>,
    scroll: Option<ScrollState>,
) -> PaneFrame {
    let cols = buf.area.width;
    let rows = buf.area.height;
    let links = |row: u16| pane.map(|p| p.link_ranges(row)).unwrap_or_default();
    let full = last.map(|l| l.area != buf.area).unwrap_or(true);
    let rows_dirty: Vec<RowPatch> = if full {
        (0..rows)
            .map(|row| RowPatch {
                row,
                cells: row_cells(buf, row, cols, palette),
                links: links(row),
            })
            .collect()
    } else {
        let last = last.expect("diff needs a previous buffer");
        (0..rows)
            .filter(|row| row_changed(buf, last, *row, cols))
            .map(|row| RowPatch {
                row,
                cells: row_cells(buf, row, cols, palette),
                links: links(row),
            })
            .collect()
    };
    PaneFrame { pane_id, cols, rows, full, rows_dirty, cursor, scroll }
}

/// Convert a `Rect`-scoped buffer (a pane cache) into a standalone buffer
/// starting at (0,0), so a pane frame is always shipped as its own grid.
pub(crate) fn detach_buffer(src: &Buffer) -> Buffer {
    let w = src.area.width;
    let h = src.area.height;
    let mut out = Buffer::empty(Rect::new(0, 0, w, h));
    for (i, cell) in src.content.iter().enumerate() {
        if let Some(dst) = out.content.get_mut(i) {
            *dst = cell.clone();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn palette() -> [ColorRgb; 16] {
        [ColorRgb::new(0, 0, 0); 16]
    }

    #[test]
    fn full_pane_frame_includes_every_row() {
        let buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        let frame = pane_frame(1, &buf, None, None, &palette(), None, None);
        assert!(frame.full);
        assert_eq!(frame.pane_id, 1);
        assert_eq!(frame.rows_dirty.len(), 3);
        assert!(frame.rows_dirty.iter().all(|p| p.links.is_empty()));
    }

    #[test]
    fn pane_frame_diff_sends_only_changed_rows() {
        let a = Buffer::empty(Rect::new(0, 0, 4, 3));
        let mut b = Buffer::empty(Rect::new(0, 0, 4, 3));
        b.cell_mut((1, 0)).unwrap().set_symbol("X");
        let frame = pane_frame(1, &b, Some(&a), None, &palette(), None, None);
        assert!(!frame.full);
        assert_eq!(frame.rows_dirty.len(), 1);
        assert_eq!(frame.rows_dirty[0].row, 0);
        assert_eq!(frame.rows_dirty[0].cells[1].text, "X");
    }

    #[test]
    fn pane_frame_carries_links_and_scroll() {
        let buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        let scroll = ScrollState { offset: 2, total: 10, screen: 3 };
        let frame = pane_frame(1, &buf, None, Some((1, 1)), &palette(), None, Some(scroll));
        assert!(frame.rows_dirty.iter().all(|p| p.links.is_empty()));
        assert_eq!(frame.scroll, Some(scroll));
        assert_eq!(frame.cursor, Some((1, 1)));
    }

    #[test]
    fn detach_buffer_relocates_to_origin() {
        let mut src = Buffer::empty(Rect::new(10, 20, 3, 2));
        src.cell_mut((10, 20)).unwrap().set_symbol("A");
        src.cell_mut((11, 20)).unwrap().set_symbol("B");
        let out = detach_buffer(&src);
        assert_eq!((out.area.width, out.area.height), (3, 2));
        let mut out = out;
        assert_eq!(out.cell_mut((0, 0)).unwrap().symbol(), "A");
        assert_eq!(out.cell_mut((1, 0)).unwrap().symbol(), "B");
    }
}
