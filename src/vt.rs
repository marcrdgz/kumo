//! Safe Rust bindings to the `libghostty-vt` C API.
//!
//! `libghostty-vt` is the headless terminal emulator core extracted from
//! Ghostty. It parses VT escape sequences and maintains the full emulator
//! state (screen, scrollback, styles, cursor, modes) without requiring a
//! GPU or windowing surface, which is what lets the TUI render terminal
//! cells into a ratatui buffer.
//!
//! The C library is compiled at build time from `vendor/libghostty-vt` by
//! `src-tui/build.rs`. This module declares the subset of the C API used by
//! the TUI plus a safe `Terminal` wrapper.
#![allow(dead_code)]

use std::ffi::c_void;
use std::ptr;

// ---------------------------------------------------------------------------
// C primitive types
// ---------------------------------------------------------------------------

/// Opaque terminal handle (`GhosttyTerminal`).
pub type TerminalHandle = *mut c_void;
/// Opaque render state handle (`GhosttyRenderState`).
pub type RenderStateHandle = *mut c_void;
/// Opaque render-state row iterator (`GhosttyRenderStateRowIterator`).
pub type RowIteratorHandle = *mut c_void;
/// Opaque render-state row cells iterator (`GhosttyRenderStateRowCells`).
pub type RowCellsHandle = *mut c_void;

/// Result codes for libghostty-vt operations.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Result {
    Success = 0,
    OutOfMemory = -1,
    InvalidValue = -2,
    OutOfSpace = -3,
    NoValue = -4,
}

impl Result {
    pub fn is_ok(self) -> bool {
        self == Result::Success
    }
}

/// Terminal initialization options.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TerminalOptions {
    pub cols: u16,
    pub rows: u16,
    pub max_scrollback: usize,
}

/// RGB color value.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorRgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// A caller-provided byte buffer for output APIs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Buffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}

/// A borrowed string (C `GhosttyString`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StringSlice {
    pub ptr: *const u8,
    pub len: usize,
}

/// Terminal size information for size reports (C `GhosttySizeReportSize`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// Primary device attributes (DA1).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceAttributesPrimary {
    pub conformance_level: u16,
    pub features: [u16; 64],
    pub num_features: usize,
}

/// Secondary device attributes (DA2).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceAttributesSecondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}

/// Tertiary device attributes (DA3).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceAttributesTertiary {
    pub unit_id: u32,
}

/// Device attributes for all three DA levels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceAttributes {
    pub primary: DeviceAttributesPrimary,
    pub secondary: DeviceAttributesSecondary,
    pub tertiary: DeviceAttributesTertiary,
}

/// Color scheme reported for CSI ? 996 n queries.
pub const COLOR_SCHEME_LIGHT: i32 = 0;
pub const COLOR_SCHEME_DARK: i32 = 1;

// ---------------------------------------------------------------------------
// Native selection types (GhosttyPoint / GridRef / Selection)
// ---------------------------------------------------------------------------

/// A coordinate in the terminal grid (column + row).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointCoordinate {
    pub x: u16,
    pub y: u32,
}

/// Tagged value of a `GhosttyPoint`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union PointValue {
    pub coordinate: PointCoordinate,
    pub _padding: [u64; 2],
}

/// A point in the terminal grid under a coordinate system.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Point {
    pub tag: i32,
    pub value: PointValue,
}

pub const POINT_TAG_VIEWPORT: i32 = 1;
pub const POINT_TAG_SCREEN: i32 = 2;

fn viewport_point(x: u16, y: u32) -> Point {
    Point { tag: POINT_TAG_VIEWPORT, value: PointValue { coordinate: PointCoordinate { x, y } } }
}

fn screen_point(x: u16, y: u32) -> Point {
    Point { tag: POINT_TAG_SCREEN, value: PointValue { coordinate: PointCoordinate { x, y } } }
}

/// A resolved reference to a terminal cell position.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GridRef {
    pub size: usize,
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}

/// A snapshot selection range defined by two grid references (inclusive).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Selection {
    pub size: usize,
    pub start: GridRef,
    pub end: GridRef,
    pub rectangle: bool,
}

/// Row-local selected cell range from the render state (inclusive).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStateRowSelection {
    pub size: usize,
    pub start_x: u16,
    pub end_x: u16,
}

/// Options for one-shot formatting of a terminal selection.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SelectionFormatOptions {
    pub size: usize,
    pub emit: i32,
    pub unwrap: bool,
    pub trim: bool,
    pub selection: *const Selection,
}

/// Plain-text formatter output.
pub const FORMAT_PLAIN: i32 = 0;
/// Terminal option id for the active screen selection.
pub const TERMINAL_OPT_SELECTION: i32 = 21;
/// Render-state row data id for the row-local selection range.
pub const ROW_DATA_SELECTION: i32 = 4;

