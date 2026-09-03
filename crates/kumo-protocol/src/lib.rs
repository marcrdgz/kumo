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
//! crate (`src/frames.rs`).
//!
//! # Architecture: smart renderer / dumb viewport
//!
//! The daemon is the **single source of truth** for everything it has open —
//! sessions, the semantic layout tree (splits in ratios, not pixels), the PTYs,
//! and per-pane terminal content. It never renders chrome. Clients receive two
//! streams and draw all their own chrome (borders, sidebar, status bar, menus,
//! popups):
//!
//! - **Layout** ([`DaemonEvent::Layout`]): the semantic tree of sessions →
//!   splits (with ratios) → panes (title, cwd, agent status, terminal flags).
//!   Clients compute actual geometry and draw their own chrome.
//! - **Pane content** ([`DaemonEvent::PaneFrame`]): each pane's terminal grid
//!   (rendered by the daemon's Ghostty core) with per-row link ranges and the
//!   scrollback state, streamed on change.
//!
//! Everything else is a **command** ([`Command`]) — sessions, panes, agents,
//! input, subscriptions, chrome actions (rename, worktrees, theme) — so the
//! whole multiplexer can be driven from the CLI, the TUI, the desktop app, or a
//! script.

use std::io::{self, Read};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "crossterm")]
mod crossterm;

/// Protocol version. Bump on breaking wire changes; the daemon rejects clients
/// with a mismatched version. v5 removes the composed-grid channel entirely:
/// every client draws its own chrome from the semantic layout + per-pane
/// content, and the chrome actions (rename, worktrees, theme) travel as
/// commands. v6 introduces tabs: sessions → tabs → panes. v7 adds the custom
/// theme payload to `DaemonEvent::Theme`. v8 adds copy-mode (scroll + search).
/// v9 adds `DaemonEvent::Toast`: agent lifecycle transitions pushed as
/// transient corner toasts to attached viewers. v10 extends `AgentStatus`
/// with `done` (finished-but-unseen) and `unknown` (classification failed).
/// v11 adds agent orchestration primitives: `AgentWait`, `AgentPrompt`,
/// `AgentRead`, `AgentStart`, `AgentRename`, `PaneWaitOutput`, `AgentBroadcast`
/// and their result events. v12 adds declarative layout: `LayoutExport/Apply`
/// for workspaces and checkpoints.
pub const PROTOCOL_VERSION: u32 = 12;
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
/// (handles wide chars, styles, and cells that were cleared). `links` spans the
/// hyperlinks / plain-text URLs covering this row, so clients can underline
/// them while a link modifier is held and open them on click.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct RowPatch {
    pub row: u16,
    pub cells: Vec<WireCell>,
    /// Links covering this row (empty when none). Columns are row-relative.
    pub links: Vec<LinkRange>,
}

/// One clickable link spanning a row: an OSC 8 hyperlink URI or a plain-text
/// `scheme://` URL detected on the row.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct LinkRange {
    /// First column of the link.
    pub start: u16,
    /// One past the last column of the link.
    pub end: u16,
    /// The URL.
    pub url: String,
}

/// Scrollback state of a pane's terminal, for client-drawn scrollbars.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrollState {
    /// Rows scrolled above the viewport.
    pub offset: u16,
    /// Total rows in the scrollback buffer (viewport included).
    pub total: u16,
    /// Rows visible in the viewport.
    pub screen: u16,
}

/// One search hit in copy-mode: a single-row substring in screen coordinates.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct CopyHit {
    /// Screen row (0 = top of scrollback).
    pub row: u32,
    /// First column of the hit (inclusive).
    pub start_col: u16,
    /// One past the last column of the hit (exclusive).
    pub end_col: u16,
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
    /// Scrollback state of the pane's terminal, when it has scrollback.
    pub scroll: Option<ScrollState>,
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
///
/// Not `Eq` (contains [`AgentInfo`], whose float metrics are sampled).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
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
    /// Whether the pane's child has enabled mouse reporting (SGR mouse): the
    /// client forwards the whole mouse gesture to it instead of selecting text.
    pub mouse_reporting: bool,
    /// Whether the pane is on the terminal's alternate screen.
    pub alt_screen: bool,
}

