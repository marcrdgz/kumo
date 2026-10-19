//! Client-side view: all chrome state, geometry, input handling, mouse, and
//! ratatui rendering for the smart terminal client. Mirrors the classic TUI's
//! look, but every piece of chrome lives here — the daemon streams only the
//! semantic layout and per-pane content.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::Frame;
use ratatui::Terminal;

use kumo_core::color::ColorRgb;
use kumo_core::layout::{self, PaneGeom, TreeGeom};
use kumo_core::theme::{self, OwnedTheme, THEMES};
use kumo_protocol::{
    AgentStatus, Command, CopyHit, DaemonEvent, Layout, LayoutNode, LinkRange, PaneFrame, ScrollState,
    SessionLayout, SplitDir, ToastKind, WireBranch, WireCell, WireWorktree,
};

use crate::cli::agent_skill::{self, AGENT_SKILL_TARGETS, AgentSkillStatus, AgentSkillTarget};
use crate::cli::bindings::{self, Action, Binding, Chord, Dir, link_modifiers};
use crate::cli::chrome::{fill, put, text};
use crate::cli::mouse::sgr_mouse;
use crate::cli::status_bar::{self, SlotContext};
use crate::cli::text_input::{TextEdit, TextInput};
use kumo_core::config::{SidebarLayout, StatusBarConfig, StatusWidget, ToastPosition};

mod copy_mode;
mod hit_test;
mod mouse;
mod render;
mod sidebar;

/// Width of the left sidebar (its last column is the separator).
const SIDEBAR_WIDTH: u16 = 25;
const PROJECT_DEFAULT_WIDTH: u16 = 28;
const MIN_SIDEBAR_WIDTH: u16 = 20;
const MAX_SIDEBAR_WIDTH: u16 = 50;
/// Height of the tab bar (row 0, above panes).
const TAB_H: u16 = 1;
/// Height of the status bar (last row) — dynamic via `View::status_h()`.
const STATUS_H: u16 = 1;
/// How long the `leader+q` pane-number overlay stays up without a keypress.
const PANE_NUMBERS_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long transient status-bar messages stay up.
const TOAST_TIMEOUT: Duration = Duration::from_secs(2);
/// How long an agent-lifecycle corner toast stays up.
const AGENT_TOAST_TIMEOUT: Duration = Duration::from_secs(5);
/// Height of an agent-lifecycle corner toast (border + title + body + border).
const AGENT_TOAST_H: u16 = 4;
/// Maximum corner toasts stacked at once (oldest dropped first).
const MAX_AGENT_TOASTS: usize = 3;
/// Width of an agent-lifecycle corner toast.
const AGENT_TOAST_W: u16 = 34;
/// Braille spinner frames cycled while an agent is `Working`.
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Interval between spinner frames (~8 fps).
const SPINNER_PERIOD: Duration = Duration::from_millis(120);
/// Keep the progress state visible long enough to be perceived before the
/// small local file write completes.
const SKILL_INSTALL_MIN_VISIBLE: Duration = Duration::from_millis(350);
/// GitHub is useful ambient context, but should never become a hot API poll.
const GITHUB_REFRESH: Duration = Duration::from_secs(60);
/// Right-aligned badge drawn on a zoomed pane's top border.
const ZOOM_BADGE: &str = " ⤢ zoom ";
/// Label of the MENU button in the status bar.
const MENU_BTN: &str = " MENU ";
/// Items shown in the status-bar menu dropdown.
const MENU_ITEMS: [&str; 5] = ["config", "settings", "reload", "keybinds", "detach"];
/// Size of the session-name popup.
const SESSION_POPUP_W: u16 = 44;
const SESSION_POPUP_H: u16 = 7;
/// Size of the copy-mode search popup (rectangle, black bg, red border).
const COPY_SEARCH_W: u16 = 50;
const COPY_SEARCH_H: u16 = 5;

fn tab_width(name: &str, has_agent_indicator: bool) -> u16 {
    let n = name.chars().count() as u16;
    n.saturating_add(2).saturating_add(if has_agent_indicator { 2 } else { 0 })
}
fn lighten(c: RColor, amt: u8) -> RColor {
    match c {
        RColor::Rgb(r, g, b) => RColor::Rgb(r.saturating_add(amt), g.saturating_add(amt), b.saturating_add(amt)),
        _ => c,
    }
}

/// Foreground with readable contrast against a theme's input surface. Built-in
/// themes use a light neutral surface; custom themes may choose a dark one.
fn input_fg(theme: &OwnedTheme) -> RColor {
    contrast_fg(theme, theme.input_bg)
}

/// Text color for chrome drawn on a filled theme color (selected rows, badges,
/// focused fields). Dark text is kept while it meets WCAG AA (4.5:1), which
/// holds for every built-in accent; dark custom fills get light text.
fn contrast_fg(theme: &OwnedTheme, bg: RColor) -> RColor {
    let rgb = match bg {
        RColor::Rgb(r, g, b) => Some((r, g, b)),
        RColor::Indexed(i) => theme.palette.get(i as usize).map(|c| (c.r, c.g, c.b)),
        RColor::Black | RColor::DarkGray | RColor::Red | RColor::Blue | RColor::Magenta => return RColor::White,
        _ => return RColor::Black,
    };
    let Some((r, g, b)) = rgb else { return RColor::Black };
    let linear = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    // Black reaches 4.5:1 at (L + 0.05) / 0.05 >= 4.5, i.e. L >= 0.175.
    if luminance >= 0.175 { RColor::Black } else { RColor::White }
}

fn input_style(theme: &OwnedTheme) -> Style {
    Style::default().fg(input_fg(theme)).bg(theme.input_bg)
}

fn input_hint_style(theme: &OwnedTheme) -> Style {
    input_style(theme).add_modifier(Modifier::DIM)
}

fn sidebar_active_bg(theme: &OwnedTheme) -> RColor {
    // worktree selection — subtle but visible, a touch darker than before
    lighten(theme.panel_sep, 28)
}

fn agent_active_bg(theme: &OwnedTheme) -> RColor {
    // agent focused — slightly brighter than the worktree block so the cursor pops
    lighten(theme.panel_sep, 40)
}

/// Blend an RGB color toward `bg` by `num/den` (indexed colors resolve through
/// the theme palette first). Used to make idle chrome recede next to accents.
fn dim_toward(c: RColor, bg: ColorRgb, palette: &[ColorRgb; 16], num: u32, den: u32) -> RColor {
    let (r, g, b) = match c {
        RColor::Rgb(r, g, b) => (r, g, b),
        RColor::Indexed(i) => match palette.get(i as usize) {
            Some(cc) => (cc.r, cc.g, cc.b),
            None => return c,
        },
        _ => return c,
    };
    let mix = |cv: u8, bv: u8| -> u8 { ((cv as u32 * (den - num) + bv as u32 * num) / den).min(255) as u8 };
    RColor::Rgb(mix(r, bg.r), mix(g, bg.g), mix(b, bg.b))
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Mode {
    Normal,
    Leader,
    Copy,
    /// Agent-inbox popup owns the keyboard.
    Inbox,
}

#[derive(Clone)]
struct CopyState {
    pane_id: u64,
    cursor: (u16, u16),
    anchor: Option<(u16, u16)>,
    linewise: bool,
    // search
    search_active: bool,
    search_input: String,
    search_cursor: usize,
    search_forward: bool,
    search_query: Option<String>,
    hits: Vec<CopyHit>,
    hit_idx: Option<usize>,
}

/// Agent-inbox popup state (active while `Mode::Inbox`).
#[derive(Clone, Copy)]
struct Inbox {
    /// Index into the full attention-sorted agent list.
    sel: usize,
}

/// Geometry of the divided sidebar layout (stacked spaces + agent panels),
/// computed once per frame from the live layout.
struct DividedPanels {
    spaces_visible: bool,
    agents_visible: bool,
    spaces_items: Vec<SidebarRow>,
    agents_items: Vec<SidebarRow>,
    /// Rows of spaces content currently visible.
    spaces_h: usize,
    /// Rows of agent entries currently visible (label sits above them).
    agents_h: usize,
    /// Row of the panel divider (`u16::MAX` when only one panel is shown).
    divider_y: u16,
    /// Row of the agent panel label (`2` when the spaces panel is hidden).
    agents_label_y: u16,
}

/// Fixed geometry for the project navigator and its compact, bottom-anchored
/// agent inbox. The spaces list is the only scrolling region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProjectSidebarGeometry {
    tiny: bool,
    spaces_h: usize,
    new_session_y: u16,
    divider_y: u16,
    agents_header_y: u16,
    agent_rows: usize,
}

/// In-flight sidebar panel divider drag: (start row, spaces height at start).
#[derive(Clone, Copy)]
struct SidebarDrag {
    start_y: u16,
    start_split: u16,
}