/// Scrollbar state for the terminal viewport.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// Style color tag.
pub const STYLE_COLOR_NONE: i32 = 0;
pub const STYLE_COLOR_PALETTE: i32 = 1;
pub const STYLE_COLOR_RGB: i32 = 2;

#[repr(C)]
pub union StyleColorValue {
    pub palette: u8,
    pub rgb: ColorRgb,
    pub _padding: u64,
}

// Unions cannot derive `Clone`/`Copy` in Rust, so implement them manually.
#[allow(clippy::non_canonical_clone_impl)]
impl Clone for StyleColorValue {
    fn clone(&self) -> Self {
        StyleColorValue { _padding: unsafe { self._padding } }
    }
}

impl Copy for StyleColorValue {}

/// Style color (tagged union).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StyleColor {
    pub tag: i32,
    pub value: StyleColorValue,
}

/// Complete terminal cell style. Sized struct; the C side validates `size`,
/// so it must always hold `size_of::<Style>()`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Style {
    pub size: usize,
    pub fg_color: StyleColor,
    pub bg_color: StyleColor,
    pub underline_color: StyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: i32,
}

impl Style {
    fn new() -> Self {
        Style {
            size: size_of::<Style>(),
            fg_color: StyleColor { tag: STYLE_COLOR_NONE, value: StyleColorValue { _padding: 0 } },
            bg_color: StyleColor { tag: STYLE_COLOR_NONE, value: StyleColorValue { _padding: 0 } },
            underline_color: StyleColor { tag: STYLE_COLOR_NONE, value: StyleColorValue { _padding: 0 } },
            bold: false,
            italic: false,
            faint: false,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            underline: 0,
        }
    }
}

/// Scroll viewport behavior tag.
pub const SCROLL_VIEWPORT_TOP: i32 = 0;
pub const SCROLL_VIEWPORT_BOTTOM: i32 = 1;
pub const SCROLL_VIEWPORT_DELTA: i32 = 2;
pub const SCROLL_VIEWPORT_ROW: i32 = 3;

#[repr(C)]
pub union ScrollViewportValue {
    pub delta: isize,
    pub row: usize,
    _padding: [u64; 2],
}

/// Scroll viewport tagged union.
#[repr(C)]
pub struct ScrollViewport {
    pub tag: i32,
    pub value: ScrollViewportValue,
}

// ---------------------------------------------------------------------------
// Terminal data / option identifiers
// ---------------------------------------------------------------------------

pub const TERMINAL_OPT_USERDATA: i32 = 0;
pub const TERMINAL_OPT_WRITE_PTY: i32 = 1;
pub const TERMINAL_OPT_BELL: i32 = 2;
pub const TERMINAL_OPT_ENQUIRY: i32 = 3;
pub const TERMINAL_OPT_XTVERSION: i32 = 4;
pub const TERMINAL_OPT_TITLE_CHANGED: i32 = 5;
pub const TERMINAL_OPT_SIZE: i32 = 6;
pub const TERMINAL_OPT_COLOR_SCHEME: i32 = 7;
pub const TERMINAL_OPT_DEVICE_ATTRIBUTES: i32 = 8;
pub const TERMINAL_OPT_COLOR_FOREGROUND: i32 = 11;
pub const TERMINAL_OPT_COLOR_BACKGROUND: i32 = 12;
pub const TERMINAL_OPT_COLOR_CURSOR: i32 = 13;
pub const TERMINAL_OPT_COLOR_PALETTE: i32 = 14;

pub const TERMINAL_DATA_COLS: i32 = 1;
pub const TERMINAL_DATA_ROWS: i32 = 2;
pub const TERMINAL_DATA_SCROLLBAR: i32 = 9;
pub const TERMINAL_DATA_MOUSE_TRACKING: i32 = 11;
pub const TERMINAL_DATA_SCROLLBACK_ROWS: i32 = 15;
pub const TERMINAL_DATA_COLOR_FOREGROUND: i32 = 18;
pub const TERMINAL_DATA_COLOR_BACKGROUND: i32 = 19;

/// DEC private mode 25 (DECTCEM): cursor visible.
pub const MODE_CURSOR_VISIBLE: u16 = 25;
/// DEC private mode 1047: alternate screen.
pub const MODE_ALT_SCREEN: u16 = 1047;
/// ANSI mode 2004: bracketed paste.
pub const MODE_BRACKETED_PASTE: u16 = 2004;

// ---------------------------------------------------------------------------
// Render state data identifiers
// ---------------------------------------------------------------------------

pub const RENDER_DATA_CURSOR_VIEWPORT_HAS_VALUE: i32 = 14;
pub const RENDER_DATA_CURSOR_VIEWPORT_X: i32 = 15;
pub const RENDER_DATA_CURSOR_VIEWPORT_Y: i32 = 16;