/// A node in the semantic layout tree: a split of two subtrees with a ratio
/// (0..1, the share of `a`), or a single pane. Clients compute pixel/cell
/// geometry from these proportions; the daemon never ships coordinates.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum LayoutNode {
    Pane(LayoutPane),
    Split {
        /// Stable split id (matches the daemon's layout tree), so clients can
        /// target a specific split for absolute ratio drags.
        id: u64,
        dir: SplitDir,
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

/// One tab (window) inside a session: its own pane tree and focus/zoom.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TabLayout {
    pub id: u64,
    pub name: String,
    /// The focused pane in this tab.
    pub focus: u64,
    /// When zoomed, only the focused pane is shown full-size.
    pub zoom: bool,
    pub root: Option<Box<LayoutNode>>,
}

/// One session's semantic tree, as pushed to layout subscribers.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SessionLayout {
    pub name: String,
    pub workspace: std::path::PathBuf,
    /// Index of the active tab in `tabs`.
    pub active_tab: usize,
    pub tabs: Vec<TabLayout>,
    /// Git branch of the session's workspace (name + ahead/behind), for the
    /// client's sidebar.
    pub branch: Option<WireBranch>,
    // Deprecated mirrors of the active tab — kept for desktop compat and smooth upgrade.
    #[serde(default)]
    pub focus: u64,
    #[serde(default)]
    pub zoom: bool,
    #[serde(default)]
    pub root: Option<Box<LayoutNode>>,
}

/// Git branch state of a session's workspace, as shown in the sidebar.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WireBranch {
    pub name: String,
    /// Commits ahead of the upstream (shown as `↑N`).
    pub ahead: u32,
    /// Commits behind the upstream (shown as `~N`).
    pub behind: u32,
}

/// One row of the worktree picker: a git worktree plus kumo-side flags.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WireWorktree {
    pub path: std::path::PathBuf,
    /// Checked-out branch; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// True when this is the repository's main worktree.
    pub is_main: bool,
    /// True when a kumo session is already open in this worktree.
    pub open: bool,
}

/// A startup update notice, keyed so the client can dismiss it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WireNotice {
    /// Stable key used for dismissal (`UpdateDismiss`).
    pub key: String,
    /// Human display string, e.g. `nightly (Aug 16)`.
    pub display: String,
}

/// Full theme payload sent when the custom theme is active. Colors are raw
/// RGB triples `0xRRGGBB` split as `[r,g,b]` so the wire stays independent of
/// `ratatui`/`kumo-core`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WireTheme {
    pub name: String,
    pub palette: [[u8; 3]; 16],
    pub term_fg: [u8; 3],
    pub term_bg: [u8; 3],
    pub term_cursor: [u8; 3],
    pub fg: [u8; 3],
    pub accent: [u8; 3],
    pub secondary: [u8; 3],
    pub panel_sep: [u8; 3],
    pub panel_muted: [u8; 3],
    pub border_idle: [u8; 3],
    pub green: [u8; 3],
    pub orange: [u8; 3],
    pub red: [u8; 3],
    pub input_bg: [u8; 3],
}

/// The full layout snapshot pushed on change.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Layout {
    /// Name of the active session, if any.
    pub active: Option<String>,
    pub sessions: Vec<SessionLayout>,
}

