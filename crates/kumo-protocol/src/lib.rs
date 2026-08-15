//! Wire protocol between the kumo daemon and its clients (TUI client, GPUI
//! desktop app, mobile, and the control CLI).
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
//! # Architecture: smart renderer / dumb viewport
//!
//! The daemon is the **single source of truth** for everything it has open —
//! sessions, the semantic layout tree (splits in ratios, not pixels), the PTYs,
//! and per-pane terminal content. It does **not** render chrome: no borders,
//! box-drawing characters, sidebar, or status bar ever enter the wire. Clients
//! receive two things and draw everything themselves:
//!
//! - **Layout** ([`DaemonEvent::Layout`]): the semantic tree of sessions →
//!   splits (with ratios) → panes (title, cwd, agent status). Clients compute
//!   actual geometry and draw their own chrome.
//! - **Pane content** ([`DaemonEvent::PaneFrame`]): each pane's terminal grid
//!   (rendered by the daemon's Ghostty core), streamed on change.
//!
//! Everything else is a **command** ([`Command`]) — sessions, panes, agents,
//! input, subscriptions — so the whole multiplexer can be driven from the CLI,
//! the TUI, the desktop app, or a script.

use std::io::{self, Read};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "crossterm")]
mod crossterm;

/// Protocol version. Bump on breaking wire changes; the daemon rejects clients
/// with a mismatched version. v4 switches from rendered frames to the
/// semantic-layout + per-pane-content model, and from `ClientMsg`/`ServerMsg`
/// to the tmux-style `Command`/`DaemonEvent` protocol.
pub const PROTOCOL_VERSION: u32 = 4;
/// Upper bound for a single frame payload (a full 80x24 grid fits comfortably).
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// What kind of client is attaching. The daemon routes delivery channels based
/// on it (e.g. interactive viewers vs. one-shot CLI commands).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClientKind {
    /// The `kumo` TUI client in a host terminal.
    Terminal,
    /// A native desktop app.
    Desktop,
    /// A small-screen / read-only client (mobile).
    Mobile,
    /// The control CLI (`kumo session ...`): sends one command, reads replies.
    Cli,
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
// Render frame (per-pane content)
// ---------------------------------------------------------------------------

/// A serialized rendered cell (the grid the daemon draws for ONE pane).
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

/// One pane's terminal grid, streamed on change. `full` = every row included
/// (first frame or after a resize); otherwise only dirty rows.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PaneFrame {
    pub pane_id: u64,
    pub cols: u16,
    pub rows: u16,
    pub full: bool,
    pub rows_dirty: Vec<RowPatch>,
    /// Terminal cursor position, if the pane shows one.
    pub cursor: Option<(u16, u16)>,
}

/// The daemon's whole composed UI (panes + borders + chrome), rendered
/// daemon-side and streamed to full-attach TUI clients. `full` = every row
/// (first frame / resize); otherwise only dirty rows.
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
// Semantic layout tree
// ---------------------------------------------------------------------------

/// Split orientation. `V` = side-by-side columns, `H` = stacked rows.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDir {
    Vertical,
    Horizontal,
}

/// Direction of a keyboard pane-resize / focus move.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResizeDir {
    Left,
    Down,
    Up,
    Right,
}

/// One pane as it appears in the semantic tree.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct LayoutPane {
    pub id: u64,
    /// Display title (custom name or a default `shell N` / `AI CLI` label).
    pub title: String,
    /// Working directory the pane is currently in (follow-workspace).
    pub cwd: std::path::PathBuf,
    /// Whether the pane is (or hosts) an AI CLI.
    pub is_ai: bool,
    /// The running AI CLI, when this pane hosts one.
    pub agent: Option<AgentInfo>,
}

/// A node in the semantic layout tree: a split of two subtrees with a ratio
/// (0..1, the share of `a`), or a single pane. Clients compute pixel/cell
/// geometry from these proportions; the daemon never ships coordinates.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum LayoutNode {
    Pane(LayoutPane),
    Split {
        dir: SplitDir,
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

/// One session's semantic tree, as pushed to layout subscribers.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SessionLayout {
    pub name: String,
    pub workspace: std::path::PathBuf,
    /// The focused pane in this session.
    pub focus: u64,
    /// When zoomed, only the focused pane is shown full-size.
    pub zoom: bool,
    pub root: Option<Box<LayoutNode>>,
}

/// The full layout snapshot pushed on change.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Layout {
    /// Name of the active session, if any.
    pub active: Option<String>,
    pub sessions: Vec<SessionLayout>,
}