pub const RENDER_DATA_ROW_ITERATOR: i32 = 4;
pub const RENDER_STATE_DATA_DIRTY: i32 = 3;
pub const RENDER_STATE_OPTION_DIRTY: i32 = 0;
pub const ROW_DATA_DIRTY: i32 = 1;
pub const ROW_OPTION_DIRTY: i32 = 0;
pub const DIRTY_FALSE: i32 = 0;
pub const DIRTY_PARTIAL: i32 = 1;
pub const DIRTY_FULL: i32 = 2;

pub const ROW_DATA_CELLS: i32 = 3;

pub const ROW_CELLS_DATA_STYLE: i32 = 2;
pub const ROW_CELLS_DATA_BG_COLOR: i32 = 5;
pub const ROW_CELLS_DATA_FG_COLOR: i32 = 6;
pub const ROW_CELLS_DATA_GRAPHEMES_UTF8: i32 = 9;

// ---------------------------------------------------------------------------
// C function declarations
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ghostty_color_palette_default(out: *mut ColorRgb);

    fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut TerminalHandle,
        options: TerminalOptions,
    ) -> Result;
    fn ghostty_terminal_free(terminal: TerminalHandle);
    fn ghostty_terminal_reset(terminal: TerminalHandle);
    fn ghostty_terminal_resize(
        terminal: TerminalHandle,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result;
    fn ghostty_terminal_set(terminal: TerminalHandle, option: i32, value: *const c_void) -> Result;
    fn ghostty_terminal_vt_write(terminal: TerminalHandle, data: *const u8, len: usize);
    fn ghostty_terminal_scroll_viewport(terminal: TerminalHandle, behavior: ScrollViewport);
    fn ghostty_terminal_get(terminal: TerminalHandle, data: i32, out: *mut c_void) -> Result;
    fn ghostty_terminal_mode_get(terminal: TerminalHandle, mode: u16, out_value: *mut bool) -> Result;

    fn ghostty_render_state_new(
        allocator: *const c_void,
        state: *mut RenderStateHandle,
    ) -> Result;
    fn ghostty_render_state_free(state: RenderStateHandle);
    fn ghostty_render_state_update(state: RenderStateHandle, terminal: TerminalHandle) -> Result;
    fn ghostty_render_state_get(state: RenderStateHandle, data: i32, out: *mut c_void) -> Result;
    fn ghostty_render_state_set(state: RenderStateHandle, option: i32, value: *const c_void) -> Result;
    fn ghostty_render_state_row_iterator_new(
        allocator: *const c_void,
        out_iterator: *mut RowIteratorHandle,
    ) -> Result;
    fn ghostty_render_state_row_iterator_free(iterator: RowIteratorHandle);
    fn ghostty_render_state_row_iterator_next(iterator: RowIteratorHandle) -> bool;
    fn ghostty_render_state_row_get(
        iterator: RowIteratorHandle,
        data: i32,
        out: *mut c_void,
    ) -> Result;
    fn ghostty_render_state_row_set(
        iterator: RowIteratorHandle,
        option: i32,
        value: *const c_void,
    ) -> Result;
    fn ghostty_render_state_row_cells_new(
        allocator: *const c_void,
        out_cells: *mut RowCellsHandle,
    ) -> Result;
    fn ghostty_render_state_row_cells_free(cells: RowCellsHandle);
    fn ghostty_render_state_row_cells_next(cells: RowCellsHandle) -> bool;
    fn ghostty_render_state_row_cells_get(
        cells: RowCellsHandle,
        data: i32,
        out: *mut c_void,
    ) -> Result;

    fn ghostty_terminal_grid_ref(
        terminal: TerminalHandle,
        point: Point,
        out_ref: *mut GridRef,
    ) -> Result;
    fn ghostty_terminal_selection_format_buf(
        terminal: TerminalHandle,
        options: SelectionFormatOptions,
        buf: *mut u8,
        buf_len: usize,
        out_written: *mut usize,
    ) -> Result;
}

// ---------------------------------------------------------------------------
// Safe wrapper
// ---------------------------------------------------------------------------

/// Write pty callback: forwards query responses to the installed pty writer.
unsafe extern "C" fn write_pty_cb(
    _term: TerminalHandle,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() {
        return;
    }
    // USERDATA points at a heap `Option<*mut dyn Write + Send>` slot (None =
    // no writer installed yet).
    let cell: *mut Option<*mut (dyn std::io::Write + Send)> = userdata as *mut _;
    let Some(writer) = *cell else {
        return;
    };
    let writer = &mut *writer;
    let bytes = std::slice::from_raw_parts(data, len);
    let _ = writer.write_all(bytes);
}

