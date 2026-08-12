//! Wire protocol between the thin terminal client and the kumo daemon.
//!
//! Framing: `[u32 LE length][bincode payload]`. Control and render messages are
//! bincode-serialized; the payload is never longer than `MAX_FRAME_LEN`. Input
//! travels as structured key events (not raw bytes) so the daemon's app can
//! interpret leader keys, navigation, etc. itself.

use std::io::{self, Read};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Protocol version. Bump on breaking wire changes; the daemon rejects clients
/// with a mismatched version. The daemon is unreleased, so it starts at 1;
/// once 0.4.0 ships, wire changes must bump it.
pub const PROTOCOL_VERSION: u32 = 2;
/// Upper bound for a single frame payload (a full 80x24 grid fits comfortably).
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Wire input events
// ---------------------------------------------------------------------------

/// Serialized `KeyCode` mirror (crossterm types are not `Serialize`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Esc,
    Null,
    F(u8),
    Char(char),
    Media,
    Modifier,
}

impl WireKeyCode {
    pub fn from_crossterm(code: crossterm::event::KeyCode) -> Self {
        use crossterm::event::KeyCode as K;
        match code {
            K::Backspace => Self::Backspace,
            K::Enter => Self::Enter,
            K::Left => Self::Left,
            K::Right => Self::Right,
            K::Up => Self::Up,
            K::Down => Self::Down,
            K::Home => Self::Home,
            K::End => Self::End,
            K::PageUp => Self::PageUp,
            K::PageDown => Self::PageDown,
            K::Tab => Self::Tab,
            K::BackTab => Self::BackTab,
            K::Delete => Self::Delete,
            K::Insert => Self::Insert,
            K::Esc => Self::Esc,
            K::Null => Self::Null,
            K::F(n) => Self::F(n),
            K::Char(c) => Self::Char(c),
            K::CapsLock
            | K::ScrollLock
            | K::NumLock
            | K::PrintScreen
            | K::Pause
            | K::Menu
            | K::KeypadBegin => Self::Modifier,
            _ => Self::Media,
        }
    }

    pub fn to_crossterm(self) -> crossterm::event::KeyCode {
        use crossterm::event::KeyCode as K;
        match self {
            Self::Backspace => K::Backspace,
            Self::Enter => K::Enter,
            Self::Left => K::Left,
            Self::Right => K::Right,
            Self::Up => K::Up,
            Self::Down => K::Down,
            Self::Home => K::Home,
            Self::End => K::End,
            Self::PageUp => K::PageUp,
            Self::PageDown => K::PageDown,
            Self::Tab => K::Tab,
            Self::BackTab => K::BackTab,
            Self::Delete => K::Delete,
            Self::Insert => K::Insert,
            Self::Esc => K::Esc,
            Self::Null => K::Null,
            Self::F(n) => K::F(n),
            Self::Char(c) => K::Char(c),
            _ => K::Null,
        }
    }
}

/// Serialized key modifiers (bitfield; crossterm uses `KeyModifiers`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WireModifiers(u8);

impl WireModifiers {
    fn from_crossterm(m: crossterm::event::KeyModifiers) -> Self {
        use crossterm::event::KeyModifiers as M;
        let mut bits = 0u8;
        if m.contains(M::SHIFT) {
            bits |= 1 << 0;
        }
        if m.contains(M::CONTROL) {
            bits |= 1 << 1;
        }
        if m.contains(M::ALT) {
            bits |= 1 << 2;
        }
        if m.contains(M::SUPER) {
            bits |= 1 << 3;
        }
        if m.contains(M::HYPER) {
            bits |= 1 << 4;
        }
        if m.contains(M::META) {
            bits |= 1 << 5;
        }
        Self(bits)
    }

    fn to_crossterm(self) -> crossterm::event::KeyModifiers {
        use crossterm::event::KeyModifiers as M;
        let mut m = M::empty();
        if self.0 & (1 << 0) != 0 {
            m |= M::SHIFT;
        }
        if self.0 & (1 << 1) != 0 {
            m |= M::CONTROL;
        }
        if self.0 & (1 << 2) != 0 {
            m |= M::ALT;
        }
        if self.0 & (1 << 3) != 0 {
            m |= M::SUPER;
        }
        if self.0 & (1 << 4) != 0 {
            m |= M::HYPER;
        }
        if self.0 & (1 << 5) != 0 {
            m |= M::META;
        }
        m
    }
}

/// A serialized key press.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct WireKeyEvent {
    pub code: WireKeyCode,
    pub modifiers: WireModifiers,
}

impl WireKeyEvent {
    pub fn to_crossterm(self) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(self.code.to_crossterm(), self.modifiers.to_crossterm())
    }
}

