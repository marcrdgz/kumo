//! Wire protocol between the kumo daemon and its clients (the TUI client, the
//! GPUI desktop app, and future mobile clients).
//!
//! Framing: `[u32 LE length][bincode payload]`. Messages are bincode-serialized;
//! the payload is never longer than `MAX_FRAME_LEN`.
//!
//! This crate is deliberately pure: it depends only on `serde`/`bincode`, never
//! on `ratatui`, `crossterm`, or the terminal emulator, so any client can speak
//! the protocol without dragging in the whole kumo stack. Conversions to and
//! from host types (`crossterm` events, `ratatui` buffers) live in the kumo
//! crate (`src/wireconv.rs`, `src/frames.rs`).
//!
//! The daemon is the single source of truth for everything it has open
//! (sessions, panes, agents); this protocol is how it hands that context to
//! clients with different capabilities:
//!
//! - **Full attach** (`ClientMsg::Hello` + `ServerMsg::Frame`): the daemon
//!   renders its whole UI headlessly and streams dirty-row cell patches. Used
//!   by the TUI client and the desktop app's main view.
//! - **Snapshot** (`ClientMsg::SubscribeSnapshot` + `ServerMsg::Snapshot`):
//!   structured sessions/panes/agents, pushed on change. Drives native
//!   sidebars, session lists, and mobile overviews.
//! - **Pane frames** (`ClientMsg::SubscribePane` + `ServerMsg::PaneFrame`):
//!   a single pane rendered as its own grid, for per-pane views (mobile) and
//!   native pane layout (desktop) later.
//!
//! Clients declare what they want in [`ClientMsg::Hello`] (`kind`), so the
//! daemon can route the right channels per client.

use std::io::{self, Read};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "crossterm")]
mod crossterm;

/// Protocol version. Bump on breaking wire changes; the daemon rejects clients
/// with a mismatched version. v3 adds per-pane geometry (`PaneRect`), the
/// session's focused pane, and the `FocusPane` / `SetSidebar` control messages
/// for native clients that paint panes themselves.
pub const PROTOCOL_VERSION: u32 = 3;
/// Upper bound for a single frame payload (a full 80x24 grid fits comfortably).
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// What kind of client is attaching. The daemon routes delivery channels
/// (full frames vs. snapshots vs. pane frames) based on it.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClientKind {
    /// The `kumo` TUI client in a host terminal: full-attach render loop.
    Terminal,
    /// A native desktop app: full-attach render loop + snapshot sidebar.
    Desktop,
    /// A small-screen / read-only client (mobile): snapshot + pane frames.
    Mobile,
}

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

/// Serialized key modifiers (bitfield; crossterm uses `KeyModifiers`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WireModifiers(u8);

impl WireModifiers {
    /// No modifiers.
    pub fn none() -> Self {
        Self(0)
    }

    /// Build from a raw bitfield (bit 0 = shift, 1 = control, 2 = alt,
    /// 3 = super, 4 = hyper, 5 = meta).
    pub fn from_raw(bits: u8) -> Self {
        Self(bits)
    }