/// Size callback: report the current terminal geometry for XTWINOPS queries.
unsafe extern "C" fn size_cb(
    term: TerminalHandle,
    _userdata: *mut c_void,
    out_size: *mut SizeReportSize,
) -> bool {
    if out_size.is_null() {
        return false;
    }
    let mut size = SizeReportSize { rows: 0, columns: 0, cell_width: 0, cell_height: 0 };
    if ghostty_terminal_get(term, TERMINAL_DATA_COLS, &mut size.columns as *mut u16 as *mut c_void).is_ok()
        && ghostty_terminal_get(term, TERMINAL_DATA_ROWS, &mut size.rows as *mut u16 as *mut c_void).is_ok()
    {
        *out_size = size;
        true
    } else {
        false
    }
}

/// Color scheme callback: report dark.
unsafe extern "C" fn color_scheme_cb(
    _term: TerminalHandle,
    _userdata: *mut c_void,
    out_scheme: *mut i32,
) -> bool {
    if out_scheme.is_null() {
        return false;
    }
    *out_scheme = COLOR_SCHEME_DARK;
    true
}

/// ENQ callback: no response.
unsafe extern "C" fn enquiry_cb(_term: TerminalHandle, _userdata: *mut c_void) -> StringSlice {
    static EMPTY: &[u8] = b"";
    StringSlice { ptr: EMPTY.as_ptr(), len: 0 }
}

/// A viewport cell with resolved colors and a UTF-8 grapheme.
#[derive(Clone, Debug)]
pub struct RenderCell {
    pub text: String,
    pub fg: ColorRgb,
    pub bg: ColorRgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub faint: bool,
}

/// A terminal emulator instance backed by libghostty-vt.
///
/// Owns the terminal handle plus the render state used to draw the viewport
/// into a cell buffer. Operations that touch the render state require `&mut`
/// because the state is lazily refreshed from the terminal.
pub struct Terminal {
    term: TerminalHandle,
    render: RenderStateHandle,
    rows: u16,
    cols: u16,
    /// Scratch buffer for `graphemes_utf8` queries; reused across renders.
    scratch: Vec<u8>,
    /// Cached viewport cursor position from the last `refresh`.
    cursor: Option<(u16, u16)>,
    /// Cached default foreground/background from the last `refresh`.
    default_fg: ColorRgb,
    default_bg: ColorRgb,
    /// Cached scrollbar state from the last `refresh`.
    scrollbar: TerminalScrollbar,
    /// Heap cell holding the active pty writer as an `Option` (None = not
    /// installed). Its address is the USERDATA passed to the write_pty
    /// callback, which writes responses through the current writer.
    sink_cell: Box<Option<*mut (dyn std::io::Write + Send)>>,
}

impl Terminal {
    /// Create a terminal of `cols` x `rows` with `max_scrollback` history rows
    /// and configure the Catppuccin theme as default colors.
    pub fn new(cols: u16, rows: u16, max_scrollback: usize, palette: &[ColorRgb; 16]) -> anyhow::Result<Terminal> {
        let mut term: TerminalHandle = ptr::null_mut();
        let options = TerminalOptions {
            cols: cols.max(1),
            rows: rows.max(1),
            max_scrollback,
        };
        unsafe {
            if !ghostty_terminal_new(ptr::null(), &mut term, options).is_ok() || term.is_null() {
                return Err(anyhow::anyhow!("libghostty-vt: failed to create terminal"));
            }
        }

        let mut render: RenderStateHandle = ptr::null_mut();
        unsafe {
            if !ghostty_render_state_new(ptr::null(), &mut render).is_ok() || render.is_null() {
                ghostty_terminal_free(term);
                return Err(anyhow::anyhow!("libghostty-vt: failed to create render state"));
            }
        }

        // Build a full 256-entry palette from the ghostty default and override
        // the ANSI 0-15 entries with the theme colors.
        let mut full_palette = [ColorRgb::new(0, 0, 0); 256];
        unsafe {
            ghostty_color_palette_default(full_palette.as_mut_ptr());
        }
        full_palette[..16].copy_from_slice(palette);

        let fg = ColorRgb::new(0xcd, 0xd6, 0xf4);
        let bg = ColorRgb::new(0x1e, 0x1e, 0x2e);
        let cursor = ColorRgb::new(0xb4, 0xbe, 0xfe);

        unsafe {
            ghostty_terminal_set(term, TERMINAL_OPT_COLOR_FOREGROUND, &fg as *const _ as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_COLOR_BACKGROUND, &bg as *const _ as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_COLOR_CURSOR, &cursor as *const _ as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_COLOR_PALETTE, full_palette.as_ptr() as *const c_void);
        }

        // The write_pty callback needs a stable place to find the current pty
        // writer. Allocate a heap cell and use it as the USERDATA pointer.
        let sink_cell: Box<Option<*mut (dyn std::io::Write + Send)>> = Box::new(None);
        let userdata = (&*sink_cell) as *const Option<*mut (dyn std::io::Write + Send)> as *mut c_void;
        unsafe {
            ghostty_terminal_set(term, TERMINAL_OPT_USERDATA, userdata as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_WRITE_PTY, write_pty_cb as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_SIZE, size_cb as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_COLOR_SCHEME, color_scheme_cb as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_ENQUIRY, enquiry_cb as *const c_void);
        }

        Ok(Terminal {
            term,
            render,
            rows,
            cols,
            scratch: Vec::with_capacity(64),
            cursor: None,
            default_fg: fg,
            default_bg: bg,
            scrollbar: TerminalScrollbar::default(),
            sink_cell,
        })
    }