/// In-flight sidebar width drag (project layout).
#[derive(Clone, Copy)]
struct SidebarWidthDrag {
    start_x: u16,
    start_width: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SidebarReorderItem {
    Project(String),
    Worktree { project: String, worktree: String, session_idx: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SidebarReorderTarget {
    Project(String),
    Worktree { project: String, worktree: String },
}

struct SidebarReorderDrag {
    source: SidebarReorderItem,
    source_y: u16,
    target: SidebarReorderTarget,
    after: bool,
    moved: bool,
}

struct TabReorderDrag {
    session: String,
    tab_id: u64,
    tab_name: String,
    source_idx: usize,
    target_idx: usize,
    after: bool,
    moved: bool,
}

/// Finder item for the workspace finder (`leader+f`).
#[derive(Clone, Debug)]
#[allow(dead_code)]
enum FinderItem {
    Session { idx: usize, name: String },
    Tab { session_idx: usize, tab_idx: usize, session_name: String, tab_name: String },
}

struct WorkspaceFinder {
    open: bool,
    input: String,
    cursor: usize,
    items: Vec<FinderItem>,
    filtered: Vec<usize>,
    selected: usize,
    scroll: usize,
}

/// Ordering style of the agent panel in the divided sidebar: state-grouped
/// sections or the classic rank-sorted workspace rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum AgentPanelOrder {
    #[default]
    Grouped,
    Ranked,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Sessions,
    Agents,
}

impl SidebarTab {
    fn label(self) -> &'static str {
        match self {
            SidebarTab::Sessions => "sessions",
            SidebarTab::Agents => "agents",
        }
    }
    fn from_section(s: kumo_core::config::SidebarSection) -> Self {
        match s {
            kumo_core::config::SidebarSection::Sessions => SidebarTab::Sessions,
            kumo_core::config::SidebarSection::Agents => SidebarTab::Agents,
        }
    }
}

fn border_chars(style: kumo_core::config::BorderStyle) -> (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str) {
    match style {
        kumo_core::config::BorderStyle::Single => ("┌", "┐", "└", "┘", "─", "│"),
        kumo_core::config::BorderStyle::Rounded => ("╭", "╮", "╰", "╯", "─", "│"),
        kumo_core::config::BorderStyle::Double => ("╔", "╗", "╚", "╝", "═", "║"),
        kumo_core::config::BorderStyle::Heavy => ("┏", "┓", "┗", "┛", "━", "┃"),
        kumo_core::config::BorderStyle::Hidden => (" ", " ", " ", " ", " ", " "),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PopupTarget {
    NewSession,
    #[allow(dead_code)]
    NewWorktree(usize),
    RenamePane(u64),
    RenameSession(usize),
    RenameTab { session: usize, tab: usize },
}

#[derive(Clone, Copy, PartialEq)]
enum PopupBtn {
    Enter,
    Cancel,
}

struct Popup {
    open: bool,
    target: Option<PopupTarget>,
    name: String,
    cursor: usize,
    error: Option<String>,
    hover: Option<PopupBtn>,
}

#[derive(Clone, Copy, PartialEq)]
enum CtxTarget {
    Pane(u64),
    Session(usize),
    Tab(usize, usize), // (session_idx, tab_idx)
}

struct Menu {
    open: bool,
    selected: usize,
}

struct CtxMenu {
    open: bool,
    x: u16,
    y: u16,
    selected: usize,
    target: CtxTarget,
}

#[derive(Clone, Copy, PartialEq)]
enum SettingsTab {
    Appearance,
    Agents,
    About,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            SettingsTab::Appearance => "appearance",
            SettingsTab::Agents => "agents",
            SettingsTab::About => "about",
        }
    }
}

const SETTINGS_TABS: [SettingsTab; 3] = [
    SettingsTab::Appearance,
    SettingsTab::Agents,
    SettingsTab::About,
];

struct SettingsPanel {
    open: bool,
    tab: usize,
    selected: usize,
    skill_installing: Option<(AgentSkillTarget, AgentSkillAction, Instant)>,
    skill_result: Option<SkillInstallResult>,
    skill_feedback: Option<(AgentSkillTarget, bool, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentSkillAction {
    Install,
    Update,
    Uninstall,
}

type SkillInstallResult = (
    AgentSkillTarget,
    AgentSkillAction,
    std::result::Result<PathBuf, String>,
);

struct KeybindOverlay {
    open: bool,
    input: String,
    cursor: usize,
    selected: usize,
    scroll: usize,
}

struct WorktreePicker {
    open: bool,
    session: usize,
    items: Vec<WireWorktree>,
    selected: usize,
    scroll: u16,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WorktreeCreateTab {
    Smart,
    Github,
    Jira,
    Branch,
    Name,
}

impl WorktreeCreateTab {
    fn label(self) -> &'static str {
        match self {
            WorktreeCreateTab::Smart => "Smart",
            WorktreeCreateTab::Github => "GitHub",
            WorktreeCreateTab::Jira => "Jira",
            WorktreeCreateTab::Branch => "Branch",
            WorktreeCreateTab::Name => "Name",
        }
    }
    #[allow(dead_code)]
    fn from_idx(idx: usize) -> Self {
        match idx {
            1 => Self::Github,
            2 => Self::Jira,
            3 => Self::Branch,
            4 => Self::Name,
            _ => Self::Smart,
        }
    }
    #[allow(dead_code)]
    fn idx(self) -> usize {
        match self {
            WorktreeCreateTab::Smart => 0,
            WorktreeCreateTab::Github => 1,
            WorktreeCreateTab::Jira => 2,
            WorktreeCreateTab::Branch => 3,
            WorktreeCreateTab::Name => 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WorktreeCreateFocus {
    CreateFrom,
    BranchOverride,
    Note,
    Agent,
}

struct WorktreeCreateDialog {
    open: bool,
    session: usize,
    tab: WorktreeCreateTab,
    create_from: String,
    cursor: usize,
    branch_override: String,
    branch_cursor: usize,
    note: String,
    note_cursor: usize,
    agent: String,
    agent_cursor: usize,
    advanced: bool,
    focus: WorktreeCreateFocus,
    error: Option<String>,
}

struct SessionCloseConfirm {
    open: bool,
    session_idx: usize,
    session_name: String,
    path: PathBuf,
    mode: WorktreeRemovalMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorktreeRemovalMode {
    Safe,
    Force,
}

/// One agent-lifecycle corner toast pushed by the daemon (blocked / finished).
#[derive(Clone)]
struct AgentToast {
    pane_id: u64,
    kind: ToastKind,
    title: String,
    body: String,
    born: Instant,
}

/// One pane's client-side grid, rebuilt from `PaneFrame`s.
#[derive(Clone)]
struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<WireCell>>,
    cursor: Option<(u16, u16)>,
    scroll: Option<ScrollState>,
    links: HashMap<u16, Vec<LinkRange>>,
    /// Cached ratatui cells per row; `None` = stale, rebuilt lazily on render.
    /// Only dirty rows are rebuilt, avoiding O(cols × rows) work per frame.
    rendered: Vec<Option<Vec<ratatui::buffer::Cell>>>,
    /// Rows that changed since the last render (from `PaneFrame.rows_dirty`).
    dirty_rows: HashSet<u16>,
}

impl Grid {
    fn apply(&mut self, frame: &PaneFrame) {
        if frame.full || self.cells.is_empty() {
            self.cols = frame.cols as usize;
            self.rows = frame.rows as usize;
            self.cells = vec![Vec::new(); self.rows];
            self.rendered = vec![None; self.rows];
            self.dirty_rows = HashSet::new();
        }
        for patch in &frame.rows_dirty {
            if (patch.row as usize) < self.rows {
                self.cells[patch.row as usize] = patch.cells.clone();
                // Invalidate the cached render for this row
                if let Some(slot) = self.rendered.get_mut(patch.row as usize) {
                    *slot = None;
                }
                self.dirty_rows.insert(patch.row);
            }
            self.links.insert(patch.row, patch.links.clone());
        }
        self.cursor = frame.cursor;
        self.scroll = frame.scroll;
    }

    /// Get the rendered ratatui cells for a row, rebuilding only if the row
    /// changed since the last render. This avoids O(cols × rows) work per frame
    /// by only rebuilding dirty rows.
    /// 
    /// Note: The cache stores only base terminal styles. Selection highlights
    /// and link underlines are applied at blit time in `render_pane_content()`
    /// because they change independently of terminal content (client-side state).
    fn get_rendered_row(
        &mut self,
        row: usize,
    ) -> Option<&Vec<ratatui::buffer::Cell>> {
        if row >= self.rows {
            return None;
        }
        // Ensure the rendered cache is initialized to the right size
        if self.rendered.len() != self.rows {
            self.rendered = vec![None; self.rows];
        }
        // Rebuild the row if it's not cached
        if self.rendered.get(row).map(|r| r.is_none()).unwrap_or(true) {
            let cells = self.cells.get(row)?;
            let mut rendered = Vec::with_capacity(cells.len());
            for cell in cells.iter() {
                let style = cell_style(cell);
                let ch = if cell.text.trim().is_empty() { " " } else { &cell.text };
                let mut ratatui_cell = ratatui::buffer::Cell::default();
                ratatui_cell.set_symbol(ch);
                ratatui_cell.set_style(style);
                if cell.cell_width == 0 {
                    // Continuation cell after a wide glyph: skip it so the wide
                    // glyph's right half is never overwritten.
                    ratatui_cell.set_diff_option(ratatui::buffer::CellDiffOption::Skip);
                } else if cell.cell_width == 2 {
                    // Pin the width so ratatui emits this glyph as 2 columns
                    // even when `unicode-width` under-counts it (text-presentation
                    // emoji like ⚡ ✨ are `So` symbols rated 1 column but drawn 2).
                    // Otherwise the terminal shifts the rest of the row right and
                    // a full-line highlight's last cell lands on the pane border.
                    ratatui_cell.set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(
                        std::num::NonZeroU16::new(2).expect("2 is non-zero"),
                    ));
                }
                rendered.push(ratatui_cell);
            }
            if let Some(slot) = self.rendered.get_mut(row) {
                *slot = Some(rendered);
            }
        }
        self.rendered.get(row).and_then(|r| r.as_ref())
    }
}

/// A text selection inside a pane (viewport-relative coordinates).
#[derive(Clone, Copy, PartialEq)]
struct Sel {
    pane_id: u64,
    start: (u16, u16),
    end: (u16, u16),
}

/// A press in a mouse-reporting pane: the whole gesture is forwarded to the
/// pane so the app does its own text selection.
#[derive(Clone, Copy)]
struct PendingClick {
    pane_id: u64,
    col: u16,
    row: u16,
}

/// An in-flight divider drag (the daemon owns the ratios).
#[derive(Clone, Copy)]
struct SplitDrag {
    split_id: u64,
    dir: SplitDir,
    area: Rect,
}

/// One sidebar row, shared by rendering and mouse hit-testing.
#[derive(Clone, PartialEq, Debug)]
#[allow(dead_code)]
enum SidebarRow {
    Header(String),
    Spacer,
    /// A panel label row in the divided layout ("spaces" / "agents"), with an
    /// optional right-aligned descriptor.
    PanelLabel(String, Option<String>),
    /// Agent state group header in the divided layout, with its count.
    GroupHeader(AgentStatus, usize),
    /// A muted placeholder line (e.g. the agents panel with no agents).
    Dim(String),
    /// Draggable divider between the spaces and agents panels (divided layout).
    Divider,
    Session(usize),
    Branch(usize, WireBranch),
    AgentDir(usize, u64, String, AgentStatus),
    AgentName(usize, u64, String, AgentStatus),
    NewSession,
    // Project-structured layout rows
    Search,
    SectionLabel(String, Option<String>),
    ProjectHeader { key: String, label: String },
    Worktree(usize),
    InlineAgent(usize, u64, String, AgentStatus),
    AgentInboxHeader { blocked: usize, done: usize, working: usize },
    CompactAgent(usize, u64, String, AgentStatus),
    DropIndicator,
}

pub struct View {
    out: UnixStream,
    cols: u16,
    rows: u16,
    mode: Mode,
    leader: Chord,
    keymap: Vec<Binding>,
    layout: Option<Layout>,
    grids: HashMap<u64, Grid>,
    rects: Vec<(u64, Rect)>,
    splitters: Vec<(u64, SplitDir, Rect, Rect)>,
    subscribed: HashSet<u64>,
    sent_sizes: HashMap<u64, (u16, u16)>,
    theme_idx: usize,
    custom_theme: Option<OwnedTheme>,
    sidebar_open: bool,
    sidebar_tab: SidebarTab,
    sidebar_scroll: (u16, u16),
    tab_hover: Option<usize>,
    tab_rects: Vec<(usize, Rect, Rect)>, // (tab_idx, pill rect, close rect)
    tab_scroll: usize,
    tab_reorder_drag: Option<TabReorderDrag>,
    plus_rect: Option<Rect>,
    popup: Popup,
    menu: Menu,
    ctx_menu: CtxMenu,
    keybind_overlay: KeybindOverlay,
    settings: SettingsPanel,
    worktree_picker: WorktreePicker,
    worktree_create: WorktreeCreateDialog,
    session_close_confirm: SessionCloseConfirm,
    pane_numbers: Option<Instant>,
    status_msg: Option<(String, Instant)>,
    notice: Option<(String, Instant)>,
    /// Agent-lifecycle toasts, newest last (stacking order depends on the
    /// configured anchor corner).
    agent_toasts: Vec<AgentToast>,
    /// Where the toast stack is anchored (`[notifications] position`).
    toast_pos: ToastPosition,
    update_notice: Option<(String, String)>,
    link_mods: bool,
    drag: Option<SplitDrag>,
    sel: Option<Sel>,
    pending_click: Option<PendingClick>,
    /// Wheel bytes accumulated since the last flush, one buffer per pane.
    /// Coalesced by the input batch so fast scrolling costs one `PaneWrite`
    /// per pane instead of one IPC frame per wheel tick.
    pending_wheel: HashMap<u64, Vec<u8>>,
    copy: Option<CopyState>,
    /// Full agent-inbox selection while `Mode::Inbox` is active.
    inbox: Option<Inbox>,
    /// Test-only override of the `[sidebar] layout` config.
    sidebar_layout_override: Option<SidebarLayout>,
    /// User-adjusted spaces-panel height in rows (divided layout); `None`
    /// keeps the automatic sizing.
    sidebar_split: Option<u16>,
    /// In-flight divider drag (divided layout).
    sidebar_drag: Option<SidebarDrag>,
    /// Sorting of the agent panel (divided layout); toggled by its label.
    agents_panel_order: AgentPanelOrder,
    sidebar_width: u16,
    sidebar_width_drag: Option<SidebarWidthDrag>,
    sidebar_reorder_drag: Option<SidebarReorderDrag>,
    finder: WorkspaceFinder,
    sidebar_hover: Option<usize>,
    /// Project groups collapsed in the project sidebar. This is client-only
    /// view state: it intentionally does not affect daemon/session state.
    collapsed_projects: HashSet<String>,
    project_order: Vec<String>,
    worktree_order: HashMap<String, Vec<String>>,
    sidebar_filter: String,
    sidebar_filter_active: bool,
    dirty: bool,
    detach_requested: bool,
    /// A write to the daemon socket failed. Most call sites ignore `send`'s
    /// result, so the render loop polls this to reconnect instead of the
    /// client silently running against a dead connection.
    connection_lost: bool,
    status_bar: StatusBarConfig,
    hostname: String,
    clock_str: String,
    clock_next: Instant,
    is_ssh: bool,
    github_status: Option<status_bar::GitHubStatus>,
    github_workspace: Option<PathBuf>,
    github_query: Option<PathBuf>,
    github_refresh_at: Instant,
    github_tx: Sender<(PathBuf, Option<status_bar::GitHubStatus>)>,
    github_rx: Receiver<(PathBuf, Option<status_bar::GitHubStatus>)>,
    skill_install_tx: Sender<SkillInstallResult>,
    skill_install_rx: Receiver<SkillInstallResult>,
    /// Current frame of the agent-activity spinner.
    spinner_frame: usize,
    /// When to advance the spinner next (kept ~8 fps, not per loop tick).
    spinner_next: Instant,
}

/// Whether a layout subtree contains an agent whose status is `Working`.
fn node_has_working(node: Option<&LayoutNode>) -> bool {
    let Some(root) = node else { return false };
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        match n {
            LayoutNode::Pane(p) => {
                if p.agent.as_ref().map(|a| a.status == AgentStatus::Working).unwrap_or(false) {
                    return true;
                }
            }
            LayoutNode::Split { a, b, .. } => {
                stack.push(a);
                stack.push(b);
            }
        }
    }
    false
}

impl View {
    pub fn new(out: UnixStream, cols: u16, rows: u16) -> Self {
        let leader = match kumo_core::config::leader() {            Some(raw) => match bindings::parse_chord(&raw) {
                Some(chord) => chord,
                None => {
                    log::warn!("kumo: invalid leader key {:?}; falling back to ctrl+b", raw);
                    bindings::LEADER
                }
            },
            None => bindings::LEADER,
        };
        let keymap = bindings::build_keymap(&kumo_core::config::keymap_bindings());
        let status_bar = kumo_core::config::status_bar();
        let toast_pos = kumo_core::config::toast_position();
        let hostname = status_bar::cached_hostname();
        let is_ssh = status_bar::is_ssh_session();
        let clock_str = status_bar::format_clock(&status_bar.widgets.clock.format);
        // Next minute boundary — refresh once per minute; we compute remaining secs
        // to the next minute so the first tick is aligned, not 60s from startup.
        let now_secs = chrono::Local::now().timestamp() % 60;
        let rem = (60 - now_secs).max(1) as u64;
        let clock_next = Instant::now() + Duration::from_secs(rem);
        let (github_tx, github_rx) = mpsc::channel();
        let (skill_install_tx, skill_install_rx) = mpsc::channel();
        let sidebar_width = kumo_core::config::sidebar().width.unwrap_or_else(|| {
            if kumo_core::config::sidebar().layout == SidebarLayout::Project { PROJECT_DEFAULT_WIDTH } else { SIDEBAR_WIDTH }
        }).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        let ui_state = kumo_core::ui_state::load();
        let mut view = View {
            out,
            cols: cols.max(2),
            rows: rows.max(2),
            mode: Mode::Normal,
            leader,
            keymap,
            layout: None,
            grids: HashMap::new(),
            rects: Vec::new(),
            splitters: Vec::new(),
            subscribed: HashSet::new(),
            sent_sizes: HashMap::new(),
            theme_idx: kumo_core::theme::DEFAULT_THEME_IDX,
            custom_theme: None,
            sidebar_open: cols >= 80,
            sidebar_tab: SidebarTab::Sessions,
            sidebar_scroll: (0, u16::MAX),
            popup: Popup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            menu: Menu { open: false, selected: 0 },
            ctx_menu: CtxMenu { open: false, x: 0, y: 0, selected: 0, target: CtxTarget::Pane(0) },
            keybind_overlay: KeybindOverlay { open: false, input: String::new(), cursor: 0, selected: 0, scroll: 0 },
            settings: SettingsPanel {
                open: false,
                tab: 0,
                selected: kumo_core::theme::DEFAULT_THEME_IDX,
                skill_installing: None,
                skill_result: None,
                skill_feedback: None,
            },
            worktree_picker: WorktreePicker { open: false, session: 0, items: Vec::new(), selected: 0, scroll: 0, error: None },
            worktree_create: WorktreeCreateDialog { open: false, session: 0, tab: WorktreeCreateTab::Smart, create_from: String::new(), cursor: 0, branch_override: String::new(), branch_cursor: 0, note: String::new(), note_cursor: 0, agent: String::new(), agent_cursor: 0, advanced: false, focus: WorktreeCreateFocus::CreateFrom, error: None },
            session_close_confirm: SessionCloseConfirm { open: false, session_idx: 0, session_name: String::new(), path: PathBuf::new(), mode: WorktreeRemovalMode::Safe },
            pane_numbers: None,
            status_msg: None,
            notice: None,
            agent_toasts: Vec::new(),
            toast_pos,
            update_notice: None,
            link_mods: false,
            drag: None,
            sel: None,
            pending_click: None,
            pending_wheel: HashMap::new(),
            copy: None,
            inbox: None,
            sidebar_layout_override: None,
            sidebar_split: None,
            sidebar_drag: None,
            agents_panel_order: AgentPanelOrder::default(),
            sidebar_width,
            sidebar_width_drag: None,
            sidebar_reorder_drag: None,
            finder: WorkspaceFinder { open: false, input: String::new(), cursor: 0, items: Vec::new(), filtered: Vec::new(), selected: 0, scroll: 0 },
            sidebar_hover: None,
            collapsed_projects: ui_state.collapsed_projects,
            project_order: ui_state.project_order,
            worktree_order: ui_state.worktree_order,
            sidebar_filter: String::new(),
            sidebar_filter_active: false,
            tab_hover: None,
            tab_rects: Vec::new(),
            tab_scroll: 0,
            tab_reorder_drag: None,
            plus_rect: None,
            dirty: true,
            detach_requested: false,
            connection_lost: false,
            status_bar,
            hostname,
            clock_str,
            clock_next,
            is_ssh,
            github_status: None,
            github_workspace: None,
            github_query: None,
            github_refresh_at: Instant::now(),
            github_tx,
            github_rx,
            skill_install_tx,
            skill_install_rx,
            spinner_frame: 0,
            spinner_next: Instant::now(),
        };
        let _ = view.send(&Command::SubscribeLayout);
        view
    }

    fn effective_sidebar_width(&self) -> u16 {
        self.sidebar_width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH).min(self.cols.saturating_sub(1).max(MIN_SIDEBAR_WIDTH))
    }

    fn status_h(&self) -> u16 {
        if self.status_bar.enabled { STATUS_H } else { 0 }
    }

    /// First row of the bottom chrome (status bar). Overlay shadows stop
    /// above it so popups never repaint chrome.
    fn shadow_floor(&self) -> u16 {
        self.rows.saturating_sub(self.status_h())
    }

    fn status_bar_contains(&self, w: StatusWidget) -> bool {
        self.status_bar.left.contains(&w) || self.status_bar.center.contains(&w) || self.status_bar.right.contains(&w)
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Whether a transient overlay (pane numbers, toast, notice) is currently
    /// up, so the loop keeps re-rendering until it expires — the client-side
    /// equivalent of the daemon's forced frame.
    pub fn has_transient(&mut self) -> bool {
        let now = Instant::now();
        if self.poll_agent_skill_install(now) {
            return true;
        }
        if self.poll_github(now) {
            return true;
        }
        // Clock tick: repaint once per minute when the clock widget is visible.
        if self.status_bar.enabled && self.status_bar_contains(StatusWidget::Clock) && now >= self.clock_next {
            self.clock_str = status_bar::format_clock(&self.status_bar.widgets.clock.format);
            let rem = (60 - (chrono::Local::now().timestamp() % 60)).max(1) as u64;
            self.clock_next = now + Duration::from_secs(rem);
            self.mark_dirty();
            return true;
        }
        if self.pane_numbers.is_some() {
            return true;
        }
        // Spinner: advance ~8 fps while any agent is working so the frame keeps
        // repainting; between ticks the loop idles as usual.
        if self.any_agent_working() && now >= self.spinner_next {
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
            self.spinner_next = now + SPINNER_PERIOD;
            self.mark_dirty();
            return true;
        }
        if self.status_msg.as_ref().map(|(_, t)| now.duration_since(*t) < TOAST_TIMEOUT).unwrap_or(false) {
            return true;
        }
        if self.notice.as_ref().map(|(_, t)| now.duration_since(*t) < TOAST_TIMEOUT).unwrap_or(false) {
            return true;
        }
        if self.agent_toasts.iter().any(|t| now.duration_since(t.born) < AGENT_TOAST_TIMEOUT) {
            return true;
        }
        false
    }

    fn poll_agent_skill_install(&mut self, now: Instant) -> bool {
        while let Ok(result) = self.skill_install_rx.try_recv() {
            self.settings.skill_result = Some(result);
        }

        let Some((target, action, started)) = self.settings.skill_installing else {
            return false;
        };
        if now.duration_since(started) < SKILL_INSTALL_MIN_VISIBLE {
            return true;
        }
        let Some((result_target, result_action, result)) = self.settings.skill_result.take() else {
            return true;
        };
        if result_target != target || result_action != action {
            self.settings.skill_result = Some((result_target, result_action, result));
            return true;
        }

        self.settings.skill_installing = None;
        self.settings.skill_feedback = Some(match result {
            Ok(path) => {
                let message = match action {
                    AgentSkillAction::Install => format!("installed at {}", path.display()),
                    AgentSkillAction::Update => format!("updated at {}", path.display()),
                    AgentSkillAction::Uninstall => format!("removed from {}", path.display()),
                };
                (target, true, message)
            }
            Err(error) => (target, false, error),
        });
        self.mark_dirty();
        true
    }

    fn poll_github(&mut self, now: Instant) -> bool {
        if !self.status_bar.enabled || !self.status_bar_contains(StatusWidget::Github) {
            self.github_status = None;
            self.github_workspace = None;
            return false;
        }

        let workspace = self.active_session().map(|session| session.workspace.clone());
        if workspace != self.github_workspace {
            self.github_workspace = workspace.clone();
            self.github_status = None;
            self.github_refresh_at = now;
        }

        let mut changed = false;
        while let Ok((queried, status)) = self.github_rx.try_recv() {
            if self.github_query.as_ref() == Some(&queried) {
                self.github_query = None;
            }
            if self.github_workspace.as_ref() == Some(&queried) {
                if self.github_status != status {
                    self.github_status = status;
                    changed = true;
                }
                self.github_refresh_at = now + GITHUB_REFRESH;
            }
        }

        if self.github_query.is_none() && now >= self.github_refresh_at {
            if let Some(workspace) = workspace {
                let tx = self.github_tx.clone();
                self.github_query = Some(workspace.clone());
                self.github_refresh_at = now + GITHUB_REFRESH;
                std::thread::spawn(move || {
                    let status = status_bar::query_github(&workspace);
                    let _ = tx.send((workspace, status));
                });
            }
        }

        if changed {
            self.mark_dirty();
        }
        changed
    }
    pub fn detach_requested(&self) -> bool {
        self.detach_requested
    }

    pub fn connection_lost(&self) -> bool {
        self.connection_lost
    }

    /// Show a transient notice line (e.g. after the client reconnected).
    pub fn show_notice(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
        self.mark_dirty();
    }

    fn send(&mut self, cmd: &Command) -> Result<()> {
        use std::io::Write;
        let frame = kumo_core::protocol::encode(cmd)?;
        // Only a socket write failure means the daemon is gone; an encode
        // error is a local bug and must not trigger a reconnect.
        if let Err(e) = self.out.write_all(&frame).and_then(|()| self.out.flush()) {
            self.connection_lost = true;
            return Err(e.into());
        }
        Ok(())
    }

    /// Send the wheel ticks accumulated for the current input batch as one
    /// `PaneWrite` per pane. Called by the render loop after draining input;
    /// anything that writes pane bytes mid-batch (keys, clicks, pastes) flushes
    /// it first to preserve ordering.
    pub fn flush_wheel(&mut self) -> Result<()> {
        for (pane_id, bytes) in std::mem::take(&mut self.pending_wheel) {
            self.send(&Command::PaneWrite { pane_id, bytes })?;
        }
        Ok(())
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn themes_len(&self) -> usize {
        THEMES.len() + if self.custom_theme.is_some() { 1 } else { 0 }
    }

    fn all_themes(&self) -> Vec<OwnedTheme> {
        theme::all_themes(self.custom_theme.clone())
    }

    fn current_theme(&self) -> OwnedTheme {
        if self.theme_idx < THEMES.len() {
            OwnedTheme::from(THEMES[self.theme_idx])
        } else if self.theme_idx == THEMES.len() {
            self.custom_theme
                .clone()
                .unwrap_or_else(|| OwnedTheme::from(THEMES[theme::DEFAULT_THEME_IDX]))
        } else {
            OwnedTheme::from(THEMES[theme::DEFAULT_THEME_IDX])
        }
    }

    #[allow(dead_code)]
    fn theme_at(&self, idx: usize) -> Option<OwnedTheme> {
        self.all_themes().get(idx).cloned()
    }

    // ------------------------------------------------------------------
    // Daemon events
    // ------------------------------------------------------------------

    pub fn on_event(&mut self, ev: DaemonEvent) {
        match ev {
            DaemonEvent::Layout { layout } => self.on_layout(layout),
            DaemonEvent::PaneFrame { frame } => self.on_pane_frame(frame),
            DaemonEvent::Theme { idx, custom } => {
                let mut dirty = false;
                if let Some(w) = custom {
                    let owned = theme::wire_to_owned(w);
                    if self.custom_theme.as_ref() != Some(&owned) {
                        self.custom_theme = Some(owned);
                        dirty = true;
                    }
                }
                let len = self.themes_len();
                if idx < len && idx != self.theme_idx {
                    self.theme_idx = idx;
                    dirty = true;
                } else if dirty && idx == self.theme_idx {
                    // palette of active custom changed
                }
                if dirty {
                    self.mark_dirty();
                }
            }
            DaemonEvent::UpdateNotice { notice } => {
                self.update_notice = notice.map(|n| (n.key, n.display));
                self.mark_dirty();
            }
            DaemonEvent::Toast { pane_id, kind, title, body } => {
                if self.toast_pos == ToastPosition::Off {
                    return;
                }
                self.agent_toasts.push(AgentToast { pane_id, kind, title, body, born: Instant::now() });
                if self.agent_toasts.len() > MAX_AGENT_TOASTS {
                    self.agent_toasts.remove(0);
                }
                self.mark_dirty();
            }
            DaemonEvent::Worktrees { items } => {
                self.worktree_picker.items = items;
                self.worktree_picker.error = None;
                self.worktree_picker.selected = 0;
                self.worktree_picker.scroll = 0;
                self.mark_dirty();
            }
            DaemonEvent::Reply { message } => {
                self.status_msg = Some((message, Instant::now()));
                self.mark_dirty();
            }
            DaemonEvent::ConfigReloaded { notice } => {
                self.notice = Some((notice, Instant::now()));
                self.ensure_sidebar_tab_visible();
                // Status bar is client-local but reloaded from the same config.
                kumo_core::config::invalidate_cache();
                let new_bar = kumo_core::config::status_bar();
                let enabled_changed = new_bar.enabled != self.status_bar.enabled;
                self.status_bar = new_bar;
                self.toast_pos = kumo_core::config::toast_position();
                self.hostname = status_bar::cached_hostname();
                self.is_ssh = status_bar::is_ssh_session();
                self.clock_str = status_bar::format_clock(&self.status_bar.widgets.clock.format);
                let rem = (60 - (chrono::Local::now().timestamp() % 60)).max(1) as u64;
                self.clock_next = Instant::now() + Duration::from_secs(rem);
                let new_width = kumo_core::config::sidebar().width.unwrap_or_else(|| {
                    if kumo_core::config::sidebar().layout == SidebarLayout::Project { PROJECT_DEFAULT_WIDTH } else { SIDEBAR_WIDTH }
                }).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                let width_changed = new_width != self.sidebar_width;
                self.sidebar_width = new_width;
                if enabled_changed || width_changed {
                    self.recompute_geometry();
                }
                self.mark_dirty();
            }
            DaemonEvent::CopySearchResults { pane_id, query, hits } => {
                let mut jump_to: Option<(u32, u16)> = None; // (row, start_col)
                let mut status: Option<String> = None;
                let is_relevant = if let Some(cs) = self.copy.as_mut() {
                    if cs.pane_id == pane_id {
                        cs.search_query = Some(query.clone());
                        cs.hits = hits.clone();
                        cs.hit_idx = if hits.is_empty() { None } else { Some(0) };
                        if !hits.is_empty() {
                            jump_to = Some((hits[0].row, hits[0].start_col));
                        }
                        if hits.is_empty() {
                            status = Some(format!("no matches for {:?}", query));
                        } else {
                            status = Some(format!("{} matches for {:?}", hits.len(), query));
                        }
                        true
                    } else { false }
                } else { false };
                if is_relevant {
                    if let Some((row, start_col)) = jump_to {
                        let dims = self.copy_pane_dims(pane_id);
                        let half = dims.map(|(_, h)| h / 2).unwrap_or(0) as u32;
                        let target = row.saturating_sub(half);
                        let _ = self.send(&Command::CopyScrollTo { pane_id, row: target });
                        if let Some(cs) = self.copy.as_mut() {
                            let half_col = dims.map(|(w,_)| w).unwrap_or(80);
                            let h = dims.map(|(_,h)| h).unwrap_or(24);
                            let cur_y = if row < half { row as u16 } else { half as u16 };
                            cs.cursor = (start_col.min(half_col.saturating_sub(1)), cur_y.min(h.saturating_sub(1)));
                        }
                    }
                    if let Some(msg) = status {
                        self.status_msg = Some((msg, Instant::now()));
                    }
                }
                self.mark_dirty();
            }
            _ => {}
        }
    }

    fn on_layout(&mut self, layout: Layout) {
        self.layout = Some(layout);
        self.ensure_sidebar_tab_visible();
        self.update_tab_rects();
        self.recompute_geometry();
    }

    fn on_pane_frame(&mut self, frame: PaneFrame) {
        let pid = frame.pane_id;
        let grid = self.grids.entry(pid).or_insert_with(|| Grid {
            cols: 0,
            rows: 0,
            cells: Vec::new(),
            cursor: None,
            scroll: None,
            links: HashMap::new(),
            rendered: Vec::new(),
            dirty_rows: HashSet::new(),
        });
        grid.apply(&frame);
        self.mark_dirty();
    }

    // ------------------------------------------------------------------
    // Geometry
    // ------------------------------------------------------------------

    fn active_session(&self) -> Option<&SessionLayout> {
        let layout = self.layout.as_ref()?;
        let name = layout.active.as_deref()?;
        layout.sessions.iter().find(|s| s.name == name)
    }

    fn active_tab(&self) -> Option<&kumo_protocol::TabLayout> {
        let s = self.active_session()?;
        s.tabs.get(s.active_tab)
    }

    fn session_zoom(&self) -> bool {
        self.active_tab().map(|t| t.zoom).unwrap_or(false)
    }

    /// Whether any pane across all sessions hosts an agent currently `Working`.
    fn any_agent_working(&self) -> bool {
        self.layout
            .as_ref()
            .map(|l| l.sessions.iter().any(|s| s.tabs.iter().any(|t| node_has_working(t.root.as_deref()))))
            .unwrap_or(false)
    }

    fn spinner_char(&self) -> &'static str {
        SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()]
    }

    fn worktree_primary_status(&self, session_idx: usize) -> Option<AgentStatus> {
        let layout = self.layout.as_ref()?;
        let sess = layout.sessions.get(session_idx)?;
        let mut best: Option<(u8, AgentStatus)> = None;
        for (_, pane) in session_panes_all(sess) {
            if !pane.is_ai { continue; }
            if let Some(a) = pane.agent.as_ref() {
                let rank = Self::agent_rank(a.status);
                if best.map(|(r, _)| rank < r).unwrap_or(true) {
                    best = Some((rank, a.status));
                }
            }
        }
        best.map(|(_, s)| s)
    }

    fn panes_area(&self) -> Rect {
        let w = self.effective_sidebar_width();
        let x = if self.sidebar_open {
            (w + 1).min(self.cols.saturating_sub(1))
        } else {
            0
        };
        Rect::new(x, TAB_H, self.cols.saturating_sub(x), self.rows.saturating_sub(self.status_h() + TAB_H))
    }

    /// Lay the active tab's tree out over the pane area (wire tree → the
    /// crate's geometry, which tiles panes flush and yields exact totals).
    fn active_geom(&self) -> TreeGeom {
        let mut geom = TreeGeom::default();
        let area = self.panes_area();
        if let Some(tab) = self.active_tab() {
            if tab.zoom {
                geom.panes.push(PaneGeom { pane_id: tab.focus, rect: area });
            } else if let Some(root) = &tab.root {
                let node = wire_to_layout(root);
                layout::compute_geometry(&node, area, &mut geom);
            }
        }
        geom
    }

    /// Recompute geometry from the layout and sync subscriptions + sizes with
    /// the daemon (PaneResize drives the pane grids the daemon streams).
    fn recompute_geometry(&mut self) {
        let geom = self.active_geom();
        self.rects = geom.panes.into_iter().map(|p| (p.pane_id, p.rect)).collect();
        self.splitters = geom
            .splitters
            .into_iter()
            .map(|s| {
                let dir = match s.dir {
                    kumo_core::layout::SplitDir::V => SplitDir::Vertical,
                    kumo_core::layout::SplitDir::H => SplitDir::Horizontal,
                };
                (s.split_id, dir, s.area, s.rect)
            })
            .collect();
        let mut want: HashSet<u64> = HashSet::new();
        for (pid, rect) in self.rects.clone() {
            want.insert(pid);
            let inner = PaneGeom { pane_id: pid, rect }.inner();
            let dims = (inner.width.max(1), inner.height.max(1));
            if self.sent_sizes.get(&pid) != Some(&dims) {
                self.sent_sizes.insert(pid, dims);
                let _ = self.send(&Command::PaneResize { pane_id: pid, cols: dims.0, rows: dims.1 });
            }
            if self.subscribed.insert(pid) {
                let _ = self.send(&Command::SubscribePane { pane_id: pid });
            }
        }
        for pid in self.subscribed.difference(&want).copied().collect::<Vec<_>>() {
            self.subscribed.remove(&pid);
            let _ = self.send(&Command::UnsubscribePane { pane_id: pid });
            self.grids.remove(&pid);
            self.sel = None;
        }
        // If the pane we were copying left / was closed, leave copy-mode
        let copy_pid = self.copy.as_ref().map(|c| c.pane_id);
        if let Some(pid) = copy_pid {
            if !want.contains(&pid) {
                self.copy = None;
                self.mode = Mode::Normal;
            } else if let Some((w, h)) = self.copy_pane_dims(pid) {
                if let Some(copy) = self.copy.as_mut() {
                    copy.cursor.0 = copy.cursor.0.min(w.saturating_sub(1));
                    copy.cursor.1 = copy.cursor.1.min(h.saturating_sub(1));
                    if let Some(a) = copy.anchor.as_mut() {
                        a.0 = a.0.min(w.saturating_sub(1));
                        a.1 = a.1.min(h.saturating_sub(1));
                    }
                }
            }
        }
        self.update_tab_rects();
        self.mark_dirty();
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<(u64, Rect)> {
        self.rects.iter().copied().find(|(_, r)| r.contains(Position::new(x, y)))
    }

    fn splitter_at(&self, x: u16, y: u16) -> Option<(u64, SplitDir, Rect)> {
        self.splitters.iter().copied().find(|(_, _, _, sep)| sep.contains(Position::new(x, y))).map(|(id, dir, area, _)| (id, dir, area))
    }

    fn pane_toward(&self, focus: u64, dir: Dir) -> Option<u64> {
        let cur = self.rects.iter().find(|(pid, _)| *pid == focus)?;
        let (fx, fy, fw, fh) = (cur.1.x, cur.1.y, cur.1.width, cur.1.height);
        let mut best: Option<(u64, u32)> = None;
        for &(pid, r) in &self.rects {
            if pid == focus {
                continue;
            }
            let score = match dir {
                Dir::Left => (r.right() <= fx).then(|| {
                    fy.abs_diff(r.y).min((fy + fh).abs_diff(r.y + r.height)) as u32
                        + fx.saturating_sub(r.right()) as u32
                }),
                Dir::Right => (r.x >= fx + fw).then(|| {
                    fy.abs_diff(r.y).min((fy + fh).abs_diff(r.y + r.height)) as u32
                        + r.x.saturating_sub(fx + fw) as u32
                }),
                Dir::Up => (r.bottom() <= fy).then(|| {
                    fx.abs_diff(r.x).min((fx + fw).abs_diff(r.x + r.width)) as u32
                        + fy.saturating_sub(r.bottom()) as u32
                }),
                Dir::Down => (r.y >= fy + fh).then(|| {
                    fx.abs_diff(r.x).min((fx + fw).abs_diff(r.x + r.width)) as u32
                        + r.y.saturating_sub(fy + fh) as u32
                }),
            };
            if let Some(score) = score {
                if best.map(|(_, s)| score < s).unwrap_or(true) {
                    best = Some((pid, score));
                }
            }
        }
        best.map(|(pid, _)| pid)
    }

    fn cycle_pane(&self, focus: u64) -> Option<u64> {
        let ids: Vec<u64> = self.rects.iter().map(|(pid, _)| *pid).collect();
        if ids.len() < 2 {
            return None;
        }
        let idx = ids.iter().position(|p| *p == focus).unwrap_or(usize::MAX);
        Some(ids[(idx + 1) % ids.len()])
    }

    fn cycle_session(&self, delta: isize) -> Option<String> {
        let layout = self.layout.as_ref()?;
        let names: Vec<&String> = layout.sessions.iter().map(|s| &s.name).collect();
        if names.is_empty() {
            return None;
        }
        let idx = names
            .iter()
            .position(|n| Some(*n) == layout.active.as_ref())
            .unwrap_or(0);
        let next = ((idx as isize + delta).rem_euclid(names.len() as isize)) as usize;
        Some(names[next].clone())
    }

    fn session_at(&self, n: usize) -> Option<String> {
        let layout = self.layout.as_ref()?;
        layout.sessions.get(n.saturating_sub(1)).map(|s| s.name.clone())
    }

    fn cycle_tab(&self, delta: isize) -> Option<String> {
        let sess = self.active_session()?;
        if sess.tabs.is_empty() { return None; }
        let cur = sess.active_tab as isize;
        let next = ((cur + delta).rem_euclid(sess.tabs.len() as isize)) as usize;
        Some(sess.tabs[next].name.clone())
    }

    fn tab_at_index(&self, n: usize) -> Option<String> {
        let sess = self.active_session()?;
        sess.tabs.get(n.saturating_sub(1)).map(|t| t.name.clone())
    }

    /// Aggregate the AI state for a tab. The first value records whether the
    /// tab has an agent at all; the second is the actionable status to render.
    /// Idle and unknown agents keep the reserved slot blank. The precedence
    /// mirrors the attention order used by the Agent Inbox.
    fn tab_agent_state(&self, tab: &kumo_protocol::TabLayout) -> (bool, Option<AgentStatus>) {
        fn visit(node: &LayoutNode, has_agent: &mut bool, found: &mut Option<AgentStatus>) {
            if *found == Some(AgentStatus::Blocked) {
                return;
            }
            match node {
                LayoutNode::Pane(p) => {
                    if !p.is_ai {
                        return;
                    }
                    *has_agent = true;
                    let Some(agent) = p.agent.as_ref() else { return };
                    match agent.status {
                        AgentStatus::Blocked => *found = Some(AgentStatus::Blocked),
                        AgentStatus::Done => {
                            if !matches!(*found, Some(AgentStatus::Blocked | AgentStatus::Done)) {
                                *found = Some(AgentStatus::Done);
                            }
                        }
                        AgentStatus::Working => {
                            if found.is_none() {
                                *found = Some(AgentStatus::Working);
                            }
                        }
                        AgentStatus::Idle | AgentStatus::Unknown => {}
                    }
                }
                LayoutNode::Split { a, b, .. } => {
                    visit(a, has_agent, found);
                    visit(b, has_agent, found);
                }
            }
        }

        let mut has_agent = false;
        let mut found = None;
        if let Some(root) = tab.root.as_deref() {
            visit(root, &mut has_agent, &mut found);
        }
        (has_agent, found)
    }

    fn tab_agent_indicator(&self, tab: &kumo_protocol::TabLayout) -> Option<AgentStatus> {
        self.tab_agent_state(tab).1
    }

    fn tab_has_agent(&self, tab: &kumo_protocol::TabLayout) -> bool {
        self.tab_agent_state(tab).0
    }

    fn tab_agent_marker(&self, status: AgentStatus) -> &'static str {
        match status {
            AgentStatus::Blocked => "!",
            AgentStatus::Done => "✓",
            AgentStatus::Working => self.spinner_char(),
            AgentStatus::Idle | AgentStatus::Unknown => "",
        }
    }

    fn tabs_area(&self) -> Rect {
        let w = self.effective_sidebar_width();
        let x = if self.sidebar_open { (w + 1).min(self.cols.saturating_sub(1)) } else { 0 };
        Rect::new(x, 0, self.cols.saturating_sub(x), TAB_H)
    }

    fn tab_left_arrow_rect(&self) -> Option<Rect> {
        let area = self.tabs_area();
        if self.tab_scroll == 0 { return None; }
        Some(Rect::new(area.x, area.y, 1, 1))
    }
    fn tab_right_arrow_rect(&self) -> Option<Rect> {
        let area = self.tabs_area();
        let sess = self.active_session()?;
        let has_left = self.tab_scroll > 0;
        // Reserve for plus (3) + gap (1) and maybe right arrow
        let mut cur: u16 = 0;
        let plus_reserve: u16 = 4; // 3 for plus + 1 gap
        let base_avail = area.width.saturating_sub(if has_left {1} else {0});
        // First check without right arrow
        let mut visible_end = self.tab_scroll;
        let avail_no_arrow = base_avail.saturating_sub(plus_reserve);
        for idx in self.tab_scroll..sess.tabs.len() {
            let w = tab_width(&sess.tabs[idx].name, self.tab_has_agent(&sess.tabs[idx])) + 1;
            if cur + w - 1 > avail_no_arrow { break; }
            cur += w;
            visible_end = idx + 1;
        }
        if visible_end < sess.tabs.len() {
            return Some(Rect::new(area.x + area.width - 1, area.y, 1, 1));
        }
        // Check with right arrow reserved (if we would need it, avail reduces)
        // But if no overflow without arrow, no arrow needed
        None
    }

    fn ensure_tab_visible(&mut self) {
        let Some(sess) = self.active_session().cloned() else { return };
        if sess.tabs.is_empty() { return; }
        let area = self.tabs_area();
        let mut scroll = self.tab_scroll.min(sess.tabs.len().saturating_sub(1));
        if sess.active_tab < scroll {
            scroll = sess.active_tab;
        } else {
            let has_left = scroll > 0;
            // Reserve for plus (3+1) and maybe right arrow
            let plus_reserve: u16 = 4;
            let avail = area.width.saturating_sub(if has_left {1} else {0}).saturating_sub(plus_reserve).saturating_sub(1);
            let mut cur: u16 = 0;
            let mut visible_end = scroll;
            for idx in scroll..sess.tabs.len() {
                let w = tab_width(&sess.tabs[idx].name, self.tab_has_agent(&sess.tabs[idx])) + 1;
                if cur + w - 1 > avail { break; }
                cur += w;
                visible_end = idx + 1;
            }
            if sess.active_tab >= visible_end {
                for s in (0..=sess.active_tab).rev() {
                    let has_l = s > 0;
                    let av = area.width.saturating_sub(if has_l {1} else {0}).saturating_sub(plus_reserve).saturating_sub(1);
                    let mut c: u16 = 0;
                    let mut e = s;
                    for idx in s..sess.tabs.len() {
                        let w = tab_width(&sess.tabs[idx].name, self.tab_has_agent(&sess.tabs[idx])) + 1;
                        if c + w - 1 > av { break; }
                        c += w;
                        e = idx + 1;
                        if e > sess.active_tab { break; }
                    }
                    if s <= sess.active_tab && sess.active_tab < e {
                        scroll = s;
                        break;
                    }
                }
            }
        }
        self.tab_scroll = scroll;
    }

    fn update_tab_rects(&mut self) {
        self.tab_rects.clear();
        self.plus_rect = None;
        let Some(sess) = self.active_session().cloned() else { return };
        if sess.tabs.is_empty() {
            // Still show plus even with no tabs? single session has at least 1 tab, but handle
            let area = self.tabs_area();
            if area.width >= 3 {
                self.plus_rect = Some(Rect::new(area.x + if self.tab_scroll>0 {1} else {0}, area.y, 3, 1));
            }
            return;
        }
        let area = self.tabs_area();
        if area.width == 0 { return; }
        self.ensure_tab_visible();
        let has_left = self.tab_scroll > 0;
        let leading_drop_gap = self
            .tab_reorder_drag
            .as_ref()
            .map(|drag| drag.moved && !drag.after && drag.target_idx == self.tab_scroll)
            .unwrap_or(false);
        let mut cur_x = area.x + if has_left { 1 } else { 0 } + u16::from(leading_drop_gap);
        // Reserve for right arrow and plus
        let has_right = self.tab_right_arrow_rect().is_some();
        let plus_w: u16 = 3;
        let right_bound = (area.x as i32 + area.width as i32 - if has_right {1} else {0} - plus_w as i32 - 1).max(area.x as i32) as u16;
        for idx in self.tab_scroll..sess.tabs.len() {
            let tab = &sess.tabs[idx];
            let w = tab_width(&tab.name, self.tab_has_agent(tab));
            if cur_x + w > right_bound { break; }
            let pill = Rect::new(cur_x, area.y, w, 1);
            let close = Rect::new(cur_x + w - 1, area.y, 1, 1);
            self.tab_rects.push((idx, pill, close));
            cur_x += w + 1;
        }
        // Plus button after last visible tab
        let right_edge = (area.x as i32 + area.width as i32 - if has_right {1} else {0}).max(area.x as i32) as u16;
        if cur_x + plus_w <= right_edge {
            self.plus_rect = Some(Rect::new(cur_x, area.y, plus_w, 1));
        } else if self.tab_rects.is_empty() && area.width >= plus_w {
            // Fallback: show plus alone if no tabs fit
            self.plus_rect = Some(Rect::new(area.x + if has_left {1} else {0}, area.y, plus_w, 1));
        }
    }

    fn tab_hit(&self, x: u16, y: u16) -> Option<(usize, bool)> {
        if y != 0 { return None; }
        let area = self.tabs_area();
        if !area.contains(Position::new(x, y)) { return None; }
        // Check arrows first
        if let Some(r) = self.tab_left_arrow_rect() {
            if r.contains(Position::new(x, y)) { return None; } // handled separately
        }
        if let Some(r) = self.tab_right_arrow_rect() {
            if r.contains(Position::new(x, y)) { return None; }
        }
        for (idx, pill, close) in &self.tab_rects {
            if pill.contains(Position::new(x, y)) {
                // The close cell is always drawn, so it is clickable without a
                // prior hover.
                let is_close = close.contains(Position::new(x, y));
                return Some((*idx, is_close));
            }
        }
        None
    }

    fn begin_tab_reorder(&mut self, tab_idx: usize) -> bool {
        let Some((session, tab_id, tab_name)) = self.active_session().and_then(|session| {
            session
                .tabs
                .get(tab_idx)
                .map(|tab| (session.name.clone(), tab.id, tab.name.clone()))
        }) else {
            return false;
        };
        self.tab_reorder_drag = Some(TabReorderDrag {
            session,
            tab_id,
            tab_name,
            source_idx: tab_idx,
            target_idx: tab_idx,
            after: false,
            moved: false,
        });
        self.mark_dirty();
        true
    }

    fn update_tab_reorder(&mut self, x: u16) -> bool {
        if self.tab_reorder_drag.is_none() { return false; }
        let target = self
            .tab_rects
            .iter()
            .min_by_key(|(_, pill, _)| {
                let center = pill.x as i32 + pill.width as i32 / 2;
                (x as i32 - center).unsigned_abs()
            })
            .map(|(idx, pill, _)| (*idx, x >= pill.x + pill.width / 2));
        if let Some(drag) = self.tab_reorder_drag.as_mut() {
            drag.moved = true;
            if let Some((target_idx, after)) = target {
                drag.target_idx = target_idx;
                drag.after = after;
            }
        }
        self.update_tab_rects();
        self.mark_dirty();
        true
    }

    fn finish_tab_reorder(&mut self) -> bool {
        let Some(drag) = self.tab_reorder_drag.take() else { return false };
        if drag.moved {
            if let Some(to_index) = tab_reorder_index(drag.source_idx, drag.target_idx, drag.after) {
                let _ = self.send(&Command::TabMove {
                    session: drag.session,
                    tab_id: drag.tab_id,
                    to_index,
                });
            }
        } else {
            let _ = self.send(&Command::TabFocus { session: drag.session, tab: drag.tab_name });
        }
        self.update_tab_rects();
        self.mark_dirty();
        true
    }

    fn tab_reorder_indicator_x(&self) -> Option<u16> {
        let drag = self.tab_reorder_drag.as_ref().filter(|drag| drag.moved)?;
        let (_, pill, _) = self.tab_rects.iter().find(|(idx, _, _)| *idx == drag.target_idx)?;
        Some(if drag.after { pill.right() } else { pill.x.saturating_sub(1) })
    }

    // ------------------------------------------------------------------
    // Input: keys
    // ------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        self.flush_wheel()?;
        self.set_link_mods(key.modifiers.intersects(link_modifiers()));
        if self.session_close_confirm.open {
            self.on_session_close_confirm_key(key);
            return Ok(());
        }
        if self.worktree_create.open {
            self.on_worktree_create_key(key);
            return Ok(());
        }
        if self.popup.open {
            self.on_popup_key(key);
            return Ok(());
        }
        if self.menu.open {
            self.on_menu_key(key)?;
            return Ok(());
        }
        if self.ctx_menu.open {
            self.on_ctx_menu_key(key)?;
            return Ok(());
        }
        if self.keybind_overlay.open {
            self.on_overlay_key(key)?;
            return Ok(());
        }
        if self.settings.open {
            self.on_settings_key(key);
            return Ok(());
        }
        if self.worktree_picker.open {
            self.on_picker_key(key);
            return Ok(());
        }
        if self.pane_numbers.is_some() {
            self.on_pane_number_key(key);
            return Ok(());
        }
        if self.finder.open {
            self.on_finder_key(key);
            return Ok(());
        }
        if self.sidebar_filter_active {
            match key.code {
                KeyCode::Esc => { self.sidebar_filter.clear(); self.sidebar_filter_active = false; self.mark_dirty(); return Ok(()); }
                KeyCode::Backspace => { self.sidebar_filter.pop(); self.mark_dirty(); return Ok(()); }
                KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) => {
                    self.sidebar_filter.push(c); self.sidebar_scroll.0 = 0; self.mark_dirty(); return Ok(());
                }
                _ => return Ok(()),
            }
        }

        if self.mode == Mode::Copy {
            self.on_copy_key(key)?;
            return Ok(());
        }
        if self.mode == Mode::Inbox {
            self.on_inbox_key(key);
            return Ok(());
        }

        let leader = self.leader.is_leader(key);
        match self.mode {
            Mode::Normal => {
                if leader {
                    self.mode = Mode::Leader;
                    self.mark_dirty();
                    return Ok(());
                }
                let wire: kumo_protocol::WireKeyEvent = key.into();
                self.send(&Command::Input { key: wire })?;
            }
            Mode::Leader => {
                if leader || key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                    self.mark_dirty();
                    return Ok(());
                }
                self.leader_command(key)?;
            }
            Mode::Copy => unreachable!("handled above"),
            Mode::Inbox => unreachable!("handled above"),
        }
        Ok(())
    }