/// One AI CLI running inside a pane.
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

/// One session, as reported to `kumo session list` (metadata only; the full
/// semantic tree travels via [`DaemonEvent::Layout`]).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SessionInfo {
    pub name: String,
    pub workspace: std::path::PathBuf,
    pub pane_count: usize,
    pub zoomed: bool,
    pub active: bool,
    pub focus: Option<u64>,
    /// AI CLIs running inside this session.
    pub agents: Vec<AgentInfo>,
}

/// One agent status line, as reported to `kumo agent status`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct AgentStatusLine {
    pub session: String,
    pub pane_id: u64,
    pub name: String,
    pub status: AgentStatus,
}

// ---------------------------------------------------------------------------
// Commands (client -> daemon)
// ---------------------------------------------------------------------------

/// A tmux-style command sent by any client (CLI, TUI, desktop app, script).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Command {
    // -- attach / lifecycle ------------------------------------------------
    /// Attach as an interactive viewer of the given size. The daemon responds
    /// with `Welcome`, then pushes `Layout` and `PaneFrame` streams.
    Attach {
        protocol: u32,
        kind: ClientKind,
        cols: u16,
        rows: u16,
    },
    /// Detach (a viewer leaves; the daemon keeps running).
    Detach,
    /// Stop the daemon (killing its panes).
    KillServer,
    /// `kumo update`: restart the daemon in place, inheriting live PTYs.
    Restart,
    /// Re-read the config and apply it live.
    ReloadConfig,

    // -- sessions -----------------------------------------------------------
    /// `kumo session list`: reply with `SessionList`.
    SessionList,
    /// `kumo session new [DIR] [--name NAME]`: create and focus a session.
    SessionNew {
        name: Option<String>,
        workspace: Option<std::path::PathBuf>,
    },
    /// `kumo session kill NAME`: close a session (and its panes).
    SessionKill {
        name: String,
    },
    /// Focus the named session (interactive switch).
    SessionFocus {
        name: String,
    },

    // -- panes ---------------------------------------------------------------
    /// Split a pane (default: the focused one in `session`).
    PaneSplit {
        session: String,
        dir: SplitDir,
        is_ai: bool,
    },
    /// Close a pane (default: the focused one in `session`).
    PaneClose {
        session: String,
        pane_id: Option<u64>,
    },
    /// Focus a specific pane in a session.
    PaneFocus {
        session: String,
        pane_id: u64,
    },
    /// Nudge the ratio of the split separating the focused pane from its
    /// neighbor in `dir`.
    PaneResizeRatio {
        session: String,
        dir: ResizeDir,
    },
    /// Swap the focused pane with its sibling.
    PaneSwap {
        session: String,
    },
    /// Mirror the layout (rotate the split tree).
    LayoutRotate {
        session: String,
    },
    /// Toggle a session's zoom (only the focused pane shown full-size).
    SessionZoom {
        session: String,
    },
    /// Send key events to a pane (default: the focused one in `session`).
    PaneSendKeys {
        session: String,
        pane_id: Option<u64>,
        keys: Vec<WireKeyEvent>,
    },
    /// Resize a pane's terminal to `cols` x `rows` (clients drive geometry
    /// from the semantic tree).
    PaneResize {
        pane_id: u64,
        cols: u16,
        rows: u16,
    },
    /// Set the daemon's composed grid size (full-attach TUI clients; the
    /// daemon lays panes out within it and streams composed frames).
    Resize {
        cols: u16,
        rows: u16,
    },

    // -- agents --------------------------------------------------------------
    /// Spawn an AI CLI in a new pane (default program = the configured AI cmd).
    AgentSpawn {
        session: String,
        program: Option<String>,
    },
    /// `kumo agent status`: reply with every running agent.
    AgentStatus,
    /// Kill the AI CLI running in a pane (closes the pane).
    AgentKill {
        session: String,
        pane_id: u64,
    },

    // -- interactive input (attached viewers) -------------------------------
    /// A key pressed in the focused pane's terminal.
    Input {
        key: WireKeyEvent,
    },
    /// Text pasted (bracketed paste) into the focused pane.
    Paste {
        text: String,
    },
    /// A mouse event in the focused pane's terminal.
    Mouse {
        event: WireMouseEvent,
    },

    // -- subscriptions --------------------------------------------------------
    /// Start receiving `DaemonEvent::Layout` pushes (viewers).
    SubscribeLayout,
    /// Start receiving `DaemonEvent::PaneFrame` for one pane.
    SubscribePane {
        pane_id: u64,
    },
    /// Stop receiving `DaemonEvent::PaneFrame` for one pane.
    UnsubscribePane {
        pane_id: u64,
    },
}

