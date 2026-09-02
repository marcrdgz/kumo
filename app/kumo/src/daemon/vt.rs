//! Safe Rust bindings to the `libghostty-vt` C API.
//!
//! `libghostty-vt` is the headless terminal emulator core extracted from
//! Ghostty. It parses VT escape sequences and maintains the full emulator
//! state (screen, scrollback, styles, cursor, modes) without requiring a
//! GPU or windowing surface, which is what lets the daemon render terminal
//! cells into a ratatui buffer.
//!
//! The C library is compiled at build time from `vendor/libghostty-vt` by
//! `app/daemon/build.rs`. This module declares the subset of the C API used
//! by the daemon plus a safe `Terminal` wrapper.
#![allow(dead_code)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

use kumo_core::color::ColorRgb;

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
/// Opaque snapshot decoder handle (`GhosttySnapshotDecoder`).
pub type SnapshotDecoderHandle = *mut c_void;

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
}

/// Terminal mode configuration (mode + value).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TerminalModeConfig {
    pub mode: u16,
    pub value: bool,
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

/// Clipboard location (which clipboard to write to).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClipboardLocation(pub i32);

/// A single MIME representation of clipboard content.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClipboardContent {
    pub mime: StringSlice,
    pub data: StringSlice,
}

/// A semantic, atomic clipboard write request.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClipboardWrite {
    pub size: usize,
    pub location: ClipboardLocation,
    pub contents: *const ClipboardContent,
    pub contents_len: usize,
}

/// Result of a clipboard write operation.
#[repr(i32)]
#[derive(Clone, Copy)]
pub enum ClipboardWriteResult {
    Success = 0,
    Denied = 1,
    Unsupported = 2,
    Busy = 3,
    InvalidData = 4,
    IoError = 5,
}

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
    Point {
        tag: POINT_TAG_VIEWPORT,
        value: PointValue {
            coordinate: PointCoordinate { x, y },
        },
    }
}

fn screen_point(x: u16, y: u32) -> Point {
    Point {
        tag: POINT_TAG_SCREEN,
        value: PointValue {
            coordinate: PointCoordinate { x, y },
        },
    }
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

/// One search hit in screen coordinates (0 = top of scrollback).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub row: u32,
    pub start_col: u16,
    pub end_col: u16,
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
        StyleColorValue {
            _padding: unsafe { self._padding },
        }
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
            fg_color: StyleColor {
                tag: STYLE_COLOR_NONE,
                value: StyleColorValue { _padding: 0 },
            },
            bg_color: StyleColor {
                tag: STYLE_COLOR_NONE,
                value: StyleColorValue { _padding: 0 },
            },
            underline_color: StyleColor {
                tag: STYLE_COLOR_NONE,
                value: StyleColorValue { _padding: 0 },
            },
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

/// Render-state cursor information (sized struct for GHOSTTY_RENDER_STATE_DATA_CURSOR).
#[repr(C)]
pub struct RenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: i32,
}

impl RenderStateCursor {
    pub fn new() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            viewport_has_value: false,
            viewport_x: 0,
            viewport_y: 0,
            wide_tail: false,
            visible: false,
            blinking: false,
            password_input: false,
            visual_style: 0,
        }
    }
}

/// Render-state color information (sized struct for GHOSTTY_RENDER_STATE_DATA_COLORS).
#[repr(C)]
pub struct RenderStateColors {
    pub size: usize,
    pub background: ColorRgb,
    pub foreground: ColorRgb,
}

impl RenderStateColors {
    pub fn new() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            background: ColorRgb { r: 0, g: 0, b: 0 },
            foreground: ColorRgb { r: 0, g: 0, b: 0 },
        }
    }
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
pub const TERMINAL_OPT_PWD_CHANGED: i32 = 25;
pub const TERMINAL_OPT_CLIPBOARD_WRITE: i32 = 26;
pub const TERMINAL_OPT_MODE: i32 = 34;
/// Maximum number of physical lines retained in scrollback. Set after
/// `ghostty_terminal_new` (the vendored constructor no longer takes it).
pub const TERMINAL_OPT_SCROLLBACK_MAX_LINES: i32 = 28;
pub const TERMINAL_OPT_CONTINUATION_MAX_BYTES: i32 = 31;

pub const TERMINAL_DATA_COLS: i32 = 1;
pub const TERMINAL_DATA_ROWS: i32 = 2;
pub const TERMINAL_DATA_SCROLLBAR: i32 = 9;
pub const TERMINAL_DATA_MOUSE_TRACKING: i32 = 11;
/// Window title set via OSC 0 / OSC 2 (borrowed `GhosttyString`). Claude Code
/// paints its live state here: a braille spinner while working, `✳ ` idle.
pub const TERMINAL_DATA_TITLE: i32 = 12;
pub const TERMINAL_DATA_PWD: i32 = 13;
pub const TERMINAL_DATA_TOTAL_ROWS: i32 = 14;
pub const TERMINAL_DATA_SCROLLBACK_ROWS: i32 = 15;
pub const TERMINAL_DATA_COLOR_FOREGROUND: i32 = 18;
pub const TERMINAL_DATA_COLOR_BACKGROUND: i32 = 19;
pub const TERMINAL_DATA_MODE: i32 = 37;

/// DEC private mode 25 (DECTCEM): cursor visible.
pub const MODE_CURSOR_VISIBLE: u16 = 25;
/// DEC private mode 1047: alternate screen.
pub const MODE_ALT_SCREEN: u16 = 1047;
/// DEC private mode 1000: normal mouse reporting (press/release).
pub const MODE_MOUSE_NORMAL: u16 = 1000;
/// ANSI mode 2004: bracketed paste.
pub const MODE_BRACKETED_PASTE: u16 = 2004;
/// DEC private mode 2027: grapheme cluster mode.
pub const MODE_GRAPHEME_CLUSTER: u16 = 2027;

// ---------------------------------------------------------------------------
// Render state data identifiers
// ---------------------------------------------------------------------------

pub const RENDER_DATA_CURSOR_VIEWPORT_HAS_VALUE: i32 = 14;
pub const RENDER_DATA_CURSOR_VIEWPORT_X: i32 = 15;
pub const RENDER_DATA_CURSOR_VIEWPORT_Y: i32 = 16;

pub const RENDER_DATA_ROW_ITERATOR: i32 = 4;
pub const RENDER_STATE_DATA_DIRTY: i32 = 3;
pub const RENDER_STATE_DATA_CURSOR: i32 = 18;
pub const RENDER_STATE_DATA_COLORS: i32 = 19;
pub const RENDER_STATE_OPTION_DIRTY: i32 = 0;
pub const ROW_DATA_DIRTY: i32 = 1;
pub const ROW_OPTION_DIRTY: i32 = 0;
pub const DIRTY_FALSE: i32 = 0;
pub const DIRTY_PARTIAL: i32 = 1;
pub const DIRTY_FULL: i32 = 2;

pub const ROW_DATA_CELLS: i32 = 3;

pub const ROW_CELLS_DATA_RAW: i32 = 1;
pub const ROW_CELLS_DATA_STYLE: i32 = 2;
pub const ROW_CELLS_DATA_BG_COLOR: i32 = 5;
pub const ROW_CELLS_DATA_FG_COLOR: i32 = 6;
pub const ROW_CELLS_DATA_GRAPHEMES_UTF8: i32 = 9;

/// Cell data id for `ghostty_cell_get`: whether the cell is part of an
/// OSC 8 hyperlink.
pub const CELL_DATA_HAS_HYPERLINK: i32 = 7;
/// Cell data id for semantic content (OSC 133).
pub const CELL_DATA_SEMANTIC_CONTENT: i32 = 9;
/// Row data id for semantic prompt state (OSC 133).
pub const ROW_DATA_SEMANTIC_PROMPT: i32 = 6;

pub type GhosttyRow = u64;

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowSemanticPrompt {
    None = 0,
    Prompt = 1,
    Continuation = 2,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellSemanticContent {
    Output = 0,
    Input = 1,
    Prompt = 2,
}

/// Structured prompt block extracted from OSC 133 semantic markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptBlock {
    pub prompt_start: u32,
    pub prompt_end: u32,
    pub output_start: u32,
    pub output_end: u32,
    pub prompt_text: String,
    pub output_text: String,
}