    /// Install the writer that query responses are written to. The pointer
    /// must point into a stable heap allocation (e.g. a `Box`'s contents) and
    /// stay valid for the lifetime of the terminal.
    pub fn set_write_sink(&mut self, writer: *mut (dyn std::io::Write + Send)) {
        *self.sink_cell = Some(writer);
    }

    /// Install a linear selection covering two viewport coordinates
    /// (inclusive), matching a mouse drag. Returns false if either endpoint
    /// is outside the grid. The terminal converts the refs into owned tracked
    /// state, so the selection survives subsequent output/scroll.
    pub fn set_selection(&mut self, start: (u16, u16), end: (u16, u16)) -> bool {
        let mut start_ref =
            GridRef { size: size_of::<GridRef>(), node: ptr::null_mut(), x: 0, y: 0 };
        let mut end_ref = GridRef { size: size_of::<GridRef>(), node: ptr::null_mut(), x: 0, y: 0 };
        unsafe {
            if !ghostty_terminal_grid_ref(self.term, viewport_point(start.0, start.1 as u32), &mut start_ref).is_ok()
                || !ghostty_terminal_grid_ref(self.term, viewport_point(end.0, end.1 as u32), &mut end_ref).is_ok()
            {
                return false;
            }
        }
        let selection = Selection {
            size: size_of::<Selection>(),
            start: start_ref,
            end: end_ref,
            rectangle: false,
        };
        unsafe {
            ghostty_terminal_set(self.term, TERMINAL_OPT_SELECTION, &selection as *const Selection as *const c_void)
                .is_ok()
        }
    }

    /// Clear the active terminal selection.
    pub fn clear_selection(&mut self) {
        unsafe {
            ghostty_terminal_set(self.term, TERMINAL_OPT_SELECTION, ptr::null());
        }
    }

    /// Extract the active selection as plain text: soft-wrapped lines are
    /// unwrapped and trailing whitespace is trimmed, matching Ghostty's
    /// clipboard behavior.
    pub fn selected_text(&mut self) -> Option<String> {
        let options = SelectionFormatOptions {
            size: size_of::<SelectionFormatOptions>(),
            emit: FORMAT_PLAIN,
            unwrap: true,
            trim: true,
            selection: ptr::null(),
        };
        unsafe {
            let mut written = 0usize;
            let res =
                ghostty_terminal_selection_format_buf(self.term, options, ptr::null_mut(), 0, &mut written);
            if res != Result::OutOfSpace || written == 0 {
                return None;
            }
            let mut buf = vec![0u8; written];
            let mut filled = 0usize;
            if !ghostty_terminal_selection_format_buf(self.term, options, buf.as_mut_ptr(), buf.len(), &mut filled)
                .is_ok()
            {
                return None;
            }
            buf.truncate(filled.min(buf.len()));
            Some(String::from_utf8_lossy(&buf).into_owned())
        }
    }