// ---------------------------------------------------------------------------
// Daemon events (daemon -> client)
// ---------------------------------------------------------------------------

/// Everything the daemon pushes to a client: command replies and streams.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum DaemonEvent {
    /// Reply to `Command::Attach`.
    Welcome {
        protocol: u32,
    },
    /// `leader+d` / detach: this viewer should disconnect; the daemon lives on.
    Detach,
    /// The daemon is stopping (last session closed / `kill`).
    Shutdown,
    /// The daemon is restarting itself (`kumo update`); reconnect with retries.
    Restarting,
    /// Reply to `ReloadConfig`.
    ConfigReloaded {
        notice: String,
    },
    /// Generic reply to a one-shot command (CLI): human-readable outcome.
    Reply {
        message: String,
    },
    /// Reply to `SessionList`.
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    /// Reply to `AgentStatus`.
    AgentStatus {
        agents: Vec<AgentStatusLine>,
    },
    /// Pushed to `SubscribeLayout` subscribers whenever sessions/panes/agents
    /// change: the full semantic tree.
    Layout {
        layout: Layout,
    },
    /// A subscribed pane's terminal grid.
    PaneFrame {
        frame: PaneFrame,
    },
    /// The daemon's composed UI (borders + chrome included), streamed to
    /// full-attach TUI clients.
    Composed {
        frame: FrameMsg,
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
        let msg = DaemonEvent::Welcome { protocol: PROTOCOL_VERSION };
        let bytes = encode(&msg).unwrap();
        let mut reader = FrameReader::default();
        let mut out = Vec::new();
        // Split across arbitrary chunk boundaries.
        for chunk in bytes.chunks(3) {
            reader.push(chunk, &mut out);
        }
        assert_eq!(out.len(), 1);
        let decoded: DaemonEvent = bincode::serde::decode_from_slice(&out[0], bincode::config::standard())
            .unwrap()
            .0;
        assert_eq!(decoded, msg);
    }

    #[test]
    fn read_write_framed_roundtrip() {
        let msg = DaemonEvent::Shutdown;
        let mut buf = Vec::new();
        write_framed(&mut buf, &msg).unwrap();
        let decoded: DaemonEvent = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, msg);
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
    fn layout_tree_roundtrip() {
        let layout = Layout {
            active: Some("session-1".into()),
            sessions: vec![SessionLayout {
                name: "session-1".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                focus: 11,
                zoom: false,
                root: Some(Box::new(LayoutNode::Split {
                    dir: SplitDir::Vertical,
                    ratio: 0.7,
                    a: Box::new(LayoutNode::Pane(LayoutPane {
                        id: 11,
                        title: " shell ".into(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        is_ai: false,
                        agent: None,
                    })),
                    b: Box::new(LayoutNode::Pane(LayoutPane {
                        id: 12,
                        title: " opencode ".into(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        is_ai: true,
                        agent: Some(AgentInfo { name: "opencode".into(), status: AgentStatus::Blocked }),
                    })),
                })),
            }],
        };
        let mut buf = Vec::new();
        write_framed(&mut buf, &DaemonEvent::Layout { layout: layout.clone() }).unwrap();
        let decoded: DaemonEvent = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, DaemonEvent::Layout { layout });
    }

    #[test]
    fn commands_roundtrip() {
        let cmds = vec![
            Command::SessionList,
            Command::SessionNew { name: Some("session-2".into()), workspace: None },
            Command::PaneSplit { session: "session-1".into(), dir: SplitDir::Vertical, is_ai: false },
            Command::PaneSendKeys {
                session: "session-1".into(),
                pane_id: None,
                keys: vec![WireKeyEvent::new(WireKeyCode::Char('a'), WireModifiers::none())],
            },
            Command::AgentSpawn { session: "session-1".into(), program: Some("opencode".into()) },
            Command::AgentStatus,
        ];
        for cmd in cmds {
            let mut buf = Vec::new();
            write_framed(&mut buf, &cmd).unwrap();
            let decoded: Command = read_framed(&mut &buf[..]).unwrap();
            assert_eq!(decoded, cmd);
        }
    }

    #[test]
    fn agent_status_labels() {
        assert_eq!(AgentStatus::Working.label(), "working");
        assert_eq!(AgentStatus::Blocked.label(), "blocked");
        assert_eq!(AgentStatus::Idle.label(), "idle");
    }
}