    pub fn on_paste(&mut self, text: &str) {
        let _ = self.flush_wheel();
        if self.session_close_confirm.open {
            return;
        }
        if self.worktree_create.open {
            self.worktree_create_paste(text);
            return;
        }
        if self.popup.open
            || self.menu.open
            || self.ctx_menu.open
            || self.keybind_overlay.open
            || self.settings.open
            || self.worktree_picker.open
            || self.finder.open
            || self.pane_numbers.is_some()
            || self.mode == Mode::Leader
            || self.mode == Mode::Copy
            || self.mode == Mode::Inbox
        {
            return;
        }
        let _ = self.send(&Command::Paste { text: text.to_string() });
    }

    fn set_link_mods(&mut self, held: bool) {
        if self.link_mods != held {
            self.link_mods = held;
            self.mark_dirty();
        }
    }

    fn leader_command(&mut self, key: KeyEvent) -> Result<()> {
        self.mode = Mode::Normal;
        let chord = Chord::new(key.code, key.modifiers);
        if let Some(binding) = self.keymap.iter().find(|b| b.key == chord) {
            self.run_action(binding.action)?;
        } else {
            self.mark_dirty();
        }
        Ok(())
    }

    fn run_action(&mut self, action: Action) -> Result<()> {
        if action == Action::OpenSidebarFilter {
            if self.sidebar_layout() == SidebarLayout::Project {
                self.sidebar_open = true;
                self.sidebar_filter_active = true;
                self.sidebar_filter.clear();
                self.recompute_geometry();
                self.mark_dirty();
            }
            return Ok(());
        }
        let Some(session) = self.active_session().map(|s| s.name.clone()) else {
            self.notice = Some(("no active session".to_string(), Instant::now()));
            self.mark_dirty();
            return Ok(());
        };
        match action {
            Action::SplitVertical => {
                let _ = self.send(&Command::PaneSplit { session, dir: SplitDir::Vertical, is_ai: false });
            }
            Action::SplitHorizontal => {
                let _ = self.send(&Command::PaneSplit { session, dir: SplitDir::Horizontal, is_ai: false });
            }
            Action::SplitAi => {
                let _ = self.send(&Command::PaneSplit { session, dir: SplitDir::Vertical, is_ai: true });
            }
            Action::NewSession => self.open_session_popup(),
            Action::NewWorktree => {
                let idx = self.session_index(&session).unwrap_or(0);
                self.open_worktree_popup(idx);
            }
            Action::ClosePane => {
                let _ = self.send(&Command::PaneClose { session, pane_id: None });
            }
            Action::Zoom => {
                let _ = self.send(&Command::SessionZoom { session });
            }
            Action::Focus(dir) => {
                let focus = self.active_tab().map(|t| t.focus).unwrap_or(0);
                if let Some(pid) = self.pane_toward(focus, dir) {
                    let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                }
            }
            Action::Resize(dir) => {
                let dir = match dir {
                    kumo_core::layout::ResizeDir::Left => kumo_protocol::ResizeDir::Left,
                    kumo_core::layout::ResizeDir::Down => kumo_protocol::ResizeDir::Down,
                    kumo_core::layout::ResizeDir::Up => kumo_protocol::ResizeDir::Up,
                    kumo_core::layout::ResizeDir::Right => kumo_protocol::ResizeDir::Right,
                };
                let _ = self.send(&Command::PaneResizeRatio { session, dir });
            }
            Action::CyclePane => {
                let focus = self.active_tab().map(|t| t.focus).unwrap_or(0);
                if let Some(pid) = self.cycle_pane(focus) {
                    let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                }
            }
            Action::SwapPanes => {
                let _ = self.send(&Command::PaneSwap { session });
            }
            Action::RotateLayout => {
                let _ = self.send(&Command::LayoutRotate { session });
            }
            Action::ShowPaneNumbers => {
                self.pane_numbers = Some(Instant::now());
                self.mark_dirty();
            }
            Action::NextTab => {
                if let Some(name) = self.cycle_tab(1) {
                    let _ = self.send(&Command::TabFocus { session, tab: name });
                }
            }
            Action::PrevTab => {
                if let Some(name) = self.cycle_tab(-1) {
                    let _ = self.send(&Command::TabFocus { session, tab: name });
                }
            }
            Action::JumpTab(n) => {
                if let Some(name) = self.tab_at_index(n as usize) {
                    let _ = self.send(&Command::TabFocus { session, tab: name });
                }
            }
            Action::NewTab => {
                let _ = self.send(&Command::TabNew { session, name: None, workspace: None });
            }
            Action::CloseTab => {
                if let Some(idx) = self.tab_hover {
                    if let Some(sess) = self.active_session() {
                        if let Some(tab) = sess.tabs.get(idx) {
                            let _ = self.send(&Command::TabClose { session: session.clone(), tab: Some(tab.name.clone()) });
                            self.mark_dirty();
                            return Ok(());
                        }
                    }
                }
                let _ = self.send(&Command::TabClose { session, tab: None });
            }
            Action::RenameTab => {
                if let Some(idx) = self.tab_hover {
                    if let Some(s_idx) = self.layout.as_ref().and_then(|l| l.sessions.iter().position(|s| s.name == session)) {
                        self.open_rename_tab_popup_for(s_idx, idx);
                        self.mark_dirty();
                        return Ok(());
                    }
                }
                self.open_rename_tab_popup();
            }
            Action::NextSession => {
                if let Some(name) = self.cycle_session(1) {
                    let _ = self.send(&Command::SessionFocus { name });
                }
            }
            Action::PrevSession => {
                if let Some(name) = self.cycle_session(-1) {
                    let _ = self.send(&Command::SessionFocus { name });
                }
            }
            Action::JumpSession(n) => {
                if let Some(name) = self.session_at(n as usize) {
                    let _ = self.send(&Command::SessionFocus { name });
                }
            }
            Action::ToggleSidebar => {
                self.sidebar_open = !self.sidebar_open;
                self.recompute_geometry();
            }
            Action::Detach => {
                let _ = self.send(&Command::Detach);
                self.detach_requested = true;
            }
            Action::ShowKeybinds => self.open_keybind_overlay(),
            Action::EnterCopyMode => self.enter_copy_mode(),
            Action::EnterCopyModeSearch => self.enter_copy_mode_with_search(true),
            Action::OpenSidebarFilter => unreachable!("sidebar filter handled before session lookup"),
            Action::AgentInbox => self.open_inbox(),
            Action::WorkspaceFinder => self.open_finder(),
            Action::OpenSettings => self.open_settings(),
            Action::RenameSession => {
                if let Some(idx) = self.session_index(&session) {
                    self.open_rename_session_popup(idx);
                }
            }
            Action::CloseSession => {
                let _ = self.send(&Command::SessionKill { name: session });
            }
        }
        self.mark_dirty();
        Ok(())
    }