    /// Feed raw VT-encoded output from the PTY into the emulator.
    pub fn write(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        unsafe {
            ghostty_terminal_vt_write(self.term, data.as_ptr(), data.len());
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        unsafe {
            ghostty_terminal_resize(self.term, cols, rows, 0, 0);
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn reset(&mut self) {
        unsafe {
            ghostty_terminal_reset(self.term);
        }
    }

    /// Scroll the viewport by `delta` rows (positive scrolls down).
    pub fn scroll(&mut self, delta: i32) {
        let behavior = ScrollViewport {
            tag: SCROLL_VIEWPORT_DELTA,
            value: ScrollViewportValue { delta: delta as isize },
        };
        unsafe {
            ghostty_terminal_scroll_viewport(self.term, behavior);
        }
    }

    /// Scroll the viewport back to the active area.
    pub fn scroll_bottom(&mut self) {
        let behavior = ScrollViewport {
            tag: SCROLL_VIEWPORT_BOTTOM,
            value: ScrollViewportValue { _padding: [0; 2] },
        };
        unsafe {
            ghostty_terminal_scroll_viewport(self.term, behavior);
        }
    }

    /// True when any mouse tracking mode is active.
    pub fn mouse_tracking(&self) -> bool {
        let mut out: bool = false;
        unsafe {
            ghostty_terminal_get(self.term, TERMINAL_DATA_MOUSE_TRACKING, &mut out as *mut bool as *mut c_void);
        }
        out
    }

    /// Current value of a terminal mode.
    pub fn mode_get(&self, mode: u16) -> bool {
        let mut out: bool = false;
        unsafe {
            ghostty_terminal_mode_get(self.term, mode, &mut out);
        }
        out
    }

    /// Refresh render state, viewport cursor, default colors, and scrollbar
    /// from the terminal. Called once per frame before reading cells.
    pub fn refresh(&mut self) {
        unsafe {
            if !ghostty_render_state_update(self.render, self.term).is_ok() {
                return;
            }
        }

        let mut fg = self.default_fg;
        let mut bg = self.default_bg;
        unsafe {
            let mut c: ColorRgb = self.default_fg;
            if ghostty_terminal_get(self.term, TERMINAL_DATA_COLOR_FOREGROUND, &mut c as *mut _ as *mut c_void).is_ok() {
                fg = c;
            }
            let mut c: ColorRgb = self.default_bg;
            if ghostty_terminal_get(self.term, TERMINAL_DATA_COLOR_BACKGROUND, &mut c as *mut _ as *mut c_void).is_ok() {
                bg = c;
            }
        }
        self.default_fg = fg;
        self.default_bg = bg;

        let mut scrollbar = TerminalScrollbar::default();
        unsafe {
            ghostty_terminal_get(self.term, TERMINAL_DATA_SCROLLBAR, &mut scrollbar as *mut _ as *mut c_void);
        }
        self.scrollbar = scrollbar;

        let mut has: bool = false;
        let mut cx: u16 = 0;
        let mut cy: u16 = 0;
        unsafe {
            ghostty_render_state_get(self.render, RENDER_DATA_CURSOR_VIEWPORT_HAS_VALUE, &mut has as *mut bool as *mut c_void);
            if has {
                ghostty_render_state_get(self.render, RENDER_DATA_CURSOR_VIEWPORT_X, &mut cx as *mut u16 as *mut c_void);
                ghostty_render_state_get(self.render, RENDER_DATA_CURSOR_VIEWPORT_Y, &mut cy as *mut u16 as *mut c_void);
            }
        }
        self.cursor = if has { Some((cx, cy)) } else { None };
    }

    /// Global render-state dirty level (DIRTY_FALSE/PARTIAL/FULL) after a
    /// `refresh`.
    pub fn render_dirty_level(&self) -> i32 {
        let mut d: i32 = 0;
        unsafe {
            ghostty_render_state_get(self.render, RENDER_STATE_DATA_DIRTY, &mut d as *mut i32 as *mut c_void);
        }
        d
    }

    /// Indices of the rows that changed since the last render-state update.
    pub fn dirty_rows(&self) -> Vec<usize> {
        let mut out = Vec::new();
        unsafe {
            let mut iter: RowIteratorHandle = ptr::null_mut();
            if !ghostty_render_state_row_iterator_new(ptr::null(), &mut iter).is_ok() {
                return out;
            }
            ghostty_render_state_get(self.render, RENDER_DATA_ROW_ITERATOR, &mut iter as *mut RowIteratorHandle as *mut c_void);
            let mut row = 0usize;
            while ghostty_render_state_row_iterator_next(iter) {
                let mut d: bool = false;
                ghostty_render_state_row_get(iter, ROW_DATA_DIRTY, &mut d as *mut bool as *mut c_void);
                if d {
                    out.push(row);
                }
                row += 1;
            }
            ghostty_render_state_row_iterator_free(iter);
        }
        out
    }

    /// Reset the per-row dirty flags and the global dirty state to clean, so
    /// the next `refresh` only reports changes since this point.
    pub fn clear_dirty(&mut self) {
        unsafe {
            let mut iter: RowIteratorHandle = ptr::null_mut();
            if !ghostty_render_state_row_iterator_new(ptr::null(), &mut iter).is_ok() {
                return;
            }
            ghostty_render_state_get(self.render, RENDER_DATA_ROW_ITERATOR, &mut iter as *mut RowIteratorHandle as *mut c_void);
            let clear: bool = false;
            while ghostty_render_state_row_iterator_next(iter) {
                ghostty_render_state_row_set(iter, ROW_OPTION_DIRTY, &clear as *const bool as *const c_void);
            }
            ghostty_render_state_row_iterator_free(iter);
        }
        let clean: i32 = DIRTY_FALSE;
        unsafe {
            ghostty_render_state_set(self.render, RENDER_STATE_OPTION_DIRTY, &clean as *const i32 as *const c_void);
        }
    }

    /// Viewport-relative cursor position from the last `refresh`.
    pub fn cursor_pos(&self) -> Option<(u16, u16)> {
        self.cursor
    }

    /// Text of the current viewport, one line per row (trailing whitespace
    /// trimmed per row). Used by the agent-state detection.
    pub fn screen_text(&mut self) -> String {
        use std::collections::HashMap;
        self.refresh();
        let mut cells: HashMap<usize, Vec<(usize, char)>> = HashMap::new();
        self.for_each_cell(|row, col, rc, _selected, _row_dirty| {
            let ch = rc.text.chars().next().unwrap_or(' ');
            cells.entry(row).or_default().push((col, ch));
        });
        let mut rows: Vec<usize> = cells.keys().copied().collect();
        rows.sort_unstable();
        let mut out = String::new();
        for row in rows {
            let mut cols = cells[&row].clone();
            cols.sort_unstable_by_key(|(c, _)| *c);
            let line: String = cols.into_iter().map(|(_, ch)| ch).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Scrollbar state from the last `refresh`.
    pub fn scrollbar(&self) -> TerminalScrollbar {
        self.scrollbar
    }

    pub fn default_fg(&self) -> ColorRgb {
        self.default_fg
    }

    pub fn default_bg(&self) -> ColorRgb {
        self.default_bg
    }

    /// Whether the cursor is visible (DEC mode 25) and inside the viewport.
    pub fn cursor_visible(&self) -> bool {
        self.mode_get(MODE_CURSOR_VISIBLE) && self.cursor.is_some()
    }

    /// Iterate every populated cell of the current viewport.
    ///
    /// `f` is called with `(viewport_row, viewport_col, &RenderCell, selected,
    /// row_dirty)` for each cell that either has text or carries an explicit
    /// background. `selected` is true inside the terminal's active selection;
    /// `row_dirty` is true when the row changed since the last render-state
    /// update.
    pub fn for_each_cell(&mut self, mut f: impl FnMut(usize, usize, &RenderCell, bool, bool)) {
        unsafe {
            let mut iter: RowIteratorHandle = ptr::null_mut();
            if !ghostty_render_state_row_iterator_new(ptr::null(), &mut iter).is_ok() {
                return;
            }
            // Populate the iterator's row data from the render state.
            ghostty_render_state_get(self.render, RENDER_DATA_ROW_ITERATOR, &mut iter as *mut RowIteratorHandle as *mut c_void);
            let mut cells: RowCellsHandle = ptr::null_mut();
            if !ghostty_render_state_row_cells_new(ptr::null(), &mut cells).is_ok() {
                ghostty_render_state_row_iterator_free(iter);
                return;
            }

            let mut row_idx: usize = 0;
            while ghostty_render_state_row_iterator_next(iter) {
                let mut row_dirty: bool = false;
                ghostty_render_state_row_get(iter, ROW_DATA_DIRTY, &mut row_dirty as *mut bool as *mut c_void);
                // Row-local selection range (inclusive), if this row intersects.
                let mut row_sel = RenderStateRowSelection {
                    size: size_of::<RenderStateRowSelection>(),
                    start_x: 0,
                    end_x: 0,
                };
                let sel_ok = ghostty_render_state_row_get(
                    iter,
                    ROW_DATA_SELECTION,
                    &mut row_sel as *mut RenderStateRowSelection as *mut c_void,
                )
                .is_ok();
                // Populate the cells container with the current row's data.
                ghostty_render_state_row_get(iter, ROW_DATA_CELLS, &mut cells as *mut RowCellsHandle as *mut c_void);
                let mut col_idx: usize = 0;
                while ghostty_render_state_row_cells_next(cells) {
                    if let Some(rc) = self.read_cell(cells) {
                        let selected = sel_ok
                            && col_idx >= row_sel.start_x as usize
                            && col_idx <= row_sel.end_x as usize;
                        f(row_idx, col_idx, &rc, selected, row_dirty);
                    }
                    col_idx += 1;
                }
                row_idx += 1;
            }

            ghostty_render_state_row_cells_free(cells);
            ghostty_render_state_row_iterator_free(iter);
        }
    }

    /// Read the render-state cell the `cells` iterator currently points at.
    fn read_cell(&mut self, cells: RowCellsHandle) -> Option<RenderCell> {
        unsafe {
            // Read the cell's grapheme cluster with a reusable scratch buffer,
            // growing it only when a cell holds more text than fits (so the
            // common single-codepoint case needs one FFI call, not two).
            let mut out = Buffer { ptr: self.scratch.as_mut_ptr(), cap: self.scratch.len(), len: 0 };
            let mut res =
                ghostty_render_state_row_cells_get(cells, ROW_CELLS_DATA_GRAPHEMES_UTF8, &mut out as *mut Buffer as *mut c_void);
            if res == Result::OutOfSpace {
                self.scratch.resize(out.len, 0);
                out.ptr = self.scratch.as_mut_ptr();
                out.cap = self.scratch.len();
                out.len = 0;
                res =
                    ghostty_render_state_row_cells_get(cells, ROW_CELLS_DATA_GRAPHEMES_UTF8, &mut out as *mut Buffer as *mut c_void);
            }
            let has_text = res.is_ok() && out.len > 0;
            let text = if has_text {
                String::from_utf8_lossy(&self.scratch[..out.len]).into_owned()
            } else {
                String::new()
            };

            let mut style = Style::new();
            ghostty_render_state_row_cells_get(cells, ROW_CELLS_DATA_STYLE, &mut style as *mut Style as *mut c_void);

            let mut fg = self.default_fg;
            let mut bg = self.default_bg;
            let fg_ok = ghostty_render_state_row_cells_get(cells, ROW_CELLS_DATA_FG_COLOR, &mut fg as *mut ColorRgb as *mut c_void).is_ok();
            let bg_ok = ghostty_render_state_row_cells_get(cells, ROW_CELLS_DATA_BG_COLOR, &mut bg as *mut ColorRgb as *mut c_void).is_ok();

            // Skip fully blank cells with no explicit background; the renderer
            // fills those with the default background.
            if !has_text && !bg_ok {
                return None;
            }

            let _ = fg_ok;
            Some(RenderCell {
                text,
                fg,
                bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline != 0,
                inverse: style.inverse,
                faint: style.faint,
            })
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ghostty_render_state_free(self.render);
            ghostty_terminal_free(self.term);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> [ColorRgb; 16] {
        [ColorRgb::new(0x00, 0x00, 0x00); 16]
    }

    fn collect(t: &mut Terminal) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        t.for_each_cell(|r, c, rc, _selected, _row_dirty| {
            if !rc.text.is_empty() {
                out.push((r, c, rc.text.clone()));
            }
        });
        out
    }

    #[test]
    fn renders_plain_text() {
        let mut t = Terminal::new(10, 4, 100, &palette()).unwrap();
        t.write(b"hello");
        t.refresh();
        let cells = collect(&mut t);
        let texts: Vec<&str> = cells.iter().map(|(_, _, s)| s.as_str()).collect();
        assert_eq!(texts, vec!["h", "e", "l", "l", "o"]);
    }

    #[test]
    fn renders_ansi_colors_and_styles() {
        let mut t = Terminal::new(10, 4, 100, &palette()).unwrap();
        t.write(b"\x1b[31mred\x1b[1mbold\x1b[0mplain");
        t.refresh();
        let cells = collect(&mut t);
        let texts: Vec<&str> = cells.iter().map(|(_, _, s)| s.as_str()).collect();
        assert_eq!(texts, vec!["r", "e", "d", "b", "o", "l", "d", "p", "l", "a", "i", "n"]);
    }

    #[test]
    fn responds_to_da_query() {
        // The shell blocks on DA/DSR queries until the terminal answers them.
        // Verify ghostty emits responses through the write_pty callback.
        struct BufWriter {
            buf: Vec<u8>,
        }
        impl std::io::Write for BufWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.buf.extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut buf = BufWriter { buf: Vec::new() };
        let sink: *mut (dyn std::io::Write + Send) = &mut buf as *mut _;
        let mut t = Terminal::new(10, 4, 100, &palette()).unwrap();
        t.set_write_sink(sink);
        t.write(b"\x1b[c"); // DA1
        let s = String::from_utf8_lossy(&buf.buf);
        assert!(
            !s.is_empty(),
            "no response to DA query"
        );
        assert!(s.starts_with("\x1b[?"), "expected CSI ? response, got {s:?}");
    }

    #[test]
    fn writes_cursor_and_scroll() {
        let mut t = Terminal::new(10, 4, 100, &palette()).unwrap();
        t.write(b"line1\nline2\nline3\nline4\nline5");
        t.refresh();
        assert!(t.cursor_pos().is_some());
        t.scroll(-2);
        t.refresh();
        assert!(t.scrollbar().total > 0);
    }

    #[test]
    fn native_selection_highlights_and_extracts() {
        let mut t = Terminal::new(20, 5, 100, &palette()).unwrap();
        t.write(b"hello world\nsecond line");
        t.refresh();
        assert!(t.set_selection((0, 0), (4, 0)), "set_selection should succeed");
        t.refresh();
        let mut selected = Vec::new();
        t.for_each_cell(|r, c, rc, s, _row_dirty| {
            if s {
                selected.push((r, c, rc.text.clone()));
            }
        });
        assert!(!selected.is_empty(), "row should intersect the selection");
        assert_eq!(t.selected_text().as_deref(), Some("hello"));

        // Clearing removes the highlight.
        t.clear_selection();
        t.refresh();
        let mut after = 0usize;
        t.for_each_cell(|_, _, _, s, _row_dirty| {
            if s {
                after += 1;
            }
        });
        assert_eq!(after, 0);
    }
}

