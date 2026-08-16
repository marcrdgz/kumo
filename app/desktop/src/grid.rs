//! Rendering helpers for the native desktop client.
//!
//! The daemon streams per-pane grids (`PaneFrame`s); each is assembled into a
//! retained [`Grid`] and then painted by a custom GPUI element. `row_runs`
//! turns one row of `WireCell`s into shaped text runs with per-cell colors.

use gpui::{Font, Hsla, TextRun, TextStyle, UnderlineStyle, px};

use kumo_protocol::{PaneFrame, WireCell};

/// A retained cell grid assembled from `PaneFrame` row patches (the daemon sends
/// full frames on subscribe/resize, then only dirty rows).
#[derive(Default, Clone)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<WireCell>>,
    cursor: Option<(u16, u16)>,
    /// Cached paintable art per row; `None` = stale, rebuilt lazily on paint.
    art: Vec<Option<RowArt>>,
}

impl Grid {
    pub fn rows(&self) -> u16 {
        self.rows as u16
    }

    pub fn cursor(&self) -> Option<(u16, u16)> {
        self.cursor
    }

    /// The paintable art for a row, rebuilding it only when the row changed
    /// since the last paint (adjacent same-style cells merged into one run).
    pub fn row_art_cached(
        &mut self,
        row: u16,
        font: &Font,
        default_fg: Hsla,
        default_bg: Hsla,
    ) -> Option<&RowArt> {
        if self.art.get(row as usize)?.is_none() {
            let cells = self.cells.get(row as usize)?.clone();
            self.art[row as usize] = Some(row_art(&cells, font, default_fg, default_bg));
        }
        self.art[row as usize].as_ref()
    }

    pub fn apply(&mut self, frame: &PaneFrame) {
        if frame.full {
            self.cols = frame.cols as usize;
            self.rows = frame.rows as usize;
            self.cells = vec![vec![blank_cell(); self.cols]; self.rows];
            self.art = vec![None; self.rows];
        } else if self.rows != frame.rows as usize || self.cols != frame.cols as usize {
            return;
        } else {
            for patch in &frame.rows_dirty {
                if let Some(slot) = self.art.get_mut(patch.row as usize) {
                    *slot = None;
                }
            }
        }
        for patch in &frame.rows_dirty {
            let Some(row) = self.cells.get_mut(patch.row as usize) else { continue };
            for (x, cell) in patch.cells.iter().enumerate() {
                if let Some(slot) = row.get_mut(x) {
                    *slot = cell.clone();
                }
            }
        }
        self.cursor = frame.cursor;
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

/// A horizontal background span over one row, in cell units (`x` and `w` are
/// column offsets/widths so the caller scales them by the cell width).
#[derive(Clone, Copy, Debug)]
pub struct BgSpan {
    pub x: f32,
    pub w: f32,
    pub color: Hsla,
}

/// Everything needed to paint one row: text + runs, plus merged background
/// spans (painted as quads by the caller, before the text).
#[derive(Clone)]
pub struct RowArt {
    pub text: String,
    pub runs: Vec<TextRun>,
    pub bg: Vec<BgSpan>,
}

/// 0xRRGGBB -> opaque `Hsla`.
fn color(hex: u32) -> Hsla {
    gpui::rgba((hex << 8) | 0xff).into()
}

/// 0xRRGGBB -> `Hsla` at [`BG_ALPHA`].
fn color_a(hex: u32) -> Hsla {
    gpui::rgba((hex << 8) | ((BG_ALPHA * 255.0).round() as u32)).into()
}

/// Alpha for painted cell backgrounds: opaque enough for `ls` coloring and
/// selections to read clearly, translucent enough to keep the glass look.
const BG_ALPHA: f32 = 0.92;

fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

/// Turn one row of cells into paintable art. Adjacent cells with the same
/// resolved style (fg, bg, bold, italic, underline, faint) are merged into a
/// single run, so a typical row shapes as a handful of runs instead of one per
/// cell. Continuation cells after a wide grapheme are skipped for text but
/// still contribute to background coverage.
///
/// Backgrounds are only painted where the program set one (or inverse video
/// implies one); cells with a default background stay transparent so the
/// frosted-glass chrome shows through.
pub fn row_art(cells: &[WireCell], font: &Font, default_fg: Hsla, default_bg: Hsla) -> RowArt {
    let mut text = String::with_capacity(cells.len());
    let mut runs: Vec<TextRun> = Vec::new();
    let mut bg: Vec<BgSpan> = Vec::new();
    let mut open: Option<OpenRun> = None;
    for (x, cell) in cells.iter().enumerate() {
        if cell.cell_width == 0 {
            // Continuation cell: no glyph, but cover it with the wide cell's bg.
            if let Some(hex) = cell.bg {
                push_bg(&mut bg, x as f32, 1.0, color_a(hex));
            }
            continue;
        }
        let symbol = if cell.text.is_empty() { " " } else { cell.text.as_str() };
        let len = symbol.len();
        if len == 0 {
            continue;
        }
        let mut fg = cell.fg.map(color);
        let mut bgc = cell.bg.map(color_a);
        if cell.inverse {
            // True inverse: swap fg and bg, filling either side with the defaults.
            std::mem::swap(&mut fg, &mut bgc);
            let ifg = fg.take().unwrap_or(default_bg);
            let ibg = bgc.take().unwrap_or_else(|| with_alpha(default_fg, BG_ALPHA));
            fg = Some(ifg);
            bgc = Some(ibg);
        }
        let mut fg = fg.unwrap_or(default_fg);
        if cell.faint {
            fg.fade_out(0.5);
        }
        if let Some(bgc) = bgc {
            push_bg(&mut bg, x as f32, cell.cell_width.max(1) as f32, bgc);
        }
        let mut run_font = font.clone();
        if cell.bold {
            run_font.weight = gpui::FontWeight::BOLD;
        }
        if cell.italic {
            run_font.style = gpui::FontStyle::Italic;
        }
        // Merge into the open run while the style is identical.
        let merges = match open.as_ref() {
            Some(o) => o.fg == fg && o.underline == cell.underline && o.font == run_font,
            None => false,
        };
        if !merges {
            if let Some(o) = open.take() {
                runs.push(close_run(o));
            }
            open = Some(OpenRun { len: 0, fg, font: run_font, underline: cell.underline });
        }
        text.push_str(symbol);
        open.as_mut().expect("a run is always open").len += len;
    }
    if let Some(o) = open.take() {
        runs.push(close_run(o));
    }
    RowArt { text, runs, bg }
}

/// The style shared by the cells of the run currently being accumulated.
struct OpenRun {
    len: usize,
    fg: Hsla,
    font: Font,
    underline: bool,
}

fn close_run(o: OpenRun) -> TextRun {
    let underline = if o.underline {
        Some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false })
    } else {
        None
    };
    TextRun {
        len: o.len,
        font: o.font,
        color: o.fg,
        background_color: None,
        underline,
        strikethrough: None,
    }
}