    // ------------------------------------------------------------------
    // Agent inbox: keyboard focus of the sidebar agent panel
    // ------------------------------------------------------------------

    /// Every agent in attention order for the full inbox popup.
    fn inbox_entries(&self) -> Vec<(usize, u64, AgentStatus)> {
        self.all_agent_entries()
            .into_iter()
            .map(|(_, i, pid, status, _)| (i, pid, status))
            .collect()
    }

    /// Open the full agent inbox popup. It is independent of sidebar layout
    /// and remains useful when there are no alerts or the sidebar is hidden.
    fn open_inbox(&mut self) {
        if self.cols < 28 || self.rows < 11 {
            self.notice = Some(("terminal too small for the full agent inbox".to_string(), Instant::now()));
            self.mark_dirty();
            return;
        }
        self.mode = Mode::Inbox;
        self.inbox = Some(Inbox { sel: 0 });
        self.mark_dirty();
    }

    fn close_inbox(&mut self) {
        self.mode = Mode::Normal;
        self.inbox = None;
        self.mark_dirty();
    }

    /// Whether `(session_idx, pane_id)` is the selected inbox entry.
    fn inbox_selected(&self, si: usize, pid: u64) -> bool {
        if self.mode != Mode::Inbox {
            return false;
        }
        let sel = self.inbox.as_ref().map(|i| i.sel).unwrap_or(0);
        self.inbox_entries()
            .get(sel)
            .map(|(s, p, _)| *s == si && *p == pid)
            .unwrap_or(false)
    }

    /// Focus the pane behind the selected inbox entry (switching session/tab)
    /// and leave the focus mode.
    fn inbox_jump(&mut self) {
        let sel = self.inbox.as_ref().map(|i| i.sel).unwrap_or(0);
        let Some(&(si, pid, _)) = self.inbox_entries().get(sel) else {
            self.close_inbox();
            return;
        };
        let session = self.layout.as_ref().and_then(|l| l.sessions.get(si)).map(|s| s.name.clone());
        if let Some(session) = session {
            let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
        }
        self.close_inbox();
    }