/// One AI CLI running inside a pane.
///
/// Not `Eq`: `cpu` is a float (sampled daemon-side), so the sidebar's
/// micro-pill metrics can render live values.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct AgentInfo {
    /// Short AI CLI name, e.g. "opencode".
    pub name: String,
    /// Lifecycle status inferred from the pane's terminal buffer.
    pub status: AgentStatus,
    /// Sampled CPU usage of the agent's process tree, as a percentage of one
    /// core (0.0 when the daemon could not sample it). `#[serde(default)]`
    /// keeps older daemons wire-compatible with this client.
    #[serde(default)]
    pub cpu: f32,
    /// Resident memory of the agent's process tree, in kibibytes.
    #[serde(default)]
    pub mem_kb: u64,
    /// The kumo pane id this agent runs in.
    #[serde(default)]
    pub pane_id: u64,
    /// Position of the agent's pane inside its tab (1-based, layout order).
    /// `#[serde(default)]` keeps older daemons wire-compatible.
    #[serde(default)]
    pub pane_index: u64,
    /// Position of its tab inside the session (1-based). Folded into the
    /// composite `s1:t2:p3` addressing the CLI prints for humans.
    #[serde(default)]
    pub tab_index: u64,
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
    /// Finished-but-unseen: went idle while its pane was not focused; focus
    /// marks it seen (back to Idle).
    Done,
    /// A recognized agent whose classification failed.
    Unknown,
}

/// The transition behind a [`DaemonEvent::Toast`]: which agent lifecycle
/// change the client should announce as a corner toast.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    /// A working agent became blocked (waiting for an approval).
    Blocked,
    /// A working agent finished its task (fell back to idle).
    Finished,
}

impl AgentStatus {
    /// Lowercase display label.
    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Idle => "idle",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
        }
    }
}

/// The `until` predicate for `Command::AgentWait` / `AgentPrompt --wait`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentWaitKind {
    Blocked,
    Done,
    Idle,
    Working,
}

impl AgentWaitKind {
    pub fn label(self) -> &'static str {
        match self {
            AgentWaitKind::Blocked => "blocked",
            AgentWaitKind::Done => "done",
            AgentWaitKind::Idle => "idle",
            AgentWaitKind::Working => "working",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "blocked" => Some(AgentWaitKind::Blocked),
            "done" => Some(AgentWaitKind::Done),
            "idle" => Some(AgentWaitKind::Idle),
            "working" => Some(AgentWaitKind::Working),
            _ => None,
        }
    }

    pub fn matches(self, status: AgentStatus) -> bool {
        match self {
            AgentWaitKind::Blocked => status == AgentStatus::Blocked,
            AgentWaitKind::Done => status == AgentStatus::Done,
            AgentWaitKind::Idle => status == AgentStatus::Idle,
            AgentWaitKind::Working => status == AgentStatus::Working,
        }
    }
}

/// Source selector for `Command::AgentRead`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentReadSource {
    Visible,
    Recent,
    Detection,
    Traceback,
}

/// Declarative pane spec for `layout export/apply` (per-pane cwd/env/cmd).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PaneSpec {
    /// Working directory for the pane (relative or absolute). `None` = session workspace.
    #[serde(default)]
    pub cwd: Option<std::path::PathBuf>,
    /// Command to run in the pane (program + args as a shell string). `None` = shell.
    #[serde(default)]
    pub command: Option<String>,
    /// Whether this pane is an AI CLI.
    #[serde(default)]
    pub is_ai: bool,
    /// Optional title override.
    #[serde(default)]
    pub title: Option<String>,
}

/// Declarative layout node for export/apply (mirrors `LayoutNode` without live ids).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum LayoutNodeSpec {
    Pane(PaneSpec),
    Split {
        dir: SplitDir,
        ratio: f32,
        a: Box<LayoutNodeSpec>,
        b: Box<LayoutNodeSpec>,
    },
}

/// One tab in a declarative layout spec.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TabSpec {
    pub name: String,
    #[serde(default)]
    pub root: Option<Box<LayoutNodeSpec>>,
    #[serde(default)]
    pub zoom: bool,
}

/// Declarative layout spec (sessions → tabs → pane tree with per-pane cwd/env).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct LayoutSpec {
    pub tabs: Vec<TabSpec>,
}

impl AgentReadSource {
    pub fn label(self) -> &'static str {
        match self {
            AgentReadSource::Visible => "visible",
            AgentReadSource::Recent => "recent",
            AgentReadSource::Detection => "detection",
            AgentReadSource::Traceback => "traceback",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "visible" => Some(AgentReadSource::Visible),
            "recent" => Some(AgentReadSource::Recent),
            "detection" => Some(AgentReadSource::Detection),
            "traceback" => Some(AgentReadSource::Traceback),
            _ => None,
        }
    }
}