impl From<crossterm::event::KeyEvent> for WireKeyEvent {
    fn from(k: crossterm::event::KeyEvent) -> Self {
        Self {
            code: WireKeyCode::from_crossterm(k.code),
            modifiers: WireModifiers::from_crossterm(k.modifiers),
        }
    }
}

// ---------------------------------------------------------------------------
// Wire mouse events
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireMouseKind {
    Down(WireMouseButton),
    Up(WireMouseButton),
    Drag(WireMouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

/// A serialized mouse event (crossterm types are not `Serialize`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct WireMouseEvent {
    pub kind: WireMouseKind,
    pub col: u16,
    pub row: u16,
    pub modifiers: WireModifiers,
}

fn wire_button_to_crossterm(b: WireMouseButton) -> crossterm::event::MouseButton {
    match b {
        WireMouseButton::Left => crossterm::event::MouseButton::Left,
        WireMouseButton::Right => crossterm::event::MouseButton::Right,
        WireMouseButton::Middle => crossterm::event::MouseButton::Middle,
    }
}

fn crossterm_button_to_wire(b: crossterm::event::MouseButton) -> WireMouseButton {
    match b {
        crossterm::event::MouseButton::Left => WireMouseButton::Left,
        crossterm::event::MouseButton::Right => WireMouseButton::Right,
        _ => WireMouseButton::Middle,
    }
}

impl From<crossterm::event::MouseEvent> for WireMouseEvent {
    fn from(m: crossterm::event::MouseEvent) -> Self {
        use crossterm::event::MouseEventKind as K;
        let kind = match m.kind {
            K::Down(b) => WireMouseKind::Down(crossterm_button_to_wire(b)),
            K::Up(b) => WireMouseKind::Up(crossterm_button_to_wire(b)),
            K::Drag(b) => WireMouseKind::Drag(crossterm_button_to_wire(b)),
            K::Moved => WireMouseKind::Moved,
            K::ScrollUp => WireMouseKind::ScrollUp,
            K::ScrollDown => WireMouseKind::ScrollDown,
            K::ScrollLeft => WireMouseKind::ScrollLeft,
            K::ScrollRight => WireMouseKind::ScrollRight,
        };
        Self {
            kind,
            col: m.column,
            row: m.row,
            modifiers: WireModifiers::from_crossterm(m.modifiers),
        }
    }
}

impl WireMouseEvent {
    pub fn to_crossterm(self) -> crossterm::event::MouseEvent {
        use crossterm::event::MouseEventKind as K;
        let kind = match self.kind {
            WireMouseKind::Down(b) => K::Down(wire_button_to_crossterm(b)),
            WireMouseKind::Up(b) => K::Up(wire_button_to_crossterm(b)),
            WireMouseKind::Drag(b) => K::Drag(wire_button_to_crossterm(b)),
            WireMouseKind::Moved => K::Moved,
            WireMouseKind::ScrollUp => K::ScrollUp,
            WireMouseKind::ScrollDown => K::ScrollDown,
            WireMouseKind::ScrollLeft => K::ScrollLeft,
            WireMouseKind::ScrollRight => K::ScrollRight,
        };
        crossterm::event::MouseEvent {
            kind,
            column: self.col,
            row: self.row,
            modifiers: self.modifiers.to_crossterm(),
        }
    }
}

// ---------------------------------------------------------------------------
// Render frame
// ---------------------------------------------------------------------------

/// A serialized rendered cell (the grid the daemon draws).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WireCell {
    /// Grapheme to draw; empty for cells with no content.
    pub text: String,
    /// Packed 0xRRGGBB foreground; `None` = default.
    pub fg: Option<u32>,
    /// Packed 0xRRGGBB background; `None` = default.
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub faint: bool,
    /// Cell width in columns: `2` for a wide grapheme, `0` for the
    /// continuation cell after a wide grapheme, `1` otherwise. The client skips
    /// continuation cells so wide characters are not overwritten.
    pub cell_width: u16,
}

impl WireCell {
    fn from_ratatui(cell: &ratatui::buffer::Cell) -> Self {
        use ratatui::buffer::CellWidth;
        use ratatui::style::{Color, Modifier};
        let fg = match cell.fg {
            Color::Rgb(r, g, b) => Some(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)),
            _ => None,
        };
        let bg = match cell.bg {
            Color::Rgb(r, g, b) => Some(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)),
            _ => None,
        };
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
        Self {
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
}

/// One dirty row: its index plus every cell, so the client can repaint it fully
/// (handles wide chars, styles, and cells that were cleared).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct RowPatch {
    pub row: u16,
    pub cells: Vec<WireCell>,
}