    fn on_inbox_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
            self.close_inbox();
            return;
        }
        let len = self.inbox_entries().len();
        if len == 0 {
            return;
        }
        let page = self.agent_inbox_visible_rows().max(2) - 1;
        let cur = self.inbox.as_ref().map(|i| i.sel).unwrap_or(0).min(len - 1);
        let next = match key.code {
            KeyCode::Char('j') | KeyCode::Down => (cur + 1).min(len - 1),
            KeyCode::Char('k') | KeyCode::Up => cur.saturating_sub(1),
            KeyCode::PageDown => (cur + page).min(len - 1),
            KeyCode::PageUp => cur.saturating_sub(page),
            KeyCode::Home => 0,
            KeyCode::End => len - 1,
            KeyCode::Enter | KeyCode::Char('\n') => {
                self.inbox_jump();
                return;
            }
            _ => cur,
        };
        if let Some(inb) = self.inbox.as_mut() {
            inb.sel = next;
        }
        self.mark_dirty();
    }

    fn on_inbox_mouse(&mut self, m: MouseEvent) -> Result<()> {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = self.agent_inbox_item_at(m.column, m.row) {
                    if let Some(inbox) = self.inbox.as_mut() {
                        inbox.sel = idx;
                    }
                    self.inbox_jump();
                    return Ok(());
                }
                if !self
                    .agent_inbox_rect()
                    .map(|rect| rect.contains(Position::new(m.column, m.row)))
                    .unwrap_or(false)
                {
                    self.close_inbox();
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let len = self.inbox_entries().len();
                if len > 0 {
                    let cur = self.inbox.as_ref().map(|inbox| inbox.sel).unwrap_or(0).min(len - 1);
                    let next = if m.kind == MouseEventKind::ScrollUp {
                        cur.saturating_sub(1)
                    } else {
                        (cur + 1).min(len - 1)
                    };
                    if let Some(inbox) = self.inbox.as_mut() {
                        inbox.sel = next;
                    }
                    self.mark_dirty();
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Workspace finder (`leader+f`)
    // ------------------------------------------------------------------

    fn open_finder(&mut self) {
        if self.sidebar_layout() != SidebarLayout::Project {
            self.notice = Some(("finder available only in project sidebar (set [sidebar] layout = \"project\")".to_string(), Instant::now()));
            self.mark_dirty();
            return;
        }
        let mut items = Vec::new();
        if let Some(layout) = &self.layout {
            for (i, s) in layout.sessions.iter().enumerate() {
                items.push(FinderItem::Session { idx: i, name: s.name.clone() });
                for (ti, tab) in s.tabs.iter().enumerate() {
                    items.push(FinderItem::Tab { session_idx: i, tab_idx: ti, session_name: s.name.clone(), tab_name: tab.name.clone() });
                }
            }
        }
        if items.is_empty() {
            self.notice = Some(("no workspaces to find".to_string(), Instant::now()));
            self.mark_dirty();
            return;
        }
        self.finder.items = items;
        self.finder.input.clear();
        self.finder.cursor = 0;
        self.finder.selected = 0;
        self.finder.scroll = 0;
        self.finder.filtered = (0..self.finder.items.len()).collect();
        self.finder.open = true;
        self.mark_dirty();
    }

    fn close_finder(&mut self) {
        self.finder.open = false;
        self.mark_dirty();
    }

    fn finder_rebuild(&mut self) {
        let q = self.finder.input.to_ascii_lowercase();
        if q.is_empty() {
            self.finder.filtered = (0..self.finder.items.len()).collect();
        } else {
            let mut scored: Vec<(usize, u8)> = Vec::new();
            for (idx, item) in self.finder.items.iter().enumerate() {
                let hay = match item {
                    FinderItem::Session { name, .. } => name.clone(),
                    FinderItem::Tab { session_name, tab_name, .. } => format!("{session_name} {tab_name} {session_name}/{tab_name}"),
                };
                let hay_l = hay.to_ascii_lowercase();
                if hay_l.contains(&q) {
                    let rank = if hay_l.starts_with(&q) { 0 } else if hay_l.contains(&format!(" {q}")) { 1 } else { 2 };
                    scored.push((idx, rank));
                }
            }
            scored.sort_by_key(|(_, r)| *r);
            self.finder.filtered = scored.into_iter().map(|(i,_)| i).collect();
        }
        self.finder.selected = 0;
        self.finder.scroll = 0;
    }

    fn finder_commit(&mut self) {
        let fidx = self.finder.filtered.get(self.finder.selected).copied();
        let Some(fi) = fidx.and_then(|i| self.finder.items.get(i).cloned()) else {
            self.close_finder(); return;
        };
        self.close_finder();
        match fi {
            FinderItem::Session { name, .. } => { let _ = self.send(&Command::SessionFocus { name }); }
            FinderItem::Tab { session_name, tab_name, .. } => { let _ = self.send(&Command::TabFocus { session: session_name, tab: tab_name }); }
        }
    }

    fn on_finder_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.close_finder(); return;
        }
        match key.code {
            KeyCode::Enter => { self.finder_commit(); }
            KeyCode::Up | KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.finder.selected > 0 { self.finder.selected -= 1; if self.finder.selected < self.finder.scroll { self.finder.scroll = self.finder.selected; } self.mark_dirty(); }
            }
            KeyCode::Down | KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.finder.selected + 1 < self.finder.filtered.len() { self.finder.selected += 1; let max_vis = 8usize; if self.finder.selected >= self.finder.scroll + max_vis { self.finder.scroll += 1; } self.mark_dirty(); }
            }
            _ => match TextInput::new(&mut self.finder.input, &mut self.finder.cursor).handle_key(key) {
                TextEdit::Changed => {
                    self.finder_rebuild();
                    self.mark_dirty();
                }
                TextEdit::Moved => self.mark_dirty(),
                TextEdit::Ignored => {}
            },
        }
    }

    fn finder_rect(&self) -> Option<Rect> {
        if !self.finder.open { return None; }
        let w = 48.min(self.cols.saturating_sub(4)).max(30);
        let h = 12.min(self.rows.saturating_sub(4)).max(8);
        Some(Rect::new((self.cols - w)/2, (self.rows - h)/2, w, h))
    }

    #[allow(dead_code)]
    fn finder_input_at(&self, x: u16, y: u16) -> bool {
        self.finder_rect().map(|r| y==r.y+1 && x>r.x && x<r.x+r.width-1).unwrap_or(false)
    }

    fn finder_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let r = self.finder_rect()?;
        if y <= r.y+2 || y >= r.y+r.height-1 { return None; }
        if x < r.x+1 || x >= r.x+r.width-1 { return None; }
        let idx = (y - (r.y+3)) as usize + self.finder.scroll;
        if idx < self.finder.filtered.len() { Some(idx) } else { None }
    }

    fn session_index(&self, name: &str) -> Option<usize> {
        self.layout.as_ref()?.sessions.iter().position(|s| s.name == name)
    }

    fn on_pane_number_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                if let Some(&(pid, _)) = self.rects.get(n.saturating_sub(1)) {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                }
            }
            _ => {}
        }
        self.pane_numbers = None;
        self.mark_dirty();
    }

    // ------------------------------------------------------------------
    // Overlays (popup / menu / ctx / keybind / settings / picker)
    // ------------------------------------------------------------------

    fn open_session_popup(&mut self) {
        let name = self.next_session_name();
        self.popup.name = name.clone();
        self.popup.cursor = name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::NewSession);
        self.popup.open = true;
        self.menu.open = false;
        self.mark_dirty();
    }

    fn next_session_name(&self) -> String {
        let names: Vec<String> = self
            .layout
            .as_ref()
            .map(|l| l.sessions.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        let mut n = 1;
        loop {
            let cand = format!("session-{n}");
            if !names.contains(&cand) {
                return cand;
            }
            n += 1;
        }
    }

    fn open_rename_popup(&mut self, pid: u64) {
        let name = self.pane_label(pid);
        self.popup.name = name.trim().to_string();
        self.popup.cursor = self.popup.name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::RenamePane(pid));
        self.popup.open = true;
        self.ctx_menu.open = false;
        self.mark_dirty();
    }

    fn open_settings(&mut self) {
        self.settings.open = true;
        self.settings.tab = 0;
        self.settings.selected = self.theme_idx;
        self.settings.skill_feedback = None;
        self.mark_dirty();
    }

    fn open_rename_session_popup(&mut self, idx: usize) {
        let name = self
            .layout
            .as_ref()
            .and_then(|l| l.sessions.get(idx))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        self.popup.name = name.clone();
        self.popup.cursor = name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::RenameSession(idx));
        self.popup.open = true;
        self.ctx_menu.open = false;
        self.mark_dirty();
    }

    fn open_rename_tab_popup(&mut self) {
        let (s_idx, t_idx) = self.active_session().map(|s| {
            let s_idx = self.layout.as_ref().and_then(|l| l.sessions.iter().position(|x| x.name==s.name)).unwrap_or(0);
            (s_idx, s.active_tab)
        }).unwrap_or((0,0));
        self.open_rename_tab_popup_for(s_idx, t_idx);
    }
    fn open_rename_tab_popup_for(&mut self, s_idx: usize, t_idx: usize) {
        let name = self.layout.as_ref().and_then(|l| l.sessions.get(s_idx)).and_then(|s| s.tabs.get(t_idx)).map(|t| t.name.clone()).unwrap_or_default();
        self.popup.name = name.clone();
        self.popup.cursor = name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::RenameTab { session: s_idx, tab: t_idx });
        self.popup.open = true;
        self.ctx_menu.open = false;
        self.mark_dirty();
    }

    fn open_worktree_popup(&mut self, idx: usize) {
        self.ctx_menu.open = false;
        let ws = self
            .layout
            .as_ref()
            .and_then(|l| l.sessions.get(idx))
            .map(|s| s.workspace.clone());
        if ws.as_deref().and_then(kumo_core::worktrees::repo_root).is_none() {
            self.notice = Some((
                format!("{}: not a git repository", ws.map(|w| w.display().to_string()).unwrap_or_default()),
                Instant::now(),
            ));
            self.mark_dirty();
            return;
        }
        self.worktree_create.open = true;
        self.worktree_create.session = idx;
        self.worktree_create.tab = WorktreeCreateTab::Smart;
        self.worktree_create.create_from = String::new();
        self.worktree_create.cursor = 0;
        self.worktree_create.branch_override = String::new();
        self.worktree_create.branch_cursor = 0;
        self.worktree_create.note = String::new();
        self.worktree_create.note_cursor = 0;
        self.worktree_create.agent = String::new();
        self.worktree_create.agent_cursor = 0;
        self.worktree_create.advanced = false;
        self.worktree_create.focus = WorktreeCreateFocus::CreateFrom;
        self.worktree_create.error = None;
        self.mark_dirty();
    }

    fn open_worktree_picker(&mut self, idx: usize) {
        self.ctx_menu.open = false;
        self.worktree_picker.session = idx;
        self.worktree_picker.items = Vec::new();
        self.worktree_picker.selected = 0;
        self.worktree_picker.scroll = 0;
        self.worktree_picker.error = None;
        let session = self
            .layout
            .as_ref()
            .and_then(|l| l.sessions.get(idx))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let _ = self.send(&Command::WorktreeList { session });
        self.worktree_picker.open = true;
        self.mark_dirty();
    }

    fn pick_worktree(&mut self, idx: usize) {
        self.worktree_picker.open = false;
        let Some(row) = self.worktree_picker.items.get(idx) else { return };
        let session = self
            .layout
            .as_ref()
            .and_then(|l| l.sessions.get(self.worktree_picker.session))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let path = row.path.clone();
        let _ = self.send(&Command::WorktreeOpen { session, path });
        self.mark_dirty();
    }

    fn worktree_picker_move(&mut self, delta: isize) {
        let n = self.worktree_picker.items.len();
        if n == 0 {
            return;
        }
        let idx = (self.worktree_picker.selected as isize + delta).rem_euclid(n as isize) as usize;
        self.worktree_picker.selected = idx;
        self.worktree_picker_keep_visible();
        self.mark_dirty();
    }

    fn worktree_picker_keep_visible(&mut self) {
        let visible = self.worktree_picker_visible_rows();
        let sel = self.worktree_picker.selected as u16;
        if sel < self.worktree_picker.scroll {
            self.worktree_picker.scroll = sel;
        } else if sel >= self.worktree_picker.scroll + visible {
            self.worktree_picker.scroll = sel - visible + 1;
        }
    }

    fn worktree_picker_visible_rows(&self) -> u16 {
        self.worktree_picker_rect().map(|r| r.height.saturating_sub(5)).unwrap_or(0)
    }

    fn worktree_picker_rect(&self) -> Option<Rect> {
        if !self.worktree_picker.open {
            return None;
        }
        let (w, h) = (self.cols, self.rows);
        let width = 72u16.min(w.saturating_sub(4)).max(24);
        let height = (self.worktree_picker.items.len() as u16 + 5)
            .min(h.saturating_sub(4))
            .max(6);
        if w < width || h < height {
            return None;
        }
        Some(Rect::new((w - width) / 2, (h - height) / 2, width, height))
    }

    fn worktree_picker_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.worktree_picker_rect()?;
        let body_top = dd.y + 3;
        let end = dd.bottom().saturating_sub(2);
        if x < dd.x + 1 || x >= dd.right().saturating_sub(1) || y < body_top || y >= end {
            return None;
        }
        let idx = self.worktree_picker.scroll as usize + (y - body_top) as usize;
        (idx < self.worktree_picker.items.len()).then_some(idx)
    }

    fn worktree_create_rect(&self) -> Option<Rect> {
        if !self.worktree_create.open {
            return None;
        }
        let (w, h) = (self.cols, self.rows);
        let width = 72u16.min(w.saturating_sub(4)).max(44);
        let height = if self.worktree_create.advanced { 16 } else { 11 };
        let height = height.min(h.saturating_sub(4)).max(10);
        if w < width || h < height {
            return None;
        }
        Some(Rect::new((w - width) / 2, (h - height) / 2, width, height))
    }

    fn worktree_create_tab_at(&self, x: u16, y: u16) -> Option<WorktreeCreateTab> {
        let dd = self.worktree_create_rect()?;
        let tabs_y = dd.y + 2;
        if y != tabs_y {
            return None;
        }
        // Tabs row from x+2, each tab approx label len + 2
        let mut cur = dd.x + 2;
        for tab in [
            WorktreeCreateTab::Smart,
            WorktreeCreateTab::Github,
            WorktreeCreateTab::Branch,
            WorktreeCreateTab::Name,
        ] {
            let label = tab.label();
            let w = (label.chars().count() as u16) + 2; // padded
            let rect = Rect::new(cur, tabs_y, w, 1);
            if rect.contains(Position::new(x, y)) {
                return Some(tab);
            }
            cur += w + 1;
        }
        None
    }

    fn worktree_create_input_rect(&self, focus: WorktreeCreateFocus) -> Option<Rect> {
        let dd = self.worktree_create_rect()?;
        let inner_w = dd.width.saturating_sub(4);
        let input_w = inner_w;
        let (x, y) = match focus {
            WorktreeCreateFocus::CreateFrom => (dd.x + 2, dd.y + 4),
            WorktreeCreateFocus::BranchOverride => (dd.x + 2, dd.y + 8),
            WorktreeCreateFocus::Note => (dd.x + 2, dd.y + 10),
            WorktreeCreateFocus::Agent => (dd.x + 2, dd.y + 12),
        };
        // When not advanced, only CreateFrom is visible
        if !self.worktree_create.advanced && focus != WorktreeCreateFocus::CreateFrom {
            return None;
        }
        // When advanced closed, the y for other inputs is off-screen
        if y >= dd.bottom() {
            return None;
        }
        Some(Rect::new(x, y, input_w, 1))
    }

    fn worktree_create_advanced_toggle_rect(&self) -> Option<Rect> {
        let dd = self.worktree_create_rect()?;
        let y = dd.y + 6;
        if y >= dd.bottom() {
            return None;
        }
        Some(Rect::new(dd.x + 2, y, 12, 1))
    }

    fn worktree_create_button_rect(&self, is_create: bool) -> Option<Rect> {
        let dd = self.worktree_create_rect()?;
        let y = dd.bottom().saturating_sub(2);
        let create_label = " create ";
        let cancel_label = " cancel ";
        let create_w = create_label.chars().count() as u16 + 2;
        let cancel_w = cancel_label.chars().count() as u16 + 2;
        let total_w = create_w + 1 + cancel_w;
        let start_x = dd.x + dd.width.saturating_sub(total_w + 2);
        if is_create {
            Some(Rect::new(start_x, y, create_w, 1))
        } else {
            Some(Rect::new(start_x + create_w + 1, y, cancel_w, 1))
        }
    }

    fn session_close_confirm_rect(&self) -> Option<Rect> {
        if !self.session_close_confirm.open {
            return None;
        }
        let (w, h) = (self.cols, self.rows);
        let width = 60u16.min(w.saturating_sub(4)).max(30);
        let height = 7u16;
        if w < width || h < height {
            return None;
        }
        Some(Rect::new((w - width) / 2, (h - height) / 2, width, height))
    }

    fn session_close_confirm_button_rect(&self, is_yes: bool) -> Option<Rect> {
        let dd = self.session_close_confirm_rect()?;
        let y = dd.bottom().saturating_sub(2);
        let yes_label = " Remove (y) ";
        let no_label = " Cancel (n) ";
        let yes_w = yes_label.chars().count() as u16 + 2;
        let no_w = no_label.chars().count() as u16 + 2;
        let total_w = yes_w + 1 + no_w;
        let start_x = dd.x + dd.width.saturating_sub(total_w + 2);
        if is_yes {
            Some(Rect::new(start_x, y, yes_w, 1))
        } else {
            Some(Rect::new(start_x + yes_w + 1, y, no_w, 1))
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.worktree_picker.open = false;
            self.mark_dirty();
            return;
        }
        if self.worktree_picker.items.is_empty() {
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.worktree_picker_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.worktree_picker_move(-1),
            KeyCode::Enter => self.pick_worktree(self.worktree_picker.selected),
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Char('x') | KeyCode::Char('X') => self.clear_worktree_checkpoint(),
            KeyCode::Delete | KeyCode::Backspace => self.clear_worktree_checkpoint(),
            _ => {}
        }
    }

    fn clear_worktree_checkpoint(&mut self) {
        if self.worktree_picker.items.is_empty() {
            return;
        }
        let idx = self.worktree_picker.selected;
        let Some(row) = self.worktree_picker.items.get(idx).cloned() else { return };
        if row.comment.is_none() && row.status.is_none() {
            return;
        }
        let session = self
            .layout
            .as_ref()
            .and_then(|l| l.sessions.get(self.worktree_picker.session))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let path = row.path.clone();
        let _ = self.send(&Command::WorktreeSet {
            session: session.clone(),
            path: path.clone(),
            comment: Some(String::new()),
            status: Some(String::new()),
        });
        if let Some(item) = self.worktree_picker.items.get_mut(idx) {
            item.comment = None;
            item.status = None;
        }
        self.mark_dirty();
        let _ = self.send(&Command::WorktreeList { session });
    }

    fn on_popup_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.popup.open = false;
            self.mark_dirty();
            return;
        }
        if key.code == KeyCode::Enter {
            self.commit_name();
        } else {
            TextInput::new(&mut self.popup.name, &mut self.popup.cursor).handle_key(key);
        }
        self.mark_dirty();
    }

    fn on_worktree_create_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.worktree_create.open = false;
            self.mark_dirty();
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => self.commit_worktree_create(),
            KeyCode::Tab => {
                // Tab cycles focus when advanced open; otherwise no-op
                if self.worktree_create.advanced {
                    self.worktree_create_next_focus(1);
                }
            }
            KeyCode::BackTab => {
                if self.worktree_create.advanced {
                    self.worktree_create_next_focus(-1);
                }
            }
            KeyCode::Char('a') if ctrl => {
                self.worktree_create.advanced = !self.worktree_create.advanced;
                if !self.worktree_create.advanced {
                    self.worktree_create.focus = WorktreeCreateFocus::CreateFrom;
                }
                self.mark_dirty();
            }
            _ => {
                let (text, cursor) = self.worktree_create_focused_mut();
                TextInput::new(text, cursor).handle_key(key);
            }
        }
        self.mark_dirty();
    }

    fn worktree_create_focused_mut(&mut self) -> (&mut String, &mut usize) {
        match self.worktree_create.focus {
            WorktreeCreateFocus::CreateFrom => (&mut self.worktree_create.create_from, &mut self.worktree_create.cursor),
            WorktreeCreateFocus::BranchOverride => (&mut self.worktree_create.branch_override, &mut self.worktree_create.branch_cursor),
            WorktreeCreateFocus::Note => (&mut self.worktree_create.note, &mut self.worktree_create.note_cursor),
            WorktreeCreateFocus::Agent => (&mut self.worktree_create.agent, &mut self.worktree_create.agent_cursor),
        }
    }

    fn worktree_create_paste(&mut self, text: &str) {
        // Paste sanitizes newlines to spaces for these fields
        let clean = text.replace(['\n', '\r'], " ");
        let (s, cursor) = self.worktree_create_focused_mut();
        TextInput::new(s, cursor).insert_str(&clean);
        self.mark_dirty();
    }

    fn worktree_create_next_focus(&mut self, delta: isize) {
        if !self.worktree_create.advanced {
            return;
        }
        let order = [
            WorktreeCreateFocus::CreateFrom,
            WorktreeCreateFocus::BranchOverride,
            WorktreeCreateFocus::Note,
            WorktreeCreateFocus::Agent,
        ];
        let idx = order.iter().position(|f| *f == self.worktree_create.focus).unwrap_or(0) as isize;
        let next = (idx + delta).rem_euclid(order.len() as isize) as usize;
        self.worktree_create.focus = order[next];
    }

    fn commit_worktree_create(&mut self) {
        let cf = self.worktree_create.create_from.trim().to_string();
        if cf.is_empty() {
            self.worktree_create.error = Some("Create from cannot be empty".to_string());
            self.mark_dirty();
            return;
        }
        let session = self.layout.as_ref().and_then(|l| l.sessions.get(self.worktree_create.session)).map(|s| s.name.clone()).unwrap_or_default();
        let branch_override = self.worktree_create.branch_override.trim().to_string();
        let branch = if branch_override.is_empty() { String::new() } else { branch_override };
        let note = self.worktree_create.note.trim().to_string();
        let note_opt = if note.is_empty() { None } else { Some(note) };
        let agent = self.worktree_create.agent.trim().to_string();
        let agent_opt = if agent.is_empty() { None } else { Some(agent) };
        // Determine name/from based on tab and content
        let (name_opt, from_opt) = match self.worktree_create.tab {
            WorktreeCreateTab::Smart => {
                if kumo_core::worktrees::parse_pr_number(&cf).is_some() || cf.contains("://") {
                    (None, Some(cf.clone()))
                } else {
                    (Some(cf.clone()), None)
                }
            }
            WorktreeCreateTab::Github => (None, Some(cf.clone())),
            WorktreeCreateTab::Jira => (None, Some(cf.clone())),
            WorktreeCreateTab::Branch => (None, Some(cf.clone())),
            WorktreeCreateTab::Name => (Some(cf.clone()), None),
        };
        // Validate branch override early (daemon will also validate)
        if !branch.is_empty() {
            if let Err(e) = kumo_core::worktrees::validate_branch_name(&branch) {
                self.worktree_create.error = Some(e);
                self.mark_dirty();
                return;
            }
        }
        self.worktree_create.open = false;
        let is_ai = true;
        let _ = self.send(&Command::WorktreeCreate { session, branch, from: from_opt, note: note_opt, agent: agent_opt, is_ai, name: name_opt });
        self.mark_dirty();
    }

    fn on_session_close_confirm_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            // Cancel — don't close session at all
            self.session_close_confirm.open = false;
            self.mark_dirty();
            return;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.submit_worktree_removal();
                self.mark_dirty();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.session_close_confirm.open = false;
                self.mark_dirty();
            }
            KeyCode::Enter => {
                self.submit_worktree_removal();
                self.mark_dirty();
            }
            _ => {}
        }
    }

    fn submit_worktree_removal(&mut self) {
        let path = self.session_close_confirm.path.clone();
        let session = self.session_close_confirm.session_name.clone();
        let force = self.session_close_confirm.mode == WorktreeRemovalMode::Force;
        self.session_close_confirm.open = false;
        let _ = self.send(&Command::WorktreeRemove { session, path, force });
    }

    fn commit_name(&mut self) {
        let name = self.popup.name.trim().to_string();
        if name.is_empty() {
            self.popup.error = Some("name cannot be empty".to_string());
            self.mark_dirty();
            return;
        }
        match self.popup.target {
            Some(PopupTarget::NewSession) => {
                if self.layout.as_ref().map(|l| l.sessions.iter().any(|s| s.name == name)).unwrap_or(false) {
                    self.popup.error = Some(format!("a session named '{name}' already exists"));
                    self.mark_dirty();
                    return;
                }
                self.popup.open = false;
                let _ = self.send(&Command::SessionNew { name: Some(name), workspace: None });
            }
            Some(PopupTarget::NewWorktree(idx)) => {
                let session = self
                    .layout
                    .as_ref()
                    .and_then(|l| l.sessions.get(idx))
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                self.popup.open = false;
                let _ = self.send(&Command::WorktreeCreate { session, branch: name, from: None, note: None, agent: None, is_ai: false, name: None });
            }
            Some(PopupTarget::RenamePane(pid)) => {
                let session = self.active_session().map(|s| s.name.clone());
                if let Some(session) = session {
                    self.popup.open = false;
                    let _ = self.send(&Command::PaneRename { session, pane_id: pid, name });
                }
            }
            Some(PopupTarget::RenameSession(idx)) => {
                let old = self
                    .layout
                    .as_ref()
                    .and_then(|l| l.sessions.get(idx))
                    .map(|s| s.name.clone());
                if let Some(old) = old {
                    self.popup.open = false;
                    let _ = self.send(&Command::SessionRename { session: old, new_name: name });
                }
            }
            Some(PopupTarget::RenameTab { session, tab }) => {
                let (sname, tname) = self.layout.as_ref().and_then(|l| l.sessions.get(session)).and_then(|s| s.tabs.get(tab).map(|t| (s.name.clone(), t.name.clone()))).unwrap_or_default();
                if !sname.is_empty() {
                    self.popup.open = false;
                    let _ = self.send(&Command::TabRename { session: sname, tab: tname, new_name: name });
                }
            }
            None => {}
        }
        self.mark_dirty();
    }

    fn on_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.menu.open = false;
            self.mark_dirty();
            return Ok(());
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.menu.selected = (self.menu.selected + 1) % MENU_ITEMS.len();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.menu.selected = self.menu.selected.saturating_sub(1);
            }
            KeyCode::Enter => self.menu_select(self.menu.selected)?,
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn menu_select(&mut self, idx: usize) -> Result<()> {
        let action = MENU_ITEMS.get(idx).copied().unwrap_or("detach");
        self.menu.open = false;
        match action {
            "config" => {
                let session = self.active_session().map(|s| s.name.clone());
                if let Some(session) = session {
                    let _ = self.send(&Command::OpenConfig { session });
                }
            }
            "reload" => {
                let _ = self.send(&Command::ReloadConfig);
            }
            "keybinds" => self.open_keybind_overlay(),
            "settings" => self.open_settings(),
            "detach" => {
                let _ = self.send(&Command::Detach);
                self.detach_requested = true;
            }
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn ctx_items(&self) -> &'static [&'static str] {
        match self.ctx_menu.target {
            CtxTarget::Pane(_) if self.session_zoom() => {
                &["rename", "unzoom", "split vertical", "split horizontal", "close"]
            }
            CtxTarget::Pane(_) => &["rename", "zoom", "split vertical", "split horizontal", "close"],
            CtxTarget::Session(idx)
                if !self.layout.as_ref().and_then(|layout| layout.sessions.get(idx)).map(|session| session.is_linked_worktree).unwrap_or(false) =>
            {
                &["rename", "new worktree", "open worktree", "close session"]
            }
            CtxTarget::Session(_) => &[
                "rename",
                "new worktree",
                "open worktree",
                "close session",
                "remove worktree",
                "force remove worktree",
            ],
            CtxTarget::Tab(_, _) => &["new tab", "rename", "close"],
        }
    }

    fn open_worktree_removal_confirm(&mut self, idx: usize, mode: WorktreeRemovalMode) {
        let Some((session_name, workspace)) = self
            .layout
            .as_ref()
            .and_then(|l| l.sessions.get(idx))
            .map(|session| (session.name.clone(), session.workspace.clone())) else { return };
        if !self
            .layout
            .as_ref()
            .and_then(|layout| layout.sessions.get(idx))
            .map(|session| session.is_linked_worktree)
            .unwrap_or(false)
        {
            self.notice = Some(("only a registered linked worktree can be removed".to_string(), Instant::now()));
            return;
        }
        self.session_close_confirm.open = true;
        self.session_close_confirm.session_idx = idx;
        self.session_close_confirm.session_name = session_name;
        self.session_close_confirm.path = workspace;
        self.session_close_confirm.mode = mode;
    }

    fn open_ctx_menu(&mut self, x: u16, y: u16, target: CtxTarget) {
        self.ctx_menu.open = true;
        self.ctx_menu.x = x;
        self.ctx_menu.y = y;
        self.ctx_menu.selected = 0;
        self.ctx_menu.target = target;
        self.mark_dirty();
    }

    fn on_ctx_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.ctx_menu.open = false;
            self.mark_dirty();
            return Ok(());
        }
        let items = self.ctx_items();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.ctx_menu.selected = (self.ctx_menu.selected + 1) % items.len();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.ctx_menu.selected = self.ctx_menu.selected.saturating_sub(1);
            }
            KeyCode::Enter => self.ctx_menu_select(self.ctx_menu.selected)?,
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn ctx_menu_select(&mut self, idx: usize) -> Result<()> {
        let items = self.ctx_items();
        let action = items.get(idx).copied().unwrap_or("close");
        let target = self.ctx_menu.target;
        self.ctx_menu.open = false;
        match action {
            "rename" => match target {
                CtxTarget::Pane(pid) => self.open_rename_popup(pid),
                CtxTarget::Session(idx) => self.open_rename_session_popup(idx),
                CtxTarget::Tab(s_idx, t_idx) => self.open_rename_tab_popup_for(s_idx, t_idx),
            },
            "new tab" => {
                if let CtxTarget::Tab(s_idx, _) = target {
                    if let Some(name) = self.layout.as_ref().and_then(|l| l.sessions.get(s_idx)).map(|s| s.name.clone()) {
                        let _ = self.send(&Command::TabNew { session: name, name: None, workspace: None });
                    }
                } else if let Some(sess) = self.active_session().map(|s| s.name.clone()) {
                    let _ = self.send(&Command::TabNew { session: sess, name: None, workspace: None });
                }
            }
            "new worktree" => {
                if let CtxTarget::Session(idx) = target {
                    self.open_worktree_popup(idx);
                }
            }
            "open worktree" => {
                if let CtxTarget::Session(idx) = target {
                    self.open_worktree_picker(idx);
                }
            }
            "split vertical" => {
                if let CtxTarget::Pane(pid) = target {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneSplit { session: session.clone(), dir: SplitDir::Vertical, is_ai: false });
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                }
            }
            "split horizontal" => {
                if let CtxTarget::Pane(pid) = target {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneSplit { session: session.clone(), dir: SplitDir::Horizontal, is_ai: false });
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                }
            }
            "zoom" => {
                if let CtxTarget::Pane(pid) = target {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneFocus { session: session.clone(), pane_id: pid });
                        let _ = self.send(&Command::SessionZoom { session });
                    }
                }
            }
            "unzoom" => {
                let session = self.active_session().map(|s| s.name.clone());
                if let Some(session) = session {
                    let _ = self.send(&Command::SessionZoom { session });
                }
            }
            "close" | "close session" => match target {
                CtxTarget::Pane(pid) => {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneClose { session, pane_id: Some(pid) });
                    }
                }
                CtxTarget::Session(idx) => {
                    let sess = self.layout.as_ref().and_then(|l| l.sessions.get(idx));
                    if let Some(s) = sess {
                        let name = s.name.clone();
                        let _ = self.send(&Command::SessionKill { name });
                    }
                }
                CtxTarget::Tab(s_idx, t_idx) => {
                    if let Some(sess) = self.layout.as_ref().and_then(|l| l.sessions.get(s_idx)) {
                        let tname = sess.tabs.get(t_idx).map(|t| t.name.clone()).unwrap_or_default();
                        let _ = self.send(&Command::TabClose { session: sess.name.clone(), tab: Some(tname) });
                    }
                }
            },
            "remove worktree" => {
                if let CtxTarget::Session(idx) = target {
                    self.open_worktree_removal_confirm(idx, WorktreeRemovalMode::Safe);
                }
            }
            "force remove worktree" => {
                if let CtxTarget::Session(idx) = target {
                    self.open_worktree_removal_confirm(idx, WorktreeRemovalMode::Force);
                }
            }
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn open_keybind_overlay(&mut self) {
        self.keybind_overlay.open = true;
        self.keybind_overlay.input.clear();
        self.keybind_overlay.cursor = 0;
        self.keybind_overlay.selected = 0;
        self.keybind_overlay.scroll = 0;
        self.mark_dirty();
    }

    fn on_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.keybind_overlay.open = false;
            self.mark_dirty();
            return Ok(());
        }
        match key.code {
            KeyCode::Down | KeyCode::Tab => {
                let len = command_palette_matches(&self.keymap, &self.keybind_overlay.input).len();
                if len > 0 {
                    self.keybind_overlay.selected = (self.keybind_overlay.selected + 1).min(len - 1);
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.keybind_overlay.selected = self.keybind_overlay.selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                let matches = command_palette_matches(&self.keymap, &self.keybind_overlay.input);
                if let Some(action) = matches.get(self.keybind_overlay.selected).map(|entry| entry.action) {
                    self.keybind_overlay.open = false;
                    self.run_action(action)?;
                    return Ok(());
                }
            }
            _ => {
                let overlay = &mut self.keybind_overlay;
                if TextInput::new(&mut overlay.input, &mut overlay.cursor).handle_key(key) == TextEdit::Changed {
                    overlay.selected = 0;
                    overlay.scroll = 0;
                }
            }
        }
        self.normalize_command_palette_scroll();
        self.mark_dirty();
        Ok(())
    }

    fn normalize_command_palette_scroll(&mut self) {
        let Some(dd) = self.keybind_overlay_rect() else { return };
        let len = command_palette_matches(&self.keymap, &self.keybind_overlay.input).len();
        self.keybind_overlay.selected = self.keybind_overlay.selected.min(len.saturating_sub(1));
        let visible = dd.height.saturating_sub(5) as usize;
        if self.keybind_overlay.selected < self.keybind_overlay.scroll {
            self.keybind_overlay.scroll = self.keybind_overlay.selected;
        } else if self.keybind_overlay.selected >= self.keybind_overlay.scroll + visible.max(1) {
            self.keybind_overlay.scroll = self.keybind_overlay.selected + 1 - visible.max(1);
        }
    }

    fn keybind_overlay_rect(&self) -> Option<Rect> {
        let (w, h) = (self.cols, self.rows);
        let max_keys = self.keymap.iter().map(|b| b.keys.chars().count()).max().unwrap_or(4) as u16;
        let max_desc = bindings::palette_entries(&self.keymap)
            .iter()
            .map(|entry| palette_action_desc(entry.action).chars().count())
            .max()
            .unwrap_or(10) as u16;
        let inner = (max_keys + 2 + max_desc).max(20);
        let width = (inner + 6).min(w.saturating_sub(4));
        let lines = command_palette_matches(&self.keymap, &self.keybind_overlay.input).len();
        let available = h.saturating_sub(4);
        if available < 7 {
            return None;
        }
        let height_cap = (((h as u32 * 3) / 5) as u16).max(7).min(available);
        let height = ((lines + 5) as u16).min(height_cap).max(7);
        if w < width || h < height {
            return None;
        }
        Some(Rect::new((w - width) / 2, (h - height) / 2, width, height))
    }

    fn command_palette_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let rect = self.keybind_overlay_rect()?;
        if x <= rect.x || x >= rect.right().saturating_sub(1) || y < rect.y + 3 || y >= rect.bottom().saturating_sub(2) {
            return None;
        }
        let item = self.keybind_overlay.scroll + (y - (rect.y + 3)) as usize;
        (item < command_palette_matches(&self.keymap, &self.keybind_overlay.input).len()).then_some(item)
    }

    fn on_settings_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.settings.open = false;
            self.mark_dirty();
            return;
        }
        let tab = SETTINGS_TABS.get(self.settings.tab).copied().unwrap_or(SettingsTab::Appearance);
        match key.code {
            KeyCode::Tab => self.settings_set_tab((self.settings.tab + 1) % SETTINGS_TABS.len()),
            KeyCode::Char('l') | KeyCode::Right => {
                self.settings_set_tab((self.settings.tab + 1).min(SETTINGS_TABS.len() - 1));
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.settings_set_tab(self.settings.tab.saturating_sub(1));
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let len = match tab {
                    SettingsTab::Appearance => self.themes_len(),
                    SettingsTab::Agents => AGENT_SKILL_TARGETS.len(),
                    SettingsTab::About => 0,
                };
                self.settings.selected = (self.settings.selected + 1).min(len.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if tab != SettingsTab::About {
                    self.settings.selected = self.settings.selected.saturating_sub(1);
                }
            }
            KeyCode::Enter => self.settings_activate_item(self.settings.selected),
            _ => {}
        }
        self.mark_dirty();
    }

    fn settings_set_tab(&mut self, idx: usize) {
        if idx >= SETTINGS_TABS.len() {
            return;
        }
        self.settings.tab = idx;
        self.settings.selected =
            if SETTINGS_TABS[idx] == SettingsTab::Appearance { self.theme_idx } else { 0 };
    }

    fn settings_activate_item(&mut self, idx: usize) {
        match SETTINGS_TABS.get(self.settings.tab).copied().unwrap_or(SettingsTab::Appearance) {
            SettingsTab::Appearance => {
                let _ = self.send(&Command::SetTheme { idx });
            }
            SettingsTab::Agents => self.toggle_agent_skill(idx),
            SettingsTab::About => {}
        }
    }

    fn toggle_agent_skill(&mut self, idx: usize) {
        if self.settings.skill_installing.is_some() {
            return;
        }
        let Some(&target) = AGENT_SKILL_TARGETS.get(idx) else {
            return;
        };
        let action = match target.status() {
            AgentSkillStatus::Missing => AgentSkillAction::Install,
            AgentSkillStatus::Outdated => AgentSkillAction::Update,
            AgentSkillStatus::Current => AgentSkillAction::Uninstall,
        };
        self.settings.skill_installing = Some((target, action, Instant::now()));
        self.settings.skill_result = None;
        self.settings.skill_feedback = None;
        let tx = self.skill_install_tx.clone();
        std::thread::spawn(move || {
            let result = match action {
                AgentSkillAction::Install | AgentSkillAction::Update => {
                    agent_skill::install(target)
                }
                AgentSkillAction::Uninstall => agent_skill::uninstall(target),
            }
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send((target, action, result));
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wire_to_layout(node: &LayoutNode) -> kumo_core::layout::Node {
    match node {
        LayoutNode::Pane(p) => kumo_core::layout::Node::Pane { id: p.id },
        LayoutNode::Split { id, dir, ratio, a, b } => kumo_core::layout::Node::Split {
            id: *id,
            dir: match dir {
                SplitDir::Vertical => kumo_core::layout::SplitDir::V,
                SplitDir::Horizontal => kumo_core::layout::SplitDir::H,
            },
            ratio: *ratio,
            a: Box::new(wire_to_layout(a)),
            b: Box::new(wire_to_layout(b)),
        },
    }
}

#[allow(dead_code)]
fn pane_count(s: &SessionLayout) -> usize {
    s.tabs.iter().map(|t| {
        let mut n = 0;
        let mut stack: Vec<&LayoutNode> = Vec::new();
        if let Some(root) = &t.root { stack.push(root); }
        while let Some(node) = stack.pop() {
            match node {
                LayoutNode::Pane(_) => n += 1,
                LayoutNode::Split { a, b, .. } => { stack.push(a); stack.push(b); }
            }
        }
        n
    }).sum()
}

fn find_pane_in_session(s: &SessionLayout, pid: u64) -> Option<&kumo_protocol::LayoutPane> {
    for tab in &s.tabs {
        if let Some(root) = &tab.root {
            if let Some(p) = find_pane(root, pid) { return Some(p); }
        }
    }
    None
}

fn session_panes_all(s: &SessionLayout) -> Vec<(u64, kumo_protocol::LayoutPane)> {
    let mut out = Vec::new();
    for tab in &s.tabs {
        out.extend(session_panes(&tab.root));
    }
    out
}

fn find_pane(node: &LayoutNode, pid: u64) -> Option<&kumo_protocol::LayoutPane> {
    match node {
        LayoutNode::Pane(p) if p.id == pid => Some(p),
        LayoutNode::Split { a, b, .. } => find_pane(a, pid).or_else(|| find_pane(b, pid)),
        _ => None,
    }
}

fn session_panes(node: &Option<Box<LayoutNode>>) -> Vec<(u64, kumo_protocol::LayoutPane)> {
    let mut out = Vec::new();
    let mut stack: Vec<&LayoutNode> = Vec::new();
    if let Some(n) = node {
        stack.push(n);
    }
    while let Some(n) = stack.pop() {
        match n {
            LayoutNode::Pane(p) => out.push((p.id, p.clone())),
            LayoutNode::Split { a, b, .. } => {
                stack.push(a);
                stack.push(b);
            }
        }
    }
    out
}

fn cell_style(cell: &WireCell) -> Style {
    let mut style = Style::default();
    if let Some(fg) = cell.fg {
        style = style.fg(RColor::Rgb(((fg >> 16) & 0xff) as u8, ((fg >> 8) & 0xff) as u8, (fg & 0xff) as u8));
    }
    if let Some(bg) = cell.bg {
        style = style.bg(RColor::Rgb(((bg >> 16) & 0xff) as u8, ((bg >> 8) & 0xff) as u8, (bg & 0xff) as u8));
    }
    if cell.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if cell.faint {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

/// Normalize a selection into ((top_row, left_col), (bottom_row, right_col)) —
/// both corners in `(row, col)` order so the render's tuple comparisons are
/// consistent with `selection_text`'s row-major walk.
fn sel_corners(sel: &Sel) -> ((u16, u16), (u16, u16)) {
    let (c0, r0) = sel.start;
    let (c1, r1) = sel.end;
    if r1 < r0 || (r1 == r0 && c1 < c0) {
        ((r1, c1), (r0, c0))
    } else {
        ((r0, c0), (r1, c1))
    }
}

/// Truncate a label in the middle while keeping both ends visible.
///
/// This works on Unicode scalar values rather than byte offsets, so a narrow
/// sidebar cannot split a multi-byte character. For one or two columns the
/// ellipsis is kept as the only reliable indication that content was cut.
fn fit_middle_label(name: &str, avail: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= avail {
        return name.to_string();
    }
    match avail {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let kept = avail - 1;
            let left = kept.div_ceil(2);
            let right = kept - left;
            let mut shown: String = chars[..left].iter().collect();
            shown.push('…');
            shown.extend(chars[chars.len() - right..].iter());
            shown
        }
    }
}

/// Truncate a git branch name to `avail` columns, preserving its category when
/// possible and otherwise using the shared middle truncation.
fn fit_branch_name(name: &str, avail: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= avail {
        return name.to_string();
    }
    if let Some(slash) = chars.iter().position(|ch| *ch == '/') {
        let category_len = slash + 1;
        // Keep at least one differentiating suffix character after the
        // category and ellipsis; otherwise the generic middle form is clearer.
        if avail >= category_len + 2 {
            let suffix_len = avail - category_len - 1;
            let mut shown: String = chars[..category_len].iter().collect();
            shown.push('…');
            shown.extend(chars[chars.len() - suffix_len..].iter());
            return shown;
        }
    }
    fit_middle_label(name, avail)
}

/// Short display form of a workspace path, e.g. `.../kumo`.
fn short_workspace(ws: &std::path::Path) -> String {
    let text = ws.to_string_lossy();
    if let Some(base) = ws.file_name() {
        let base = base.to_string_lossy();
        if ws.parent().is_some() {
            format!(".../{base}")
        } else {
            base.into_owned()
        }
    } else {
        text.into_owned()
    }
}

fn session_project_key(session: &SessionLayout) -> String {
    kumo_core::ui_state::canonical_project_key(
        session.project_root.as_deref().unwrap_or(&session.workspace),
    )
}

/// Display label shared by project groups and agent context. A worktree uses
/// its project's basename when available; standalone sessions use their
/// workspace basename, then finally the session name.
fn project_label(session: &SessionLayout) -> String {
    session
        .project_root
        .as_deref()
        .or(Some(session.workspace.as_path()))
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| session.name.clone())
}

/// Human-readable Agent Inbox hierarchy for a pane's session and tab.
fn agent_inbox_location_label(session: &SessionLayout, tab: &str) -> String {
    let mut parts = vec![project_label(session), session.name.clone()];
    if !tab.is_empty() {
        parts.push(tab.to_string());
    }
    parts.join(" › ")
}

fn session_worktree_key(session: &SessionLayout) -> String {
    kumo_core::ui_state::canonical_project_key(&session.workspace)
}

fn move_sidebar_item(order: &mut Vec<String>, source: &str, target: &str, after: bool) -> bool {
    if source == target { return false; }
    let Some(source_idx) = order.iter().position(|key| key == source) else { return false };
    let Some(target_idx) = order.iter().position(|key| key == target) else { return false };
    let target_idx = target_idx.saturating_sub(usize::from(source_idx < target_idx));
    let insert_idx = target_idx + usize::from(after);
    if insert_idx == source_idx { return false; }
    order.remove(source_idx);
    order.insert(insert_idx, source.to_string());
    true
}

fn tab_reorder_index(source_idx: usize, target_idx: usize, after: bool) -> Option<usize> {
    if source_idx == target_idx { return None; }
    let target_idx = target_idx.saturating_sub(usize::from(source_idx < target_idx));
    let insert_idx = target_idx + usize::from(after);
    (insert_idx != source_idx).then_some(insert_idx)
}

fn palette_action_desc(action: Action) -> &'static str {
    match action {
        Action::Focus(Dir::Left) => "focus pane left",
        Action::Focus(Dir::Down) => "focus pane down",
        Action::Focus(Dir::Up) => "focus pane up",
        Action::Focus(Dir::Right) => "focus pane right",
        Action::Resize(kumo_core::layout::ResizeDir::Left) => "resize pane left",
        Action::Resize(kumo_core::layout::ResizeDir::Down) => "resize pane down",
        Action::Resize(kumo_core::layout::ResizeDir::Up) => "resize pane up",
        Action::Resize(kumo_core::layout::ResizeDir::Right) => "resize pane right",
        Action::JumpSession(1) => "jump to session 1",
        Action::JumpSession(2) => "jump to session 2",
        Action::JumpSession(3) => "jump to session 3",
        Action::JumpSession(4) => "jump to session 4",
        Action::JumpSession(5) => "jump to session 5",
        Action::JumpSession(6) => "jump to session 6",
        Action::JumpSession(7) => "jump to session 7",
        Action::JumpSession(8) => "jump to session 8",
        Action::JumpSession(9) => "jump to session 9",
        Action::JumpTab(1) => "jump to tab 1",
        Action::JumpTab(2) => "jump to tab 2",
        Action::JumpTab(3) => "jump to tab 3",
        Action::JumpTab(4) => "jump to tab 4",
        Action::JumpTab(5) => "jump to tab 5",
        Action::JumpTab(6) => "jump to tab 6",
        Action::JumpTab(7) => "jump to tab 7",
        Action::JumpTab(8) => "jump to tab 8",
        Action::JumpTab(9) => "jump to tab 9",
        Action::JumpSession(_) => "jump to session",
        Action::JumpTab(_) => "jump to tab",
        other => bindings::action_desc(other),
    }
}

fn command_palette_matches<'a>(keymap: &'a [Binding], query: &str) -> Vec<bindings::PaletteEntry<'a>> {
    let needles: Vec<String> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
    bindings::palette_entries(keymap)
        .into_iter()
        .filter(|entry| {
            let haystack = format!(
                "{} {} {} {} {}",
                bindings::action_id(entry.action),
                palette_action_desc(entry.action),
                entry.group().label(),
                entry.chord_label(),
                entry.binding.map(|b| b.keys.as_str()).unwrap_or_default()
            )
            .to_ascii_lowercase();
            needles.iter().all(|needle| haystack.contains(needle))
        })
        .collect()
}