/// One pane inside a tab, for the `kumo pane list` / `kumo tab list` views.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PaneInfo {
    pub id: u64,
    /// Display label: custom name, then the AI CLI name, then `shell N`.
    pub label: String,
    /// Whether this pane is the tab's focused pane.
    pub active: bool,
}

/// Minimal info about one tab, for `kumo tab list`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TabInfo {
    pub id: u64,
    pub name: String,
    pub pane_count: usize,
    pub zoomed: bool,
    pub active: bool,
    pub focus: Option<u64>,
    /// Panes in this tab, in layout order. `#[serde(default)]` keeps older
    /// daemons wire-compatible with this client.
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
}

/// One session, as reported to `kumo session list` (metadata only; the full
/// semantic tree travels via [`DaemonEvent::Layout`]).
///
/// Not `Eq` (contains [`AgentInfo`]).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SessionInfo {
    pub name: String,
    pub workspace: std::path::PathBuf,
    pub tab_count: usize,
    pub pane_count: usize,
    pub zoomed: bool,
    pub active: bool,
    pub active_tab: Option<String>,
    pub focus: Option<u64>,
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
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
    /// Position of the agent's pane inside its tab (1-based, layout order).
    #[serde(default)]
    pub pane_index: u64,
    /// Position of its tab inside the session (1-based).
    #[serde(default)]
    pub tab_index: u64,
}

/// The evidence region a marker was found in, for `kumo agent explain`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum EvidenceRegion {
    /// The recent screen-buffer tail (200 rows pinned to the buffer bottom).
    Screen,
    /// Text below the last horizontal rule (Claude's live prompt/forms area).
    Form,
    /// Bottom rows pinned to opencode's prompt footer.
    Footer,
    /// The OSC window title.
    Title,
}

impl EvidenceRegion {
    /// Lowercase display label.
    pub fn label(self) -> &'static str {
        match self {
            EvidenceRegion::Screen => "screen",
            EvidenceRegion::Form => "form",
            EvidenceRegion::Footer => "footer",
            EvidenceRegion::Title => "title",
        }
    }
}

/// One matched detection marker, for `kumo agent explain`: which agent, which
/// signal kind, the marker text/glyph, and where it was found.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct AgentMarkerMatch {
    /// Agent that matched, e.g. "opencode".
    pub agent: String,
    /// Signal kind: "blocked", "working", or "idle".
    pub kind: String,
    /// Marker phrase or glyph description, e.g. "permission required" or
    /// "braille spinner".
    pub marker: String,
    /// Region the marker was found in.
    pub region: EvidenceRegion,
}

/// Why a pane reads its reported status — for `kumo agent explain`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentIdleReason {
    /// Explicit idle markers matched (listed in the report).
    IdleMarkers,
    /// Recognized agent, no signal matched: classification fell back to
    /// Unknown (the opposite of an idle proof).
    UnknownFallback,
    /// The pane's process is dead; status is forced to Idle.
    DeadPane,
    /// Not a scanned AI CLI: the default (Idle) applies.
    NotAnAgent,
    /// Raw Idle with a Working/Done predecessor while unfocused: Done
    /// (finished-but-unseen).
    UnseenFinish,
    /// Prior Done, pane focused again: marked seen, back to Idle.
    SeenAfterFocus,
    /// A blocked/working signal matched (status is not Idle).
    Active,
}

impl AgentIdleReason {
    /// Lowercase display label.
    pub fn label(self) -> &'static str {
        match self {
            AgentIdleReason::IdleMarkers => "idle markers",
            AgentIdleReason::UnknownFallback => "no-signal fallback (unknown)",
            AgentIdleReason::DeadPane => "dead pane",
            AgentIdleReason::NotAnAgent => "not an AI CLI (default)",
            AgentIdleReason::UnseenFinish => "unseen finish (done)",
            AgentIdleReason::SeenAfterFocus => "done then focused (seen)",
            AgentIdleReason::Active => "active signal",
        }
    }
}