/// A render frame. Rows are sent as patches; `full` means every row is included
/// and the client should clear the screen first (first frame, or a resize).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct FrameMsg {
    pub cols: u16,
    pub rows: u16,
    pub full: bool,
    pub rows_dirty: Vec<RowPatch>,
    /// Host-terminal cursor position, if the app wants one shown.
    pub cursor: Option<(u16, u16)>,
}

impl FrameMsg {
    /// A frame containing every row (`full = true`): for a client's first
    /// attach or after a resize.
    pub fn full_frame(buf: &ratatui::buffer::Buffer, cursor: Option<(u16, u16)>) -> Self {
        let cols = buf.area.width;
        let rows = buf.area.height;
        let rows_dirty = (0..rows)
            .map(|row| RowPatch {
                row,
                cells: row_cells(buf, row, cols),
            })
            .collect();
        Self { cols, rows, full: true, rows_dirty, cursor }
    }

    /// A frame containing only the rows that changed since `last` (same size).
    pub fn diff_frame(
        buf: &ratatui::buffer::Buffer,
        last: &ratatui::buffer::Buffer,
        cursor: Option<(u16, u16)>,
    ) -> Self {
        let cols = buf.area.width;
        let rows = buf.area.height;
        let rows_dirty = (0..rows)
            .filter(|row| row_changed(buf, last, *row, cols))
            .map(|row| RowPatch { row, cells: row_cells(buf, row, cols) })
            .collect();
        Self { cols, rows, full: false, rows_dirty, cursor }
    }
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
fn row_cells(buf: &ratatui::buffer::Buffer, row: u16, cols: u16) -> Vec<WireCell> {
    let s = row as usize * cols as usize;
    let e = s + cols as usize;
    let cells: Vec<WireCell> = buf.content[s..e].iter().map(WireCell::from_ratatui).collect();
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
fn row_changed(buf: &ratatui::buffer::Buffer, last: &ratatui::buffer::Buffer, row: u16, cols: u16) -> bool {
    let s = row as usize * cols as usize;
    let e = s + cols as usize;
    buf.content[s..e] != last.content[s..e]
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ClientMsg {
    Hello {
        protocol: u32,
        cols: u16,
        rows: u16,
    },
    Input {
        key: WireKeyEvent,
    },
    Mouse {
        event: WireMouseEvent,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// The client is detaching; the daemon keeps running.
    Detach,
    /// `kumo ls`: request the session list (non-TUI client).
    ListSessions,
    /// `kumo kill`: stop the daemon (killing its panes).
    KillServer,
    /// `kumo new [WORKSPACE]` against a running daemon: create a fresh session
    /// and focus it. `workspace` is the client-resolved dir (its cwd when no
    /// explicit arg was given; the daemon falls back to its own if unusable).
    NewSession {
        workspace: Option<std::path::PathBuf>,
    },
}

/// One session, as reported to `kumo ls`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SessionInfo {
    pub name: String,
    pub workspace: std::path::PathBuf,
    pub panes: usize,
    pub zoomed: bool,
    pub active: bool,
    /// AI CLIs running inside this session (name + lifecycle status), so a
    /// blocked agent is visible from `kumo ls` without attaching.
    pub agents: Vec<AgentInfo>,
}

/// One AI CLI running inside a session, as reported to `kumo ls`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct AgentInfo {
    /// Short AI CLI name, e.g. "opencode".
    pub name: String,
    /// Lifecycle status inferred from the pane's terminal buffer.
    pub status: AgentStatus,
}

/// Wire copy of [`crate::agents::AgentStatus`]: the AI agent's lifecycle state.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentStatus {
    /// Actively producing output (working on a task).
    Working,
    /// Quiet but waiting for a command approval.
    Blocked,
    /// Quiet and idle.
    Idle,
}

impl AgentStatus {
    /// Lowercase display label for `kumo ls` output.
    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Idle => "idle",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ServerMsg {
    Welcome {
        protocol: u32,
    },
    Frame {
        frame: FrameMsg,
    },
    /// `leader+d`: the daemon asks this client to disconnect.
    Detach,
    /// The daemon is stopping (last session closed / `kumo kill`).
    Shutdown,
    /// Response to `ListSessions`.
    SessionList {
        sessions: Vec<SessionInfo>,
    },
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Encode a message as a length-prefixed bincode payload.
pub fn encode<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>> {
    let payload = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Incremental reader that turns a byte stream into framed messages.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Feed a chunk of bytes; yields every complete frame it contained.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<Vec<u8>>) {
        self.buf.extend_from_slice(data);
        loop {
            if self.buf.len() < 4 {
                return;
            }
            let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
            if len > MAX_FRAME_LEN {
                // Unrecoverable framing corruption; drop the connection.
                self.buf.clear();
                return;
            }
            if self.buf.len() < 4 + len {
                return;
            }
            out.push(self.buf[4..4 + len].to_vec());
            self.buf.drain(..4 + len);
        }
    }
}

/// Read one message from `reader` using the length-prefixed framing.
pub fn read_framed<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(anyhow::anyhow!("oversized protocol frame ({len} bytes)"));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    let (msg, _): (T, usize) =
        bincode::serde::decode_from_slice(&payload, bincode::config::standard())?;
    Ok(msg)
}

pub fn write_framed<T: serde::Serialize>(writer: &mut impl io::Write, msg: &T) -> Result<()> {
    let bytes = encode(msg)?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    #[test]
    fn key_roundtrip() {
        for code in [
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyCode::F(12),
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyCode::Backspace,
        ] {
            let k = crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::CONTROL);
            let wire: WireKeyEvent = k.into();
            let back = wire.to_crossterm();
            assert_eq!(back.code, code);
            assert!(back.modifiers.contains(crossterm::event::KeyModifiers::CONTROL));
        }
    }

