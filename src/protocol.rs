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
/// with a mismatched version.
pub const PROTOCOL_VERSION: u32 = 1;
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
}

impl WireCell {
    fn from_ratatui(cell: &ratatui::buffer::Cell) -> Self {
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
        Self {
            text: cell.symbol().to_string(),
            fg,
            bg,
            bold: m.contains(Modifier::BOLD),
            italic: m.contains(Modifier::ITALIC),
            underline: m.contains(Modifier::UNDERLINED),
            inverse: m.contains(Modifier::REVERSED),
            faint: m.contains(Modifier::DIM),
        }
    }
}

/// A full rendered grid (phase 1: no row diffs yet).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct FrameMsg {
    pub cols: u16,
    pub rows: u16,
    /// Row-major cells.
    pub cells: Vec<WireCell>,
    /// Host-terminal cursor position, if the app wants one shown.
    pub cursor: Option<(u16, u16)>,
}

impl FrameMsg {
    pub fn from_buffer(buf: &ratatui::buffer::Buffer, cursor: Option<(u16, u16)>) -> Self {
        Self {
            cols: buf.area.width,
            rows: buf.area.height,
            cells: buf.content.iter().map(WireCell::from_ratatui).collect(),
            cursor,
        }
    }
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
}