// ---------------------------------------------------------------------------
// C function declarations
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ghostty_color_palette_default(out: *mut ColorRgb);

    fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut TerminalHandle,
        cols: u16,
        rows: u16,
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
    fn ghostty_terminal_get_multi(
        terminal: TerminalHandle,
        count: usize,
        keys: *const i32,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> Result;

    fn ghostty_render_state_new(allocator: *const c_void, state: *mut RenderStateHandle) -> Result;
    fn ghostty_render_state_free(state: RenderStateHandle);
    fn ghostty_render_state_update(state: RenderStateHandle, terminal: TerminalHandle) -> Result;
    fn ghostty_render_state_get(state: RenderStateHandle, data: i32, out: *mut c_void) -> Result;
    fn ghostty_render_state_get_multi(
        state: RenderStateHandle,
        count: usize,
        keys: *const i32,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> Result;
    fn ghostty_render_state_set(
        state: RenderStateHandle,
        option: i32,
        value: *const c_void,
    ) -> Result;
    fn ghostty_render_state_clean(state: RenderStateHandle) -> Result;
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
    fn ghostty_render_state_row_get_multi(
        iterator: RowIteratorHandle,
        count: usize,
        keys: *const i32,
        values: *mut *mut c_void,
        out_written: *mut usize,
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
    fn ghostty_cell_get(cell: u64, data: i32, out: *mut c_void) -> Result;
    fn ghostty_grid_ref_cell(ref_: *const GridRef, out_cell: *mut u64) -> Result;
    fn ghostty_grid_ref_row(ref_: *const GridRef, out_row: *mut GhosttyRow) -> Result;
    fn ghostty_row_get(row: GhosttyRow, data: i32, out: *mut c_void) -> Result;
    fn ghostty_grid_ref_hyperlink_uri(
        ref_: *const GridRef,
        buf: *mut u8,
        buf_len: usize,
        out_len: *mut usize,
    ) -> Result;

    /// Display width of a single codepoint in grid cells (0, 1, or 2),
    /// using the exact width table the terminal uses when laying out text.
    fn ghostty_unicode_codepoint_width(cp: u32) -> u8;

    /// Measures the first grapheme cluster in `cps` with the terminal's exact
    /// segmentation and cluster width rules (VS16/VS15, ZWJ, skin tones,
    /// regional indicators). Returns the number of codepoints consumed and
    /// stores the cluster's width (0-2) in `width`.
    fn ghostty_unicode_grapheme_width(cps: *const u32, len: usize, width: *mut u8) -> usize;

    fn ghostty_snapshot_encode_alloc(
        terminal: TerminalHandle,
        allocator: *const c_void,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> Result;
    fn ghostty_free(allocator: *const c_void, ptr: *mut u8, len: usize);
    fn ghostty_snapshot_decoder_new_buf(
        allocator: *const c_void,
        decoder: *mut SnapshotDecoderHandle,
        ptr: *const u8,
        len: usize,
    ) -> Result;
    fn ghostty_snapshot_decoder_decode(
        decoder: SnapshotDecoderHandle,
        terminal: *mut TerminalHandle,
    ) -> Result;
    fn ghostty_snapshot_decoder_free(decoder: SnapshotDecoderHandle);
}

// ---------------------------------------------------------------------------
// Safe wrapper
// ---------------------------------------------------------------------------

/// Terminal display width in grid cells (0, 1, or 2) of a full grapheme
/// cluster, using the exact rules the terminal applies when printing text:
/// VS16 forces emoji presentation (wide), VS15 forces text presentation
/// (narrow), and ZWJ/skin-tone/regional-indicator sequences widen the
/// cluster. This matches how the client terminal lays the cell out, so the
/// daemon can mirror it without maintaining its own width tables.
pub fn grapheme_width(text: &str) -> u8 {
    let mut cps = [0u32; 32];
    let mut n = 0;
    for c in text.chars() {
        if n == cps.len() {
            // Extremely long clusters (rare) are measured from a heap buffer.
            let cps: Vec<u32> = text.chars().map(|c| c as u32).collect();
            let mut width: u8 = 0;
            unsafe {
                ghostty_unicode_grapheme_width(cps.as_ptr(), cps.len(), &mut width);
            }
            return width;
        }
        cps[n] = c as u32;
        n += 1;
    }
    let mut width: u8 = 0;
    unsafe {
        ghostty_unicode_grapheme_width(cps.as_ptr(), n, &mut width);
    }
    width
}

/// Terminal display width of a single codepoint in grid cells (0, 1, or 2).
pub fn codepoint_width(cp: char) -> u8 {
    unsafe { ghostty_unicode_codepoint_width(cp as u32) }
}

/// Shared callback cell installed as the terminal's USERDATA. Holds the
/// current pty writer (for query responses) and the last OSC 7 / OSC 9 /
/// OSC 1337 reported pwd.
struct CbCell {
    writer: Option<*mut (dyn std::io::Write + Send)>,
    /// Raw pwd bytes the shell reported (a `file://` URI for OSC 7, a bare
    /// path for OSC 9 / OSC 1337). Empty when cleared or never reported.
    pwd: Vec<u8>,
    /// Number of BEL characters received since last check. Used to trigger
    /// audible alerts when programs ring the terminal bell.
    bell_count: u32,
    /// Text from the last OSC 52 clipboard write request. The daemon checks
    /// this after each PTY event and copies it to the system clipboard.
    clipboard_text: String,
}

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
    // USERDATA points at a heap `CbCell`; `writer` is None until a writer is
    // installed (`set_write_sink`).
    let cell = userdata as *mut CbCell;
    let Some(writer) = (*cell).writer else {
        return;
    };
    let writer = &mut *writer;
    let bytes = std::slice::from_raw_parts(data, len);
    let _ = writer.write_all(bytes);
}

/// Pwd changed callback (OSC 7 / OSC 9 / OSC 1337): record the reported value
/// into the callback cell so `Terminal::pwd` can hand it out.
unsafe extern "C" fn pwd_changed_cb(term: TerminalHandle, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let cell = userdata as *mut CbCell;
    let mut slice = StringSlice {
        ptr: ptr::null(),
        len: 0,
    };
    if ghostty_terminal_get(
        term,
        TERMINAL_DATA_PWD,
        &mut slice as *mut StringSlice as *mut c_void,
    )
    .is_ok()
    {
        (*cell).pwd = if slice.ptr.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(slice.ptr, slice.len).to_vec()
        };
    }
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
    let mut size = SizeReportSize {
        rows: 0,
        columns: 0,
        cell_width: 0,
        cell_height: 0,
    };
    if ghostty_terminal_get(
        term,
        TERMINAL_DATA_COLS,
        &mut size.columns as *mut u16 as *mut c_void,
    )
    .is_ok()
        && ghostty_terminal_get(
            term,
            TERMINAL_DATA_ROWS,
            &mut size.rows as *mut u16 as *mut c_void,
        )
        .is_ok()
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
    StringSlice {
        ptr: EMPTY.as_ptr(),
        len: 0,
    }
}

/// Bell callback: increment the bell counter so the daemon can trigger alerts.
unsafe extern "C" fn bell_cb(_term: TerminalHandle, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let cell = userdata as *mut CbCell;
    (*cell).bell_count = (*cell).bell_count.saturating_add(1);
}

/// Clipboard write callback: handle OSC 52 clipboard write requests.
unsafe extern "C" fn clipboard_write_cb(
    _term: TerminalHandle,
    userdata: *mut c_void,
    write: *const ClipboardWrite,
) -> ClipboardWriteResult {
    if userdata.is_null() || write.is_null() {
        return ClipboardWriteResult::InvalidData;
    }
    let write = &*write;
    if write.contents_len == 0 {
        // Clear clipboard request
        return ClipboardWriteResult::Success;
    }
    // Find the first text/plain content
    for i in 0..write.contents_len {
        let content = &*write.contents.add(i);
        let mime = std::slice::from_raw_parts(content.mime.ptr, content.mime.len);
        let data = std::slice::from_raw_parts(content.data.ptr, content.data.len);
        // Accept text/plain or empty mime (defaults to text/plain)
        if mime.is_empty() || mime == b"text/plain" {
            if let Ok(text) = std::str::from_utf8(data) {
                // Store the text in the callback cell for the daemon to process
                let cell = userdata as *mut CbCell;
                (*cell).clipboard_text = text.to_string();
                return ClipboardWriteResult::Success;
            }
        }
    }
    ClipboardWriteResult::Unsupported
}

/// Percent-decode a `file://` URI path (`%20` → space, `%2F` → `/`).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode one hex nibble.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Find URL-like spans in `line`, returning `(start, end)` byte offsets and
/// the matched text for each `scheme://` link. This is the plain-text fallback
/// for Cmd+click: terminal output often prints URLs as bare text (e.g. `next
/// dev`'s "Local: http://localhost:3000"), with no OSC 8 markup.
///
/// A match starts at a scheme (`http://`, `https://`, `ftp://`, …) and runs
/// through the URL, stopping at whitespace/control bytes, an unbalanced `)`,
/// or trailing punctuation (`, . ; : ! ?`) that is not part of the URL.
pub fn find_urls(line: &str) -> Vec<(usize, usize, String)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        // Scheme: [A-Za-z][A-Za-z0-9+.-]* followed by "://".
        let mut j = i + 1;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'+' | b'-' | b'.'))
        {
            j += 1;
        }
        if j + 2 < bytes.len() && bytes[j] == b':' && bytes[j + 1] == b'/' && bytes[j + 2] == b'/' {
            let content = j + 3;
            let mut end = content;
            let mut parens = 0usize;
            while end < bytes.len() {
                let b = bytes[end];
                if b.is_ascii_whitespace() || b.is_ascii_control() {
                    break;
                }
                if b == b'(' {
                    parens += 1;
                } else if b == b')' {
                    if parens == 0 {
                        break;
                    }
                    parens -= 1;
                }
                end += 1;
            }
            // Trim trailing punctuation that is not part of the URL.
            let mut t = end;
            while t > content && matches!(bytes[t - 1], b'.' | b',' | b';' | b':' | b'!' | b'?') {
                t -= 1;
            }
            if t > content {
                let url = line[i..t].to_string();
                out.push((i, t, url));
                i = t;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A viewport cell with resolved colors and a UTF-8 grapheme.
#[derive(Clone, Debug)]
pub struct RenderCell<'a> {
    pub text: &'a str,
    pub fg: ColorRgb,
    pub bg: ColorRgb,
    /// Whether the app set an explicit foreground (else use the terminal's).
    pub has_fg: bool,
    /// Whether the app set an explicit background (else use the terminal's).
    pub has_bg: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub faint: bool,
    /// Whether the cell is part of an OSC 8 hyperlink.
    pub hyperlink: bool,
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
    /// Heap cell holding the callback state (active pty writer + last reported
    /// pwd). Its address is the USERDATA passed to the callbacks.
    cell: Box<CbCell>,
}

/// Build a full 256-entry palette from the ghostty default and override the
/// ANSI 0-15 entries with the theme colors, then install foreground,
/// background, cursor, and palette on `term`.
fn set_terminal_colors(
    term: TerminalHandle,
    palette: &[ColorRgb; 16],
    fg: ColorRgb,
    bg: ColorRgb,
    cursor: ColorRgb,
) {
    let mut full_palette = [ColorRgb::new(0, 0, 0); 256];
    unsafe {
        ghostty_color_palette_default(full_palette.as_mut_ptr());
    }
    full_palette[..16].copy_from_slice(palette);
    unsafe {
        ghostty_terminal_set(
            term,
            TERMINAL_OPT_COLOR_FOREGROUND,
            &fg as *const _ as *const c_void,
        );
        ghostty_terminal_set(
            term,
            TERMINAL_OPT_COLOR_BACKGROUND,
            &bg as *const _ as *const c_void,
        );
        ghostty_terminal_set(
            term,
            TERMINAL_OPT_COLOR_CURSOR,
            &cursor as *const _ as *const c_void,
        );
        ghostty_terminal_set(
            term,
            TERMINAL_OPT_COLOR_PALETTE,
            full_palette.as_ptr() as *const c_void,
        );
    }
}

impl Terminal {
    /// Create a terminal of `cols` x `rows` with `max_scrollback` history rows
    /// and configure the given theme colors (ANSI palette + defaults) as its
    /// starting colors.
    pub fn new(
        cols: u16,
        rows: u16,
        max_scrollback: usize,
        palette: &[ColorRgb; 16],
        fg: ColorRgb,
        bg: ColorRgb,
        cursor: ColorRgb,
    ) -> anyhow::Result<Terminal> {
        let mut term: TerminalHandle = ptr::null_mut();
        unsafe {
            if !ghostty_terminal_new(ptr::null(), &mut term, cols.max(1), rows.max(1)).is_ok()
                || term.is_null()
            {
                return Err(anyhow::anyhow!("libghostty-vt: failed to create terminal"));
            }
        }

        let mut render: RenderStateHandle = ptr::null_mut();
        unsafe {
            if !ghostty_render_state_new(ptr::null(), &mut render).is_ok() || render.is_null() {
                ghostty_terminal_free(term);
                return Err(anyhow::anyhow!(
                    "libghostty-vt: failed to create render state"
                ));
            }
        }

        set_terminal_colors(term, palette, fg, bg, cursor);

        // The constructor no longer takes a scrollback limit; configure it
        // here so the grid actually grows into history at most `max_scrollback`
        // lines. Reading TERMINAL_DATA_TOTAL_ROWS then reports content rows,
        // not capacity (agent detection's `bottom_text` relies on it).
        let sb_lines: usize = max_scrollback;
        unsafe {
            let _ = ghostty_terminal_set(
                term,
                TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                &sb_lines as *const usize as *const c_void,
            );
        }

        // Enable continuation tracking so an unfinished VT/UTF-8 sequence can be
        // snapshotted (required for lossless restart). 64KB matches the
        // snapshot example and is well below the decoder's 65MiB limit.
        let cont_limit: usize = 64 * 1024;
        unsafe {
            let _ = ghostty_terminal_set(
                term,
                TERMINAL_OPT_CONTINUATION_MAX_BYTES,
                &cont_limit as *const usize as *const c_void,
            );
        }

        // The callbacks need a stable place to find the current pty writer
        // and the last reported pwd. Allocate a heap cell and use it as the
        // USERDATA pointer.
        let cell: Box<CbCell> = Box::new(CbCell {
            writer: None,
            pwd: Vec::new(),
            bell_count: 0,
            clipboard_text: String::new(),
        });
        let userdata = (&*cell) as *const CbCell as *mut c_void;
        unsafe {
            ghostty_terminal_set(term, TERMINAL_OPT_USERDATA, userdata as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_WRITE_PTY, write_pty_cb as *const c_void);
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_PWD_CHANGED,
                pwd_changed_cb as *const c_void,
            );
            ghostty_terminal_set(term, TERMINAL_OPT_SIZE, size_cb as *const c_void);
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_COLOR_SCHEME,
                color_scheme_cb as *const c_void,
            );
            ghostty_terminal_set(term, TERMINAL_OPT_ENQUIRY, enquiry_cb as *const c_void);
            ghostty_terminal_set(term, TERMINAL_OPT_BELL, bell_cb as *const c_void);
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_CLIPBOARD_WRITE,
                clipboard_write_cb as *const c_void,
            );
        }

        // Enable grapheme cluster mode (DEC private mode 2027) so multi-codepoint
        // graphemes (emoji flags, ZWJ sequences, etc.) are kept together in a
        // single cell instead of being split across multiple cells.
        let mode_config = TerminalModeConfig {
            mode: MODE_GRAPHEME_CLUSTER,
            value: true,
        };
        unsafe {
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_MODE,
                &mode_config as *const TerminalModeConfig as *const c_void,
            );
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
            cell,
        })
    }

    /// Encode the complete terminal state (screen + scrollback + continuation)
    /// into a snapshot byte buffer. Returns `None` if the terminal is in a
    /// non-ground VT state but continuation tracking was not enabled early
    /// enough (the example sets it before first write, which we now do).
    pub fn snapshot_encode(&self) -> Option<Vec<u8>> {
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let res = unsafe {
            ghostty_snapshot_encode_alloc(self.term, std::ptr::null(), &mut ptr, &mut len)
        };
        if !res.is_ok() || ptr.is_null() {
            return None;
        }
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        let out = slice.to_vec();
        unsafe { ghostty_free(std::ptr::null(), ptr, len) };
        Some(out)
    }

    /// Create a `Terminal` from a snapshot byte buffer. The snapshot already
    /// contains cols/rows/colors, but we re-install callbacks, the current
    /// theme's palette, and the scrollback limit so the new terminal integrates
    /// with the daemon's PTY writer.
    pub fn from_snapshot(
        bytes: &[u8],
        max_scrollback: usize,
        palette: &[ColorRgb; 16],
        fg: ColorRgb,
        bg: ColorRgb,
        cursor: ColorRgb,
    ) -> anyhow::Result<Terminal> {
        let mut decoder: SnapshotDecoderHandle = std::ptr::null_mut();
        let res = unsafe {
            ghostty_snapshot_decoder_new_buf(
                std::ptr::null(),
                &mut decoder,
                bytes.as_ptr(),
                bytes.len(),
            )
        };
        if !res.is_ok() || decoder.is_null() {
            return Err(anyhow::anyhow!(
                "libghostty-vt: failed to create snapshot decoder"
            ));
        }
        let mut term: TerminalHandle = std::ptr::null_mut();
        let res = unsafe { ghostty_snapshot_decoder_decode(decoder, &mut term) };
        unsafe { ghostty_snapshot_decoder_free(decoder) };
        if !res.is_ok() || term.is_null() {
            return Err(anyhow::anyhow!("libghostty-vt: snapshot decode failed"));
        }

        let mut render: RenderStateHandle = std::ptr::null_mut();
        unsafe {
            if !ghostty_render_state_new(std::ptr::null(), &mut render).is_ok() || render.is_null()
            {
                ghostty_terminal_free(term);
                return Err(anyhow::anyhow!(
                    "libghostty-vt: failed to create render state for snapshot"
                ));
            }
        }

        // Re-apply theme palette (snapshot carries its own palette, but the
        // daemon's current theme may differ). Also re-enable continuation for
        // future snapshots and a stable scrollback limit.
        set_terminal_colors(term, palette, fg, bg, cursor);
        let sb_lines: usize = max_scrollback;
        let cont_limit: usize = 64 * 1024;
        unsafe {
            let _ = ghostty_terminal_set(
                term,
                TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                &sb_lines as *const usize as *const std::ffi::c_void,
            );
            let _ = ghostty_terminal_set(
                term,
                TERMINAL_OPT_CONTINUATION_MAX_BYTES,
                &cont_limit as *const usize as *const std::ffi::c_void,
            );
        }

        let cell: Box<CbCell> = Box::new(CbCell {
            writer: None,
            pwd: Vec::new(),
            bell_count: 0,
            clipboard_text: String::new(),
        });
        let userdata = (&*cell) as *const CbCell as *mut std::ffi::c_void;
        unsafe {
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_USERDATA,
                userdata as *const std::ffi::c_void,
            );
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_WRITE_PTY,
                write_pty_cb as *const std::ffi::c_void,
            );
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_PWD_CHANGED,
                pwd_changed_cb as *const std::ffi::c_void,
            );
            ghostty_terminal_set(term, TERMINAL_OPT_SIZE, size_cb as *const std::ffi::c_void);
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_COLOR_SCHEME,
                color_scheme_cb as *const std::ffi::c_void,
            );
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_ENQUIRY,
                enquiry_cb as *const std::ffi::c_void,
            );
            ghostty_terminal_set(term, TERMINAL_OPT_BELL, bell_cb as *const std::ffi::c_void);
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_CLIPBOARD_WRITE,
                clipboard_write_cb as *const std::ffi::c_void,
            );
        }
        let mode_config = TerminalModeConfig {
            mode: MODE_GRAPHEME_CLUSTER,
            value: true,
        };
        unsafe {
            ghostty_terminal_set(
                term,
                TERMINAL_OPT_MODE,
                &mode_config as *const TerminalModeConfig as *const std::ffi::c_void,
            );
        }

        // Pull initial rows/cols from the decoded terminal so the wrapper stays
        // consistent; fallback to 80x24 if query fails.
        let (rows, cols) = {
            let mut out_rows: u16 = 24;
            let mut out_cols: u16 = 80;
            unsafe {
                let _ = ghostty_terminal_get(
                    term,
                    TERMINAL_DATA_ROWS,
                    &mut out_rows as *mut u16 as *mut std::ffi::c_void,
                );
                let _ = ghostty_terminal_get(
                    term,
                    TERMINAL_DATA_COLS,
                    &mut out_cols as *mut u16 as *mut std::ffi::c_void,
                );
                if out_cols == 0 || out_rows == 0 {
                    out_cols = 80;
                    out_rows = 24;
                }
            }
            (out_rows, out_cols)
        };

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
            cell,
        })
    }

    /// Swap the terminal's default colors and ANSI palette to `theme`. Called
    /// on every pane when the active theme changes; the next `refresh` picks
    /// up the new defaults.
    pub fn apply_theme(
        &mut self,
        palette: &[ColorRgb; 16],
        fg: ColorRgb,
        bg: ColorRgb,
        cursor: ColorRgb,
    ) {
        set_terminal_colors(self.term, palette, fg, bg, cursor);
        self.default_fg = fg;
        self.default_bg = bg;
    }

    /// Install the writer that query responses are written to. The pointer
    /// must point into a stable heap allocation (e.g. a `Box`'s contents) and
    /// stay valid for the lifetime of the terminal.
    pub fn set_write_sink(&mut self, writer: *mut (dyn std::io::Write + Send)) {
        self.cell.writer = Some(writer);
    }

    /// The last working directory reported by the pane's shell via OSC 7
    /// (`file://` URI), OSC 9, or OSC 1337, decoded to a local path. Returns
    /// `None` when the shell never reported one (or cleared it).
    pub fn pwd(&self) -> Option<PathBuf> {
        if self.cell.pwd.is_empty() {
            return None;
        }
        let raw = String::from_utf8_lossy(&self.cell.pwd);
        let path = if let Some(rest) = raw.strip_prefix("file://") {
            // `file:///tmp/x` and `file://host/tmp/x` both mean `/tmp/x`: the
            // authority is the machine that owns the path. For a local shell
            // it is this machine (fish reports its hostname, not
            // "localhost"); for a remote ssh pane it is the remote host, whose
            // path will not exist locally — the follow logic's is_dir guard
            // drops it, so the decoded path is still the right value to report.
            let path = rest
                .split_once('/')
                .map(|(_, p)| format!("/{p}"))
                .unwrap_or_else(|| rest.to_string());
            percent_decode(&path)
        } else {
            raw.into_owned()
        };
        let pb = PathBuf::from(path);
        (!pb.as_os_str().is_empty()).then_some(pb)
    }

    /// Return and reset the number of BEL characters received since the last
    /// call. Used by the daemon to trigger audible alerts when programs ring
    /// the terminal bell.
    pub fn take_bell_count(&mut self) -> u32 {
        let count = self.cell.bell_count;
        self.cell.bell_count = 0;
        count
    }

    /// Return and reset the text from the last OSC 52 clipboard write request.
    /// Used by the daemon to copy text to the system clipboard when programs
    /// request it via OSC 52.
    pub fn take_clipboard_text(&mut self) -> Option<String> {
        if self.cell.clipboard_text.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.cell.clipboard_text))
        }
    }

    /// Install a linear selection covering two viewport coordinates
    /// (inclusive), matching a mouse drag. Returns false if either endpoint
    /// is outside the grid. The terminal converts the refs into owned tracked
    /// state, so the selection survives subsequent output/scroll.
    /// Build a linear selection between two viewport coordinates (inclusive).
    fn build_selection(&self, start: (u16, u16), end: (u16, u16)) -> Option<Selection> {
        let mut start_ref = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        let mut end_ref = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        unsafe {
            if !ghostty_terminal_grid_ref(
                self.term,
                viewport_point(start.0, start.1 as u32),
                &mut start_ref,
            )
            .is_ok()
                || !ghostty_terminal_grid_ref(
                    self.term,
                    viewport_point(end.0, end.1 as u32),
                    &mut end_ref,
                )
                .is_ok()
            {
                return None;
            }
        }
        Some(Selection {
            size: size_of::<Selection>(),
            start: start_ref,
            end: end_ref,
            rectangle: false,
        })
    }

    /// Install a linear selection covering two viewport coordinates
    /// (inclusive), matching a mouse drag. Returns false if either endpoint
    /// is outside the grid. The terminal converts the refs into owned tracked
    /// state, so the selection survives subsequent output/scroll.
    pub fn set_selection(&mut self, start: (u16, u16), end: (u16, u16)) -> bool {
        let Some(selection) = self.build_selection(start, end) else {
            return false;
        };
        unsafe {
            ghostty_terminal_set(
                self.term,
                TERMINAL_OPT_SELECTION,
                &selection as *const Selection as *const c_void,
            )
            .is_ok()
        }
    }

    /// Clear the active terminal selection.
    pub fn clear_selection(&mut self) {
        unsafe {
            ghostty_terminal_set(self.term, TERMINAL_OPT_SELECTION, ptr::null());
        }
    }

    /// The OSC 8 hyperlink URI at viewport position (x, y), or `None` when the
    /// cell has no hyperlink. The viewport point maps through the terminal's
    /// coordinate system, so this resolves correctly even when scrolled into
    /// scrollback.
    pub fn hyperlink_at(&self, x: u16, y: u16) -> Option<String> {
        let mut ref_ = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        unsafe {
            if !ghostty_terminal_grid_ref(self.term, viewport_point(x, y as u32), &mut ref_).is_ok()
            {
                return None;
            }
            let mut written = 0usize;
            let res = ghostty_grid_ref_hyperlink_uri(&ref_, ptr::null_mut(), 0, &mut written);
            if res != Result::OutOfSpace || written == 0 {
                return None;
            }
            let mut buf = vec![0u8; written];
            let mut filled = 0usize;
            if !ghostty_grid_ref_hyperlink_uri(&ref_, buf.as_mut_ptr(), buf.len(), &mut filled)
                .is_ok()
            {
                return None;
            }
            buf.truncate(filled.min(buf.len()));
            Some(String::from_utf8_lossy(&buf).into_owned())
        }
    }

    /// The plain text of viewport row `y` (unwrapped, untrimmed so cell columns
    /// map 1:1 onto the string), used for plain-text URL detection.
    pub fn row_text(&self, y: u16) -> String {
        if y >= self.rows {
            return String::new();
        }
        let last = self.cols.saturating_sub(1);
        let Some(sel) = self.build_selection((0, y), (last, y)) else {
            return String::new();
        };
        self.format_selection_opts(&sel, true, false)
    }

    /// The clickable link at viewport position (x, y): an OSC 8 hyperlink URI
    /// first, else a plain-text `scheme://` URL detected on the row (matching
    /// what a normal terminal does for e.g. `next dev` output).
    pub fn link_at(&self, x: u16, y: u16) -> Option<String> {
        if let Some(uri) = self.hyperlink_at(x, y) {
            return Some(uri);
        }
        let line = self.row_text(y);
        if line.is_empty() {
            return None;
        }
        let target = x as usize;
        // Cell column -> byte offset (URLs are ASCII so the mapping is exact;
        // a wide char earlier on the row would skew it, an accepted edge case).
        let byte_at = line
            .char_indices()
            .nth(target)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        find_urls(&line)
            .iter()
            .find(|(s, e, _)| *s <= byte_at && byte_at < *e)
            .map(|(_, _, url)| url.clone())
    }

    /// Extract the text between two viewport coordinates (inclusive) as plain
    /// text: soft-wrapped lines are unwrapped and trailing whitespace is
    /// trimmed, matching Ghostty's clipboard behavior. Builds a fresh
    /// selection from the current viewport, so repaints during a drag can't
    /// shift the tracked active selection.
    pub fn selection_text(&mut self, start: (u16, u16), end: (u16, u16)) -> Option<String> {
        let selection = self.build_selection(start, end)?;
        let text = self.format_selection(&selection);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Read the text of the last `lines` rows of the terminal's active screen
    /// buffer (including scrollback), independent of the viewport scroll
    /// position. Used for agent-state detection so scrolling the viewport
    /// never changes the detected state. Reads via the selection formatter on
    pub fn bottom_text(&self, lines: usize) -> String {
        let mut total: usize = 0;
        unsafe {
            if !ghostty_terminal_get(
                self.term,
                TERMINAL_DATA_TOTAL_ROWS,
                &mut total as *mut usize as *mut c_void,
            )
            .is_ok()
                || total == 0
            {
                return String::new();
            }
        }
        let mut cols: usize = 0;
        unsafe {
            if !ghostty_terminal_get(
                self.term,
                TERMINAL_DATA_COLS,
                &mut cols as *mut usize as *mut c_void,
            )
            .is_ok()
            {
                return String::new();
            }
        }
        let start = total.saturating_sub(lines);
        let Some(selection) = self.build_screen_selection(
            (0, start as u32),
            (cols.saturating_sub(1) as u16, (total - 1) as u32),
        ) else {
            return String::new();
        };
        self.format_selection(&selection)
    }

    /// The terminal window title set via OSC 0 / OSC 2 (e.g. Claude Code's
    /// spinner status). Empty when no title has been set. The C string is
    /// borrowed and invalidated by the next `write`, so it's copied here.
    pub fn title(&self) -> String {
        let mut t = StringSlice {
            ptr: ptr::null(),
            len: 0,
        };
        unsafe {
            if !ghostty_terminal_get(
                self.term,
                TERMINAL_DATA_TITLE,
                &mut t as *mut StringSlice as *mut c_void,
            )
            .is_ok()
            {
                return String::new();
            }
        }
        if t.ptr.is_null() || t.len == 0 {
            return String::new();
        }
        unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(t.ptr, t.len)).into_owned() }
    }

    /// Build a linear selection between two screen-buffer coordinates
    /// (inclusive), independent of the viewport scroll position.
    fn build_screen_selection(&self, start: (u16, u32), end: (u16, u32)) -> Option<Selection> {
        let mut start_ref = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        let mut end_ref = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        unsafe {
            if !ghostty_terminal_grid_ref(self.term, screen_point(start.0, start.1), &mut start_ref)
                .is_ok()
                || !ghostty_terminal_grid_ref(self.term, screen_point(end.0, end.1), &mut end_ref)
                    .is_ok()
            {
                return None;
            }
        }
        Some(Selection {
            size: size_of::<Selection>(),
            start: start_ref,
            end: end_ref,
            rectangle: false,
        })
    }

    /// Format a selection as plain text (unwrap + trim), via the selection
    /// formatter.
    fn format_selection(&self, selection: &Selection) -> String {
        self.format_selection_opts(selection, true, true)
    }

    /// Format a selection as plain text with explicit unwrap/trim behavior.
    /// `trim = false` preserves leading/trailing whitespace so cell columns map
    /// 1:1 onto the returned string (needed for plain-text URL detection).
    fn format_selection_opts(&self, selection: &Selection, unwrap: bool, trim: bool) -> String {
        let options = SelectionFormatOptions {
            size: size_of::<SelectionFormatOptions>(),
            emit: FORMAT_PLAIN,
            unwrap,
            trim,
            selection,
        };
        unsafe {
            let mut written = 0usize;
            let res = ghostty_terminal_selection_format_buf(
                self.term,
                options,
                ptr::null_mut(),
                0,
                &mut written,
            );
            if res != Result::OutOfSpace || written == 0 {
                return String::new();
            }
            let mut buf = vec![0u8; written];
            let mut filled = 0usize;
            if !ghostty_terminal_selection_format_buf(
                self.term,
                options,
                buf.as_mut_ptr(),
                buf.len(),
                &mut filled,
            )
            .is_ok()
            {
                return String::new();
            }
            buf.truncate(filled.min(buf.len()));
            String::from_utf8_lossy(&buf).into_owned()
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
            value: ScrollViewportValue {
                delta: delta as isize,
            },
        };
        unsafe {
            ghostty_terminal_scroll_viewport(self.term, behavior);
        }
    }

    /// Scroll so that `row` (screen coordinate, 0 = top of scrollback) becomes
    /// the first visible row of the viewport. Clamped by libghostty-vt.
    pub fn scroll_to_row(&mut self, row: usize) {
        let behavior = ScrollViewport {
            tag: SCROLL_VIEWPORT_ROW,
            value: ScrollViewportValue { row },
        };
        unsafe {
            ghostty_terminal_scroll_viewport(self.term, behavior);
        }
    }

    /// Total number of rows in the screen buffer (viewport + scrollback).
    pub fn total_rows(&self) -> usize {
        let mut total: usize = 0;
        unsafe {
            if !ghostty_terminal_get(
                self.term,
                TERMINAL_DATA_TOTAL_ROWS,
                &mut total as *mut usize as *mut c_void,
            )
            .is_ok()
            {
                return 0;
            }
        }
        total
    }

    /// Plain text of one screen-buffer row (0 = top of scrollback), trimmed.
    /// Uses the screen-point selection so it is viewport-independent.
    pub fn screen_row_text(&self, row: u32) -> String {
        let cols = self.cols as u32;
        if cols == 0 {
            return String::new();
        }
        let last = cols.saturating_sub(1) as u16;
        let Some(sel) = self.build_screen_selection((0, row), (last, row)) else {
            return String::new();
        };
        self.format_selection(&sel)
    }

    /// Text of an inclusive screen range `[start,end]` (viewport-independent).
    pub fn screen_range_text(&self, start_row: u32, end_row: u32) -> String {
        if start_row > end_row {
            return String::new();
        }
        let total = self.total_rows() as u32;
        if total == 0 || start_row >= total {
            return String::new();
        }
        let end_row = end_row.min(total - 1);
        let cols = self.cols as u32;
        if cols == 0 {
            return String::new();
        }
        let last = cols.saturating_sub(1) as u16;
        let Some(sel) = self.build_screen_selection((0, start_row), (last, end_row)) else {
            return String::new();
        };
        self.format_selection(&sel)
    }

    /// Semantic prompt state of a screen row (0 = top of scrollback) derived
    /// from OSC 133. `None` when the terminal cannot provide the row.
    pub fn row_semantic_prompt(&self, row: u32) -> Option<RowSemanticPrompt> {
        let mut gref = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        unsafe {
            if !ghostty_terminal_grid_ref(self.term, screen_point(0, row), &mut gref).is_ok() {
                return None;
            }
            let mut grow: GhosttyRow = 0;
            if !ghostty_grid_ref_row(&gref, &mut grow as *mut GhosttyRow).is_ok() {
                return None;
            }
            let mut out: i32 = 0;
            if !ghostty_row_get(grow, ROW_DATA_SEMANTIC_PROMPT, &mut out as *mut i32 as *mut c_void).is_ok() {
                return None;
            }
            match out {
                0 => Some(RowSemanticPrompt::None),
                1 => Some(RowSemanticPrompt::Prompt),
                2 => Some(RowSemanticPrompt::Continuation),
                _ => None,
            }
        }
    }

    /// Semantic content of a single cell (OSC 133).
    pub fn cell_semantic_at(&self, x: u16, row: u32) -> Option<CellSemanticContent> {
        let mut gref = GridRef {
            size: size_of::<GridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        unsafe {
            if !ghostty_terminal_grid_ref(self.term, screen_point(x, row), &mut gref).is_ok() {
                return None;
            }
            let mut cell: u64 = 0;
            if !ghostty_grid_ref_cell(&gref, &mut cell as *mut u64).is_ok() {
                return None;
            }
            let mut out: i32 = 0;
            if !ghostty_cell_get(cell, CELL_DATA_SEMANTIC_CONTENT, &mut out as *mut i32 as *mut c_void).is_ok() {
                return None;
            }
            match out {
                0 => Some(CellSemanticContent::Output),
                1 => Some(CellSemanticContent::Input),
                2 => Some(CellSemanticContent::Prompt),
                _ => None,
            }
        }
    }

    /// The last completed prompt block (OSC 133). When the terminal is mid-
    /// command (most recent prompt has no following prompt), this returns the
    /// previous command's output (`prev prompt_end+1 .. last prompt_start-1`).
    /// When the last command is still running (no new prompt yet), it returns
    /// the current output (`prompt_end+1 .. bottom`). `None` when no semantic
    /// prompt markers exist yet.
    pub fn last_prompt_block(&self) -> Option<PromptBlock> {
        let total = self.total_rows() as u32;
        if total == 0 {
            return None;
        }
        // Collect prompt starts
        let mut prompts: Vec<u32> = Vec::new();
        for r in 0..total {
            if self.row_semantic_prompt(r) == Some(RowSemanticPrompt::Prompt) {
                prompts.push(r);
            }
        }
        if prompts.is_empty() {
            return None;
        }
        // Helper to expand continuation rows
        let prompt_end_of = |start: u32| -> u32 {
            let mut end = start;
            while end + 1 < total && self.row_semantic_prompt(end + 1) == Some(RowSemanticPrompt::Continuation) {
                end += 1;
            }
            end
        };
        if prompts.len() == 1 {
            let ps = prompts[0];
            let pe = prompt_end_of(ps);
            if pe + 1 >= total {
                return None;
            }
            let out_start = pe + 1;
            let out_end = total - 1;
            let prompt_text = self.screen_range_text(ps, pe);
            let output_text = self.screen_range_text(out_start, out_end);
            if output_text.trim().is_empty() {
                return None;
            }
            return Some(PromptBlock {
                prompt_start: ps,
                prompt_end: pe,
                output_start: out_start,
                output_end: out_end,
                prompt_text,
                output_text,
            });
        }
        // >=2 prompts: last completed is previous prompt's output till last prompt start -1
        let last = *prompts.last().unwrap();
        let prev = prompts[prompts.len() - 2];
        let pe_prev = prompt_end_of(prev);
        // output of previous command
        let out_start = pe_prev + 1;
        let out_end = last.saturating_sub(1);
        if out_start <= out_end {
            let prompt_text = self.screen_range_text(prev, pe_prev);
            let output_text = self.screen_range_text(out_start, out_end);
            if !output_text.trim().is_empty() {
                return Some(PromptBlock {
                    prompt_start: prev,
                    prompt_end: pe_prev,
                    output_start: out_start,
                    output_end: out_end,
                    prompt_text,
                    output_text,
                });
            }
        }
        // Fallback: current running output after last prompt
        let pe_last = prompt_end_of(last);
        if pe_last + 1 < total {
            let out_start = pe_last + 1;
            let out_end = total - 1;
            let prompt_text = self.screen_range_text(last, pe_last);
            let output_text = self.screen_range_text(out_start, out_end);
            if !output_text.trim().is_empty() {
                return Some(PromptBlock {
                    prompt_start: last,
                    prompt_end: pe_last,
                    output_start: out_start,
                    output_end: out_end,
                    prompt_text,
                    output_text,
                });
            }
        }
        None
    }

    /// Whether the terminal has seen any semantic prompt marker (OSC 133).
    pub fn has_semantic_prompt(&self) -> bool {
        let total = self.total_rows() as u32;
        for r in (total.saturating_sub(200))..total {
            if self.row_semantic_prompt(r) == Some(RowSemanticPrompt::Prompt) {
                return true;
            }
        }
        false
    }

    /// Search the entire screen buffer for `query` (plain substring, case-
    /// insensitive). Returns one hit per occurrence (single-row only; wrapped
    /// lines are not joined). Hit columns are char offsets in the row's
    /// trimmed text, so they map 1:1 to viewport columns when the row is
    /// visible.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        if query.is_empty() {
            return Vec::new();
        }
        let q_lower = query.to_lowercase();
        let q_len = query.chars().count() as u16;
        if q_len == 0 {
            return Vec::new();
        }
        let total = self.total_rows();
        let mut hits = Vec::new();
        for r in 0..total {
            let line = self.screen_row_text(r as u32);
            if line.is_empty() {
                continue;
            }
            let lower = line.to_lowercase();
            let mut search_from = 0usize;
            while let Some(byte_off) = lower[search_from..].find(&q_lower) {
                let abs_byte = search_from + byte_off;
                let start_col = line[..abs_byte].chars().count() as u16;
                let end_col = start_col.saturating_add(q_len);
                hits.push(SearchHit {
                    row: r as u32,
                    start_col,
                    end_col,
                });
                search_from = abs_byte + q_lower.len();
                if search_from >= lower.len() {
                    break;
                }
            }
        }
        hits
    }

    /// True when any mouse tracking mode is active.
    pub fn mouse_tracking(&self) -> bool {
        let mut out: bool = false;
        unsafe {
            ghostty_terminal_get(
                self.term,
                TERMINAL_DATA_MOUSE_TRACKING,
                &mut out as *mut bool as *mut c_void,
            );
        }
        out
    }

    /// Current value of a terminal mode.
    pub fn mode_get(&self, mode: u16) -> bool {
        let mut config = TerminalModeConfig { mode, value: false };
        unsafe {
            ghostty_terminal_get(
                self.term,
                TERMINAL_DATA_MODE,
                &mut config as *mut TerminalModeConfig as *mut c_void,
            );
        }
        config.value
    }

    /// Set a DEC private mode on/off. Used to restore mouse tracking (mode
    /// 1000) on a pane resumed across a daemon restart: the live app still has
    /// the mode enabled app-side, so the fresh emulator must re-learn it or
    /// kumo would grab the mouse and its fallback (which cannot scroll a
    /// full-screen app) would take over.
    pub fn mode_set(&self, mode: u16, value: bool) -> bool {
        let config = TerminalModeConfig { mode, value };
        unsafe {
            ghostty_terminal_set(
                self.term,
                TERMINAL_OPT_MODE,
                &config as *const TerminalModeConfig as *const c_void,
            )
            .is_ok()
        }
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
        let mut scrollbar = TerminalScrollbar::default();

        let keys = [
            TERMINAL_DATA_COLOR_FOREGROUND,
            TERMINAL_DATA_COLOR_BACKGROUND,
            TERMINAL_DATA_SCROLLBAR,
        ];
        let mut values: [*mut c_void; 3] = [
            &mut fg as *mut ColorRgb as *mut c_void,
            &mut bg as *mut ColorRgb as *mut c_void,
            &mut scrollbar as *mut TerminalScrollbar as *mut c_void,
        ];
        let mut written: usize = 0;
        unsafe {
            ghostty_terminal_get_multi(
                self.term,
                keys.len(),
                keys.as_ptr(),
                values.as_mut_ptr(),
                &mut written,
            );
        }
        self.default_fg = fg;
        self.default_bg = bg;
        self.scrollbar = scrollbar;

        let mut cursor_data = RenderStateCursor::new();
        unsafe {
            ghostty_render_state_get(
                self.render,
                RENDER_STATE_DATA_CURSOR,
                &mut cursor_data as *mut RenderStateCursor as *mut c_void,
            );
        }
        self.cursor = if cursor_data.viewport_has_value {
            Some((cursor_data.viewport_x, cursor_data.viewport_y))
        } else {
            None
        };
    }

    /// Global render-state dirty level (DIRTY_FALSE/PARTIAL/FULL) after a
    /// `refresh`.
    pub fn render_dirty_level(&self) -> i32 {
        let mut d: i32 = 0;
        unsafe {
            ghostty_render_state_get(
                self.render,
                RENDER_STATE_DATA_DIRTY,
                &mut d as *mut i32 as *mut c_void,
            );
        }
        d
    }

    /// Indices of the rows that changed since the last render-state update.
    pub fn dirty_rows(&self) -> Vec<usize> {
        let mut out = Vec::new();
        if self.render_dirty_level() == DIRTY_FALSE {
            return out;
        }
        unsafe {
            let mut iter: RowIteratorHandle = ptr::null_mut();
            if !ghostty_render_state_row_iterator_new(ptr::null(), &mut iter).is_ok() {
                return out;
            }
            ghostty_render_state_get(
                self.render,
                RENDER_DATA_ROW_ITERATOR,
                &mut iter as *mut RowIteratorHandle as *mut c_void,
            );
            let mut row = 0usize;
            while ghostty_render_state_row_iterator_next(iter) {
                let mut d: bool = false;
                ghostty_render_state_row_get(
                    iter,
                    ROW_DATA_DIRTY,
                    &mut d as *mut bool as *mut c_void,
                );
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
    /// If `rows` is empty, uses `ghostty_render_state_clean()` for efficiency.
    /// Otherwise, only clears the specified rows.
    pub fn clear_dirty(&mut self, rows: &[usize]) {
        if rows.is_empty() {
            unsafe {
                ghostty_render_state_clean(self.render);
            }
            return;
        }
        unsafe {
            let mut iter: RowIteratorHandle = ptr::null_mut();
            if !ghostty_render_state_row_iterator_new(ptr::null(), &mut iter).is_ok() {
                return;
            }
            ghostty_render_state_get(
                self.render,
                RENDER_DATA_ROW_ITERATOR,
                &mut iter as *mut RowIteratorHandle as *mut c_void,
            );
            let clear: bool = false;
            let mut row = 0usize;
            let mut rows_iter = rows.iter().peekable();
            while ghostty_render_state_row_iterator_next(iter) {
                if rows_iter.peek() == Some(&&row) {
                    ghostty_render_state_row_set(
                        iter,
                        ROW_OPTION_DIRTY,
                        &clear as *const bool as *const c_void,
                    );
                    rows_iter.next();
                }
                row += 1;
            }
            ghostty_render_state_row_iterator_free(iter);
        }
        let clean: i32 = DIRTY_FALSE;
        unsafe {
            ghostty_render_state_set(
                self.render,
                RENDER_STATE_OPTION_DIRTY,
                &clean as *const i32 as *const c_void,
            );
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
    pub fn for_each_cell(&mut self, mut f: impl FnMut(usize, usize, &RenderCell<'_>, bool, bool)) {
        unsafe {
            let mut iter: RowIteratorHandle = ptr::null_mut();
            if !ghostty_render_state_row_iterator_new(ptr::null(), &mut iter).is_ok() {
                return;
            }
            ghostty_render_state_get(
                self.render,
                RENDER_DATA_ROW_ITERATOR,
                &mut iter as *mut RowIteratorHandle as *mut c_void,
            );
            let mut cells: RowCellsHandle = ptr::null_mut();
            if !ghostty_render_state_row_cells_new(ptr::null(), &mut cells).is_ok() {
                ghostty_render_state_row_iterator_free(iter);
                return;
            }

            let mut row_idx: usize = 0;
            while ghostty_render_state_row_iterator_next(iter) {
                let mut row_dirty: bool = false;
                let mut row_sel = RenderStateRowSelection {
                    size: size_of::<RenderStateRowSelection>(),
                    start_x: 0,
                    end_x: 0,
                };
                let mut cells_handle: RowCellsHandle = ptr::null_mut();

                let keys = [ROW_DATA_DIRTY, ROW_DATA_SELECTION, ROW_DATA_CELLS];
                let mut values: [*mut c_void; 3] = [
                    &mut row_dirty as *mut bool as *mut c_void,
                    &mut row_sel as *mut RenderStateRowSelection as *mut c_void,
                    &mut cells_handle as *mut RowCellsHandle as *mut c_void,
                ];
                let mut written: usize = 0;
                let batch_ok = ghostty_render_state_row_get_multi(
                    iter,
                    keys.len(),
                    keys.as_ptr(),
                    values.as_mut_ptr(),
                    &mut written,
                )
                .is_ok();

                let sel_ok = batch_ok && written >= 2;
                if batch_ok && written >= 3 {
                    cells = cells_handle;
                } else if !batch_ok {
                    ghostty_render_state_row_get(
                        iter,
                        ROW_DATA_DIRTY,
                        &mut row_dirty as *mut bool as *mut c_void,
                    );
                    let sel_res = ghostty_render_state_row_get(
                        iter,
                        ROW_DATA_SELECTION,
                        &mut row_sel as *mut RenderStateRowSelection as *mut c_void,
                    );
                    let sel_ok_fallback = sel_res.is_ok();
                    ghostty_render_state_row_get(
                        iter,
                        ROW_DATA_CELLS,
                        &mut cells as *mut RowCellsHandle as *mut c_void,
                    );
                    let mut col_idx: usize = 0;
                    while ghostty_render_state_row_cells_next(cells) {
                        if let Some(rc) = self.read_cell(cells) {
                            let selected = sel_ok_fallback
                                && col_idx >= row_sel.start_x as usize
                                && col_idx <= row_sel.end_x as usize;
                            f(row_idx, col_idx, &rc, selected, row_dirty);
                        }
                        col_idx += 1;
                    }
                    row_idx += 1;
                    continue;
                }

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
    fn read_cell(&mut self, cells: RowCellsHandle) -> Option<RenderCell<'_>> {
        unsafe {
            // Read the cell's grapheme cluster with a reusable scratch buffer,
            // growing it only when a cell holds more text than fits (so the
            // common single-codepoint case needs one FFI call, not two).
            let mut out = Buffer {
                ptr: self.scratch.as_mut_ptr(),
                cap: self.scratch.len(),
                len: 0,
            };
            let mut res = ghostty_render_state_row_cells_get(
                cells,
                ROW_CELLS_DATA_GRAPHEMES_UTF8,
                &mut out as *mut Buffer as *mut c_void,
            );
            if res == Result::OutOfSpace {
                self.scratch.resize(out.len, 0);
                out.ptr = self.scratch.as_mut_ptr();
                out.cap = self.scratch.len();
                out.len = 0;
                res = ghostty_render_state_row_cells_get(
                    cells,
                    ROW_CELLS_DATA_GRAPHEMES_UTF8,
                    &mut out as *mut Buffer as *mut c_void,
                );
            }
            let has_text = res.is_ok() && out.len > 0;
            // Borrow directly from scratch to avoid per-cell String allocation.
            // The `&str` lives as long as `&mut self` is borrowed (until next
            // `read_cell` call), which is exactly the duration of the
            // `for_each_cell` closure invocation.
            let text = if has_text {
                std::str::from_utf8(&self.scratch[..out.len]).unwrap_or("")
            } else {
                ""
            };

            // OSC 8 hyperlinks live on text cells; query the raw cell's
            // hyperlink bit directly (the render state copies the page cell).
            let mut hyperlink = false;
            if has_text {
                let mut raw: u64 = 0;
                if ghostty_render_state_row_cells_get(
                    cells,
                    ROW_CELLS_DATA_RAW,
                    &mut raw as *mut u64 as *mut c_void,
                )
                .is_ok()
                {
                    let mut hl: bool = false;
                    if ghostty_cell_get(
                        raw,
                        CELL_DATA_HAS_HYPERLINK,
                        &mut hl as *mut bool as *mut c_void,
                    )
                    .is_ok()
                    {
                        hyperlink = hl;
                    }
                }
            }

            let mut style = Style::new();
            ghostty_render_state_row_cells_get(
                cells,
                ROW_CELLS_DATA_STYLE,
                &mut style as *mut Style as *mut c_void,
            );
            let has_fg = style.fg_color.tag != STYLE_COLOR_NONE;
            let has_bg = style.bg_color.tag != STYLE_COLOR_NONE;

            let mut fg = self.default_fg;
            let mut bg = self.default_bg;
            let _ = ghostty_render_state_row_cells_get(
                cells,
                ROW_CELLS_DATA_FG_COLOR,
                &mut fg as *mut ColorRgb as *mut c_void,
            );
            let _ = ghostty_render_state_row_cells_get(
                cells,
                ROW_CELLS_DATA_BG_COLOR,
                &mut bg as *mut ColorRgb as *mut c_void,
            );

            // Skip fully blank cells with no explicit background; the renderer
            // fills those with the terminal's default background.
            if !has_text && !has_bg {
                return None;
            }

            Some(RenderCell {
                text,
                fg,
                bg,
                has_fg,
                has_bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline != 0,
                inverse: style.inverse,
                faint: style.faint,
                hyperlink,
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

    fn new_term(cols: u16, rows: u16, max: usize) -> Terminal {
        let black = ColorRgb::new(0, 0, 0);
        Terminal::new(cols, rows, max, &palette(), black, black, black).unwrap()
    }

    fn collect(t: &mut Terminal) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        t.for_each_cell(|r, c, rc, _selected, _row_dirty| {
            if !rc.text.is_empty() {
                out.push((r, c, rc.text.to_string()));
            }
        });
        out
    }

    #[test]
    fn renders_plain_text() {
        let mut t = new_term(10, 4, 100);
        t.write(b"hello");
        t.refresh();
        let cells = collect(&mut t);
        let texts: Vec<&str> = cells.iter().map(|(_, _, s)| s.as_str()).collect();
        assert_eq!(texts, vec!["h", "e", "l", "l", "o"]);
    }

    #[test]
    fn renders_ansi_colors_and_styles() {
        let mut t = new_term(10, 4, 100);
        t.write(b"\x1b[31mred\x1b[1mbold\x1b[0mplain");
        t.refresh();
        let cells = collect(&mut t);
        let texts: Vec<&str> = cells.iter().map(|(_, _, s)| s.as_str()).collect();
        assert_eq!(
            texts,
            vec!["r", "e", "d", "b", "o", "l", "d", "p", "l", "a", "i", "n"]
        );
    }

    #[test]
    fn reports_pwd_from_osc7_uri() {
        let mut t = new_term(10, 4, 100);
        assert_eq!(t.pwd(), None, "no pwd before any OSC 7");
        t.write(b"\x1b]7;file:///tmp/example\x07");
        assert_eq!(
            t.pwd().as_deref(),
            Some(PathBuf::from("/tmp/example").as_path())
        );
        // A bare OSC 9 path also counts.
        t.write(b"\x1b]9;9;/tmp/osc9\x1b\\");
        assert_eq!(
            t.pwd().as_deref(),
            Some(PathBuf::from("/tmp/osc9").as_path())
        );
        // Clearing the pwd (empty OSC 7) yields None again.
        t.write(b"\x1b]7;\x07");
        assert_eq!(t.pwd(), None);
    }

    #[test]
    fn pwd_decodes_localhost_and_percent_encoding() {
        let mut t = new_term(10, 4, 100);
        t.write(b"\x1b]7;file://localhost/Users/my%20dir/proj\x07");
        assert_eq!(
            t.pwd().as_deref(),
            Some(PathBuf::from("/Users/my dir/proj").as_path())
        );
    }

    #[test]
    fn pwd_ignores_authority_hostname() {
        // fish reports its own hostname as the authority, e.g.
        // `file://My-Mac.local/Users/x`, not `localhost`.
        let mut t = new_term(10, 4, 100);
        t.write(b"\x1b]7;file://My-Mac.local/Users/marc/proj\x07");
        assert_eq!(
            t.pwd().as_deref(),
            Some(PathBuf::from("/Users/marc/proj").as_path())
        );
    }

    #[test]
    fn snapshot_preserves_scrollback() {
        let mut t = new_term(20, 5, 100);
        t.write(b"hello world\nsecond line\nthird line\n");
        t.refresh();
        let mut before = Vec::new();
        t.for_each_cell(|r, c, rc, _, _| {
            if !rc.text.is_empty() {
                before.push((r, c, rc.text.to_string()));
            }
        });
        assert!(!before.is_empty(), "before should have cells");
        let snap = t.snapshot_encode().expect("snapshot encode should succeed");
        assert!(!snap.is_empty(), "snapshot should not be empty");
        let mut t2 = Terminal::from_snapshot(
            &snap,
            100,
            &palette(),
            ColorRgb::new(0, 0, 0),
            ColorRgb::new(0, 0, 0),
            ColorRgb::new(255, 255, 255),
        )
        .expect("snapshot decode should succeed");
        t2.refresh();
        let mut after = Vec::new();
        t2.for_each_cell(|r, c, rc, _, _| {
            if !rc.text.is_empty() {
                after.push((r, c, rc.text.to_string()));
            }
        });
        assert_eq!(
            before, after,
            "visible screen should survive snapshot roundtrip: before={before:?} after={after:?}"
        );
        // Also check that we can still write after restore without panic.
        t2.write(b"new line after restore\n");
        t2.refresh();
        let mut has_cells = false;
        t2.for_each_cell(|_, _, rc, _, _| {
            if !rc.text.is_empty() {
                has_cells = true;
            }
        });
        assert!(has_cells, "terminal should still be writable after restore");
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
        let mut t = new_term(10, 4, 100);
        t.set_write_sink(sink);
        t.write(b"\x1b[c"); // DA1
        let s = String::from_utf8_lossy(&buf.buf);
        assert!(!s.is_empty(), "no response to DA query");
        assert!(
            s.starts_with("\x1b[?"),
            "expected CSI ? response, got {s:?}"
        );
    }

    #[test]
    fn writes_cursor_and_scroll() {
        let mut t = new_term(10, 4, 100);
        t.write(b"line1\nline2\nline3\nline4\nline5");
        t.refresh();
        assert!(t.cursor_pos().is_some());
        t.scroll(-2);
        t.refresh();
        assert!(t.scrollbar().total > 0);
    }

    #[test]
    fn bottom_text_reads_buffer_tail_independent_of_scroll() {
        let mut t = new_term(20, 5, 100);
        t.write(b"top\nmid\nbottom\n\nexit shell mode");
        t.refresh();
        let before = t.bottom_text(5);
        assert!(
            before.contains("exit shell mode"),
            "expected tail text, got {before:?}"
        );
        // Scrolling the viewport must not change what bottom_text reads.
        t.scroll(-3);
        t.refresh();
        let after = t.bottom_text(5);
        assert_eq!(before, after, "bottom_text changed after viewport scroll");
    }

    #[test]
    fn native_selection_highlights_and_extracts() {
        let mut t = new_term(20, 5, 100);
        t.write(b"hello world\nsecond line");
        t.refresh();
        assert!(
            t.set_selection((0, 0), (4, 0)),
            "set_selection should succeed"
        );
        t.refresh();
        let mut selected = Vec::new();
        t.for_each_cell(|r, c, rc, s, _row_dirty| {
            if s {
                selected.push((r, c, rc.text.to_string()));
            }
        });
        assert!(!selected.is_empty(), "row should intersect the selection");
        assert_eq!(t.selection_text((0, 0), (4, 0)).as_deref(), Some("hello"));

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

    #[test]
    fn osc8_hyperlinks_expose_uri_and_cell_flag() {
        let mut t = new_term(40, 4, 100);
        t.write(b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07");
        t.refresh();

        // The link cells are flagged and the URI resolves on them.
        let mut links = Vec::new();
        t.for_each_cell(|r, c, rc, _sel, _dirty| {
            if rc.hyperlink {
                links.push((r, c, rc.text.to_string()));
            }
        });
        assert_eq!(
            links,
            vec![
                (0, 0, "l".into()),
                (0, 1, "i".into()),
                (0, 2, "n".into()),
                (0, 3, "k".into()),
            ]
        );
        assert_eq!(t.hyperlink_at(0, 0).as_deref(), Some("https://example.com"));
        assert_eq!(t.hyperlink_at(3, 0).as_deref(), Some("https://example.com"));

        // No hyperlink outside the link (or on a later line).
        assert_eq!(t.hyperlink_at(5, 0), None);
        assert_eq!(t.hyperlink_at(0, 1), None);
    }

    #[test]
    fn detects_plain_text_urls_on_row() {
        let mut t = new_term(80, 4, 100);
        t.write(b"  - Local:         http://localhost:3000\r\n  - Network:       http://192.168.1.134:3000");
        t.refresh();

        // row_text preserves leading whitespace so cell columns map onto chars.
        let row = t.row_text(0);
        let url_start = row.find("http://localhost:3000").expect("url in row text");
        let col = row[..url_start].chars().count() as u16;
        assert_eq!(t.link_at(col, 0).as_deref(), Some("http://localhost:3000"));
        // One column before the URL (on the dashes) is not a link.
        assert_eq!(t.link_at(col - 1, 0), None);
        // The second row holds the network URL.
        assert_eq!(
            t.link_at(19, 1).as_deref(),
            Some("http://192.168.1.134:3000")
        );
    }

    #[test]
    fn find_urls_handles_bounds_and_punctuation() {
        let urls = find_urls("See http://example.com/a(b). and (https://x.org). tail");
        let got: Vec<String> = urls.iter().map(|(_, _, u)| u.clone()).collect();
        assert_eq!(got, vec!["http://example.com/a(b)", "https://x.org"]);
        // No scheme: not detected.
        assert!(find_urls("localhost:3000 and time: 3:00").is_empty());
    }
}