    #[test]
    fn old_session_list_without_agents_is_rejected() {
        // A pre-`agents` daemon serializes SessionInfo with 5 fields. Decoding
        // that with the current struct must fail (the client surfaces a
        // "restart your daemon" hint rather than a silent wrong answer).
        #[derive(Serialize)]
        struct OldSessionInfo {
            name: String,
            workspace: std::path::PathBuf,
            panes: usize,
            zoomed: bool,
            active: bool,
        }
        let old = OldSessionInfo {
            name: "session-1".into(),
            workspace: std::path::PathBuf::from("/tmp"),
            panes: 1,
            zoomed: false,
            active: true,
        };
        let bytes = bincode::serde::encode_to_vec(vec![old], bincode::config::standard()).unwrap();
        let decoded: Result<(Vec<SessionInfo>, usize), _> =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard());
        assert!(decoded.is_err(), "an old-format SessionInfo must not decode as the current one");
    }

    #[test]
    fn session_info_agents_roundtrip() {
        let info = SessionInfo {
            name: "session-1".into(),
            workspace: std::path::PathBuf::from("/tmp"),
            panes: 2,
            zoomed: false,
            active: true,
            agents: vec![
                AgentInfo { name: "opencode".into(), status: AgentStatus::Blocked },
                AgentInfo { name: "claude".into(), status: AgentStatus::Working },
            ],
        };
        let bytes = bincode::serde::encode_to_vec(&info, bincode::config::standard()).unwrap();
        let (back, _): (SessionInfo, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn frame_reader_reassembles_frames() {
        let msg = ServerMsg::Welcome { protocol: 1 };
        let bytes = encode(&msg).unwrap();
        let mut reader = FrameReader::default();
        let mut out = Vec::new();
        // Split across arbitrary chunk boundaries.
        for chunk in bytes.chunks(3) {
            reader.push(chunk, &mut out);
        }
        assert_eq!(out.len(), 1);
        let decoded: ServerMsg = bincode::serde::decode_from_slice(&out[0], bincode::config::standard())
            .unwrap()
            .0;
        assert_eq!(decoded, msg);
    }

    #[test]
    fn read_write_framed_roundtrip() {
        let msg = ServerMsg::Shutdown;
        let mut buf = Vec::new();
        write_framed(&mut buf, &msg).unwrap();
        let decoded: ServerMsg = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn diff_frame_sends_only_changed_rows() {
        let mut a = Buffer::empty(Rect::new(0, 0, 4, 3));
        let mut b = Buffer::empty(Rect::new(0, 0, 4, 3));
        b.cell_mut((1, 0)).unwrap().set_symbol("X");
        let frame = FrameMsg::diff_frame(&b, &a, None);
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
        let frame = FrameMsg::diff_frame(&b, &a, None);
        assert!(!frame.full);
        assert!(frame.rows_dirty.is_empty());
    }

    #[test]
    fn full_frame_includes_every_row() {
        let buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        let frame = FrameMsg::full_frame(&buf, None);
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
        let frame = FrameMsg::full_frame(&buf, None);
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
        let frame = FrameMsg::full_frame(&buf, None);
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
        let frame = FrameMsg::full_frame(&buf, None);
        let cells = &frame.rows_dirty[0].cells;
        assert_eq!(cells[0].cell_width, 2);
        assert_eq!(cells[1].cell_width, 0, "trailing empty cell is a blank, not content");
    }
}