    /// Whether the SHIFT modifier is set.
    pub fn shift(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    /// Whether the CONTROL modifier is set.
    pub fn control(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// Whether the ALT modifier is set.
    pub fn alt(self) -> bool {
        self.0 & (1 << 2) != 0
    }

    /// Whether the SUPER (cmd/win) modifier is set.
    pub fn super_key(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    /// Whether the HYPER modifier is set.
    pub fn hyper(self) -> bool {
        self.0 & (1 << 4) != 0
    }

    /// Whether the META modifier is set.
    pub fn meta(self) -> bool {
        self.0 & (1 << 5) != 0
    }

    /// Set the SHIFT bit.
    pub fn set_shift(&mut self, on: bool) -> &mut Self {
        self.set(0, on)
    }

    /// Set the CONTROL bit.
    pub fn set_control(&mut self, on: bool) -> &mut Self {
        self.set(1, on)
    }

    /// Set the ALT bit.
    pub fn set_alt(&mut self, on: bool) -> &mut Self {
        self.set(2, on)
    }

    /// Set the SUPER bit.
    pub fn set_super(&mut self, on: bool) -> &mut Self {
        self.set(3, on)
    }

    /// Set the HYPER bit.
    pub fn set_hyper(&mut self, on: bool) -> &mut Self {
        self.set(4, on)
    }

    /// Set the META bit.
    pub fn set_meta(&mut self, on: bool) -> &mut Self {
        self.set(5, on)
    }

    fn set(&mut self, bit: u8, on: bool) -> &mut Self {
        if on {
            self.0 |= 1 << bit;
        } else {
            self.0 &= !(1 << bit);
        }
        self
    }
}

/// A serialized key press.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct WireKeyEvent {
    pub code: WireKeyCode,
    pub modifiers: WireModifiers,
}

impl WireKeyEvent {
    /// Build a key event from its code and modifiers.
    pub fn new(code: WireKeyCode, modifiers: WireModifiers) -> Self {
        Self { code, modifiers }
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

/// A serialized mouse event.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct WireMouseEvent {
    pub kind: WireMouseKind,
    pub col: u16,
    pub row: u16,
    pub modifiers: WireModifiers,
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

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ClientMsg {
    Hello {
        protocol: u32,
        /// What kind of client is attaching (drives delivery routing).
        kind: ClientKind,
        cols: u16,
        rows: u16,
    },
    Input {
        key: WireKeyEvent,
    },
    /// Text pasted into the client (bracketed paste), e.g. from the OS
    /// clipboard. Carried as raw text so the daemon can strip trailing
    /// newlines and route it to the focused pane.
    Paste {
        text: String,
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
    /// `kumo update` after swapping the binary: ask the daemon to restart
    /// itself (exec the new binary) inheriting the live PTY masters, so the
    /// running panes and agents survive the update.
    Restart,
    /// `kumo reload`: re-read the config and apply it live (shell, leader,
    /// keymap bindings); new panes and future actions pick it up.
    ReloadConfig,
    /// Start pushing [`ServerMsg::Snapshot`] on every change (sidebar / mobile
    /// overviews). The daemon also responds with one snapshot immediately.
    SubscribeSnapshot,
    /// Focus the session with the given name (desktop sidebar click).
    FocusSession {
        name: String,
    },
    /// Focus a specific pane inside a session (desktop pane click). The daemon
    /// switches to the session and routes subsequent `Input` to the pane.
    FocusPane {
        session: String,
        pane_id: u64,
    },
    /// Open/close the daemon's own sidebar. Native clients paint their own
    /// chrome, so they close the daemon's to give panes the full width.
    SetSidebar {
        open: bool,
    },
    /// Start streaming [`ServerMsg::PaneFrame`] for one pane (per-pane views).
    SubscribePane {
        pane_id: u64,
    },
    /// Stop streaming [`ServerMsg::PaneFrame`] for one pane.
    UnsubscribePane {
        pane_id: u64,
    },
}

/// A pane's rectangle within its session's grid (cell coordinates).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaneRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// One pane, as reported inside a [`SessionInfo`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PaneInfo {
    pub id: u64,
    /// Display title (custom name or the default `shell N` / `AI CLI` label).
    pub title: String,
    /// Working directory the pane is currently in (follow-workspace).
    pub cwd: std::path::PathBuf,
    /// Whether the pane is (or hosts) an AI CLI.
    pub is_ai: bool,
    /// The running AI CLI, when this pane hosts one.
    pub agent: Option<AgentInfo>,
    /// Where the pane sits in its session's grid. Lets native clients paint
    /// panes themselves instead of showing the daemon's composed UI.
    pub rect: PaneRect,
}

/// One session, as reported to `kumo ls` and pushed in snapshots.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SessionInfo {
    pub name: String,
    pub workspace: std::path::PathBuf,
    pub panes: Vec<PaneInfo>,
    pub zoomed: bool,
    pub active: bool,
    /// The pane currently focused in this session (`None` when empty).
    pub focus: Option<u64>,
}

/// One AI CLI running inside a session.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct AgentInfo {
    /// Short AI CLI name, e.g. "opencode".
    pub name: String,
    /// Lifecycle status inferred from the pane's terminal buffer.
    pub status: AgentStatus,
}

/// Wire copy of the daemon's `AgentStatus`: the AI agent's lifecycle state.
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
    /// Lowercase display label.
    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Idle => "idle",
        }
    }
}

