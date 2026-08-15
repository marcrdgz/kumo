//! Daemon-side conversion of a rendered `ratatui` buffer into wire `FrameMsg`s
//! (`kumo_protocol`). The daemon renders the whole UI into a `TestBackend`
//! buffer each frame and ships dirty rows; this module owns the cell
//! serialization (colors resolved against the active theme palette, wide-char
//! continuation cells, row diffs). Kept here — not in `kumo_protocol` — because
//! it depends on `ratatui` and `crate::vt::ColorRgb`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use kumo_protocol::{FrameMsg, RowPatch, WireCell};

use crate::vt::ColorRgb;

/// Pack a ratatui `Color` into the wire's 0xRRGGBB form. Named ANSI colors map
/// to the active theme's palette (the ANSI 0-15 entries), so chrome cells
/// that style text with `Color::Black` (mode chips, menu items, input fields)
/// keep a real foreground on the client instead of falling back to the
/// terminal's default. `Reset` (and anything unnamed) stays `None`.
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
///
/// This cannot rely on ratatui's `CellDiffOption::Skip`: by the time the
/// daemon serializes `terminal.backend().buffer()`, `Terminal::draw` has
/// already diffed and normalized those cells to plain blanks, losing the flag.
/// The row's own cell widths are the only reliable signal.
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
    buf.content[s..e] != last.content[s..e]
}