/// Append a background span, merging with the previous one when contiguous and
/// of the same color.
fn push_bg(bg: &mut Vec<BgSpan>, x: f32, w: f32, color: Hsla) {
    if w <= 0.0 {
        return;
    }
    if let Some(last) = bg.last_mut() {
        if (last.x + last.w - x).abs() < f32::EPSILON && last.color == color {
            last.w += w;
            return;
        }
    }
    bg.push(BgSpan { x, w, color });
}

/// The app-wide base text style: monospace.
pub fn base_text_style(window: &mut gpui::Window) -> TextStyle {
    let mut style = window.text_style();
    style.font_family = gpui::SharedString::from("Menlo");
    style.color = hsla_from_hex(0xdddddd);
    style
}

/// A dimmer variant for sidebar/secondary text.
pub fn dim_text_style(window: &mut gpui::Window) -> TextStyle {
    let mut style = base_text_style(window);
    style.color = hsla_from_hex(0x7d7d82);
    style
}

fn hsla_from_hex(hex: u32) -> Hsla {
    gpui::rgba((hex << 8) | 0xff).into()
}

/// Cell geometry ratios for the monospace font: `line_height_ratio` =
/// line-height / font-size and `advance_ratio` = "X" advance / font-size.
/// These let the app scale cells to any size without re-measuring.
pub fn font_ratios(window: &mut gpui::Window, style: &TextStyle) -> (f32, f32) {
    let rem = window.rem_size();
    let font_size = style.font_size.to_pixels(rem);
    let line_height = style.line_height_in_pixels(rem);
    let probe = window.text_system().shape_line(
        gpui::SharedString::from("X"),
        font_size,
        &[TextRun {
            len: 1,
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let advance = f32::from(probe.width);
    (f32::from(line_height) / f32::from(font_size), advance / f32::from(font_size))
}