/// Compose a grouped agent entry label `kind · space · pane` within `avail`
/// columns: pieces drop from the back (`pane`, then `space`) until it fits,
/// and the kind is truncated as a last resort. Never longer than `avail`.
fn agent_entry_label(kind: &str, space: &str, pane: &str, avail: usize) -> String {
    let make = |parts: &[&str]| parts.join(" · ");
    let full = make(&[kind, space, pane]);
    if full.chars().count() <= avail {
        return full;
    }
    let no_pane = make(&[kind, space]);
    if no_pane.chars().count() <= avail {
        return no_pane;
    }
    if kind.chars().count() <= avail {
        return kind.to_string();
    }
    if avail < 2 {
        return kind.chars().take(avail).collect();
    }
    let t: String = kind.chars().take(avail - 1).collect();
    format!("{t}…")
}

/// Compose a ranked workspace row label `space · pane` within `avail`
/// columns: the pane drops first, the space truncates as a last resort.
/// Never longer than `avail`.
fn agent_space_pane_label(space: &str, pane: &str, avail: usize) -> String {
    let full = format!("{space} · {pane}");
    if full.chars().count() <= avail {
        return full;
    }
    if space.chars().count() <= avail {
        return space.to_string();
    }
    if avail < 2 {
        return space.chars().take(avail).collect();
    }
    let t: String = space.chars().take(avail - 1).collect();
    format!("{t}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn cell(text: &str, width: u16) -> WireCell {
        WireCell {
            text: text.into(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
            faint: false,
            cell_width: width,
        }
    }

    pub(super) fn grid() -> Grid {
        Grid { cols: 4, rows: 3, cells: Vec::new(), cursor: None, scroll: None, links: HashMap::new(), rendered: Vec::new(), dirty_rows: HashSet::new() }
    }

    pub(super) fn test_view() -> View {
        let (github_tx, github_rx) = mpsc::channel();
        let (skill_install_tx, skill_install_rx) = mpsc::channel();
        View {
            out: UnixStream::pair().unwrap().1,
            cols: 80,
            rows: 24,
            mode: Mode::Normal,
            leader: bindings::LEADER,
            keymap: bindings::build_keymap(&Default::default()),
            layout: None,
            grids: HashMap::new(),
            rects: Vec::new(),
            splitters: Vec::new(),
            subscribed: HashSet::new(),
            sent_sizes: HashMap::new(),
            theme_idx: kumo_core::theme::DEFAULT_THEME_IDX,
            custom_theme: None,
            sidebar_open: true,
            sidebar_tab: SidebarTab::Sessions,
            sidebar_scroll: (0, u16::MAX),
            popup: Popup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            menu: Menu { open: false, selected: 0 },
            ctx_menu: CtxMenu { open: false, x: 0, y: 0, selected: 0, target: CtxTarget::Pane(0) },
            keybind_overlay: KeybindOverlay { open: false, input: String::new(), cursor: 0, selected: 0, scroll: 0 },
            settings: SettingsPanel {
                open: false,
                tab: 0,
                selected: 0,
                skill_installing: None,
                skill_result: None,
                skill_feedback: None,
            },
            worktree_picker: WorktreePicker { open: false, session: 0, items: Vec::new(), selected: 0, scroll: 0, error: None },
            worktree_create: WorktreeCreateDialog { open: false, session: 0, tab: WorktreeCreateTab::Smart, create_from: String::new(), cursor: 0, branch_override: String::new(), branch_cursor: 0, note: String::new(), note_cursor: 0, agent: String::new(), agent_cursor: 0, advanced: false, focus: WorktreeCreateFocus::CreateFrom, error: None },
            session_close_confirm: SessionCloseConfirm { open: false, session_idx: 0, session_name: String::new(), path: PathBuf::new(), mode: WorktreeRemovalMode::Safe },
            pane_numbers: None,
            status_msg: None,
            notice: None,
            agent_toasts: Vec::new(),
            toast_pos: ToastPosition::default(),
            update_notice: None,
            link_mods: false,
            drag: None,
            sel: None,
            pending_click: None,
            pending_wheel: HashMap::new(),
            copy: None,
            inbox: None,
            sidebar_layout_override: None,
            sidebar_split: None,
            sidebar_drag: None,
            agents_panel_order: AgentPanelOrder::default(),
            sidebar_width: SIDEBAR_WIDTH,
            sidebar_width_drag: None,
            sidebar_reorder_drag: None,
            finder: WorkspaceFinder { open: false, input: String::new(), cursor: 0, items: Vec::new(), filtered: Vec::new(), selected: 0, scroll: 0 },
            sidebar_hover: None,
            collapsed_projects: HashSet::new(),
            project_order: Vec::new(),
            worktree_order: HashMap::new(),
            sidebar_filter: String::new(),
            sidebar_filter_active: false,
            tab_hover: None,
            tab_rects: Vec::new(),
            tab_scroll: 0,
            tab_reorder_drag: None,
            plus_rect: None,
            dirty: false,
            detach_requested: false,
            connection_lost: false,
            status_bar: StatusBarConfig::default(),
            hostname: "testhost".to_string(),
            clock_str: "12:00".to_string(),
            clock_next: Instant::now() + Duration::from_secs(60),
            is_ssh: false,
            github_status: None,
            github_workspace: None,
            github_query: None,
            github_refresh_at: Instant::now() + Duration::from_secs(60),
            github_tx,
            github_rx,
            skill_install_tx,
            skill_install_rx,
            spinner_frame: 0,
            spinner_next: Instant::now(),
        }
    }

    pub(super) fn mouse_moved(x: u16, y: u16) -> crossterm::event::MouseEvent {
        use crossterm::event::MouseEventKind;
        MouseEvent { kind: MouseEventKind::Moved, column: x, row: y, modifiers: KeyModifiers::NONE }
    }

    pub(super) fn mouse_click(x: u16, y: u16) -> crossterm::event::MouseEvent {
        MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ai_pane(pid: u64, status: AgentStatus) -> kumo_protocol::LayoutPane {
        kumo_protocol::LayoutPane {
            id: pid,
            title: " AI CLI ".to_string(),
            cwd: std::path::PathBuf::from("/tmp/work"),
            is_ai: true,
            agent: Some(kumo_protocol::AgentInfo { name: format!("agent{pid}"), status, cpu: 0.0, mem_kb: 0, pane_id: pid, pane_index: 1, tab_index: 1 }),
            mouse_reporting: false,
            alt_screen: false,
        }
    }

    /// One session `sess` with all entries as panes in one tab (left-leaning
    /// split chain, so every pane is found by the traversals).
    pub(super) fn panes_layout(entries: &[(u64, AgentStatus)]) -> Layout {
        let mut root: Option<Box<LayoutNode>> = None;
        for &(pid, status) in entries {
            let leaf = LayoutNode::Pane(ai_pane(pid, status));
            root = Some(match root.take() {
                None => Box::new(leaf),
                Some(a) => Box::new(LayoutNode::Split { id: pid, dir: SplitDir::Vertical, ratio: 0.5, a, b: Box::new(leaf) }),
            });
        }
        let focus = entries.first().map(|(p, _)| *p).unwrap_or(1);
        let tabs = vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus, zoom: false, root }];
        Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp/work"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs,
                focus,
                zoom: false,
                branch: None,
                root: None,
            }],
        }
    }

    pub(super) fn render_bench_layout(pane_ids: &[u64]) -> Layout {
        let mut root: Option<Box<LayoutNode>> = None;
        for &pid in pane_ids {
            let leaf = LayoutNode::Pane(kumo_protocol::LayoutPane {
                id: pid,
                title: format!("shell {pid}"),
                cwd: std::path::PathBuf::from("/tmp/work"),
                is_ai: false,
                agent: None,
                mouse_reporting: false,
                alt_screen: false,
            });
            root = Some(match root.take() {
                None => Box::new(leaf),
                Some(a) => Box::new(LayoutNode::Split {
                    id: pid + 10_000,
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    a,
                    b: Box::new(leaf),
                }),
            });
        }
        let focus = pane_ids.first().copied().unwrap_or(1);
        let root = root.unwrap_or_else(|| Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
            id: focus,
            title: "shell".into(),
            cwd: std::path::PathBuf::from("/tmp/work"),
            is_ai: false,
            agent: None,
            mouse_reporting: false,
            alt_screen: false,
        })));
        Layout {
            active: Some("bench".into()),
            sessions: vec![SessionLayout {
                name: "bench".into(),
                workspace: std::path::PathBuf::from("/tmp/work"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout {
                    id: 1,
                    name: "1".into(),
                    focus,
                    zoom: false,
                    root: Some(root.clone()),
                }],
                focus,
                zoom: false,
                branch: None,
                root: Some(root),
            }],
        }
    }

    pub(super) fn render_bench_frame(pid: u64, cols: u16, rows: u16, full: bool, row: u16, seed: u8) -> PaneFrame {
        let row_cells = (0..cols)
            .map(|col| cell(&char::from(b'a' + ((seed as usize + col as usize) % 26) as u8).to_string(), 1))
            .collect();
        PaneFrame {
            pane_id: pid,
            cols,
            rows,
            full,
            rows_dirty: vec![kumo_protocol::RowPatch { row, cells: row_cells, links: vec![] }],
            cursor: Some((seed as u16 % cols.max(1), row)),
            scroll: None,
        }
    }

    #[test]
    fn full_inbox_lists_all_states_in_attention_order() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Idle),
            (2, AgentStatus::Blocked),
            (3, AgentStatus::Working),
            (4, AgentStatus::Done),
            (5, AgentStatus::Unknown),
        ]));
        assert_eq!(
            view.inbox_entries(),
            vec![
                (0, 2, AgentStatus::Blocked),
                (0, 4, AgentStatus::Done),
                (0, 3, AgentStatus::Working),
                (0, 1, AgentStatus::Idle),
                (0, 5, AgentStatus::Unknown),
            ]
        );
    }

    #[test]
    fn open_inbox_without_alerts_still_opens_full_view() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Idle)]));
        view.open_inbox();
        assert_eq!(view.mode, Mode::Inbox);
        assert!(view.agent_inbox_rect().is_some());
    }

    #[test]
    fn full_inbox_does_not_trap_input_when_terminal_is_too_small() {
        let mut view = test_view();
        view.rows = 10;
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        view.open_inbox();
        assert_eq!(view.mode, Mode::Normal);
        assert!(view.inbox.is_none());
        assert!(view.notice.is_some());
    }

    #[test]
    fn minimum_full_inbox_height_keeps_list_above_details() {
        let mut view = test_view();
        view.rows = 11;
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        view.open_inbox();
        let rect = view.agent_inbox_rect().unwrap();
        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|frame| view.render_agent_inbox(frame)).unwrap();
        assert_eq!(term.backend().buffer().cell((rect.x + 4, rect.y + 3)).unwrap().symbol(), "!");
    }

    #[test]
    fn agent_inbox_renders_project_session_and_tab_context() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Blocked)]);
        let session = &mut layout.sessions[0];
        session.name = "feature".into();
        session.workspace = PathBuf::from("/tmp/my-project/.worktrees/feature");
        session.project_root = Some(PathBuf::from("/tmp/my-project"));
        session.tabs[0].name = "implementation".into();
        let Some(LayoutNode::Pane(pane)) = session.tabs[0].root.as_deref_mut() else {
            unreachable!("test layout has one pane");
        };
        pane.agent.as_mut().unwrap().name = "codex".into();
        view.layout = Some(layout);
        view.open_inbox();

        let rect = view.agent_inbox_rect().unwrap();
        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|frame| view.render_agent_inbox(frame)).unwrap();
        let row_text = |y: u16| -> String {
            (rect.x..rect.right())
                .filter_map(|x| term.backend().buffer().cell((x, y)))
                .map(|cell| cell.symbol())
                .collect::<String>()
        };

        assert!(
            row_text(rect.y + 3).contains("codex · my-project › feature › implementation"),
            "inbox row includes project, session, and tab"
        );
        assert!(
            row_text(rect.bottom() - 4).contains("codex · blocked · my-project › feature › implementation"),
            "selected detail retains status and the full hierarchy"
        );
        assert!(
            row_text(rect.bottom() - 3).contains("/tmp/my-project/.worktrees/feature"),
            "workspace path remains on its detail line"
        );
    }

    #[test]
    fn project_label_falls_back_to_workspace_then_session_name() {
        let mut session = panes_layout(&[]).sessions.remove(0);
        session.project_root = None;
        session.workspace = PathBuf::from("/tmp/my-project/.worktrees/feature");
        session.name = "fallback-session".into();
        assert_eq!(project_label(&session), "feature");

        session.workspace = PathBuf::from("/");
        assert_eq!(project_label(&session), "fallback-session");
    }

    #[test]
    fn inbox_keyboard_moves_and_enter_jumps() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked), (2, AgentStatus::Working)]));
        view.open_inbox();
        assert_eq!(view.mode, Mode::Inbox);
        assert_eq!(view.inbox.unwrap().sel, 0);
        view.on_inbox_key(key(KeyCode::Char('j')));
        assert_eq!(view.inbox.unwrap().sel, 1);
        view.on_inbox_key(key(KeyCode::Char('j')));
        assert_eq!(view.inbox.unwrap().sel, 1, "clamps at the end");
        view.on_inbox_key(key(KeyCode::Char('k')));
        assert_eq!(view.inbox.unwrap().sel, 0);
        view.on_inbox_key(key(KeyCode::Char('j')));
        view.on_inbox_key(key(KeyCode::Home));
        assert_eq!(view.inbox.unwrap().sel, 0, "Home returns to the first entry");
        // Enter focuses the pane and leaves the focus mode.
        view.on_inbox_key(key(KeyCode::Enter));
        assert_eq!(view.mode, Mode::Normal);
        assert!(view.inbox.is_none(), "inbox closed after jump");
    }

    #[test]
    fn inbox_esc_and_q_close() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        view.open_inbox();
        view.on_inbox_key(key(KeyCode::Esc));
        assert_eq!(view.mode, Mode::Normal);
        view.open_inbox();
        view.on_inbox_key(key(KeyCode::Char('q')));
        assert_eq!(view.mode, Mode::Normal);
        assert!(view.inbox.is_none());
    }

    #[test]
    fn grid_apply_full_rebuilds() {
        let mut g = grid();
        let mut frame = PaneFrame {
            pane_id: 1,
            cols: 4,
            rows: 3,
            full: true,
            rows_dirty: vec![kumo_protocol::RowPatch { row: 0, cells: vec![cell("a", 1)], links: vec![] }],
            cursor: Some((1, 1)),
            scroll: Some(ScrollState { offset: 0, total: 10, screen: 3 }),
        };
        g.apply(&frame);
        assert_eq!(g.cells.len(), 3);
        assert_eq!(g.cells[0][0].text, "a");
        assert_eq!(g.cursor, Some((1, 1)));
        assert_eq!(g.scroll.unwrap().total, 10);
        // A partial frame patches in place without resizing.
        frame.full = false;
        frame.rows_dirty = vec![kumo_protocol::RowPatch { row: 2, cells: vec![cell("z", 1)], links: vec![] }];
        g.apply(&frame);
        assert_eq!(g.cells.len(), 3, "patch must not resize");
        assert_eq!(g.cells[2][0].text, "z");
    }

    #[test]
    fn tab_agent_indicator_uses_attention_precedence_and_omits_idle() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Working),
            (2, AgentStatus::Done),
            (3, AgentStatus::Blocked),
        ]));
        let tab = &view.layout.as_ref().unwrap().sessions[0].tabs[0];
        assert_eq!(view.tab_agent_indicator(tab), Some(AgentStatus::Blocked));

        view.layout = Some(panes_layout(&[(1, AgentStatus::Working), (2, AgentStatus::Done)]));
        let tab = &view.layout.as_ref().unwrap().sessions[0].tabs[0];
        assert_eq!(view.tab_agent_indicator(tab), Some(AgentStatus::Done));

        view.layout = Some(panes_layout(&[(1, AgentStatus::Working), (2, AgentStatus::Idle)]));
        let tab = &view.layout.as_ref().unwrap().sessions[0].tabs[0];
        assert_eq!(view.tab_agent_indicator(tab), Some(AgentStatus::Working));

        view.layout = Some(panes_layout(&[(1, AgentStatus::Idle)]));
        let tab = &view.layout.as_ref().unwrap().sessions[0].tabs[0];
        assert_eq!(view.tab_agent_indicator(tab), None);
    }

    #[test]
    fn ai_pane_without_agent_info_still_reserves_tab_status_slot() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Idle)]));
        if let Some(LayoutNode::Pane(pane)) = view
            .layout
            .as_mut()
            .and_then(|layout| layout.sessions.first_mut())
            .and_then(|session| session.tabs.first_mut())
            .and_then(|tab| tab.root.as_deref_mut())
        {
            pane.is_ai = true;
            pane.agent = None;
        }
        let tab = &view.layout.as_ref().unwrap().sessions[0].tabs[0];
        assert!(view.tab_has_agent(tab));
        assert_eq!(view.tab_agent_indicator(tab), None);
        view.update_tab_rects();
        assert_eq!(view.tab_rects[0].1.width, 5);
    }

    #[test]
    fn working_tab_renders_current_spinner_and_reserves_indicator_width() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Working)]));
        view.spinner_frame = 3;
        view.update_tab_rects();
        let (_, pill, close) = view.tab_rects[0];
        assert_eq!(pill.width, 5);
        assert_eq!(close.x, pill.x + 4);

        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|frame| view.render_tab_bar(frame)).unwrap();
        let buffer = term.backend().buffer();
        assert_eq!(buffer.cell((pill.x + 2, pill.y)).unwrap().symbol(), SPINNER_FRAMES[3]);
        assert_eq!(buffer.cell((close.x, close.y)).unwrap().symbol(), "x");
    }

    #[test]
    fn narrow_agent_tabs_keep_the_status_slot_and_overflow_safely() {
        let mut view = test_view();
        view.cols = 40;
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        let first = layout.sessions[0].tabs[0].clone();
        let mut second = first.clone();
        second.id = 2;
        second.name = "second".into();
        layout.sessions[0].tabs = vec![first, second];
        view.layout = Some(layout);

        view.update_tab_rects();
        let (_, pill, close) = view.tab_rects[0];
        assert_eq!(pill.width, 5);
        assert_eq!(close.x, pill.x + 4);
        assert!(view.tab_right_arrow_rect().is_some(), "the second tab should overflow");

        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|frame| view.render_tab_bar(frame)).unwrap();
        let buffer = term.backend().buffer();
        assert_eq!(buffer.cell((pill.x + 2, pill.y)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((close.x, close.y)).unwrap().symbol(), "x");
    }

    #[test]
    fn command_palette_filters_and_runs_the_selected_action() {
        let mut view = test_view();
        view.layout = Some(one_pane_layout());
        let matches = command_palette_matches(&view.keymap, "focus right");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].action, Action::Focus(Dir::Right));

        let sidebar_was_open = view.sidebar_open;
        view.open_keybind_overlay();
        for ch in "toggle sidebar".chars() {
            view.on_overlay_key(key(KeyCode::Char(ch))).unwrap();
        }
        assert_eq!(command_palette_matches(&view.keymap, &view.keybind_overlay.input).len(), 1);
        view.on_overlay_key(key(KeyCode::Enter)).unwrap();

        assert!(!view.keybind_overlay.open);
        assert_eq!(view.sidebar_open, !sidebar_was_open);
    }

    /// Settings and session rename have no stock chord; the palette is how
    /// users discover them.
    #[test]
    fn command_palette_runs_unbound_session_and_settings_actions() {
        let mut view = test_view();
        view.layout = Some(one_pane_layout());
        let run = |view: &mut View, query: &str| {
            view.open_keybind_overlay();
            for ch in query.chars() {
                view.on_overlay_key(key(KeyCode::Char(ch))).unwrap();
            }
            assert_eq!(command_palette_matches(&view.keymap, &view.keybind_overlay.input).len(), 1, "{query}");
            view.on_overlay_key(key(KeyCode::Enter)).unwrap();
        };

        run(&mut view, "open settings");
        assert!(view.settings.open);
        view.settings.open = false;

        run(&mut view, "rename the active session");
        assert!(view.popup.open);
        assert!(matches!(view.popup.target, Some(PopupTarget::RenameSession(0))));
    }

    /// Popup fields share one editor, so readline keys work in the palette
    /// query too and editing it resets the selection.
    #[test]
    fn command_palette_query_uses_shared_text_editing() {
        let mut view = test_view();
        view.open_keybind_overlay();
        for ch in "focus right".chars() {
            view.on_overlay_key(key(KeyCode::Char(ch))).unwrap();
        }
        view.keybind_overlay.selected = 1;
        view.on_overlay_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(view.keybind_overlay.input, "focus ");
        assert_eq!(view.keybind_overlay.cursor, 6);
        assert_eq!(view.keybind_overlay.selected, 0);
    }

    #[test]
    fn command_palette_question_mark_is_searchable_input() {
        let mut view = test_view();
        view.open_keybind_overlay();
        view.on_overlay_key(key(KeyCode::Char('?'))).unwrap();
        assert!(view.keybind_overlay.open);
        assert_eq!(view.keybind_overlay.input, "?");
    }

    #[test]
    fn command_palette_is_compact_and_uses_readable_input_contrast() {
        let mut view = test_view();
        view.open_keybind_overlay();
        for ch in "toggle".chars() {
            view.on_overlay_key(key(KeyCode::Char(ch))).unwrap();
        }
        let rect = view.keybind_overlay_rect().unwrap();
        assert!(rect.height as u32 * 5 <= view.rows as u32 * 3, "palette must stay within 60% of the window");

        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|frame| view.draw(frame)).unwrap();
        let cell = term.backend().buffer().cell((rect.x + 4, rect.y + 2)).unwrap();
        assert_eq!(cell.symbol(), "t");
        assert_eq!(cell.fg, RColor::Black);
        assert_eq!(cell.bg, view.current_theme().input_bg);

        let mut dark_input_theme = view.current_theme();
        dark_input_theme.input_bg = RColor::Rgb(0x20, 0x22, 0x28);
        assert_eq!(input_fg(&dark_input_theme), RColor::White, "dark custom inputs need light text");
    }

    /// Chrome drawn on theme fills must stay readable on custom (e.g. light)
    /// themes whose accents are dark, without changing the built-in look.
    #[test]
    fn filled_chrome_text_contrasts_with_its_fill() {
        for theme in theme::all_themes(None) {
            for fill in [theme.accent, theme.secondary, theme.green] {
                assert_eq!(contrast_fg(&theme, fill), RColor::Black, "{}: built-in fills keep dark text", theme.name);
            }
        }
        let mut light = OwnedTheme::from(THEMES[0]);
        light.accent = RColor::Rgb(0x1f, 0x3a, 0x93);
        assert_eq!(contrast_fg(&light, light.accent), RColor::White, "dark accents need light text");
        light.accent = RColor::Indexed(4);
        light.palette[4] = kumo_core::color::ColorRgb::new(0x10, 0x10, 0x40);
        assert_eq!(contrast_fg(&light, light.accent), RColor::White, "indexed fills resolve through the palette");
    }

    #[test]
    fn recompute_geometry_requests_pane_sizes() {
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus: 1, zoom: false, root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane { id: 1, title: " shell ".into(), cwd: std::path::PathBuf::from("/tmp"), is_ai: false, agent: None, mouse_reporting: false, alt_screen: false }))) }],
                focus: 1,
                zoom: false,
                branch: None,
                root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                    id: 1,
                    title: " shell ".into(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    is_ai: false,
                    agent: None,
                    mouse_reporting: false,
                    alt_screen: false,
                }))),
            }],
        };
        let mut view = test_view();
        view.layout = Some(layout);
        view.recompute_geometry();
        // Pane 1 is subscribed and sized to the daemon's inner() grid.
        assert!(view.subscribed.contains(&1));
        let inner = PaneGeom { pane_id: 1, rect: view.rects[0].1 }.inner();
        assert_eq!(view.sent_sizes.get(&1), Some(&(inner.width, inner.height)));
    }

    /// Fire-and-forget sends (`let _ = self.send(..)`) must not hide a dead
    /// daemon socket: the render loop polls `connection_lost` to reconnect.
    #[test]
    fn failed_fire_and_forget_send_marks_connection_lost() {
        let (out, peer) = UnixStream::pair().unwrap();
        let mut view = test_view();
        view.out = out;
        view.layout = Some(Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus: 1, zoom: false, root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane { id: 1, title: " shell ".into(), cwd: std::path::PathBuf::from("/tmp"), is_ai: false, agent: None, mouse_reporting: false, alt_screen: false }))) }],
                focus: 1,
                zoom: false,
                branch: None,
                root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                    id: 1,
                    title: " shell ".into(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    is_ai: false,
                    agent: None,
                    mouse_reporting: false,
                    alt_screen: false,
                }))),
            }],
        });
        view.recompute_geometry();
        assert!(!view.connection_lost(), "a live socket must not be reported as lost");

        drop(peer);
        view.subscribed.clear();
        view.sent_sizes.clear();
        view.recompute_geometry();
        assert!(view.connection_lost(), "a failed write must be surfaced");
    }

    /// A toast event lands in the toast stack, keeps the loop repainting via
    /// `has_transient`, and stops once its lifetime elapses.
    #[test]
    fn agent_toast_pushes_keeps_transient_and_expires() {
        let mut view = test_view();
        assert!(view.agent_toast_rects().is_empty());
        assert!(!view.has_transient());
        view.on_event(DaemonEvent::Toast {
            pane_id: 7,
            kind: ToastKind::Blocked,
            title: "claude is blocked".into(),
            body: "~/dev/kumo".into(),
        });
        assert_eq!(view.agent_toasts.len(), 1);
        assert!(view.agent_toasts[0].pane_id == 7 && view.agent_toasts[0].kind == ToastKind::Blocked);
        assert!(!view.agent_toast_rects().is_empty(), "visible toasts have screen rects");
        assert!(view.has_transient(), "a live toast keeps the render loop hot");
        // Past the lifetime the toast no longer drives repaints; the next
        // render's `expire_timers` drops it.
        view.agent_toasts[0].born -= AGENT_TOAST_TIMEOUT + Duration::from_millis(1);
        assert!(!view.has_transient());
        view.expire_timers();
        assert!(view.agent_toasts.is_empty(), "expired toasts are dropped at render time");
    }

    #[test]
    fn github_result_updates_status_without_blocking_the_view() {
        let mut view = test_view();
        view.layout = Some(one_pane_layout());
        let workspace = view.active_session().unwrap().workspace.clone();
        view.github_workspace = Some(workspace.clone());
        view.github_query = Some(workspace.clone());
        view.github_refresh_at = Instant::now() + GITHUB_REFRESH;
        let expected = status_bar::GitHubStatus {
            number: 15,
            draft: false,
            review_decision: Some("REVIEW_REQUIRED".to_string()),
            checks: Some(status_bar::CheckState::Pending),
        };
        view.github_tx.send((workspace, Some(expected.clone()))).unwrap();

        assert!(view.poll_github(Instant::now()));
        assert_eq!(view.github_status, Some(expected));
        assert!(view.github_query.is_none());
    }

    #[test]
    fn fit_branch_name_truncates() {
        assert_eq!(fit_branch_name("very/long/feature-branch-name", 8), "very/…me");
        assert_eq!(fit_branch_name("feat/long-agent-inbox", 17), "feat/…agent-inbox");
        assert_eq!(fit_branch_name("agent-inbox", 6), "age…ox");
        assert_eq!(fit_branch_name("feat/agent-inbox", 0), "");
        assert_eq!(fit_branch_name("feat/agent-inbox", 1), "…");
        assert_eq!(fit_branch_name("feat/agent-inbox", 2), "f…");
    }

    #[test]
    fn sidebar_label_middle_truncation_preserves_unicode_context() {
        assert_eq!(fit_middle_label("project-name", 12), "project-name");
        assert_eq!(fit_middle_label("project-name", 0), "");
        assert_eq!(fit_middle_label("project-name", 1), "…");
        assert_eq!(fit_middle_label("project-name", 2), "p…");
        assert_eq!(fit_middle_label("project-name", 7), "pro…ame");
        assert_eq!(fit_middle_label("café界面", 4), "ca…面");
        assert_eq!(fit_middle_label("a-very-long-project-name", 9), "a-ve…name");
        assert_eq!(fit_middle_label("worktree-with-a-long-name", 11), "workt…-name");
    }

    pub(super) fn one_pane_layout() -> Layout {
        Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus: 1, zoom: false, root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane { id: 1, title: " shell ".into(), cwd: std::path::PathBuf::from("/tmp"), is_ai: false, agent: None, mouse_reporting: false, alt_screen: false }))) }],
                focus: 1,
                zoom: false,
                branch: None,
                root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                    id: 1,
                    title: " shell ".into(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    is_ai: false,
                    agent: None,
                    mouse_reporting: false,
                    alt_screen: false,
                }))),
            }],
        }
    }

    #[test]
    fn worktree_context_menu_separates_close_and_removal() {
        let root = std::env::temp_dir().join(format!("kumo-tui-worktrees-{}", std::process::id()));
        let linked_path = root.with_file_name(format!("{}-feature", root.file_name().unwrap().to_string_lossy()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&linked_path);
        std::fs::create_dir_all(&root).unwrap();
        let run_git = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).status().unwrap().success(), "git {args:?}");
        };
        run_git(&["init", "-q", "-b", "main", root.to_str().unwrap()]);
        run_git(&["-C", root.to_str().unwrap(), "config", "user.email", "t@t"]);
        run_git(&["-C", root.to_str().unwrap(), "config", "user.name", "t"]);
        run_git(&["-C", root.to_str().unwrap(), "commit", "-q", "--allow-empty", "-m", "initial"]);
        run_git(&["-C", root.to_str().unwrap(), "worktree", "add", "-q", "-b", "feature", linked_path.to_str().unwrap()]);

        let mut view = test_view();
        let mut layout = one_pane_layout();
        layout.sessions[0].workspace = root.clone();
        layout.sessions[0].project_root = Some(root.clone());
        let mut linked = layout.sessions[0].clone();
        linked.name = "feature".into();
        linked.workspace = linked_path.clone();
        linked.is_linked_worktree = true;
        layout.sessions.push(linked);
        let mut unregistered = layout.sessions[0].clone();
        unregistered.name = "nested".into();
        unregistered.workspace = root.join("subdir");
        std::fs::create_dir_all(&unregistered.workspace).unwrap();
        layout.sessions.push(unregistered);
        view.layout = Some(layout);

        view.ctx_menu.target = CtxTarget::Session(0);
        assert_eq!(view.ctx_items(), &["rename", "new worktree", "open worktree", "close session"]);
        view.ctx_menu.target = CtxTarget::Session(1);
        assert!(view.ctx_items().contains(&"close session"));
        assert!(view.ctx_items().contains(&"remove worktree"));
        assert!(view.ctx_items().contains(&"force remove worktree"));
        view.ctx_menu.target = CtxTarget::Session(2);
        assert!(!view.ctx_items().contains(&"remove worktree"));
        assert!(!view.ctx_items().contains(&"force remove worktree"));

        run_git(&["-C", root.to_str().unwrap(), "worktree", "remove", "--force", linked_path.to_str().unwrap()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worktree_removal_buttons_match_rendered_labels() {
        let mut view = test_view();
        view.session_close_confirm.open = true;
        let remove = view.session_close_confirm_button_rect(true).unwrap();
        let cancel = view.session_close_confirm_button_rect(false).unwrap();
        assert_eq!(remove.width, " Remove (y) ".chars().count() as u16 + 2);
        assert_eq!(cancel.width, " Cancel (n) ".chars().count() as u16 + 2);
        assert_eq!(remove.y, cancel.y);
        assert!(remove.right() < cancel.x);
    }

    #[test]
    fn tab_reorder_calculates_final_index_after_removal() {
        assert_eq!(tab_reorder_index(0, 2, true), Some(2));
        assert_eq!(tab_reorder_index(2, 0, false), Some(0));
        assert_eq!(tab_reorder_index(0, 1, false), None);
        assert_eq!(tab_reorder_index(1, 0, true), None);
        assert_eq!(tab_reorder_index(1, 1, false), None);
    }

    #[test]
    fn active_tab_uses_primary_accent_and_stable_agent_slot_before_close() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Idle)]));
        view.update_tab_rects();
        let (_, pill, close) = view.tab_rects[0];
        assert_eq!(pill.width, 5);
        assert_eq!(close.x, pill.x + 4, "agent tabs reserve a stable status cell before close");

        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|frame| view.render_tab_bar(frame)).unwrap();
        let buffer = term.backend().buffer();
        let theme = view.current_theme();
        assert_eq!(buffer.cell((pill.x, pill.y)).unwrap().bg, theme.accent);
        assert_eq!(buffer.cell((pill.x + 1, pill.y)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((pill.x + 2, pill.y)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((pill.x + 3, pill.y)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((close.x, close.y)).unwrap().symbol(), "x");
    }

    #[test]
    fn tab_reorder_reserves_a_new_leading_indicator_column() {
        let mut view = test_view();
        view.cols = 120;
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        let first = layout.sessions[0].tabs[0].clone();
        let mut second = first.clone();
        second.id = 2;
        second.name = "two".into();
        let mut third = first.clone();
        third.id = 3;
        third.name = "three".into();
        layout.sessions[0].tabs = vec![first, second, third];
        view.layout = Some(layout);
        view.update_tab_rects();
        let original_x = view.tab_rects[0].1.x;

        assert!(view.begin_tab_reorder(2));
        let drag = view.tab_reorder_drag.as_mut().unwrap();
        drag.target_idx = 0;
        drag.after = false;
        drag.moved = true;
        view.update_tab_rects();

        assert_eq!(view.tab_reorder_indicator_x(), Some(original_x));
        assert_eq!(view.tab_rects[0].1.x, original_x + 1);
    }

    #[test]
    fn agent_skill_install_result_moves_from_installing_to_installed() {
        let mut view = test_view();
        let target = AgentSkillTarget::Codex;
        view.settings.skill_installing = Some((
            target,
            AgentSkillAction::Install,
            Instant::now() - SKILL_INSTALL_MIN_VISIBLE - Duration::from_millis(1),
        ));
        view.skill_install_tx
            .send((
                target,
                AgentSkillAction::Install,
                Ok(PathBuf::from("/tmp/kumo-skill/SKILL.md")),
            ))
            .unwrap();

        assert!(view.poll_agent_skill_install(Instant::now()));
        assert!(view.settings.skill_installing.is_none());
        assert!(matches!(
            &view.settings.skill_feedback,
            Some((AgentSkillTarget::Codex, true, message)) if message.contains("installed at")
        ));
    }
}