/// Diagnostic report, as replied to `Command::AgentExplain`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct AgentExplainReport {
    pub pane_id: u64,
    pub session: String,
    /// Pane label: custom name, AI CLI name, or `shell`.
    pub cli: String,
    /// OS pid of the pane's PTY child (0 when unknown).
    pub os_pid: u64,
    pub is_ai_cli: bool,
    pub dead: bool,
    /// Whether the pane is currently the session's focused pane.
    pub focused: bool,
    /// Raw detection from the terminal snapshot.
    pub raw_status: AgentStatus,
    /// Status after the daemon's seen/done rule (what the UI displays).
    pub status: AgentStatus,
    /// Previous observed status, when known.
    #[serde(default)]
    pub prev_status: Option<AgentStatus>,
    /// The idle-fallback / verdict reason chain.
    pub idle_reason: AgentIdleReason,
    /// Every marker that matched, per agent and signal kind.
    pub markers: Vec<AgentMarkerMatch>,
    /// Human-readable precedence summary.
    pub precedence: String,
    /// Milliseconds since the pane last produced output.
    pub last_output_age_ms: u64,
    /// Sampled CPU% of the agent process tree.
    pub cpu: f32,
    /// Resident memory of the agent process tree, in KiB.
    pub mem_kb: u64,
    /// Position of the pane inside its tab (1-based, layout order).
    #[serde(default)]
    pub pane_index: u64,
    /// Position of its tab inside the session (1-based).
    #[serde(default)]
    pub tab_index: u64,
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

    // -- tabs ---------------------------------------------------------------
    /// Create a new tab in `session`.
    TabNew {
        session: String,
        name: Option<String>,
        workspace: Option<std::path::PathBuf>,
    },
    /// Close a tab in `session` (default: active tab).
    TabClose {
        session: String,
        tab: Option<String>,
    },
    /// Focus a tab in `session` (by id, name or 1-based index).
    TabFocus {
        session: String,
        tab: String,
    },
    /// Rename a tab.
    TabRename {
        session: String,
        tab: String,
        new_name: String,
    },

    // -- panes ---------------------------------------------------------------
    /// Split a pane (default: the focused one in `session`'s active tab).
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
    /// Set the ratio of a specific split to an absolute value (0..1). Used by
    /// desktop drags where the client knows exactly where the divider lands.
    PaneResizeTo {
        session: String,
        split_id: u64,
        ratio: f32,
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

    // -- chrome actions (clients draw all chrome) ----------------------------
    /// Rename a pane (custom title; the name popup's commit).
    PaneRename {
        session: String,
        pane_id: u64,
        name: String,
    },
    /// Rename a session.
    SessionRename {
        session: String,
        new_name: String,
    },
    /// List the git worktrees of a session's repository (reply: `Worktrees`).
    WorktreeList {
        session: String,
    },
    /// Create a git worktree (new branch from the repo HEAD) and open a fresh
    /// session inside it.
    WorktreeCreate {
        session: String,
        branch: String,
    },
    /// Open the session already working in `path` (or create one) — the
    /// worktree picker's confirm.
    WorktreeOpen {
        session: String,
        path: std::path::PathBuf,
    },
    /// Apply theme `idx` daemon-side (the ANSI palette re-colors every pane)
    /// and push the new `Theme` event to clients.
    SetTheme {
        idx: usize,
    },
    /// Open the config file in a new editor pane of the named session (MENU
    /// `config`): uses `$VISUAL`/`$EDITOR`/`vi` inside a vertical split.
    OpenConfig {
        session: String,
    },
    /// Write raw bytes into a specific pane (mouse-reporting forwarding, where
    /// the client knows exactly which pane the pointer is over).
    PaneWrite {
        pane_id: u64,
        bytes: Vec<u8>,
    },
    /// Scroll a specific pane's viewport by one wheel step.
    PaneScroll {
        pane_id: u64,
        up: bool,
    },
    /// Scroll a pane's viewport by an arbitrary delta (copy-mode; positive = down).
    CopyScroll {
        pane_id: u64,
        delta: i32,
    },
    /// Scroll a pane's viewport so that `row` (screen coordinate, 0 = top of
    /// scrollback) becomes the first visible row (copy-mode).
    CopyScrollTo {
        pane_id: u64,
        row: u32,
    },
    /// Search the pane's screen+scrollback for `query` (plain substring,
    /// case-insensitive). Reply: `CopySearchResults`.
    CopySearch {
        pane_id: u64,
        query: String,
    },
    /// Install a viewport selection (copy-mode). The daemon highlights it and
    /// it survives re-renders until cleared.
    CopySetSelection {
        pane_id: u64,
        start: (u16, u16),
        end: (u16, u16),
    },
    /// Clear the pane's active selection (copy-mode leave / selection reset).
    CopyClearSelection {
        pane_id: u64,
    },
    /// Query the startup update notice (reply: `UpdateNotice`).
    UpdateStatus,
    /// Dismiss the startup update banner (persisted daemon-side).
    UpdateDismiss {
        key: String,
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
    /// `kumo agent explain`: reply with a diagnostic report of why the pane
    /// reads the state it does (matched markers, evidence region, idle reason).
    AgentExplain {
        session: String,
        pane_id: u64,
    },
    /// `kumo agent wait <pane> --until blocked|done|idle`: server-owned,
    /// event-driven wait; pinned to the pane occupant so a process replacement
    /// cannot satisfy the wait.
    AgentWait {
        session: String,
        pane_id: u64,
        until: AgentWaitKind,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// `kumo agent prompt <pane> <text>` (`--wait` / `--timeout`): bracketed-
    /// paste aware submit; `--wait` races submit+wait into one server-owned
    /// request.
    AgentPrompt {
        session: String,
        pane_id: u64,
        text: String,
        #[serde(default)]
        wait: Option<AgentWaitKind>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// `kumo agent read <pane> --source visible|recent|detection|traceback`.
    AgentRead {
        session: String,
        pane_id: u64,
        source: AgentReadSource,
    },
    /// `kumo agent start --kind <agent> --pane <id> [-- <args>]`: launches an
    /// agent in an existing shell pane and returns once detection shows it ready.
    AgentStart {
        session: String,
        pane_id: u64,
        kind: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// `kumo agent rename <pane> <name>`: live alias so scripts reference agents
    /// by name, not pane id.
    AgentRename {
        session: String,
        pane_id: u64,
        name: String,
    },
    /// `kumo pane wait-output <pane> --regex <pattern>`: one-shot output waiter.
    PaneWaitOutput {
        session: String,
        pane_id: u64,
        pattern: String,
        #[serde(default)]
        is_regex: bool,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// `kumo agent broadcast <text> [--filter status]`: fan one prompt out to
    /// every AI pane in the session/tab over the existing send-keys path.
    AgentBroadcast {
        session: String,
        text: String,
        #[serde(default)]
        filter: Option<AgentStatus>,
    },

    /// `kumo layout export`: export a session/tab layout as a declarative spec.
    LayoutExport {
        session: String,
        #[serde(default)]
        tab: Option<String>,
    },
    /// `kumo layout apply`: apply a declarative layout spec to a session.
    LayoutApply {
        session: String,
        spec: LayoutSpec,
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
    /// Reply to `AgentExplain` (diagnostic report).
    AgentExplain {
        report: AgentExplainReport,
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
    /// The active theme index (chrome colors the client renders with). Pushed
    /// on attach and whenever `SetTheme` applies a new theme. When the custom
    /// theme (defined in `~/.config/kumo/config.toml` as `[theme.custom]`) is
    /// active, `custom` carries its full palette so even old clients that only
    /// know `idx` can be upgraded gracefully; missing on the wire decodes as
    /// `None` via `#[serde(default)]`.
    Theme {
        idx: usize,
        #[serde(default)]
        custom: Option<WireTheme>,
    },
    /// Reply to `WorktreeList`.
    Worktrees {
        items: Vec<WireWorktree>,
    },
    /// Reply to `UpdateStatus`: the startup update notice, if one is active.
    UpdateNotice {
        notice: Option<WireNotice>,
    },
    /// Reply to `CopySearch`: hits for `query` in `pane_id` (screen coords).
    CopySearchResults {
        pane_id: u64,
        query: String,
        hits: Vec<CopyHit>,
    },
    /// An agent lifecycle transition raised as a transient corner toast in
    /// every attached viewer. The desktop notification (macOS / notify-send)
    /// fires only when no viewer is attached to see the toast.
    Toast {
        /// The agent pane that raised the toast, for click-to-focus.
        pane_id: u64,
        kind: ToastKind,
        /// Short headline, e.g. `claude is blocked`.
        title: String,
        /// Location line (the owning session's workspace path); may be empty.
        body: String,
    },
    /// Reply to `AgentWait` / `AgentPrompt --wait`: the awaited status was reached.
    AgentWaitResult {
        pane_id: u64,
        status: AgentStatus,
    },
    /// Reply to `AgentRead`: the pane's buffer slice for the requested source.
    AgentReadResult {
        pane_id: u64,
        source: AgentReadSource,
        text: String,
        truncated: bool,
    },
    /// Reply to `PaneWaitOutput`: the output waiter matched.
    PaneWaitResult {
        pane_id: u64,
        matched: String,
    },
    /// Typed error for orchestration primitives (also surfaced as `Reply` with
    /// `error: <code>: <message>` for human CLI use).
    Error {
        code: String,
        message: String,
    },
    /// Reply to `LayoutExport`: the declarative layout spec.
    LayoutExport {
        spec: LayoutSpec,
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
    ///
    /// Returns `true` if the stream contained an oversized frame header
    /// (`len > MAX_FRAME_LEN`). This is unrecoverable framing corruption —
    /// the caller must drop the connection. The internal buffer is cleared
    /// so the `FrameReader` can be reused, but no further frames from this
    /// stream should be processed.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<Vec<u8>>) -> bool {
        self.buf.extend_from_slice(data);
        loop {
            if self.buf.len() < 4 {
                return false;
            }
            let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
            if len > MAX_FRAME_LEN {
                // Unrecoverable framing corruption; drop the connection.
                self.buf.clear();
                return true;
            }
            if self.buf.len() < 4 + len {
                return false;
            }
            out.push(self.buf[4..4 + len].to_vec());
            self.buf.drain(..4 + len);
        }
    }

    /// Whether the internal buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
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
    fn toast_roundtrip() {
        let msg = DaemonEvent::Toast {
            pane_id: 42,
            kind: ToastKind::Blocked,
            title: "claude is blocked".into(),
            body: "~/dev/kumo".into(),
        };
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
        let root = Some(Box::new(LayoutNode::Split {
            id: 7,
            dir: SplitDir::Vertical,
            ratio: 0.7,
            a: Box::new(LayoutNode::Pane(LayoutPane {
                id: 11,
                title: " shell ".into(),
                cwd: std::path::PathBuf::from("/tmp"),
                is_ai: false,
                agent: None,
                mouse_reporting: false,
                alt_screen: false,
            })),
            b: Box::new(LayoutNode::Pane(LayoutPane {
                id: 12,
                title: " opencode ".into(),
                cwd: std::path::PathBuf::from("/tmp"),
                is_ai: true,
                agent: Some(AgentInfo {
                    name: "opencode".into(),
                    status: AgentStatus::Blocked,
                    cpu: 0.7,
                    mem_kb: 6144,
                    pane_id: 12,
                    pane_index: 1,
                    tab_index: 1,
                }),
                mouse_reporting: true,
                alt_screen: true,
            })),
        }));
        let layout = Layout {
            active: Some("session-1".into()),
            sessions: vec![SessionLayout {
                name: "session-1".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                active_tab: 0,
                tabs: vec![TabLayout { id: 1, name: "1".into(), focus: 11, zoom: false, root: root.clone() }],
                branch: Some(WireBranch { name: "main".into(), ahead: 1, behind: 0 }),
                focus: 11,
                zoom: false,
                root,
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
            Command::AgentExplain { session: "session-1".into(), pane_id: 42 },
        ];
        for cmd in cmds {
            let mut buf = Vec::new();
            write_framed(&mut buf, &cmd).unwrap();
            let decoded: Command = read_framed(&mut &buf[..]).unwrap();
            assert_eq!(decoded, cmd);
        }
    }

    #[test]
    fn explain_report_roundtrip() {
        let report = AgentExplainReport {
            pane_id: 42,
            session: "session-1".into(),
            cli: "opencode".into(),
            os_pid: 1234,
            is_ai_cli: true,
            dead: false,
            focused: false,
            raw_status: AgentStatus::Idle,
            status: AgentStatus::Done,
            prev_status: Some(AgentStatus::Working),
            idle_reason: AgentIdleReason::UnseenFinish,
            markers: vec![AgentMarkerMatch {
                agent: "opencode".into(),
                kind: "idle".into(),
                marker: "ask anything".into(),
                region: EvidenceRegion::Screen,
            }],
            precedence: "blocked > working > idle > unknown".into(),
            last_output_age_ms: 321,
            cpu: 0.5,
            mem_kb: 4096,
            pane_index: 2,
            tab_index: 1,
        };
        let mut buf = Vec::new();
        write_framed(&mut buf, &DaemonEvent::AgentExplain { report: report.clone() }).unwrap();
        let decoded: DaemonEvent = read_framed(&mut &buf[..]).unwrap();
        assert_eq!(decoded, DaemonEvent::AgentExplain { report });
    }

    #[test]
    fn agent_status_labels() {
        assert_eq!(AgentStatus::Working.label(), "working");
        assert_eq!(AgentStatus::Blocked.label(), "blocked");
        assert_eq!(AgentStatus::Idle.label(), "idle");
        assert_eq!(AgentStatus::Done.label(), "done");
        assert_eq!(AgentStatus::Unknown.label(), "unknown");
    }

    #[test]
    fn orchestration_roundtrip() {
        let cmds = vec![
            Command::AgentWait { session: "s".into(), pane_id: 1, until: AgentWaitKind::Blocked, timeout_ms: Some(5000) },
            Command::AgentPrompt { session: "s".into(), pane_id: 1, text: "hi".into(), wait: Some(AgentWaitKind::Idle), timeout_ms: None },
            Command::AgentRead { session: "s".into(), pane_id: 1, source: AgentReadSource::Traceback },
            Command::AgentStart { session: "s".into(), pane_id: 1, kind: "claude".into(), args: vec!["--help".into()] },
            Command::AgentRename { session: "s".into(), pane_id: 1, name: "alpha".into() },
            Command::PaneWaitOutput { session: "s".into(), pane_id: 1, pattern: "passed|failed".into(), is_regex: true, timeout_ms: Some(30000) },
            Command::AgentBroadcast { session: "s".into(), text: "hello".into(), filter: Some(AgentStatus::Idle) },
        ];
        for cmd in cmds {
            let mut buf = Vec::new();
            write_framed(&mut buf, &cmd).unwrap();
            let decoded: Command = read_framed(&mut &buf[..]).unwrap();
            assert_eq!(decoded, cmd);
        }
        let events = vec![
            DaemonEvent::AgentWaitResult { pane_id: 1, status: AgentStatus::Blocked },
            DaemonEvent::AgentReadResult { pane_id: 1, source: AgentReadSource::Visible, text: "hi".into(), truncated: false },
            DaemonEvent::PaneWaitResult { pane_id: 1, matched: "passed".into() },
            DaemonEvent::Error { code: "agent_blocked".into(), message: "already blocked".into() },
        ];
        for ev in events {
            let mut buf = Vec::new();
            write_framed(&mut buf, &ev).unwrap();
            let decoded: DaemonEvent = read_framed(&mut &buf[..]).unwrap();
            assert_eq!(decoded, ev);
        }
    }
}