/// Serialize one `ratatui` cell into a wire cell.
fn cell_from_ratatui(cell: &ratatui::buffer::Cell, palette: &[ColorRgb; 16]) -> WireCell {
    use ratatui::buffer::CellWidth;
    use ratatui::style::Modifier;
    let fg = color_to_wire(cell.fg, palette);
    let bg = color_to_wire(cell.bg, palette);
    let m = cell.modifier;
    // A `Skip` cell is a continuation after a wide grapheme (the pane marks
    // it when the emoji/CJK char occupies two columns). `Cell::cell_width()`
    // would report the width of its blank symbol (1), so map it to 0 here to
    // let the client skip it instead of overwriting the wide char's right half.
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

/// A frame containing every row (`full = true`): for a client's first attach or
/// after a resize.
pub(crate) fn full_frame(
    buf: &Buffer,
    cursor: Option<(u16, u16)>,
    palette: &[ColorRgb; 16],
) -> FrameMsg {
    let cols = buf.area.width;
    let rows = buf.area.height;
    let rows_dirty = (0..rows)
        .map(|row| RowPatch { row, cells: row_cells(buf, row, cols, palette) })
        .collect();
    FrameMsg { cols, rows, full: true, rows_dirty, cursor }
}

/// A frame containing only the rows that changed since `last` (same size).
pub(crate) fn diff_frame(
    buf: &Buffer,
    last: &Buffer,
    cursor: Option<(u16, u16)>,
    palette: &[ColorRgb; 16],
) -> FrameMsg {
    let cols = buf.area.width;
    let rows = buf.area.height;
    let rows_dirty = (0..rows)
        .filter(|row| row_changed(buf, last, *row, cols))
        .map(|row| RowPatch { row, cells: row_cells(buf, row, cols, palette) })
        .collect();
    FrameMsg { cols, rows, full: false, rows_dirty, cursor }
}

/// Build a per-pane frame from the pane's retained render buffer (`pane_cache`),
/// with either a full frame (first subscribe / resize) or a diff against the
/// previously sent buffer.
pub(crate) fn pane_frame(
    pane_id: u64,
    buf: &Buffer,
    last: Option<&Buffer>,
    cursor: Option<(u16, u16)>,
    palette: &[ColorRgb; 16],
) -> kumo_protocol::PaneFrame {
    let cols = buf.area.width;
    let rows = buf.area.height;
    let full = last.map(|l| l.area != buf.area).unwrap_or(true);
    let rows_dirty = if full {
        (0..rows)
            .map(|row| RowPatch { row, cells: row_cells(buf, row, cols, palette) })
            .collect()
    } else {
        let last = last.expect("diff needs a previous buffer");
        (0..rows)
            .filter(|row| row_changed(buf, last, *row, cols))
            .map(|row| RowPatch { row, cells: row_cells(buf, row, cols, palette) })
            .collect()
    };
    kumo_protocol::PaneFrame { pane_id, cols, rows, full, rows_dirty, cursor }
}

/// Convert a `Rect`-scoped buffer (a pane cache at its inner rect) into a
/// standalone buffer starting at (0,0). Used so a pane frame is always shipped
/// as its own grid regardless of where the pane sits in the composed UI.
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
    fn diff_frame_sends_only_changed_rows() {
        let mut a = Buffer::empty(Rect::new(0, 0, 4, 3));
        let mut b = Buffer::empty(Rect::new(0, 0, 4, 3));
        b.cell_mut((1, 0)).unwrap().set_symbol("X");
        let frame = diff_frame(&b, &a, None, &palette());
        assert!(!frame.full);
        assert_eq!(frame.rows_dirty.len(), 1, "only the touched row should be dirty");
        assert_eq!(frame.rows_dirty[0].row, 0);
        assert_eq!(frame.rows_dirty[0].cells.len(), 4);
        assert_eq!(frame.rows_dirty[0].cells[1].text, "X");
    }

    #[test]
    fn diff_frame_all_rows_equal_is_empty() {
        let a = Buffer::empty(Rect::new(0, 0, 4, 3));
        let b = Buffer::empty(Rect::new(0, 0, 4, 3));
        let frame = diff_frame(&b, &a, None, &palette());
        assert!(!frame.full);
        assert!(frame.rows_dirty.is_empty());
    }

    #[test]
    fn full_frame_includes_every_row() {
        let buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        let frame = full_frame(&buf, None, &palette());
        assert!(frame.full);
        assert_eq!(frame.rows_dirty.len(), 3);
    }

    #[test]
    fn skip_continuation_cell_serializes_width_zero() {
        // A wide emoji followed by a `Skip` continuation cell: the client must
        // see cell_width 0 on the continuation so it does not overwrite the
        // emoji's right half.
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        buf.cell_mut((0, 0)).unwrap().set_symbol("\u{1f1ea}\u{1f1f8}"); // 🇪🇸
        buf.cell_mut((1, 0))
            .unwrap()
            .set_symbol(" ")
            .set_diff_option(ratatui::buffer::CellDiffOption::Skip);
        let frame = full_frame(&buf, None, &palette());
        let cells = &frame.rows_dirty[0].cells;
        assert_eq!(cells[0].text, "\u{1f1ea}\u{1f1f8}", "wide grapheme must be sent whole");
        assert_eq!(cells[1].cell_width, 0, "continuation cell must be skipped by the client");
    }

    #[test]
    fn continuation_after_wide_char_serializes_width_zero_post_draw() {
        // After `Terminal::draw` normalizes the buffer, the continuation cell
        // is a plain blank with no `Skip` flag — only the preceding wide cell's
        // width reveals it. The row must still mark it as cell_width 0.
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        buf.cell_mut((0, 0)).unwrap().set_symbol("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}");
        buf.cell_mut((1, 0)).unwrap().set_symbol(" ");
        buf.cell_mut((2, 0)).unwrap().set_symbol("x");
        let frame = full_frame(&buf, None, &palette());
        let cells = &frame.rows_dirty[0].cells;
        assert_eq!(cells[0].text, "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}");
        assert_eq!(cells[0].cell_width, 2);
        assert_eq!(cells[1].cell_width, 0, "continuation cell must stay width 0");
        assert_eq!(cells[2].cell_width, 1);
    }

    #[test]
    fn wide_char_at_row_end_needs_no_continuation() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.cell_mut((0, 0)).unwrap().set_symbol("\u{1f600}");
        let frame = full_frame(&buf, None, &palette());
        let cells = &frame.rows_dirty[0].cells;
        assert_eq!(cells[0].cell_width, 2);
        assert_eq!(cells[1].cell_width, 0, "trailing empty cell is a blank, not content");
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