/// One pane rendered as its own grid (per-pane channel).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PaneFrame {
    pub pane_id: u64,
    pub cols: u16,
    pub rows: u16,
    /// `true` = every row included; clear the view first.
    pub full: bool,
    pub rows_dirty: Vec<RowPatch>,
    pub cursor: Option<(u16, u16)>,
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
    /// The daemon is restarting itself for `kumo update`: drop the socket and
    /// reconnect (with retries) instead of erroring out.
    Restarting,
    /// Response to `ListSessions`.
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    /// Response to `ReloadConfig`: the outcome, as a status-bar/CLI notice.
    ConfigReloaded {
        notice: String,
    },
    /// Pushed to `SubscribeSnapshot` subscribers whenever sessions/panes/
    /// agents change.
    Snapshot {
        sessions: Vec<SessionInfo>,
    },
    /// A subscribed pane's rendered grid.
    PaneFrame {
        frame: PaneFrame,
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

    #[test]
    fn frame_reader_reassembles_frames() {
        let msg = ServerMsg::Welcome { protocol: PROTOCOL_VERSION };
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
    fn reload_messages_roundtrip() {
        let req = ClientMsg::ReloadConfig;
        let mut buf = Vec::new();
        write_framed(&mut buf, &req).unwrap();
        let decoded: ClientMsg = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, req);

        let resp = ServerMsg::ConfigReloaded { notice: "config reloaded".into() };
        let mut buf = Vec::new();
        write_framed(&mut buf, &resp).unwrap();
        let decoded: ServerMsg = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn wire_modifiers_bits_roundtrip() {
        let mut m = WireModifiers::none();
        m.set_shift(true).set_control(true).set_alt(true).set_super(true);
        assert!(m.shift() && m.control() && m.alt() && m.super_key());
        m.set_shift(false);
        assert!(!m.shift() && m.control() && m.alt() && m.super_key());
    }

    #[test]
    fn snapshot_messages_roundtrip() {
        let req = ClientMsg::SubscribeSnapshot;
        let mut buf = Vec::new();
        write_framed(&mut buf, &req).unwrap();
        let decoded: ClientMsg = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, req);

        let resp = ServerMsg::Snapshot {
            sessions: vec![SessionInfo {
                name: "session-1".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                panes: vec![PaneInfo {
                    id: 11,
                    title: " opencode ".into(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    is_ai: true,
                    agent: Some(AgentInfo { name: "opencode".into(), status: AgentStatus::Blocked }),
                    rect: PaneRect { x: 0, y: 0, width: 80, height: 24 },
                }],
                zoomed: false,
                active: true,
                focus: Some(11),
            }],
        };
        let mut buf = Vec::new();
        write_framed(&mut buf, &resp).unwrap();
        let decoded: ServerMsg = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn focus_session_and_pane_subscription_roundtrip() {
        let msgs = vec![
            ClientMsg::FocusSession { name: "session-2".into() },
            ClientMsg::FocusPane { session: "session-2".into(), pane_id: 12 },
            ClientMsg::SetSidebar { open: false },
            ClientMsg::SubscribePane { pane_id: 12 },
            ClientMsg::UnsubscribePane { pane_id: 12 },
        ];
        for req in msgs {
            let mut buf = Vec::new();
            write_framed(&mut buf, &req).unwrap();
            let decoded: ClientMsg = read_framed(&mut &buf[..]).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn agent_status_labels() {
        assert_eq!(AgentStatus::Working.label(), "working");
        assert_eq!(AgentStatus::Blocked.label(), "blocked");
        assert_eq!(AgentStatus::Idle.label(), "idle");
    }
}
