//! Client-side view: all chrome state, geometry, input handling, mouse, and
//! ratatui rendering for the smart terminal client. Mirrors the classic TUI's
//! look, but every piece of chrome lives here — the daemon streams only the
//! semantic layout and per-pane content.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::Frame;
use ratatui::Terminal;

use kumo_core::layout::{self, PaneGeom, TreeGeom};
use kumo_core::theme::{self, OwnedTheme, THEMES};
use kumo_protocol::{
    AgentStatus, Command, CopyHit, DaemonEvent, Layout, LayoutNode, LinkRange, PaneFrame, ScrollState,
    SessionLayout, SplitDir, WireBranch, WireCell, WireWorktree,
};

use crate::cli::bindings::{self, Action, Binding, Chord, Dir, link_modifiers};
use crate::cli::chrome::{fill, put, text};
use crate::cli::mouse::sgr_mouse;
use crate::cli::status_bar::{self, SlotContext};
use kumo_core::config::{StatusBarConfig, StatusWidget};

/// Width of the left sidebar (its last column is the separator).
const SIDEBAR_WIDTH: u16 = 25;
/// Height of the tab bar (row 0, above panes).
const TAB_H: u16 = 1;
/// Height of the status bar (last row) — dynamic via `View::status_h()`.
const STATUS_H: u16 = 1;
/// How long the `leader+q` pane-number overlay stays up without a keypress.
const PANE_NUMBERS_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long transient status-bar messages stay up.
const TOAST_TIMEOUT: Duration = Duration::from_secs(2);
/// Label of the MENU button in the status bar.
const MENU_BTN: &str = " MENU ";
/// Items shown in the status-bar menu dropdown.
const MENU_ITEMS: [&str; 5] = ["config", "settings", "reload", "keybinds", "detach"];
/// Size of the session-name popup.
const SESSION_POPUP_W: u16 = 44;
const SESSION_POPUP_H: u16 = 7;

fn tab_width(name: &str) -> u16 {
    let n = name.chars().count() as u16;
    (n + 4).max(6)
}
fn lighten(c: RColor, amt: u8) -> RColor {
    match c {
        RColor::Rgb(r, g, b) => RColor::Rgb(r.saturating_add(amt), g.saturating_add(amt), b.saturating_add(amt)),
        _ => c,
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Normal,
    Leader,
    Copy,
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
    About,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            SettingsTab::Appearance => "appearance",
            SettingsTab::About => "about",
        }
    }
}

const SETTINGS_TABS: [SettingsTab; 2] = [SettingsTab::Appearance, SettingsTab::About];

struct SettingsPanel {
    open: bool,
    tab: usize,
    selected: usize,
}

struct KeybindOverlay {
    open: bool,
    scroll: u16,
}

struct WorktreePicker {
    open: bool,
    session: usize,
    items: Vec<WireWorktree>,
    selected: usize,
    scroll: u16,
    error: Option<String>,
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
#[derive(Clone)]
enum SidebarRow {
    Header(String),
    Spacer,
    Session(usize),
    Branch(usize, WireBranch),
    AgentDir(usize, u64, String, AgentStatus),
    AgentName(usize, u64, String, AgentStatus),
    NewSession,
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
    plus_rect: Option<Rect>,
    popup: Popup,
    menu: Menu,
    ctx_menu: CtxMenu,
    keybind_overlay: KeybindOverlay,
    settings: SettingsPanel,
    worktree_picker: WorktreePicker,
    pane_numbers: Option<Instant>,
    status_msg: Option<(String, Instant)>,
    notice: Option<(String, Instant)>,
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
    dirty: bool,
    detach_requested: bool,
    status_bar: StatusBarConfig,
    hostname: String,
    clock_str: String,
    clock_next: Instant,
    is_ssh: bool,
}

fn render_pane_content(f: &mut Frame, pid: u64, rect: Rect, grid: &mut Grid, selected: Option<Sel>, link_mods: bool, theme: &OwnedTheme) {
    let inner = PaneGeom { pane_id: pid, rect }.inner();
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    // Pre-compute selection corners once per frame (not per cell)
    // Only apply selection if this pane is the one being selected
    let sel_corners = selected.filter(|sel| sel.pane_id == pid).map(|sel| sel_corners(&sel));
    for r in 0..grid.rows {
        if r as u16 >= inner.height {
            break;
        }
        // Get link ranges for this row (if link mods are active)
        // Clone before calling get_rendered_row() to avoid borrow conflicts
        let row_links = if link_mods {
            grid.links.get(&(r as u16)).cloned()
        } else {
            None
        };
        // Get the cached rendered row, rebuilding only if dirty
        let Some(rendered_row) = grid.get_rendered_row(r) else {
            continue;
        };
        for (c, cell) in rendered_row.iter().enumerate() {
            if c as u16 >= inner.width {
                break;
            }
            // Skip continuation cells (wide character tails). Wide glyphs at
            // the pane's right edge are already clipped by the daemon, so the
            // grid never carries a wide glyph in the last column.
            if matches!(cell.diff_option, ratatui::buffer::CellDiffOption::Skip) {
                continue;
            }
            let x = inner.x + c as u16;
            let y = inner.y + r as u16;
            if let Some(target) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                // Clone the cached cell
                *target = cell.clone();
                
                // Apply selection highlight (client-side overlay, not cached)
                if let Some(((tr, tc), (br, bc))) = sel_corners {
                    if (r as u16, c as u16) >= (tr, tc) && (r as u16, c as u16) <= (br, bc) {
                        target.set_style(target.style().add_modifier(Modifier::REVERSED));
                    }
                }
                
                // Apply link underline (client-side overlay, not cached)
                if let Some(links) = row_links.as_ref() {
                    if links.iter().any(|l| (c as u16) >= l.start && (c as u16) < l.end) {
                        target.set_style(target.style().add_modifier(Modifier::UNDERLINED));
                    }
                }
            }
        }
    }
    // Scrollbar on the last content column when the pane has scrollback.
    if let Some(scroll) = grid.scroll {
        render_scrollbar(f, inner, scroll, theme);
    }
}

fn render_scrollbar(f: &mut Frame, inner: Rect, sb: ScrollState, theme: &OwnedTheme) {
    let total = sb.total as usize;
    let screen = sb.screen as usize;
    if total <= screen || screen == 0 {
        return;
    }
    let hist = total - screen;
    let bar_h = inner.height as usize;
    let thumb = ((screen * bar_h) / total).max(1).min(bar_h);
    let off = sb.offset as usize;
    let y_max = bar_h.saturating_sub(thumb);
    let y_start = off.saturating_mul(y_max) / hist.max(1);
    let x = inner.x + inner.width.saturating_sub(1);
    for i in 0..bar_h {
        let y = inner.y + i as u16;
        if i >= y_start && i < y_start + thumb {
            put(f, x, y, "▐", Style::default().fg(theme.secondary));
        } else {
            put(f, x, y, "░", Style::default().fg(theme.panel_sep));
        }
    }
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
        let hostname = status_bar::cached_hostname();
        let is_ssh = status_bar::is_ssh_session();
        let clock_str = status_bar::format_clock(&status_bar.widgets.clock.format);
        // Next minute boundary — refresh once per minute; we compute remaining secs
        // to the next minute so the first tick is aligned, not 60s from startup.
        let now_secs = chrono::Local::now().timestamp() % 60;
        let rem = (60 - now_secs).max(1) as u64;
        let clock_next = Instant::now() + Duration::from_secs(rem);
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
            sidebar_open: true,
            sidebar_tab: SidebarTab::Sessions,
            sidebar_scroll: (0, u16::MAX),
            popup: Popup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            menu: Menu { open: false, selected: 0 },
            ctx_menu: CtxMenu { open: false, x: 0, y: 0, selected: 0, target: CtxTarget::Pane(0) },
            keybind_overlay: KeybindOverlay { open: false, scroll: 0 },
            settings: SettingsPanel { open: false, tab: 0, selected: kumo_core::theme::DEFAULT_THEME_IDX },
            worktree_picker: WorktreePicker { open: false, session: 0, items: Vec::new(), selected: 0, scroll: 0, error: None },
            pane_numbers: None,
            status_msg: None,
            notice: None,
            update_notice: None,
            link_mods: false,
            drag: None,
            sel: None,
            pending_click: None,
            pending_wheel: HashMap::new(),
            copy: None,
            tab_hover: None,
            tab_rects: Vec::new(),
            tab_scroll: 0,
            plus_rect: None,
            dirty: true,
            detach_requested: false,
            status_bar,
            hostname,
            clock_str,
            clock_next,
            is_ssh,
        };
        let _ = view.send(&Command::SubscribeLayout);
        view
    }

    fn status_h(&self) -> u16 {
        if self.status_bar.enabled { STATUS_H } else { 0 }
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
        if self.status_msg.as_ref().map(|(_, t)| now.duration_since(*t) < TOAST_TIMEOUT).unwrap_or(false) {
            return true;
        }
        if self.notice.as_ref().map(|(_, t)| now.duration_since(*t) < TOAST_TIMEOUT).unwrap_or(false) {
            return true;
        }
        false
    }

    pub fn detach_requested(&self) -> bool {
        self.detach_requested
    }

    fn send(&mut self, cmd: &Command) -> Result<()> {
        kumo_core::protocol::write_framed(&mut self.out, cmd)?;
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
        self.all_themes()
            .get(self.theme_idx)
            .cloned()
            .unwrap_or_else(|| OwnedTheme::from(THEMES[theme::DEFAULT_THEME_IDX]))
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
                let new_bar = kumo_core::config::status_bar();
                let enabled_changed = new_bar.enabled != self.status_bar.enabled;
                self.status_bar = new_bar;
                self.hostname = status_bar::cached_hostname();
                self.is_ssh = status_bar::is_ssh_session();
                self.clock_str = status_bar::format_clock(&self.status_bar.widgets.clock.format);
                let rem = (60 - (chrono::Local::now().timestamp() % 60)).max(1) as u64;
                self.clock_next = Instant::now() + Duration::from_secs(rem);
                if enabled_changed {
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

    fn panes_area(&self) -> Rect {
        let x = if self.sidebar_open {
            (SIDEBAR_WIDTH + 1).min(self.cols.saturating_sub(1))
        } else {
            0
        };
        Rect::new(x, TAB_H, self.cols.saturating_sub(x), self.rows.saturating_sub(self.status_h() + TAB_H))
    }

    /// Lay the active tab's tree out over the pane area (wire tree → the
    /// crate's geometry, which reserves a 1-cell separator per split).
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

    fn tabs_area(&self) -> Rect {
        let x = if self.sidebar_open { (SIDEBAR_WIDTH + 1).min(self.cols.saturating_sub(1)) } else { 0 };
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
            let w = tab_width(&sess.tabs[idx].name) + 1;
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
                let w = tab_width(&sess.tabs[idx].name) + 1;
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
                        let w = tab_width(&sess.tabs[idx].name) + 1;
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
        let mut cur_x = area.x + if has_left { 1 } else { 0 };
        // Reserve for right arrow and plus
        let has_right = self.tab_right_arrow_rect().is_some();
        let plus_w: u16 = 3;
        let right_bound = (area.x as i32 + area.width as i32 - if has_right {1} else {0} - plus_w as i32 - 1).max(area.x as i32) as u16;
        for idx in self.tab_scroll..sess.tabs.len() {
            let tab = &sess.tabs[idx];
            let w = tab_width(&tab.name);
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
                let is_close = close.contains(Position::new(x, y)) && self.tab_hover == Some(*idx);
                return Some((*idx, is_close));
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Input: keys
    // ------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        self.flush_wheel()?;
        self.set_link_mods(key.modifiers.intersects(link_modifiers()));
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
            self.on_overlay_key(key);
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

        if self.mode == Mode::Copy {
            self.on_copy_key(key)?;
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
        }
        Ok(())
    }

    pub fn on_paste(&mut self, text: &str) {
        let _ = self.flush_wheel();
        if self.popup.open
            || self.menu.open
            || self.ctx_menu.open
            || self.keybind_overlay.open
            || self.settings.open
            || self.worktree_picker.open
            || self.pane_numbers.is_some()
            || self.mode == Mode::Leader
            || self.mode == Mode::Copy
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
        }
        self.mark_dirty();
        Ok(())
    }

    // ------------------------------------------------------------------
    // Copy-mode
    // ------------------------------------------------------------------

    fn enter_copy_mode(&mut self) {
        // Pick the focused pane of the active tab; refuse on alt-screen.
        let Some(tab) = self.active_tab() else {
            self.notice = Some(("no active tab".to_string(), Instant::now()));
            self.mark_dirty();
            return;
        };
        let pid = tab.focus;
        if let Some(layout) = self.layout.as_ref() {
            if let Some(sess) = layout.sessions.iter().find(|s| s.name == layout.active.as_deref().unwrap_or("")) {
                for t2 in &sess.tabs {
                    if let Some(p) = Self::find_layout_pane_in_tab(t2, pid) {
                        if p.alt_screen {
                            self.notice = Some(("copy-mode unavailable in alternate screen".to_string(), Instant::now()));
                            self.mark_dirty();
                            return;
                        }
                    }
                }
            }
        }
        // Also check grids for alt_screen? Use layout Pane alt_screen above.
        let (_cols, rows) = self.copy_pane_dims(pid).unwrap_or((80, 24));
        let start_cursor = (0u16, rows.saturating_sub(1));
        self.copy = Some(CopyState {
            pane_id: pid,
            cursor: start_cursor,
            anchor: None,
            linewise: false,
            search_active: false,
            search_input: String::new(),
            search_cursor: 0,
            search_forward: true,
            search_query: None,
            hits: Vec::new(),
            hit_idx: None,
        });
        self.mode = Mode::Copy;
        self.mark_dirty();
    }

    fn enter_copy_mode_with_search(&mut self, forward: bool) {
        self.enter_copy_mode();
        if self.mode == Mode::Copy {
            if let Some(cs) = self.copy.as_mut() {
                cs.search_active = true;
                cs.search_input.clear();
                cs.search_cursor = 0;
                cs.search_forward = forward;
            }
            self.mark_dirty();
        }
    }

    fn find_layout_pane(node: &LayoutNode, pid: u64) -> Option<kumo_protocol::LayoutPane> {
        match node {
            LayoutNode::Pane(p) if p.id == pid => Some(p.clone()),
            LayoutNode::Pane(_) => None,
            LayoutNode::Split { a, b, .. } => Self::find_layout_pane(a, pid).or_else(|| Self::find_layout_pane(b, pid)),
        }
    }
    fn find_layout_pane_in_tab(tab: &kumo_protocol::TabLayout, pid: u64) -> Option<kumo_protocol::LayoutPane> {
        if let Some(root) = &tab.root {
            return Self::find_layout_pane(root, pid);
        }
        None
    }

    fn leave_copy_mode(&mut self, scroll_to_bottom: bool) {
        if let Some(cs) = self.copy.take() {
            // Clear daemon selection if any
            let _ = self.send(&Command::CopyClearSelection { pane_id: cs.pane_id });
            if scroll_to_bottom {
                let _ = self.send(&Command::CopyScrollTo { pane_id: cs.pane_id, row: u32::MAX });
                // u32::MAX is clamped by daemon to bottom; alternatively we could
                // send a dedicated bottom command. Use CopyScrollTo with MAX to trigger bottom.
                // Fallback: also send delta large to ensure bottom
                // Actually daemon clamps ROW to total-len, so MAX goes to bottom.
            }
        }
        self.mode = Mode::Normal;
        self.mark_dirty();
    }

    fn copy_pane_dims(&self, pane_id: u64) -> Option<(u16, u16)> {
        for (pid, rect) in &self.rects {
            if *pid == pane_id {
                let inner = PaneGeom { pane_id, rect: *rect }.inner();
                return Some((inner.width.max(1), inner.height.max(1)));
            }
        }
        // fallback to grid dims
        self.grids.get(&pane_id).map(|g| (g.cols as u16, g.rows as u16))
    }

    fn copy_scroll(&mut self, delta: i32) {
        if let Some(cs) = self.copy.as_ref() {
            let _ = self.send(&Command::CopyScroll { pane_id: cs.pane_id, delta });
        }
    }

    fn copy_scroll_to(&mut self, row: u32) {
        if let Some(cs) = self.copy.as_ref() {
            let _ = self.send(&Command::CopyScrollTo { pane_id: cs.pane_id, row });
        }
    }

    fn copy_jump_to_hit(&mut self, idx: usize) {
        let (pane_id, row) = {
            let Some(cs) = self.copy.as_ref() else { return; };
            if idx >= cs.hits.len() { return; }
            let hit = &cs.hits[idx];
            (cs.pane_id, hit.row)
        };
        // Scroll so hit row is visible: place it roughly centered
        // Compute: need offset so that hit row in middle of viewport.
        // Simpler: scroll to hit.row (top) then adjust. Use copy_scroll_to directly.
        // To center, scroll to hit.row saturating_sub(rows/2)
        let dims = self.copy_pane_dims(pane_id);
        let half = dims.map(|(_, h)| h / 2).unwrap_or(0) as u32;
        let target = row.saturating_sub(half);
        let _ = self.send(&Command::CopyScrollTo { pane_id, row: target });
        // Move cursor to hit start within viewport
        if let Some(cs) = self.copy.as_mut() {
            // capture hit start before any other borrow issues
            let hit_start = cs.hits.get(idx).map(|h| h.start_col).unwrap_or(0);
            if let Some(g) = self.grids.get(&pane_id) {
                if let Some(scroll) = g.scroll {
                    // viewport top = scroll.offset (screen offset)
                    let top = scroll.offset as u32;
                    let rows = dims.map(|(_, h)| h).unwrap_or(24);
                    let viewport_y = row.saturating_sub(top);
                    if viewport_y < rows as u32 {
                        cs.cursor = (hit_start.min(dims.map(|(w,_)| w.saturating_sub(1)).unwrap_or(0)), viewport_y as u16);
                    }
                } else {
                    cs.cursor.0 = hit_start;
                }
            } else {
                cs.cursor.0 = hit_start;
            }
            cs.hit_idx = Some(idx);
        }
        self.mark_dirty();
    }

    fn on_copy_key(&mut self, key: KeyEvent) -> Result<()> {
        // If search input is active, handle editing
        if self.copy.as_ref().map(|c| c.search_active).unwrap_or(false) {
            return self.on_copy_search_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let dims = self.copy.as_ref().and_then(|c| self.copy_pane_dims(c.pane_id)).unwrap_or((80, 24));
        let (cols, rows) = dims;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if !ctrl => {
                self.leave_copy_mode(true);
                return Ok(());
            }
            KeyCode::Char('h') | KeyCode::Left => self.copy_move(-1, 0, cols, rows),
            KeyCode::Char('j') | KeyCode::Down => self.copy_move(0, 1, cols, rows),
            KeyCode::Char('k') | KeyCode::Up => self.copy_move(0, -1, cols, rows),
            KeyCode::Char('l') | KeyCode::Right => self.copy_move(1, 0, cols, rows),
            KeyCode::Char('0') | KeyCode::Home => {
                if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = 0; }
                self.mark_dirty();
            }
            KeyCode::Char('$') | KeyCode::End => {
                if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = cols.saturating_sub(1); }
                // For linewise, also handle yank later
                self.mark_dirty();
            }
            KeyCode::Char('g') => {
                // g -> top of history
                self.copy_scroll_to(0);
                if let Some(cs) = self.copy.as_mut() { cs.cursor = (0, 0); }
                self.mark_dirty();
            }
            KeyCode::Char('G') => {
                // G -> bottom
                self.copy_scroll_to(u32::MAX);
                if let Some(cs) = self.copy.as_mut() { cs.cursor = (0, rows.saturating_sub(1)); }
                self.mark_dirty();
            }
            KeyCode::Char('v') if !ctrl => {
                if let Some(cs) = self.copy.as_mut() {
                    if cs.anchor.is_none() {
                        cs.anchor = Some(cs.cursor);
                        cs.linewise = false;
                        // install daemon selection for highlight
                        let pane_id = cs.pane_id;
                        let start = cs.cursor;
                        let _ = self.send(&Command::CopySetSelection { pane_id, start, end: start });
                    } else {
                        // toggle off if already selecting charwise at same point? keep
                        cs.anchor = Some(cs.cursor);
                        cs.linewise = false;
                    }
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::Char('V') => {
                if let Some(cs) = self.copy.as_mut() {
                    cs.anchor = Some(cs.cursor);
                    cs.linewise = true;
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::Char('y') | KeyCode::Enter if !ctrl => {
                self.copy_yank();
                self.leave_copy_mode(true);
            }
            KeyCode::Char('/') => {
                if let Some(cs) = self.copy.as_mut() {
                    cs.search_active = true;
                    cs.search_input.clear();
                    cs.search_cursor = 0;
                    cs.search_forward = true;
                }
                self.mark_dirty();
            }
            KeyCode::Char('?') => {
                if let Some(cs) = self.copy.as_mut() {
                    cs.search_active = true;
                    cs.search_input.clear();
                    cs.search_cursor = 0;
                    cs.search_forward = false;
                }
                self.mark_dirty();
            }
            KeyCode::Char('n') if !ctrl => self.copy_next_hit(true),
            KeyCode::Char('N') => self.copy_next_hit(false),
            _ if ctrl && matches!(key.code, KeyCode::Char('u')) => {
                let half = (rows as i32 / 2).max(1);
                self.copy_scroll(-half);
                if let Some(cs) = self.copy.as_mut() {
                    cs.cursor.1 = cs.cursor.1.saturating_sub(half as u16);
                    if cs.cursor.1 == 0 { cs.cursor.1 = 0; }
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            _ if ctrl && matches!(key.code, KeyCode::Char('d')) => {
                let half = (rows as i32 / 2).max(1);
                self.copy_scroll(half);
                if let Some(cs) = self.copy.as_mut() {
                    cs.cursor.1 = (cs.cursor.1 + half as u16).min(rows.saturating_sub(1));
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            _ if ctrl && matches!(key.code, KeyCode::Char('b')) => {
                self.copy_scroll(-(rows as i32));
                self.sync_copy_selection();
                self.mark_dirty();
            }
            _ if ctrl && matches!(key.code, KeyCode::Char('f')) => {
                self.copy_scroll(rows as i32);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::PageUp => {
                self.copy_scroll(-(rows as i32));
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::PageDown => {
                self.copy_scroll(rows as i32);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::Char('w') => self.copy_word(1, cols, rows),
            KeyCode::Char('b') => self.copy_word(-1, cols, rows),
            _ => {}
        }
        Ok(())
    }

    fn on_copy_search_key(&mut self, key: KeyEvent) -> Result<()> {
        let mut commit: Option<String> = None;
        let mut cancel = false;
        if let Some(cs) = self.copy.as_mut() {
            match key.code {
                KeyCode::Esc => { cancel = true; }
                KeyCode::Enter => {
                    commit = Some(cs.search_input.clone());
                }
                KeyCode::Backspace => {
                    if cs.search_cursor > 0 && !cs.search_input.is_empty() {
                        let mut chars: Vec<char> = cs.search_input.chars().collect();
                        let idx = cs.search_cursor.min(chars.len()).saturating_sub(1);
                        chars.remove(idx);
                        cs.search_input = chars.into_iter().collect();
                        cs.search_cursor = cs.search_cursor.saturating_sub(1);
                    }
                }
                KeyCode::Delete => {
                    let mut chars: Vec<char> = cs.search_input.chars().collect();
                    if cs.search_cursor < chars.len() {
                        chars.remove(cs.search_cursor);
                        cs.search_input = chars.into_iter().collect();
                    }
                }
                KeyCode::Left => { cs.search_cursor = cs.search_cursor.saturating_sub(1); }
                KeyCode::Right => {
                    let len = cs.search_input.chars().count();
                    if cs.search_cursor < len { cs.search_cursor += 1; }
                }
                KeyCode::Home => { cs.search_cursor = 0; }
                KeyCode::End => { cs.search_cursor = cs.search_input.chars().count(); }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    let mut chars: Vec<char> = cs.search_input.chars().collect();
                    let idx = cs.search_cursor.min(chars.len());
                    chars.insert(idx, c);
                    cs.search_input = chars.into_iter().collect();
                    cs.search_cursor += 1;
                }
                _ => {}
            }
        }
        if cancel {
            if let Some(cs) = self.copy.as_mut() {
                cs.search_active = false;
                cs.search_input.clear();
                cs.search_cursor = 0;
            }
            self.mark_dirty();
            return Ok(());
        }
        if let Some(q) = commit {
            let pane_id = self.copy.as_ref().map(|c| c.pane_id).unwrap_or(0);
            let forward = self.copy.as_ref().map(|c| c.search_forward).unwrap_or(true);
            if let Some(cs) = self.copy.as_mut() {
                cs.search_active = false;
                cs.search_query = if q.is_empty() { None } else { Some(q.clone()) };
                cs.search_forward = forward;
                // keep input for display? clear after commit but keep query
                cs.search_input.clear();
                cs.search_cursor = 0;
            }
            if !q.is_empty() {
                let _ = self.send(&Command::CopySearch { pane_id, query: q });
            } else {
                if let Some(cs) = self.copy.as_mut() {
                    cs.hits.clear();
                    cs.hit_idx = None;
                }
            }
            self.mark_dirty();
            return Ok(());
        }
        self.mark_dirty();
        Ok(())
    }

    fn copy_move(&mut self, dx: i32, dy: i32, cols: u16, rows: u16) {
        let need_scroll = {
            let Some(cs) = self.copy.as_mut() else { return; };
            let mut nx = cs.cursor.0 as i32 + dx;
            let mut ny = cs.cursor.1 as i32 + dy;
            let mut scroll_delta: Option<i32> = None;
            if nx < 0 { nx = 0; }
            if nx >= cols as i32 { nx = cols as i32 - 1; }
            if ny < 0 {
                scroll_delta = Some(-1);
                ny = 0;
            } else if ny >= rows as i32 {
                scroll_delta = Some(1);
                ny = rows as i32 - 1;
            }
            cs.cursor = (nx as u16, ny as u16);
            scroll_delta
        };
        if let Some(delta) = need_scroll {
            self.copy_scroll(delta);
        }
        self.sync_copy_selection();
        self.mark_dirty();
    }

    fn copy_word(&mut self, dir: i32, cols: u16, rows: u16) {
        // Simple word motion within viewport (no scroll across history for now)
        let Some(cs) = self.copy.as_ref() else { return; };
        let pane_id = cs.pane_id;
        let cur = cs.cursor;
        let Some(grid) = self.grids.get(&pane_id) else {
            self.copy_move(dir, 0, cols, rows);
            return;
        };
        // Build line string for current row
        let row = cur.1 as usize;
        let line = Self::grid_row_text(grid, row);
        let col = cur.0 as usize;
        let chars: Vec<char> = line.chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if dir > 0 {
            // forward: skip current word chars, then skip non-word, land at start of next word
            let mut i = col.min(chars.len());
            // if at word, skip rest of word
            if i < chars.len() && is_word(chars[i]) {
                while i < chars.len() && is_word(chars[i]) { i += 1; }
            }
            // skip non-word (spaces/punct) to next word
            while i < chars.len() && !is_word(chars[i]) { i += 1; }
            let new_col = (i as u16).min(cols.saturating_sub(1));
            if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = new_col; }
        } else {
            // backward: move to start of previous word
            let mut i = col.min(chars.len());
            if i == 0 {
                // try previous row
                if row > 0 {
                    let prev_line = Self::grid_row_text(grid, row - 1);
                    let _prev_chars: Vec<char> = prev_line.chars().collect();
                    let trimmed = prev_line.trim_end();
                    let end = trimmed.chars().count().min(cols as usize);
                    if let Some(cs) = self.copy.as_mut() {
                        cs.cursor = (end.saturating_sub(1) as u16, (row - 1) as u16);
                    }
                }
            } else {
                // step back one, skip non-word, then skip word to its start
                i = i.saturating_sub(1);
                while i > 0 && !is_word(chars[i]) { i -= 1; }
                while i > 0 && is_word(chars[i - 1]) { i -= 1; }
                if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = i as u16; }
            }
        }
        self.sync_copy_selection();
        self.mark_dirty();
    }

    fn grid_row_text(grid: &Grid, row: usize) -> String {
        if row >= grid.rows { return String::new(); }
        let cells = &grid.cells[row];
        let mut s = String::new();
        for c in cells {
            if c.cell_width == 0 { continue; }
            s.push_str(&c.text);
        }
        s.trim_end().to_string()
    }

    fn sync_copy_selection(&mut self) {
        let (pane_id, start, end, linewise) = match self.copy.as_ref() {
            Some(cs) if cs.anchor.is_some() => {
                let a = cs.anchor.unwrap();
                let b = cs.cursor;
                // Normalize order for daemon selection (viewport coords)
                let (s, e) = if cs.linewise {
                    // whole lines between rows
                    let top = a.1.min(b.1);
                    let bottom = a.1.max(b.1);
                    // need cols
                    let dims = self.copy_pane_dims(cs.pane_id).unwrap_or((80, 24));
                    ((0, top), (dims.0.saturating_sub(1), bottom))
                } else {
                    // charwise: ensure start <= end in row-major order
                    if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) }
                };
                (cs.pane_id, s, e, cs.linewise)
            }
            _ => return,
        };
        let _ = self.send(&Command::CopySetSelection { pane_id, start, end });
        let _ = linewise; // keep for future block selection parity
    }

    fn copy_yank(&mut self) {
        let Some(cs) = self.copy.as_ref() else { return; };
        let pane_id = cs.pane_id;
        let Some(grid) = self.grids.get(&pane_id) else { return; };
        let Some(anchor) = cs.anchor else {
            // No selection: yank current line? Like tmux if no selection, yank? For now do nothing
            return;
        };
        let cursor = cs.cursor;
        let linewise = cs.linewise;
        let text = if linewise {
            let top = anchor.1.min(cursor.1) as usize;
            let bottom = anchor.1.max(cursor.1) as usize;
            let mut out = String::new();
            for r in top..=bottom {
                let line = Self::grid_row_text(grid, r);
                out.push_str(&line);
                if r != bottom { out.push('\n'); }
            }
            out
        } else {
            // charwise: row-major extraction similar to Sel logic but using grid cells
            let (start, end) = if (anchor.1, anchor.0) <= (cursor.1, cursor.0) { (anchor, cursor) } else { (cursor, anchor) };
            let mut out = String::new();
            for r in start.1..=end.1 {
                let cells = grid.cells.get(r as usize);
                if cells.is_none() { continue; }
                let cells = cells.unwrap();
                let c0 = if r == start.1 { start.0 as usize } else { 0 };
                let c1 = if r == end.1 { end.0 as usize } else { cells.len().saturating_sub(1) };
                for c in c0..=c1.min(cells.len().saturating_sub(1)) {
                    let cell = &cells[c];
                    if cell.cell_width == 0 { continue; }
                    out.push_str(&cell.text);
                }
                if r != end.1 { out.push('\n'); }
            }
            // Trim trailing whitespace per row already handled? Keep as is.
            out
        };
        if !text.trim().is_empty() {
            crate::cli::util::copy_to_clipboard(&text);
            self.status_msg = Some((format!("copied {} chars", text.chars().count()), Instant::now()));
            self.notice = None;
        }
        let _ = self.send(&Command::CopyClearSelection { pane_id });
    }

    fn copy_next_hit(&mut self, forward: bool) {
        let (len, cur) = match self.copy.as_ref() {
            Some(cs) if !cs.hits.is_empty() => (cs.hits.len(), cs.hit_idx),
            _ => return,
        };
        let next = match cur {
            Some(idx) if forward => (idx + 1) % len,
            Some(idx) if !forward => (idx + len - 1) % len,
            None if forward => 0,
            None => len - 1,
            _ => 0,
        };
        self.copy_jump_to_hit(next);
    }

    fn on_copy_mouse(&mut self, m: MouseEvent) -> Result<()> {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.copy_scroll(-3);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            MouseEventKind::ScrollDown => {
                self.copy_scroll(3);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let x = m.column;
                let y = m.row;
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    if self.copy.as_ref().map(|c| c.pane_id == pid).unwrap_or(false) {
                        let inner = PaneGeom { pane_id: pid, rect }.inner();
                        if inner.contains(Position::new(x, y)) {
                            let cx = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let cy = y.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                            if let Some(cs) = self.copy.as_mut() {
                                cs.cursor = (cx, cy);
                            }
                            self.sync_copy_selection();
                            self.mark_dirty();
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let x = m.column;
                let y = m.row;
                if let Some(cs) = self.copy.as_ref() {
                    let pid = cs.pane_id;
                    if let Some((_, rect)) = self.rects.iter().find(|(id, _)| *id == pid) {
                        let inner = PaneGeom { pane_id: pid, rect: *rect }.inner();
                        if inner.contains(Position::new(x, y)) {
                            let cx = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let cy = y.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                            if let Some(cs) = self.copy.as_mut() {
                                // auto-start selection on drag if not yet anchoring
                                if cs.anchor.is_none() {
                                    cs.anchor = Some(cs.cursor);
                                    cs.linewise = false;
                                }
                                cs.cursor = (cx, cy);
                            }
                            self.sync_copy_selection();
                            self.mark_dirty();
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // nothing extra; selection already synced on drag
            }
            _ => {}
        }
        Ok(())
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
        self.popup.name = String::new();
        self.popup.cursor = 0;
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::NewWorktree(idx));
        self.popup.open = true;
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
            _ => {}
        }
    }

    fn on_popup_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc {
            self.popup.open = false;
            self.mark_dirty();
            return;
        }
        let word_back = KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => self.commit_name(),
            KeyCode::Backspace if key.modifiers.intersects(word_back) => self.popup_delete_word_backward(),
            KeyCode::Backspace => self.popup_backspace(),
            KeyCode::Char('h') if ctrl => self.popup_backspace(),
            KeyCode::Char('w') if ctrl => self.popup_delete_word_backward(),
            KeyCode::Char('u') if ctrl => self.popup_delete_to_start(),
            KeyCode::Delete if key.modifiers.intersects(word_back) => self.popup_delete_word_forward(),
            KeyCode::Delete => self.popup_delete_forward(),
            KeyCode::Left => self.popup.cursor = self.popup.cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = self.popup.name.chars().count();
                self.popup.cursor = self.popup.cursor.min(len).saturating_add(1).min(len);
            }
            KeyCode::Home => self.popup.cursor = 0,
            KeyCode::End => self.popup.cursor = self.popup.name.chars().count(),
            KeyCode::Char(c) if !ctrl => self.popup_insert(c),
            _ => {}
        }
        self.mark_dirty();
    }

    fn popup_insert(&mut self, ch: char) {
        let b = char_idx_to_byte(&self.popup.name, self.popup.cursor);
        self.popup.name.insert(b, ch);
        self.popup.cursor += 1;
    }

    fn popup_backspace(&mut self) {
        if self.popup.cursor == 0 {
            return;
        }
        let b = char_idx_to_byte(&self.popup.name, self.popup.cursor);
        let prev_len = self.popup.name[..b].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0);
        let start = b - prev_len;
        self.popup.name.replace_range(start..b, "");
        self.popup.cursor -= 1;
    }

    fn popup_delete_word_backward(&mut self) {
        let (name, cursor) = delete_word_backward(&self.popup.name, self.popup.cursor);
        self.popup.name = name;
        self.popup.cursor = cursor;
    }

    fn popup_delete_word_forward(&mut self) {
        let name = delete_word_forward(&self.popup.name, self.popup.cursor);
        self.popup.name = name;
    }

    fn popup_delete_forward(&mut self) {
        let len = self.popup.name.chars().count();
        if self.popup.cursor >= len {
            return;
        }
        let b = char_idx_to_byte(&self.popup.name, self.popup.cursor);
        let next_len = self.popup.name[b..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
        self.popup.name.replace_range(b..b + next_len, "");
    }

    fn popup_delete_to_start(&mut self) {
        let b = char_idx_to_byte(&self.popup.name, self.popup.cursor);
        self.popup.name.replace_range(..b, "");
        self.popup.cursor = 0;
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
                let _ = self.send(&Command::WorktreeCreate { session, branch: name });
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
            "settings" => {
                self.settings.open = true;
                self.settings.tab = 0;
                self.settings.selected = self.theme_idx;
            }
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
            CtxTarget::Session(_) => &["rename", "new worktree", "open worktree", "close"],
            CtxTarget::Tab(_, _) => &["new tab", "rename", "close"],
        }
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
            "close" => match target {
                CtxTarget::Pane(pid) => {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneClose { session, pane_id: Some(pid) });
                    }
                }
                CtxTarget::Session(idx) => {
                    let name = self
                        .layout
                        .as_ref()
                        .and_then(|l| l.sessions.get(idx))
                        .map(|s| s.name.clone());
                    if let Some(name) = name {
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
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn open_keybind_overlay(&mut self) {
        self.keybind_overlay.open = true;
        self.keybind_overlay.scroll = 0;
        self.mark_dirty();
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        if self.leader.is_leader(key) || key.code == KeyCode::Esc || key.code == KeyCode::Char('?') {
            self.keybind_overlay.open = false;
            self.mark_dirty();
            return;
        }
        let max = self.keybind_overlay_scroll_max();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.keybind_overlay.scroll = (self.keybind_overlay.scroll + 1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.keybind_overlay.scroll = self.keybind_overlay.scroll.saturating_sub(1);
            }
            KeyCode::Home => self.keybind_overlay.scroll = 0,
            KeyCode::End => self.keybind_overlay.scroll = max,
            _ => {}
        }
        self.mark_dirty();
    }

    fn keybind_overlay_scroll_max(&self) -> u16 {
        let Some(dd) = self.keybind_overlay_rect() else { return 0 };
        let lines = keybind_lines(&self.keymap).len();
        let visible = dd.height.saturating_sub(4) as usize;
        lines.saturating_sub(visible) as u16
    }

    fn keybind_overlay_rect(&self) -> Option<Rect> {
        let (w, h) = (self.cols, self.rows);
        let max_keys = self.keymap.iter().map(|b| b.keys.chars().count()).max().unwrap_or(4) as u16;
        let max_desc = self.keymap.iter().map(|b| b.desc.chars().count()).max().unwrap_or(10) as u16;
        let inner = (max_keys + 2 + max_desc).max(20);
        let width = (inner + 6).min(w.saturating_sub(4));
        let lines = keybind_lines(&self.keymap).len();
        let height = ((lines + 4) as u16).min(h.saturating_sub(4)).max(3);
        if w < width || h < height {
            return None;
        }
        Some(Rect::new((w - width) / 2, (h - height) / 2, width, height))
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
                if tab == SettingsTab::Appearance {
                    self.settings.selected = (self.settings.selected + 1).min(self.themes_len().saturating_sub(1));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if tab == SettingsTab::Appearance {
                    self.settings.selected = self.settings.selected.saturating_sub(1);
                }
            }
            KeyCode::Enter if tab == SettingsTab::Appearance => {
                let _ = self.send(&Command::SetTheme { idx: self.settings.selected });
            }
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

    // ------------------------------------------------------------------
    // Mouse
    // ------------------------------------------------------------------

    pub fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        // Clicks and drags write pane bytes themselves: flush the pending wheel
        // batch first so ordering against them is preserved. Hover moves and
        // further scrolling never write bytes, so they can keep coalescing.
        if !matches!(
            m.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp | MouseEventKind::Moved
        ) {
            self.flush_wheel()?;
        }
        self.set_link_mods(m.modifiers.intersects(link_modifiers()));
        if self.mode == Mode::Copy {
            return self.on_copy_mouse(m);
        }
        let x = m.column;
        let y = m.row;
        // Tab bar hover tracking (y==0) — update before click handling so close "x" appears
        if m.kind == MouseEventKind::Moved {
            let new_hover = self.tab_hit(x, y).map(|(idx, _)| idx);
            if new_hover != self.tab_hover {
                self.tab_hover = new_hover;
                self.update_tab_rects();
                self.mark_dirty();
            }
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) && self.update_notice_close_at(x, y) {
            if let Some((key, _)) = self.update_notice.clone() {
                let _ = self.send(&Command::UpdateDismiss { key });
            }
            self.update_notice = None;
            return Ok(());
        }
        if self.keybind_overlay.open {
            if matches!(
                m.kind,
                MouseEventKind::Down(_) | MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
            ) {
                self.keybind_overlay.open = false;
            }
            return Ok(());
        }
        if self.worktree_picker.open {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(i) = self.worktree_picker_item_at(x, y) {
                        self.worktree_picker.selected = i;
                        self.pick_worktree(i);
                    } else if self.worktree_picker_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        // Inside the picker but off a row: modal no-op.
                    } else {
                        self.worktree_picker.open = false;
                    }
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.worktree_picker_item_at(x, y) {
                        if self.worktree_picker.selected != i {
                            self.worktree_picker.selected = i;
                            self.mark_dirty();
                        }
                    }
                }
                MouseEventKind::ScrollDown => self.worktree_picker_move(1),
                MouseEventKind::ScrollUp => self.worktree_picker_move(-1),
                _ => {}
            }
            return Ok(());
        }
        if self.settings.open {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(tab) = self.settings_tab_at(x, y) {
                        self.settings_set_tab(tab);
                        return Ok(());
                    }
                    if let Some(i) = self.settings_item_at(x, y) {
                        self.settings.selected = i;
                        let _ = self.send(&Command::SetTheme { idx: i });
                        return Ok(());
                    }
                    if self.settings_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        return Ok(());
                    }
                    self.settings.open = false;
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.settings_item_at(x, y) {
                        if self.settings.selected != i {
                            self.settings.selected = i;
                            self.mark_dirty();
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.popup.open {
                    if let Some(btn) = self.name_popup_button_at(x, y) {
                        match btn {
                            PopupBtn::Enter => self.commit_name(),
                            PopupBtn::Cancel => self.popup.open = false,
                        }
                        return Ok(());
                    }
                    if self.name_popup_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        return Ok(());
                    }
                    self.popup.open = false;
                }
                if self.menu.open {
                    if let Some(i) = self.menu_item_at(x, y) {
                        self.menu_select(i)?;
                        return Ok(());
                    }
                    if self.menu_btn_at(x, y) {
                        self.menu.open = false;
                        return Ok(());
                    }
                    self.menu.open = false;
                }
                if self.ctx_menu.open {
                    if let Some(i) = self.ctx_menu_item_at(x, y) {
                        self.ctx_menu_select(i)?;
                        return Ok(());
                    }
                    if self.ctx_menu_at(x, y) {
                        return Ok(());
                    }
                    self.ctx_menu.open = false;
                }
                if self.menu_btn_at(x, y) {
                    self.menu.open = !self.menu.open;
                    self.menu.selected = 0;
                    return Ok(());
                }
                if y == 0 && self.tabs_area().contains(Position::new(x, y)) {
                    if let Some(r) = self.tab_left_arrow_rect() {
                        if r.contains(Position::new(x, y)) {
                            self.tab_scroll = self.tab_scroll.saturating_sub(1);
                            self.update_tab_rects();
                            self.mark_dirty();
                            return Ok(());
                        }
                    }
                    if let Some(r) = self.tab_right_arrow_rect() {
                        if r.contains(Position::new(x, y)) {
                            if let Some(sess) = self.active_session() {
                                self.tab_scroll = (self.tab_scroll + 1).min(sess.tabs.len().saturating_sub(1));
                                self.update_tab_rects();
                                self.mark_dirty();
                            }
                            return Ok(());
                        }
                    }
                    if let Some(pr) = self.plus_rect {
                        if pr.contains(Position::new(x, y)) {
                            if let Some(sess) = self.active_session().map(|s| s.name.clone()) {
                                let _ = self.send(&Command::TabNew { session: sess, name: None, workspace: None });
                            }
                            return Ok(());
                        }
                    }
                }
                if let Some((idx, is_close)) = self.tab_hit(x, y) {
                    if let Some(sess) = self.active_session().cloned() {
                        let tab_name = sess.tabs.get(idx).map(|t| t.name.clone()).unwrap_or_default();
                        if is_close {
                            let _ = self.send(&Command::TabClose { session: sess.name, tab: Some(tab_name) });
                        } else {
                            let _ = self.send(&Command::TabFocus { session: sess.name, tab: tab_name });
                        }
                    }
                    return Ok(());
                }
                if self.sidebar_open && x < SIDEBAR_WIDTH && self.sidebar_hit(x, y) {
                    return Ok(());
                }
                if let Some((split_id, dir, area)) = self.splitter_at(x, y) {
                    self.drag = Some(SplitDrag { split_id, dir, area });
                    return Ok(());
                }
                if m.modifiers.intersects(link_modifiers()) {
                    if let Some((pid, rect)) = self.pane_at(x, y) {
                        let inner = PaneGeom { pane_id: pid, rect }.inner();
                        let col = x.saturating_sub(inner.x);
                        let row = y.saturating_sub(inner.y);
                        if let Some(url) = self.link_at(pid, col, row) {
                            let session = self.active_session().map(|s| s.name.clone());
                            if let Some(session) = session {
                                let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                            }
                            crate::cli::util::open_url(&url);
                            return Ok(());
                        }
                    }
                }
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                    let inner = PaneGeom { pane_id: pid, rect }.inner();
                    let col = x.saturating_sub(inner.x);
                    let row = y.saturating_sub(inner.y);
                    let reporting = self.pane_mouse_reporting(pid);
                    if reporting {
                        self.pending_click = Some(PendingClick { pane_id: pid, col, row });
                        let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                        let bytes = sgr_mouse(b, col + 1, row + 1, false);
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    } else {
                        if let Some(old) = self.sel {
                            if old.pane_id != pid {
                                self.sel = None;
                            }
                        }
                        self.sel = Some(Sel { pane_id: pid, start: (col, row), end: (col, row) });
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.popup.open || self.menu.open {
                    return Ok(());
                }
                if self.ctx_menu_at(x, y) {
                    self.ctx_menu.open = false;
                    return Ok(());
                }
                if y == 0 && self.tabs_area().contains(Position::new(x, y)) {
                    if let Some((tab_idx, _)) = self.tab_hit(x, y) {
                        if let Some(s_idx) = self.layout.as_ref().and_then(|l| l.sessions.iter().position(|s| Some(&s.name)==l.active.as_ref())) {
                            self.open_ctx_menu(x, y, CtxTarget::Tab(s_idx, tab_idx));
                            return Ok(());
                        }
                    } else {
                        // Right-click on empty tab bar: new tab menu
                        if let Some(s_idx) = self.layout.as_ref().and_then(|l| l.sessions.iter().position(|s| Some(&s.name)==l.active.as_ref())) {
                            let t_idx = self.active_session().map(|s| s.active_tab).unwrap_or(0);
                            self.open_ctx_menu(x, y, CtxTarget::Tab(s_idx, t_idx));
                            return Ok(());
                        }
                    }
                }
                if let Some(idx) = self.sidebar_session_at(x, y) {
                    self.open_ctx_menu(x, y, CtxTarget::Session(idx));
                    return Ok(());
                }
                if let Some((pid, _)) = self.pane_at(x, y) {
                    self.open_ctx_menu(x, y, CtxTarget::Pane(pid));
                } else {
                    self.ctx_menu.open = false;
                }
                return Ok(());
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = self.drag {
                    let ratio = match drag.dir {
                        SplitDir::Vertical => {
                            (x.saturating_sub(drag.area.x)) as f32 / (drag.area.width.max(1)) as f32
                        }
                        SplitDir::Horizontal => {
                            (y.saturating_sub(drag.area.y)) as f32 / (drag.area.height.max(1)) as f32
                        }
                    };
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneResizeTo {
                            session,
                            split_id: drag.split_id,
                            ratio: ratio.clamp(0.05, 0.95),
                        });
                    }
                    return Ok(());
                }
                let sel = self.sel;
                if let Some(sel) = sel {
                    if let Some((pid, rect)) = self.pane_at(x, y) {
                        if pid == sel.pane_id {
                            let inner = PaneGeom { pane_id: pid, rect }.inner();
                            let c = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let r = y.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                            self.sel = Some(Sel { pane_id: pid, start: sel.start, end: (c, r) });
                            // Repaint live so the highlight follows the pointer.
                            self.mark_dirty();
                        }
                    }
                    return Ok(());
                }
                if let Some(pc) = self.pending_click {
                    let pos = self
                        .pane_at(x, y)
                        .filter(|(pid, _)| *pid == pc.pane_id)
                        .map(|(pid, rect)| {
                            let i = PaneGeom { pane_id: pid, rect }.inner();
                            (x.saturating_sub(i.x) + 1, y.saturating_sub(i.y) + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    let bytes = sgr_mouse(b + 32, pos.0, pos.1, false);
                    let _ = self.send(&Command::PaneWrite { pane_id: pc.pane_id, bytes });
                    return Ok(());
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                if let Some(pc) = self.pending_click.take() {
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    let up = self
                        .pane_at(x, y)
                        .filter(|(pid, _)| *pid == pc.pane_id)
                        .map(|(pid, rect)| {
                            let i = PaneGeom { pane_id: pid, rect }.inner();
                            (x.saturating_sub(i.x) + 1, y.saturating_sub(i.y) + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    let bytes = sgr_mouse(b, up.0, up.1, true);
                    let _ = self.send(&Command::PaneWrite { pane_id: pc.pane_id, bytes });
                } else if let Some(sel) = self.sel {
                    if sel.start != sel.end {
                        let text = self.selection_text(&sel);
                        if !text.is_empty() {
                            crate::cli::util::copy_to_clipboard(&text);
                            self.status_msg = Some(("copied to clipboard".to_string(), Instant::now()));
                        }
                    } else {
                        self.sel = None;
                    }
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if self.sidebar_open && self.sidebar_wheel(x, y, up) {
                    return Ok(());
                }
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                    let inner = PaneGeom { pane_id: pid, rect }.inner();
                    let col = x.saturating_sub(inner.x) + 1;
                    let row = y.saturating_sub(inner.y) + 1;
                    if self.pane_mouse_reporting(pid) {
                        let b = if up { 64 } else { 65 };
                        let bytes = sgr_mouse(b, col, row, false);
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    } else if self.pane_alt_screen(pid) {
                        let bytes: Vec<u8> = if up { b"\x1b[A".to_vec() } else { b"\x1b[B".to_vec() };
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    } else {
                        let _ = self.send(&Command::PaneScroll { pane_id: pid, up });
                    }
                }
            }
            MouseEventKind::Moved => {
                if self.popup.open {
                    // Hover highlights a popup button (repaint on change).
                    let hover = self.name_popup_button_at(x, y);
                    if self.popup.hover != hover {
                        self.popup.hover = hover;
                        self.mark_dirty();
                    }
                    return Ok(());
                }
                if self.menu.open {
                    // Modal menu: hovering moves the selection like j/k.
                    if let Some(i) = self.menu_item_at(x, y) {
                        if self.menu.selected != i {
                            self.menu.selected = i;
                            self.mark_dirty();
                        }
                    }
                    return Ok(());
                }
                if self.ctx_menu.open {
                    if let Some(i) = self.ctx_menu_item_at(x, y) {
                        if self.ctx_menu.selected != i {
                            self.ctx_menu.selected = i;
                            self.mark_dirty();
                        }
                    }
                    return Ok(());
                }
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    if self.pane_mouse_reporting(pid) {
                        let inner = PaneGeom { pane_id: pid, rect }.inner();
                        let col = x.saturating_sub(inner.x) + 1;
                        let row = y.saturating_sub(inner.y) + 1;
                        let bytes = sgr_mouse(35, col, row, false);
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    }
                }
            }
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn pane_mouse_reporting(&self, pid: u64) -> bool {
        self.active_session()
            .and_then(|s| find_pane_in_session(s, pid))
            .map(|p| p.mouse_reporting)
            .unwrap_or(false)
    }

    fn pane_alt_screen(&self, pid: u64) -> bool {
        self.active_session()
            .and_then(|s| find_pane_in_session(s, pid))
            .map(|p| p.alt_screen)
            .unwrap_or(false)
    }

    fn link_at(&self, pid: u64, col: u16, row: u16) -> Option<String> {
        let grid = self.grids.get(&pid)?;
        let links = grid.links.get(&row)?;
        links.iter().find(|l| col >= l.start && col < l.end).map(|l| l.url.clone())
    }

    fn selection_text(&self, sel: &Sel) -> String {
        let Some(grid) = self.grids.get(&sel.pane_id) else { return String::new() };
        let (mut r0, mut c0, mut r1, mut c1) = (sel.start.1, sel.start.0, sel.end.1, sel.end.0);
        if r1 < r0 || (r1 == r0 && c1 < c0) {
            std::mem::swap(&mut r0, &mut r1);
            std::mem::swap(&mut c0, &mut c1);
        }
        let mut lines = Vec::new();
        for row in r0..=r1 {
            let Some(cells) = grid.cells.get(row as usize) else { continue };
            let mut line = String::new();
            let start = if row == r0 { c0 } else { 0 };
            // Inclusive of the end column on the last row (both drag corners
            // select the cell under the pointer), like a terminal's selection.
            let end = if row == r1 { c1.saturating_add(1) } else { cells.len() as u16 };
            for (i, cell) in cells.iter().enumerate() {
                let ci = i as u16;
                if ci < start || ci >= end || cell.cell_width == 0 {
                    continue;
                }
                line.push_str(&cell.text);
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    // ------------------------------------------------------------------
    // Sidebar hit-testing
    // ------------------------------------------------------------------

    fn sidebar_footer_y(&self) -> u16 {
        self.rows.saturating_sub(self.status_h() + 1)
    }

    fn content_region_h(&self) -> u16 {
        self.sidebar_footer_y().saturating_sub(2)
    }

    fn visible_sidebar_tabs(&self) -> Vec<SidebarTab> {
        let cfg = kumo_core::config::sidebar();
        let mut out = Vec::new();
        for sec in cfg.order.iter().copied() {
            let tab = SidebarTab::from_section(sec);
            let visible = match tab {
                SidebarTab::Sessions => cfg.sections.sessions,
                SidebarTab::Agents => cfg.sections.agents,
            };
            if visible && !out.contains(&tab) {
                out.push(tab);
            }
        }
        // Fallback if config filtered everything (guard in config, but keep robust).
        if out.is_empty() {
            out.push(SidebarTab::Sessions);
        }
        out
    }

    fn ensure_sidebar_tab_visible(&mut self) {
        let visible = self.visible_sidebar_tabs();
        if !visible.contains(&self.sidebar_tab) {
            if let Some(first) = visible.first().copied() {
                self.sidebar_tab = first;
            }
        }
    }

    fn tab_at(&self, x: u16, y: u16) -> Option<SidebarTab> {
        if y != 2 || x >= SIDEBAR_WIDTH {
            return None;
        }
        let tabs = self.visible_sidebar_tabs();
        if tabs.is_empty() {
            return None;
        }
        if tabs.len() == 1 {
            return Some(tabs[0]);
        }
        // Ordered left→right: first in cfg.order is left half, second is right half.
        let half = (SIDEBAR_WIDTH / 2).max(4);
        let idx = if x < half { 0 } else { 1.min(tabs.len() - 1) };
        Some(tabs[idx])
    }

    fn sessions_content(&self) -> Vec<SidebarRow> {
        let mut out = Vec::new();
        if let Some(layout) = &self.layout {
            for (i, s) in layout.sessions.iter().enumerate() {
                out.push(SidebarRow::Session(i));
                if let Some(branch) = &s.branch {
                    out.push(SidebarRow::Branch(i, branch.clone()));
                }
            }
        }
        out.push(SidebarRow::NewSession);
        out
    }

    fn agent_rank(status: AgentStatus) -> u8 {
        match status {
            AgentStatus::Blocked => 0,
            AgentStatus::Working => 1,
            AgentStatus::Idle => 2,
        }
    }

    fn agents_content(&self) -> Vec<SidebarRow> {
        let mut out: Vec<(u8, usize, u64, SidebarRow)> = Vec::new();
        if let Some(layout) = &self.layout {
            for (i, s) in layout.sessions.iter().enumerate() {
                for (_, pane) in session_panes_all(s) {
                    if let Some(agent) = pane.agent.as_ref() {
                        if pane.is_ai {
                            let status = agent.status;
                            let rank = Self::agent_rank(status);
                            let agent_name = agent.name.clone();
                            let ws = short_workspace(&s.workspace);
                            out.push((rank, i, pane.id, SidebarRow::AgentDir(i, pane.id, ws, status)));
                            out.push((rank, i, pane.id, SidebarRow::AgentName(i, pane.id, agent_name, status)));
                        }
                    }
                }
            }
        }
        out.sort_by_key(|(rank, i, pid, _)| (*rank, *i, *pid));
        out.into_iter().map(|(_, _, _, row)| row).collect()
    }

    fn effective_sidebar_tab(&self) -> SidebarTab {
        let visible = self.visible_sidebar_tabs();
        if visible.contains(&self.sidebar_tab) { self.sidebar_tab } else { visible[0] }
    }

    fn active_tab_items(&self) -> Vec<SidebarRow> {
        match self.effective_sidebar_tab() {
            SidebarTab::Sessions => self.sessions_content(),
            SidebarTab::Agents => self.agents_content(),
        }
    }

    fn active_scroll(&self) -> u16 {
        match self.effective_sidebar_tab() {
            SidebarTab::Sessions => self.sidebar_scroll.0,
            SidebarTab::Agents => self.sidebar_scroll.1,
        }
    }

    fn set_active_scroll(&mut self, v: u16) {
        match self.effective_sidebar_tab() {
            SidebarTab::Sessions => self.sidebar_scroll.0 = v,
            SidebarTab::Agents => self.sidebar_scroll.1 = v,
        }
    }

    fn active_scroll_max(&self) -> u16 {
        let region_h = self.content_region_h() as usize;
        self.active_tab_items().len().saturating_sub(region_h) as u16
    }

    fn sidebar_rows(&self) -> Vec<(u16, SidebarRow)> {
        let mut out = vec![
            (0, SidebarRow::Header("kumo".into())),
            (1, SidebarRow::Spacer),
        ];
        let region_h = self.content_region_h() as usize;
        let items = self.active_tab_items();
        let offset = (self.active_scroll() as usize).min(items.len().saturating_sub(region_h));
        for (i, item) in items.iter().skip(offset).take(region_h).enumerate() {
            out.push((3 + i as u16, item.clone()));
        }
        out
    }

    fn sidebar_wheel(&mut self, x: u16, y: u16, up: bool) -> bool {
        if !self.sidebar_open || x >= SIDEBAR_WIDTH {
            return false;
        }
        if y < 3 || y > self.sidebar_footer_y() {
            return false;
        }
        const STEP: u16 = 3;
        let max = self.active_scroll_max();
        let scroll = if up {
            self.active_scroll().saturating_sub(STEP)
        } else {
            self.active_scroll().saturating_add(STEP).min(max)
        };
        self.set_active_scroll(scroll);
        self.mark_dirty();
        true
    }

    fn sidebar_hit(&mut self, x: u16, y: u16) -> bool {
        if let Some(tab) = self.tab_at(x, y) {
            if tab != self.sidebar_tab {
                self.sidebar_tab = tab;
                self.mark_dirty();
            }
            return true;
        }
        for (ry, row) in self.sidebar_rows() {
            if ry != y {
                continue;
            }
            match row {
                SidebarRow::Session(i) | SidebarRow::Branch(i, _) => {
                    let name = self
                        .layout
                        .as_ref()
                        .and_then(|l| l.sessions.get(i))
                        .map(|s| s.name.clone());
                    if let Some(name) = name {
                        let _ = self.send(&Command::SessionFocus { name });
                    }
                    return true;
                }
                SidebarRow::AgentDir(i, pid, _, _) | SidebarRow::AgentName(i, pid, _, _) => {
                    let name = self.layout.as_ref().and_then(|l| l.sessions.get(i)).map(|s| s.name.clone());
                    if let Some(name) = name {
                        let _ = self.send(&Command::PaneFocus { session: name, pane_id: pid });
                    }
                    return true;
                }
                SidebarRow::NewSession => {
                    self.open_session_popup();
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    fn sidebar_session_at(&self, x: u16, y: u16) -> Option<usize> {
        if !self.sidebar_open || x >= SIDEBAR_WIDTH {
            return None;
        }
        self.sidebar_rows()
            .into_iter()
            .find(|(ry, _)| *ry == y)
            .and_then(|(_, row)| match row {
                SidebarRow::Session(i) => Some(i),
                _ => None,
            })
    }

    // ------------------------------------------------------------------
    // Overlay hit-testing (menus / popup / settings)
    // ------------------------------------------------------------------

    fn menu_btn_x(&self) -> u16 {
        let mode = if self.mode == Mode::Leader { "LEADER" } else { "NORMAL" };
        format!(" {} ", mode).chars().count() as u16 + 1
    }

    fn menu_btn_rect(&self) -> Option<Rect> {
        if self.status_h() == 0 || !self.status_bar_contains(StatusWidget::Menu) {
            return None;
        }
        let bw = MENU_BTN.chars().count() as u16;
        let x = self.menu_btn_x();
        (self.cols >= x + bw).then(|| Rect::new(x, self.rows.saturating_sub(1), bw, 1))
    }

    fn menu_btn_at(&self, x: u16, y: u16) -> bool {
        self.menu_btn_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false)
    }

    fn menu_dropdown_rect(&self) -> Option<Rect> {
        if self.status_h() == 0 {
            return None;
        }
        let width = MENU_ITEMS.iter().map(|i| i.chars().count()).max().unwrap_or(0) as u16 + 4;
        let height = MENU_ITEMS.len() as u16 + 2;
        if self.cols < width || self.rows < height + 1 {
            return None;
        }
        let btn_w = MENU_BTN.chars().count() as u16;
        let x = (self.menu_btn_x() + btn_w).saturating_sub(width).min(self.cols.saturating_sub(width));
        let y = self.rows.saturating_sub(1).saturating_sub(height);
        Some(Rect::new(x, y, width, height))
    }

    fn menu_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.menu_dropdown_rect()?;
        MENU_ITEMS.iter().enumerate().position(|(i, _)| {
            let item = Rect::new(dd.x + 1, dd.y + 1 + i as u16, dd.width.saturating_sub(2), 1);
            item.contains(Position::new(x, y))
        })
    }

    fn ctx_menu_rect(&self) -> Option<Rect> {
        if !self.ctx_menu.open {
            return None;
        }
        let items = self.ctx_items();
        let width = items.iter().map(|i| i.chars().count()).max().unwrap_or(0) as u16 + 4;
        let height = items.len() as u16 + 2;
        if self.cols < width || self.rows < height {
            return None;
        }
        let px = self.ctx_menu.x;
        let py = self.ctx_menu.y;
        let x = if px.saturating_add(1) + width <= self.cols { px + 1 } else { px.saturating_sub(width) };
        let y = if py + 1 + height <= self.rows { py + 1 } else { py.saturating_sub(height) };
        Some(Rect::new(x, y, width, height))
    }

    fn ctx_menu_at(&self, x: u16, y: u16) -> bool {
        self.ctx_menu_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false)
    }

    fn ctx_menu_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.ctx_menu_rect()?;
        let items = self.ctx_items();
        items.iter().enumerate().position(|(i, _)| {
            let item = Rect::new(dd.x + 1, dd.y + 1 + i as u16, dd.width.saturating_sub(2), 1);
            item.contains(Position::new(x, y))
        })
    }

    fn name_popup_rect(&self) -> Option<Rect> {
        if self.cols < SESSION_POPUP_W || self.rows < SESSION_POPUP_H {
            return None;
        }
        Some(Rect::new(
            (self.cols - SESSION_POPUP_W) / 2,
            (self.rows - SESSION_POPUP_H) / 2,
            SESSION_POPUP_W,
            SESSION_POPUP_H,
        ))
    }

    fn name_popup_input_cursor(&self) -> Option<(u16, u16)> {
        let dd = self.name_popup_rect()?;
        let text_w = (dd.width - 4) as usize - 1;
        let name = &self.popup.name;
        let cursor = self.popup.cursor.min(name.chars().count());
        let end = cursor + 1;
        let start = end.saturating_sub(text_w);
        let col = dd.x + 2 + cursor.saturating_sub(start) as u16;
        Some((col, dd.y + 3))
    }

    fn name_popup_button_rect(&self, btn: PopupBtn) -> Option<Rect> {
        let dd = self.name_popup_rect()?;
        let label = match btn {
            PopupBtn::Enter => "⏎ enter ",
            PopupBtn::Cancel => " esc cancel ",
        };
        let w = label.chars().count() as u16;
        let x = match btn {
            PopupBtn::Enter => dd.x + 2,
            PopupBtn::Cancel => dd.x + 2 + 10,
        };
        Some(Rect::new(x, dd.y + 4, w, 1))
    }

    fn name_popup_button_at(&self, x: u16, y: u16) -> Option<PopupBtn> {
        [PopupBtn::Enter, PopupBtn::Cancel].into_iter().find(|btn| {
            self.name_popup_button_rect(*btn).map(|r| r.contains(Position::new(x, y))).unwrap_or(false)
        })
    }

    fn settings_rect(&self) -> Option<Rect> {
        if self.cols < 30 || self.rows < 12 {
            return None;
        }
        let width = (self.cols * 3 / 5).max(50).min(self.cols.saturating_sub(4));
        let height = (self.rows * 3 / 5).max(18).min(self.rows.saturating_sub(4));
        Some(Rect::new((self.cols - width) / 2, (self.rows - height) / 2, width, height))
    }

    fn settings_tabs_rect(&self) -> Option<Rect> {
        let dd = self.settings_rect()?;
        Some(Rect::new(dd.x + 2, dd.y + 2, 16, dd.height.saturating_sub(4)))
    }

    fn settings_content_rect(&self) -> Option<Rect> {
        let dd = self.settings_rect()?;
        let w = dd.width.saturating_sub(4).saturating_sub(17);
        Some(Rect::new(dd.x + 2 + 16 + 1, dd.y + 2, w, dd.height.saturating_sub(4)))
    }

    fn settings_tab_at(&self, x: u16, y: u16) -> Option<usize> {
        let tabs = self.settings_tabs_rect()?;
        SETTINGS_TABS.iter().enumerate().position(|(i, _)| {
            let item = Rect::new(tabs.x, tabs.y + i as u16, tabs.width, 1);
            item.contains(Position::new(x, y))
        })
    }

    fn settings_item_at(&self, x: u16, y: u16) -> Option<usize> {
        if SETTINGS_TABS.get(self.settings.tab).copied() != Some(SettingsTab::Appearance) {
            return None;
        }
        let content = self.settings_content_rect()?;
        let len = self.themes_len();
        (0..len).position(|i| {
            let item = Rect::new(content.x, content.y + 1 + i as u16, content.width, 1);
            item.contains(Position::new(x, y))
        })
    }

    fn update_notice_lines(&self) -> Option<(String, String)> {
        let (_, display) = self.update_notice.as_ref()?;
        Some((
            format!("New version {} available", display),
            "run 'kumo update'".to_string(),
        ))
    }

    fn update_notice_rect(&self) -> Option<Rect> {
        let (line1, line2) = self.update_notice_lines()?;
        let inner_w = line1.chars().count().max(line2.chars().count()) as u16 + 6;
        let width = inner_w + 2;
        if self.cols < width + 1 || self.rows < 4 {
            return None;
        }
        Some(Rect::new(self.cols - width - 1, 0, width, 4))
    }

    fn update_notice_close_at(&self, x: u16, y: u16) -> bool {
        let Some(r) = self.update_notice_rect() else { return false };
        x == r.x + 2 && y == r.y + 1
    }

    // ------------------------------------------------------------------
    // Rendering
    // ------------------------------------------------------------------

    pub fn on_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.cols = cols.max(2);
        self.rows = rows.max(2);
        self.recompute_geometry();
        // Clamp copy cursor inside new dims
        let pid = self.copy.as_ref().map(|c| c.pane_id);
        if let Some(pid) = pid {
            if let Some((w, h)) = self.copy_pane_dims(pid) {
                if let Some(cs) = self.copy.as_mut() {
                    cs.cursor.0 = cs.cursor.0.min(w.saturating_sub(1));
                    cs.cursor.1 = cs.cursor.1.min(h.saturating_sub(1));
                    if let Some(a) = cs.anchor.as_mut() {
                        a.0 = a.0.min(w.saturating_sub(1));
                        a.1 = a.1.min(h.saturating_sub(1));
                    }
                }
            }
        }
        Ok(())
    }

    fn expire_timers(&mut self) {
        let now = Instant::now();
        if let Some(t) = self.pane_numbers {
            if now.duration_since(t) > PANE_NUMBERS_TIMEOUT {
                self.pane_numbers = None;
            }
        }
        if let Some((_, t)) = &self.status_msg {
            if now.duration_since(*t) > TOAST_TIMEOUT {
                self.status_msg = None;
            }
        }
        if let Some((_, t)) = &self.notice {
            if now.duration_since(*t) > TOAST_TIMEOUT {
                self.notice = None;
            }
        }
    }

    /// Redraw the whole frame into `terminal`. Uses ratatui's diffing, so the
    /// host terminal only receives the changed cells.
    pub fn render_now(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        self.expire_timers();
        terminal.draw(|f| self.draw(f))?;
        self.place_cursor(terminal)?;
        self.dirty = false;
        Ok(())
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let link_mods = self.link_mods;
        let selected = self.sel;
        let theme = self.current_theme();
        // Pane frames (borders + titles + content).
        for &(pid, rect) in &self.rects {
            let focused = self.active_tab().map(|t| t.focus == pid).unwrap_or(false);
            let title = self.pane_title(pid, focused);
            self.render_pane_frame(f, rect, focused, &title);
            if let Some(grid) = self.grids.get_mut(&pid) {
                render_pane_content(f, pid, rect, grid, selected, link_mods, &theme);
            }
            // Copy-mode overlay (cursor + search hits + selection fallback)
            if let Some(cs) = self.copy.as_ref() {
                if cs.pane_id == pid {
                    self.render_copy_overlay(f, pid, rect);
                }
            }
        }
        self.render_tab_bar(f);
        self.render_pane_numbers(f);
        if self.sidebar_open {
            self.render_sidebar(f, area);
        }
        self.render_status(f);
        self.render_copy_search_bar(f);
        self.render_menu(f);
        self.render_ctx_menu(f);
        self.render_name_popup(f);
        self.render_update_notice(f);
        self.render_keybind_overlay(f);
        self.render_settings(f);
        self.render_worktree_picker(f);
    }

    fn pane_label(&self, pid: u64) -> String {
        self.active_session()
            .and_then(|s| find_pane_in_session(s, pid))
            .map(|p| p.title.clone())
            .unwrap_or_else(|| " pane ".to_string())
    }

    fn pane_title(&self, pid: u64, focused: bool) -> String {
        let base = self.pane_label(pid);
        if focused && self.session_zoom() {
            format!("{base}(zoom) ")
        } else {
            base
        }
    }

    fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, title: &str) {
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        let theme = self.current_theme();
        // A blocked AI pane glows orange even when it does not have focus.
        let blocked = self.rects.iter().any(|(pid, r)| {
            *r == rect
                && self
                    .active_session()
                    .and_then(|s| find_pane_in_session(s, *pid))
                    .map(|p| p.agent.as_ref().map(|a| a.status == AgentStatus::Blocked).unwrap_or(false))
                    .unwrap_or(false)
        });
        let border = if blocked {
            theme.orange
        } else if focused {
            theme.accent
        } else {
            theme.border_idle
        };
        let style_cfg = kumo_core::config::sidebar_borders().style;
        if style_cfg == kumo_core::config::BorderStyle::Hidden {
            // Hidden: no border, just title chip at top-left inset.
            let max = rect.width.saturating_sub(2) as usize;
            let chip = if focused {
                Style::default().fg(RColor::Black).bg(theme.accent).add_modifier(Modifier::BOLD)
            } else if blocked {
                Style::default().fg(RColor::Black).bg(theme.orange).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg).bg(RColor::Reset)
            };
            for (i, ch) in title.chars().take(max).enumerate() {
                put(f, rect.x + 1 + i as u16, rect.y, &ch.to_string(), chip);
            }
            return;
        }
        let border_style = Style::default().fg(border).bg(RColor::Reset);
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        let (tl, tr, bl, br, h, v) = border_chars(style_cfg);
        put(f, x0, y0, tl, border_style);
        put(f, x1, y0, tr, border_style);
        put(f, x0, y1, bl, border_style);
        put(f, x1, y1, br, border_style);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, h, border_style);
            put(f, x, y1, h, border_style);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, v, border_style);
            put(f, x1, y, v, border_style);
        }
        // Title chip.
        let max = rect.width.saturating_sub(2) as usize;
        let chip = if focused {
            Style::default().fg(RColor::Black).bg(theme.accent).add_modifier(Modifier::BOLD)
        } else if blocked {
            Style::default().fg(RColor::Black).bg(theme.orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg).bg(RColor::Reset)
        };
        for (i, ch) in title.chars().take(max).enumerate() {
            put(f, x0 + 1 + i as u16, y0, &ch.to_string(), chip);
        }
    }

    fn render_pane_numbers(&self, f: &mut Frame) {
        let Some(started) = self.pane_numbers else { return };
        if started.elapsed() > PANE_NUMBERS_TIMEOUT {
            return;
        }
        let theme = self.current_theme();
        if self.rects.len() < 2 {
            return;
        }
        let style = Style::default().fg(RColor::Black).bg(theme.accent).add_modifier(Modifier::BOLD);
        for (i, (_, rect)) in self.rects.iter().enumerate() {
            let Some(digit) = char::from_digit((i + 1) as u32, 10) else { continue };
            put(f, rect.x + rect.width / 2, rect.y + rect.height / 2, &digit.to_string(), style);
        }
    }

    fn render_tab_bar(&self, f: &mut Frame) {
        let Some(sess) = self.active_session() else { return };
        if sess.tabs.is_empty() { return; }
        let theme = self.current_theme();
        let area = self.tabs_area();
        // Bar background distinct from terminal
        let bar_bg = theme.panel_sep;
        fill(f, area, bar_bg);
        // Arrows for overflow
        if let Some(r) = self.tab_left_arrow_rect() {
            put(f, r.x, r.y, "‹", Style::default().fg(theme.panel_muted).bg(bar_bg));
        }
        if let Some(r) = self.tab_right_arrow_rect() {
            put(f, r.x, r.y, "›", Style::default().fg(theme.panel_muted).bg(bar_bg));
        }
        for (idx, pill, close) in &self.tab_rects {
            let Some(tab) = sess.tabs.get(*idx) else { continue };
            let active = sess.active_tab == *idx;
            let is_hover = self.tab_hover == Some(*idx);
            let pill_bg = if active { theme.accent } else { lighten(bar_bg, 14) };
            let fg = if active { RColor::Rgb(0x0a,0x0a,0x0a) } else { theme.fg };
            fill(f, *pill, pill_bg);
            let base_style = if active {
                Style::default().fg(RColor::Rgb(0x0a,0x0a,0x0a)).bg(pill_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg).bg(pill_bg)
            };
            // Draw name left-aligned at first cell; last cell reserved for x
            let name = &tab.name;
            let max_w = pill.width.saturating_sub(1);
            let name_w = (name.chars().count() as u16).min(max_w);
            text(f, pill.x, pill.y, name, base_style, name_w);
            if is_hover {
                let x_fg = if active { RColor::Rgb(0x0a, 0x0a, 0x0a) } else { theme.red };
                put(f, close.x, close.y, "x", Style::default().fg(x_fg).bg(pill_bg).add_modifier(Modifier::BOLD));
            } else if !active {
                // keep last cell as subtle close hint placeholder (dim)
                put(f, close.x, close.y, " ", base_style);
            }
        }
        // Plus button
        if let Some(pr) = self.plus_rect {
            let plus_bg = lighten(bar_bg, 10);
            fill(f, pr, plus_bg);
            let style = Style::default().fg(theme.panel_muted).bg(plus_bg).add_modifier(Modifier::BOLD);
            // Center "+" in the 3-wide pill
            put(f, pr.x + 1, pr.y, "+", style);
        }
    }

    fn render_sidebar(&self, f: &mut Frame, size: Rect) {
        let w = SIDEBAR_WIDTH.min(size.width);
        let theme = self.current_theme();
        let area = Rect::new(0, 0, w, size.height.saturating_sub(self.status_h()));
        fill(f, area, RColor::Reset);
        let bstyle = kumo_core::config::sidebar_borders().style;
        let sep_hidden = bstyle == kumo_core::config::BorderStyle::Hidden;
        if !sep_hidden {
            let (_, _, _, _, _, v) = border_chars(bstyle);
            // Sidebar separator follows pane border vertical style (rounded/single -> │)
            let sep = if bstyle == kumo_core::config::BorderStyle::Double { "║" }
                      else if bstyle == kumo_core::config::BorderStyle::Heavy { "┃" }
                      else { "│" };
            let _ = v; // keep helper unified
            for y in area.y..(area.y + area.height) {
                put(f, area.x + area.width, y, sep, Style::default().fg(theme.panel_sep));
            }
        }
        self.render_tabs(f, area, w);
        for (y, row) in self.sidebar_rows() {
            if y > area.y + area.height {
                break;
            }
            let x = area.x;
            let max = w.saturating_sub(2);
            match row {
                SidebarRow::Header(t) => {
                    let style = Style::default().fg(theme.accent).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    let pad = max.saturating_sub(t.chars().count() as u16) / 2;
                    text(f, x + pad, y, &t, style, max);
                }
                SidebarRow::Spacer => {
                    put(f, x, y, " ", Style::default().bg(RColor::Reset));
                }
                SidebarRow::Session(i) => {
                    let active = self.layout.as_ref().map(|l| l.active.as_deref() == Some(&self.session_name(i))).unwrap_or(false);
                    let bg = if active { theme.panel_sep } else { RColor::Reset };
                    let name = self.session_name(i);
                    if active {
                        fill(f, Rect::new(x, y, w, 1), bg);
                        put(f, x + 1, y, "▸", Style::default().fg(theme.accent).bg(bg));
                        text(f, x + 3, y, &name, Style::default().fg(theme.fg).bg(bg), max.saturating_sub(3));
                    } else {
                        put(f, x + 1, y, " ", Style::default().bg(bg));
                        text(f, x + 3, y, &name, Style::default().fg(theme.panel_muted).bg(bg), max.saturating_sub(3));
                    }
                }
                SidebarRow::Branch(i, b) => {
                    let active = self.layout.as_ref().map(|l| l.active.as_deref() == Some(&self.session_name(i))).unwrap_or(false);
                    let bg = if active { theme.panel_sep } else { RColor::Reset };
                    let name_color = if active { theme.fg } else { theme.panel_muted };
                    if active {
                        fill(f, Rect::new(x, y, w, 1), bg);
                    }
                    let avail = max.saturating_sub(4) as usize;
                    let suffix = match (b.ahead, b.behind) {
                        (0, 0) => String::new(),
                        (a, 0) => format!(" \u{2191}{}", a),
                        (0, be) => format!(" ~{}", be),
                        (a, be) => format!(" \u{2191}{}~{}", a, be),
                    };
                    let suffix_w = suffix.chars().count().min(avail);
                    let name_avail = avail.saturating_sub(suffix_w);
                    let shown = fit_branch_name(&b.name, name_avail);
                    text(f, x + 4, y, &shown, Style::default().fg(name_color).bg(bg), avail as u16);
                    let mut cx = x + 4 + shown.chars().count() as u16;
                    let mut remaining = (avail as u16).saturating_sub(shown.chars().count() as u16);
                    if b.ahead > 0 && remaining > 1 {
                        put(f, cx, y, " ", Style::default().bg(bg));
                        cx += 1;
                        remaining -= 1;
                        let s = format!("\u{2191}{}", b.ahead);
                        let wd = (s.chars().count() as u16).min(remaining);
                        text(f, cx, y, &s, Style::default().fg(theme.green).bg(bg), remaining);
                        cx += wd;
                        remaining = remaining.saturating_sub(wd);
                    }
                    if b.behind > 0 && remaining > 1 {
                        put(f, cx, y, " ", Style::default().bg(bg));
                        cx += 1;
                        remaining -= 1;
                        let s = format!("~{}", b.behind);
                        text(f, cx, y, &s, Style::default().fg(theme.orange).bg(bg), remaining);
                    }
                }
                SidebarRow::AgentDir(..) | SidebarRow::AgentName(..) => {
                    let (i, pid, third, status, is_dir) = match &row {
                        SidebarRow::AgentDir(i, pid, third, status) => (i, pid, third, status, true),
                        SidebarRow::AgentName(i, pid, third, status) => (i, pid, third, status, false),
                        _ => unreachable!(),
                    };
                    let session_active = self
                        .layout
                        .as_ref()
                        .map(|l| l.active.as_deref() == Some(&self.session_name(*i)))
                        .unwrap_or(false);
                    let pane_focused = self
                        .layout
                        .as_ref()
                        .and_then(|l| l.sessions.get(*i))
                        .map(|s| s.tabs.get(s.active_tab).map(|t| t.focus == *pid).unwrap_or(false))
                        .unwrap_or(false);
                    let focused = session_active && pane_focused;
                    let bg = if focused { theme.panel_sep } else { RColor::Reset };
                    if focused {
                        fill(f, Rect::new(x, y, w, 1), bg);
                    }
                    let status_color = match status {
                        AgentStatus::Working => theme.green,
                        AgentStatus::Blocked => theme.orange,
                        AgentStatus::Idle => theme.panel_muted,
                    };
                    let dot = if *status == AgentStatus::Blocked { "◉" } else { "●" };
                    let name_style = if *status == AgentStatus::Blocked {
                        Style::default().fg(status_color).bg(bg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(status_color).bg(bg)
                    };
                    if !is_dir {
                        put(f, x + 2, y, dot, Style::default().fg(status_color).bg(bg));
                    }
                    if is_dir {
                        let path_color = if focused { theme.fg } else { theme.panel_muted };
                        text(f, x + 4, y, third, Style::default().fg(path_color).bg(bg), max.saturating_sub(4));
                    } else {
                        let avail = max.saturating_sub(4) as usize;
                        let label = if *status == AgentStatus::Blocked
                            && third.chars().count() + " ·blocked".len() <= avail
                        {
                            format!("{third} ·blocked")
                        } else {
                            third.clone()
                        };
                        text(f, x + 4, y, &label, name_style, max.saturating_sub(4));
                    }
                }
                SidebarRow::NewSession => {
                    let style = Style::default().fg(theme.fg).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    text(f, x, y, "  + NEW SESSION", style, max);
                }
            }
        }
        // Scrollbar (rightmost sidebar column) when the active tab overflows.
        let scroll_x = w.saturating_sub(1);
        let region_h = self.content_region_h();
        let items = self.active_tab_items();
        if items.len() > region_h as usize {
            let offset = (self.active_scroll() as usize).min(items.len() - region_h as usize);
            draw_scrollbar(f, scroll_x, 3, region_h, offset, items.len(), &theme);
        }
    }

    fn session_name(&self, idx: usize) -> String {
        self.layout.as_ref().and_then(|l| l.sessions.get(idx)).map(|s| s.name.clone()).unwrap_or_default()
    }

    fn render_tabs(&self, f: &mut Frame, area: Rect, w: u16) {
        let theme = self.current_theme();
        let y = area.y + 2;
        let visible = self.visible_sidebar_tabs();
        if visible.is_empty() {
            return;
        }
        if visible.len() == 1 {
            let tab = visible[0];
            let bg = theme.panel_sep;
            fill(f, Rect::new(area.x, y, w, 1), bg);
            let style = Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            let label = tab.label().to_uppercase();
            let pad = w.saturating_sub(label.chars().count() as u16) / 2;
            text(f, area.x + pad, y, &label, style, w);
            return;
        }
        let half = (w / 2).max(1);
        let bstyle = kumo_core::config::sidebar_borders().style;
        let sep = if bstyle == kumo_core::config::BorderStyle::Hidden { " " }
                  else if bstyle == kumo_core::config::BorderStyle::Double { "║" }
                  else if bstyle == kumo_core::config::BorderStyle::Heavy { "┃" }
                  else { "│" };
        for (i, tab) in visible.iter().enumerate().take(2) {
            let x0 = area.x + i as u16 * half;
            let x1 = if i == 0 { x0 + half } else { area.x + w };
            let width = x1.saturating_sub(x0);
            if width == 0 {
                continue;
            }
            let active = *tab == self.sidebar_tab;
            let bg = if active { theme.panel_sep } else { RColor::Reset };
            fill(f, Rect::new(x0, y, width, 1), bg);
            let style = if active {
                Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.panel_muted).bg(bg)
            };
            let label = tab.label().to_uppercase();
            let pad = width.saturating_sub(label.chars().count() as u16) / 2;
            text(f, x0 + pad, y, &label, style, width);
        }
        put(f, area.x + half, y, sep, Style::default().fg(theme.panel_sep).bg(RColor::Reset));
    }

    fn render_status(&self, f: &mut Frame) {
        if self.status_h() == 0 {
            return;
        }
        let theme = self.current_theme();
        let area = Rect::new(0, self.rows.saturating_sub(1), self.cols, 1);
        fill(f, area, RColor::Reset);

        // Build widget contexts: all status widgets are client-side, derived from Layout.
        let session = self.active_session();
        let ctx = SlotContext {
            cfg: &self.status_bar,
            session,
            theme: &theme,
            hostname: &self.hostname,
            clock_str: &self.clock_str,
            is_ssh: self.is_ssh,
            is_leader: self.mode == Mode::Leader,
            menu_open: self.menu.open,
            sidebar_open: self.sidebar_open,
        };

        // Snapshot slots so we can iteratively drop low-priority widgets on overflow.
        let mut left_cfg = self.status_bar.left.clone();
        let mut center_cfg = self.status_bar.center.clone();
        let mut right_cfg = self.status_bar.right.clone();

        // Helper to measure a slot.
        let measure = |slot: &[StatusWidget]| -> u16 {
            let spans = status_bar::slot_spans(slot, &ctx);
            status_bar::spans_width(&spans)
        };

        // Priority of widgets to drop first when the bar overflows (least important first).
        // Mode/Menu are never dropped (they host the leader chip and the menu button).
        let drop_priority: &[StatusWidget] = &[
            StatusWidget::Hostname,
            StatusWidget::Clock,
            StatusWidget::Branch,
            StatusWidget::AgentStatus,
            StatusWidget::Session,
        ];

        // Reserve width for the right-aligned transient (leader hint or copied toast)
        // so it stays readable on a narrow terminal; it overlays the bar.
        let transient_w: u16 = if self.mode == Mode::Leader {
            let hint = bindings::leader_hint(&self.keymap);
            hint.chars().count() as u16
        } else if let Some((msg, t)) = &self.status_msg {
            if t.elapsed() < TOAST_TIMEOUT {
                msg.chars().count() as u16
            } else { 0 }
        } else { 0 };

        // Overflow loop: drop lowest-priority widget present until everything fits
        // or only Mode/Menu remain.
        loop {
            let left_w = measure(&left_cfg);
            let center_w = measure(&center_cfg);
            let right_w = measure(&right_cfg);
            let total = left_w + center_w + right_w + transient_w;
            // Gaps: 1 col between left|center and center|right when both sides non-empty
            let gaps = (if center_w > 0 && left_w > 0 { 1 } else { 0 }) + (if right_w > 0 && (center_w > 0 || left_w > 0) { 1 } else { 0 });
            if total + gaps <= area.width {
                break;
            }
            // Find the lowest-priority widget that is still present and removable.
            let mut removed = false;
            for &w in drop_priority {
                if right_cfg.contains(&w) {
                    right_cfg.retain(|&x| x != w);
                    removed = true;
                    break;
                }
                if center_cfg.contains(&w) {
                    center_cfg.retain(|&x| x != w);
                    removed = true;
                    break;
                }
                if left_cfg.contains(&w) {
                    // Never drop the last Session if it's the only left content beyond Mode/Menu,
                    // but allow dropping when overflow is severe.
                    if left_cfg.len() == 1 && left_cfg[0] == w && matches!(w, StatusWidget::Session) && left_cfg.len() == 1 {
                        // keep at least one indicator; fall through to truncation below
                        continue;
                    }
                    left_cfg.retain(|&x| x != w);
                    removed = true;
                    break;
                }
            }
            if !removed {
                break;
            }
        }

        let mut left_spans = status_bar::slot_spans(&left_cfg, &ctx);
        let mut center_spans = status_bar::slot_spans(&center_cfg, &ctx);
        let mut right_spans = status_bar::slot_spans(&right_cfg, &ctx);

        // Notice (⚠) is a left-side transient appended after the session widget,
        // like the pre-widget bar. Keep it visible even when widgets were dropped.
        if let Some((msg, t)) = &self.notice {
            if t.elapsed() < TOAST_TIMEOUT {
                if !left_spans.is_empty() {
                    left_spans.push(ratatui::text::Span::styled(" · ", Style::default().fg(theme.panel_muted)));
                }
                left_spans.push(ratatui::text::Span::styled(format!("⚠ {msg}"), Style::default().fg(theme.secondary)));
            }
        }

        let left_w = status_bar::spans_width(&left_spans);
        let center_w = status_bar::spans_width(&center_spans);
        let right_w = status_bar::spans_width(&right_spans);

        // If still too wide after dropping, truncate each slot proportionally.
        // Right is truncated first (it shares the edge with the transient).
        let gaps = (if center_w > 0 && left_w > 0 { 1 } else { 0 }) + (if right_w > 0 && (center_w > 0 || left_w > 0) { 1 } else { 0 });
        let total = left_w + center_w + right_w + transient_w + gaps;
        if total > area.width {
            let mut avail = area.width.saturating_sub(transient_w);
            // Reserve left first, then center, then right gets the remainder.
            let left_max = left_w.min(avail.saturating_sub(gaps));
            if status_bar::spans_width(&left_spans) > left_max {
                left_spans = status_bar::truncate_spans(left_spans, left_max);
            }
            avail = avail.saturating_sub(status_bar::spans_width(&left_spans)).saturating_sub(if left_spans.is_empty() {0} else {1});
            let center_max = center_w.min(avail);
            if status_bar::spans_width(&center_spans) > center_max {
                center_spans = status_bar::truncate_spans(center_spans, center_max);
            }
            avail = avail.saturating_sub(status_bar::spans_width(&center_spans)).saturating_sub(if center_spans.is_empty() {0} else {1});
            let right_max = right_w.min(avail);
            if status_bar::spans_width(&right_spans) > right_max {
                right_spans = status_bar::truncate_spans(right_spans, right_max);
            }
        }

        let left_w = status_bar::spans_width(&left_spans);
        let center_w = status_bar::spans_width(&center_spans);
        let right_w = status_bar::spans_width(&right_spans);

        if left_w > 0 {
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(left_spans)), Rect::new(area.x, area.y, left_w.min(area.width), 1));
        }
        if center_w > 0 {
            let x = area.x + (area.width.saturating_sub(center_w)) / 2;
            // Avoid overlapping left: push right if needed
            let x = x.max(area.x + left_w + if left_w > 0 { 1 } else { 0 });
            // Avoid overlapping right
            let max_w = area.width.saturating_sub(x).saturating_sub(right_w).saturating_sub(if right_w > 0 {1} else {0});
            let w = center_w.min(max_w);
            if w > 0 {
                let spans = status_bar::truncate_spans(center_spans, w);
                f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans)), Rect::new(x, area.y, w, 1));
            }
        }
        if right_w > 0 {
            let x = area.x + area.width.saturating_sub(right_w);
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(right_spans)), Rect::new(x, area.y, right_w.min(area.width), 1));
        }

        // Transient right-aligned overlay: leader hint or copied toast, both right-aligned
        // and rendered last so they overwrite any right-slot widgets underneath.
        if self.mode == Mode::Leader {
            let hint = bindings::leader_hint(&self.keymap);
            // Available space left of the right slot is already accounted for by
            // the reserve above; now draw it.
            let hint_w = hint.chars().count() as u16;
            let w = hint_w.min(area.width);
            let x = area.width.saturating_sub(w);
            let hint_style = Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD);
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(vec![ratatui::text::Span::styled(hint.chars().take(w as usize).collect::<String>(), hint_style)])), Rect::new(x, area.y, w, 1));
        } else if let Some((msg, t)) = &self.status_msg {
            if t.elapsed() < TOAST_TIMEOUT {
                let msg_w = msg.chars().count() as u16;
                let w = msg_w.min(area.width);
                let x = area.width.saturating_sub(w);
                let msg_style = Style::default().fg(RColor::White).bg(theme.accent).add_modifier(Modifier::BOLD);
                f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(vec![ratatui::text::Span::styled(msg.chars().take(w as usize).collect::<String>(), msg_style)])), Rect::new(x, area.y, w, 1));
            }
        }
    }

    fn render_update_notice(&self, f: &mut Frame) {
        let Some(rect) = self.update_notice_rect() else { return };
        let Some((line1, line2)) = self.update_notice_lines() else { return };
        let theme = self.current_theme();
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        let border = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        fill(f, rect, theme.panel_sep);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y1, "└", border);
        put(f, x1, y1, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y1, "─", border);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border);
            put(f, x1, y, "│", border);
        }
        put(f, x0 + 2, y0 + 1, "✕", Style::default().fg(theme.red).bg(theme.panel_sep).add_modifier(Modifier::BOLD));
        let inner_w = rect.width.saturating_sub(2);
        text(f, x0 + 5, y0 + 1, &line1, Style::default().fg(theme.fg).bg(theme.panel_sep), inner_w.saturating_sub(6));
        text(f, x0 + 5, y0 + 2, &line2, Style::default().fg(theme.fg).bg(theme.panel_sep), inner_w.saturating_sub(5));
    }

    fn render_menu(&self, f: &mut Frame) {
        if !self.menu.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.menu_dropdown_rect() else { return };
        let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
        draw_box(f, dd, border);
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            render_item_row(f, dd.x, dd.y + 1 + i as u16, dd.width.saturating_sub(2), item, i == self.menu.selected, &theme);
        }
    }

    fn render_ctx_menu(&self, f: &mut Frame) {
        if !self.ctx_menu.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.ctx_menu_rect() else { return };
        let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
        draw_box(f, dd, border);
        for (i, item) in self.ctx_items().iter().enumerate() {
            render_item_row(f, dd.x, dd.y + 1 + i as u16, dd.width.saturating_sub(2), item, i == self.ctx_menu.selected, &theme);
        }
    }

    fn render_name_popup(&self, f: &mut Frame) {
        if !self.popup.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.name_popup_rect() else { return };
        let (x0, y0, _x1, _y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
        let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
        draw_box(f, dd, border);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        let title_text = match self.popup.target {
            Some(PopupTarget::RenamePane(_)) => "rename pane",
            Some(PopupTarget::RenameSession(_)) => "rename session",
            Some(PopupTarget::NewWorktree(_)) => "new worktree",
            _ => "new session",
        };
        text(f, x0 + 2, y0 + 1, title_text, title, dd.width.saturating_sub(4));
        let label = Style::default().fg(theme.fg).bg(theme.panel_sep);
        let label_text = match self.popup.target {
            Some(PopupTarget::NewWorktree(_)) => "branch:",
            _ => "name:",
        };
        text(f, x0 + 2, y0 + 2, label_text, label, dd.width.saturating_sub(4));
        let field = Style::default().fg(RColor::Black).bg(theme.input_bg);
        let field_w = dd.width.saturating_sub(4);
        for cx in (x0 + 2)..(x0 + 2 + field_w) {
            put(f, cx, y0 + 3, " ", field);
        }
        let text_w = field_w as usize - 1;
        let name = &self.popup.name;
        let cursor = self.popup.cursor.min(name.chars().count());
        let end = cursor + 1;
        let start = end.saturating_sub(text_w);
        let mut col = x0 + 2;
        for (i, ch) in name.chars().enumerate() {
            if i < start {
                continue;
            }
            if i - start >= text_w {
                break;
            }
            put(f, col, y0 + 3, &ch.to_string(), field);
            col += 1;
        }
        for btn in [PopupBtn::Enter, PopupBtn::Cancel] {
            let Some(rect) = self.name_popup_button_rect(btn) else { continue };
            let label = match btn {
                PopupBtn::Enter => "⏎ enter ",
                PopupBtn::Cancel => " esc cancel ",
            };
            let hovered = self.popup.hover == Some(btn);
            let st = if hovered {
                Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD)
            };
            text(f, rect.x, rect.y, label, st, rect.width);
        }
        if let Some(err) = &self.popup.error {
            text(f, x0 + 2, y0 + 5, err, Style::default().fg(theme.orange).bg(theme.panel_sep), dd.width.saturating_sub(4));
        }
    }

    fn render_keybind_overlay(&self, f: &mut Frame) {
        if !self.keybind_overlay.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.keybind_overlay_rect() else { return };
        let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
        draw_box(f, dd, border);
        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, dd.x + 2, dd.y + 1, "keybindings", title, inner_w);
        let max_keys = self.keymap.iter().map(|b| b.keys.chars().count()).max().unwrap_or(4) as u16;
        let scroll = self.keybind_overlay.scroll as usize;
        let body_top = dd.y + 2;
        let body_bottom = dd.bottom() - 1;
        for (i, line) in keybind_lines(&self.keymap).iter().skip(scroll).enumerate() {
            let y = body_top + i as u16;
            if y >= body_bottom {
                break;
            }
            match line {
                KbLine::Header(label) => {
                    let st = Style::default().fg(theme.orange).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
                    text(f, dd.x + 2, y, label, st, inner_w);
                }
                KbLine::Bind(b) => {
                    let keys = Style::default().fg(theme.accent).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
                    let desc = Style::default().fg(theme.fg).bg(theme.panel_sep);
                    text(f, dd.x + 2, y, &b.keys, keys, max_keys);
                    text(f, dd.x + 2 + max_keys + 2, y, &b.desc, desc, inner_w.saturating_sub(max_keys + 2));
                }
            }
        }
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2, dd.bottom() - 2, "j/k: scroll · esc / ?: close", footer, inner_w);
    }

    fn render_settings(&self, f: &mut Frame) {
        if !self.settings.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.settings_rect() else { return };
        let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
        draw_box(f, dd, border);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, dd.x + 2, dd.y + 1, "settings", title, dd.width.saturating_sub(4));
        let Some(tabs) = self.settings_tabs_rect() else { return };
        for (i, tab) in SETTINGS_TABS.iter().enumerate() {
            let sel = i == self.settings.tab;
            let y = tabs.y + i as u16;
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in tabs.x..(tabs.x + tabs.width) {
                put(f, cx, y, " ", Style::default().bg(bg));
            }
            let row_fg = if sel { RColor::Black } else { theme.fg };
            put(f, tabs.x, y, "▸", Style::default().fg(row_fg).bg(bg).add_modifier(Modifier::BOLD));
            text(f, tabs.x + 2, y, tab.label(), Style::default().fg(row_fg).bg(bg), tabs.width.saturating_sub(2));
        }
        let sep_x = tabs.x + tabs.width;
        for y in tabs.y..dd.bottom() {
            put(f, sep_x, y, "│", border);
        }
        match SETTINGS_TABS.get(self.settings.tab).copied().unwrap_or(SettingsTab::Appearance) {
            SettingsTab::Appearance => self.render_settings_appearance(f, dd),
            SettingsTab::About => self.render_settings_about(f, dd),
        }
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2, dd.bottom() - 2, "j/k: move · h/l: tab · enter: apply · esc: close", footer, dd.width.saturating_sub(4));
    }

    fn render_settings_appearance(&self, f: &mut Frame, dd: Rect) {
        let theme = self.current_theme();
        let Some(content) = self.settings_content_rect() else { return };
        let label = Style::default().fg(theme.secondary).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, content.x, content.y, "theme", label, content.width);
        for (i, t) in self.all_themes().iter().enumerate() {
            let sel = i == self.settings.selected;
            let active = i == self.theme_idx;
            let y = content.y + 1 + i as u16;
            if y >= dd.bottom().saturating_sub(1) {
                break;
            }
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in content.x..(content.x + content.width) {
                put(f, cx, y, " ", Style::default().bg(bg));
            }
            let row_fg = if sel { RColor::Black } else { theme.fg };
            let marker = if active { "●" } else { "○" };
            let marker_style = if active && !sel {
                Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(row_fg).bg(bg).add_modifier(Modifier::BOLD)
            };
            put(f, content.x, y, marker, marker_style);
            text(f, content.x + 2, y, &t.name, Style::default().fg(row_fg).bg(bg), content.width.saturating_sub(2));
            if active && content.width >= 26 {
                text(f, content.x + content.width.saturating_sub(8), y, " in use ", Style::default().fg(row_fg).bg(bg), 8);
            }
        }
    }

    fn render_settings_about(&self, f: &mut Frame, dd: Rect) {
        let theme = self.current_theme();
        let Some(content) = self.settings_content_rect() else { return };
        let label = Style::default().fg(theme.secondary).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, content.x, content.y, "kumo", label, content.width);
        let session = self.active_session().map(|s| s.name.clone()).unwrap_or_default();
        let rows: [(&str, String); 5] = [
            ("version", format!("{} ({})", env!("CARGO_PKG_VERSION"), kumo_core::update::current_channel_label())),
            ("session", session),
            ("sessions", format!("{}", self.layout.as_ref().map(|l| l.sessions.len()).unwrap_or(0))),
            ("panes", format!("{}", self.rects.len())),
            ("shell", kumo_core::config::default_shell()),
        ];
        for (i, (k, v)) in rows.iter().enumerate() {
            let y = content.y + 1 + i as u16;
            if y >= dd.bottom().saturating_sub(1) {
                break;
            }
            let kst = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
            text(f, content.x, y, k, kst, 12);
            let vst = Style::default().fg(theme.fg).bg(theme.panel_sep);
            text(f, content.x + 12, y, v, vst, content.width.saturating_sub(14));
        }
    }

    fn render_worktree_picker(&self, f: &mut Frame) {
        if !self.worktree_picker.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.worktree_picker_rect() else { return };
        let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
        draw_box(f, dd, border);
        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        let count = self.worktree_picker.items.len();
        let title_text = if count == 0 { "worktrees".to_string() } else { format!("worktrees · {count}") };
        text(f, dd.x + 2, dd.y + 1, &title_text, title, inner_w);
        if let Some(err) = &self.worktree_picker.error {
            let st = Style::default().fg(theme.orange).bg(theme.panel_sep);
            text(f, dd.x + 2, dd.y + 2, err, st, inner_w);
            let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
            text(f, dd.x + 2, dd.bottom() - 2, "esc: close", footer, inner_w);
            return;
        }
        const BRANCH_COL: u16 = 24;
        let branch_x = dd.x + 3;
        let path_x = branch_x + BRANCH_COL + 1;
        let path_w = inner_w.saturating_sub(path_x - dd.x + 1);
        let header = Style::default().fg(theme.panel_muted).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, branch_x, dd.y + 2, "branch", header, BRANCH_COL.saturating_sub(2));
        if path_w > 0 {
            text(f, path_x, dd.y + 2, "path", header, path_w);
        }
        let body_top = dd.y + 3;
        let body_bottom = dd.bottom() - 2;
        let scroll = self.worktree_picker.scroll as usize;
        for (i, row) in self.worktree_picker.items.iter().enumerate().skip(scroll) {
            let y = body_top + (i - scroll) as u16;
            if y >= body_bottom {
                break;
            }
            let sel = i == self.worktree_picker.selected;
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in (dd.x + 1)..(dd.x + 1 + inner_w) {
                put(f, cx, y, " ", Style::default().bg(bg));
            }
            let fg = if sel { RColor::Black } else { theme.fg };
            put(f, dd.x + 1, y, if sel { "▸" } else { " " }, Style::default().fg(if sel { RColor::Black } else { theme.accent }).bg(bg).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() }));
            if row.open {
                put(f, dd.x + 2, y, "●", Style::default().fg(if sel { fg } else { theme.green }).bg(bg));
            }
            let branch = row.branch.as_deref().unwrap_or("(detached)");
            let branch_style = Style::default().fg(fg).bg(bg).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() });
            text(f, branch_x, y, branch, branch_style, BRANCH_COL.saturating_sub(2));
            if path_w > 0 {
                let path = fit_worktree_path(&row.path, path_w as usize);
                let path_style = Style::default().fg(if row.is_main { fg } else { theme.panel_muted }).bg(bg);
                text(f, path_x, y, &path, path_style, path_w);
            }
        }
        let items = self.worktree_picker.items.len();
        let visible = self.worktree_picker_visible_rows() as usize;
        if items > visible {
            let bar_h = body_bottom.saturating_sub(body_top);
            let thumb = ((visible * bar_h as usize) / items).max(1).min(bar_h as usize);
            let y_max = (bar_h as usize).saturating_sub(thumb);
            let y_start = self.worktree_picker.scroll as usize * y_max / (items - visible);
            for i in 0..bar_h as usize {
                let y = body_top + i as u16;
                let ch = if i >= y_start && i < y_start + thumb { "▐" } else { "░" };
                put(f, dd.right() - 2, y, ch, Style::default().fg(if i >= y_start && i < y_start + thumb { theme.secondary } else { theme.panel_sep }));
            }
        }
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2, body_bottom, "j/k: move · enter: open · esc: close", footer, inner_w);
    }

    fn render_copy_overlay(&self, f: &mut Frame, pid: u64, rect: Rect) {
        let Some(cs) = self.copy.as_ref() else { return; };
        if cs.pane_id != pid { return; }
        let Some(grid) = self.grids.get(&pid) else { return; };
        let inner = PaneGeom { pane_id: pid, rect }.inner();
        if inner.width == 0 || inner.height == 0 { return; }
        let theme = self.current_theme();
        // Search hits: underline + tint, active hit bold/reversed
        if !cs.hits.is_empty() {
            let scroll = grid.scroll;
            for (idx, hit) in cs.hits.iter().enumerate() {
                let top = scroll.map(|s| s.offset as u32).unwrap_or(0);
                let rows = inner.height as u32;
                if hit.row < top || hit.row >= top + rows { continue; }
                let y = inner.y + (hit.row - top) as u16;
                let is_active = cs.hit_idx == Some(idx);
                for c in hit.start_col..hit.end_col {
                    if c >= inner.width { break; }
                    let x = inner.x + c;
                    if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                        if is_active {
                            cell.set_style(cell.style().add_modifier(Modifier::REVERSED).add_modifier(Modifier::BOLD));
                            cell.set_bg(theme.accent);
                            cell.set_fg(RColor::Black);
                        } else {
                            cell.set_style(cell.style().add_modifier(Modifier::UNDERLINED));
                            // tint background slightly
                            cell.set_bg(theme.secondary);
                            cell.set_fg(RColor::Black);
                        }
                    }
                }
            }
        }
        // Selection fallback (client charwise highlight) when daemon selection is charwise?
        // Daemon selection already appears as REVERSED via PaneFrame; we add an extra
        // overlay for the anchor→cursor range so a client-only selection (no daemon) still shows.
        if let Some(anchor) = cs.anchor {
            let cur = cs.cursor;
            let (top, bottom, left, right) = if cs.linewise {
                let top = anchor.1.min(cur.1);
                let bottom = anchor.1.max(cur.1);
                (top, bottom, 0u16, inner.width.saturating_sub(1))
            } else {
                let (s_col, s_row) = anchor;
                let (c_col, c_row) = cur;
                let (start, end) = if (s_row, s_col) <= (c_row, c_col) { ((s_col, s_row), (c_col, c_row)) } else { ((c_col, c_row), (s_col, s_row)) };
                // We'll highlight per-row range below; store for per-cell check
                // For simplicity, use sel_corners-style check inline per cell
                // We'll just flag and handle in loop
                // Return as bounding box plus per-cell filter
                (start.1, end.1, start.0, end.0) // use as markers, filter logic uses original
            };
            // Iterate viewport rows/cols inside inner and highlight if in selection
            for r in 0..inner.height {
                for c in 0..inner.width {
                    let in_sel = if cs.linewise {
                        r >= top && r <= bottom
                    } else {
                        let s = anchor;
                        let e = cur;
                        let (sr, sc) = (s.1, s.0);
                        let (er, ec) = (e.1, e.0);
                        let (tr, tc, br, bc) = if (sr, sc) <= (er, ec) { (sr, sc, er, ec) } else { (er, ec, sr, sc) };
                        let pos = (r, c);
                        let top_pos = (tr, tc);
                        let bottom_pos = (br, bc);
                        pos >= top_pos && pos <= bottom_pos
                    };
                    if !in_sel { continue; }
                    let x = inner.x + c;
                    let y = inner.y + r;
                    if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                        // avoid double-highlighting search active hit (already bold)
                        let is_search_active = if let Some(hidx) = cs.hit_idx {
                            if let Some(hit) = cs.hits.get(hidx) {
                                let scroll = grid.scroll.map(|s| s.offset as u32).unwrap_or(0);
                                hit.row == scroll + r as u32 && c >= hit.start_col && c < hit.end_col
                            } else { false }
                        } else { false };
                        if !is_search_active {
                            cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
                        }
                    }
                    let _ = (top, bottom, left, right); // keep bindings
                }
            }
        }
        // Cursor: block cursor at copy position (always visible)
        let (cx, cy) = cs.cursor;
        if cx < inner.width && cy < inner.height {
            let x = inner.x + cx;
            let y = inner.y + cy;
            if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                // If on an active search hit, keep hit styling plus add bold
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED).add_modifier(Modifier::BOLD));
                // Use accent bg to distinguish copy cursor from terminal cursor
                // Keep fg inversion for readability
                let fg = cell.style().fg.unwrap_or(theme.fg);
                let bg = cell.style().bg.unwrap_or(RColor::Reset);
                // invert
                cell.set_fg(bg);
                cell.set_bg(theme.accent);
                let _ = fg;
            }
        }
        // Copy-mode label at pane title: add "[COPY]" chip?
        // Instead status bar shows mode; we also tint border accent when in copy
    }

    fn render_copy_search_bar(&self, f: &mut Frame) {
        let Some(cs) = self.copy.as_ref() else { return; };
        if !cs.search_active {
            // When not searching but have a query, show hint in status-like bar?
            // Render a small hint line at bottom if in copy mode
            if self.mode != Mode::Copy { return; }
            let theme = self.current_theme();
            let area = f.area();
            let y = area.bottom().saturating_sub(1);
            if y < area.y { return; }
            let bar_y = if self.status_bar.enabled { y.saturating_sub(1) } else { y };
            if bar_y < area.y { return; }
            let msg = if cs.hits.is_empty() {
                if cs.search_query.is_some() { format!(" copy: {} (no matches) — /:? search n/N next q: quit v: select y: yank", cs.search_query.as_deref().unwrap_or("")) }
                else { " copy: h/j/k/l move 0/$ g/G top/bottom v/V select y yank / ? search n/N next q: quit ".to_string() }
            } else {
                let idx = cs.hit_idx.map(|i| i+1).unwrap_or(0);
                format!(" copy: {} [{}/{}] n/N next q: quit ", cs.search_query.as_deref().unwrap_or(""), idx, cs.hits.len())
            };
            let style = Style::default().fg(RColor::Black).bg(theme.secondary);
            let rect = Rect::new(area.x, bar_y, area.width, 1);
            fill(f, rect, theme.secondary);
            text(f, rect.x + 1, rect.y, &msg, style, rect.width.saturating_sub(2));
            return;
        }
        // Active search input bar
        let theme = self.current_theme();
        let area = f.area();
        let y = area.bottom().saturating_sub(1);
        if y < area.y { return; }
        let bar_y = if self.status_bar.enabled { y.saturating_sub(1) } else { y };
        let rect = Rect::new(area.x, bar_y, area.width, 1);
        fill(f, rect, theme.input_bg);
        let prefix = if cs.search_forward { "/" } else { "?" };
        let style = Style::default().fg(RColor::Black).bg(theme.input_bg);
        let mut x = rect.x + 1;
        put(f, x, rect.y, prefix, style.add_modifier(Modifier::BOLD));
        x += 1;
        let input = &cs.search_input;
        let cursor = cs.search_cursor.min(input.chars().count());
        for (i, ch) in input.chars().enumerate() {
            let ch_style = if i == cursor { style.add_modifier(Modifier::REVERSED) } else { style };
            put(f, x, rect.y, &ch.to_string(), ch_style);
            x += 1;
            if x >= rect.right() - 1 { break; }
        }
        if cursor == input.chars().count() && x < rect.right() {
            put(f, x, rect.y, " ", style.add_modifier(Modifier::REVERSED));
        }
        // hint
        let hint = " enter: search  esc: cancel ";
        let hint_x = rect.right().saturating_sub(hint.len() as u16 + 1);
        if hint_x > x + 1 {
            text(f, hint_x, rect.y, hint, Style::default().fg(theme.panel_muted).bg(theme.input_bg), hint.len() as u16);
        }
    }

    /// Place the host-terminal cursor: the popup's input field when open, else
    /// the focused pane's terminal cursor.
    fn place_cursor<B: ratatui::backend::Backend>(&self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        if self.popup.open {
            if let Some((x, y)) = self.name_popup_input_cursor() {
                terminal.set_cursor_position((x, y))?;
                terminal.show_cursor()?;
                return Ok(());
            }
        }
        let focus = self.active_tab().map(|t| t.focus);
        if let Some(focus) = focus {
            if let Some((_, rect)) = self.rects.iter().find(|(pid, _)| *pid == focus) {
                let inner = PaneGeom { pane_id: focus, rect: *rect }.inner();
                if let Some(grid) = self.grids.get(&focus) {
                    if let Some((cx, cy)) = grid.cursor {
                        let x = inner.x + cx;
                        let y = inner.y + cy;
                        if x < inner.x + inner.width && y < inner.y + inner.height {
                            terminal.set_cursor_position((x, y))?;
                            terminal.show_cursor()?;
                            return Ok(());
                        }
                    }
                }
            }
        }
        terminal.hide_cursor()?;
        Ok(())
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

/// Truncate a git branch name to `avail` columns, appending `…` when cut.
fn fit_branch_name(name: &str, avail: usize) -> String {
    if name.chars().count() <= avail {
        name.to_string()
    } else if avail == 0 {
        String::new()
    } else {
        let mut s: String = name.chars().take(avail - 1).collect();
        s.push('…');
        s
    }
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

fn draw_box(f: &mut Frame, dd: Rect, border: Style) {
    fill(f, dd, border.bg.unwrap_or(RColor::Reset));
    let (x0, y0, x1, y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
    put(f, x0, y0, "┌", border);
    put(f, x1, y0, "┐", border);
    put(f, x0, y1, "└", border);
    put(f, x1, y1, "┘", border);
    for x in (x0 + 1)..x1 {
        put(f, x, y0, "─", border);
        put(f, x, y1, "─", border);
    }
    for y in (y0 + 1)..y1 {
        put(f, x0, y, "│", border);
        put(f, x1, y, "│", border);
    }
}

fn render_item_row(f: &mut Frame, x0: u16, y: u16, width: u16, item: &str, sel: bool, theme: &OwnedTheme) {
    let bg = if sel { theme.accent } else { theme.panel_sep };
    for cx in (x0 + 1)..(x0 + 1 + width) {
        put(f, cx, y, " ", Style::default().bg(bg));
    }
    let (marker, marker_style, label_style) = if sel {
        (
            "▸",
            Style::default().fg(RColor::Black).bg(bg).add_modifier(Modifier::BOLD),
            Style::default().fg(RColor::Black).bg(bg).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ",
            Style::default().fg(theme.accent).bg(bg),
            Style::default().fg(theme.fg).bg(bg),
        )
    };
    put(f, x0 + 1, y, marker, marker_style);
    text(f, x0 + 3, y, item, label_style, width.saturating_sub(2));
}

fn draw_scrollbar(f: &mut Frame, x: u16, y_top: u16, region_h: u16, offset: usize, total: usize, theme: &OwnedTheme) {
    if total <= region_h as usize || region_h == 0 {
        return;
    }
    let bar_h = region_h as usize;
    let thumb = ((region_h as usize * bar_h) / total).max(1).min(bar_h);
    let hist = total - region_h as usize;
    let y_max = bar_h.saturating_sub(thumb);
    let y_start = offset.saturating_mul(y_max) / hist.max(1);
    for i in 0..bar_h {
        let y = y_top + i as u16;
        let cell = f.buffer_mut().cell_mut((x, y)).unwrap();
        if i >= y_start && i < y_start + thumb {
            cell.set_symbol("▐").set_fg(theme.secondary);
        } else {
            cell.set_symbol("░").set_fg(theme.panel_sep);
        }
    }
}

/// One display row of the keybind showcase: a group header or a binding.
enum KbLine<'a> {
    Header(&'a str),
    Bind(&'a Binding),
}

fn keybind_lines<'a>(keymap: &'a [Binding]) -> Vec<KbLine<'a>> {
    let mut lines = Vec::new();
    for group in bindings::Group::ALL {
        let mut pushed = false;
        let mut last_keys: Option<&str> = None;
        for b in keymap {
            if b.group == group {
                if !pushed {
                    lines.push(KbLine::Header(group.label()));
                    pushed = true;
                }
                if last_keys == Some(b.keys.as_str()) {
                    continue;
                }
                last_keys = Some(&b.keys);
                lines.push(KbLine::Bind(b));
            }
        }
    }
    lines
}

/// Byte offset of the `ci`-th char in `s` (or `s.len()` past the end).
fn char_idx_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

/// Delete the word before `cursor` in `s`; returns the new string and cursor.
fn delete_word_backward(s: &str, cursor: usize) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let mut pos = cursor.min(chars.len());
    while pos > 0 && chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    while pos > 0 && !chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    let start = pos;
    let end = cursor.min(chars.len());
    if start == end {
        return (s.to_string(), cursor);
    }
    let sb = char_idx_to_byte(s, start);
    let eb = char_idx_to_byte(s, end);
    let mut out = s.to_string();
    out.replace_range(sb..eb, "");
    (out, start)
}

/// Delete the word after `cursor` in `s`; returns the new string.
fn delete_word_forward(s: &str, cursor: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut pos = cursor.min(len);
    while pos < len && chars[pos].is_whitespace() {
        pos += 1;
    }
    while pos < len && !chars[pos].is_whitespace() {
        pos += 1;
    }
    let start = cursor.min(len);
    if pos == start {
        return s.to_string();
    }
    let sb = char_idx_to_byte(s, start);
    let eb = char_idx_to_byte(s, pos);
    let mut out = s.to_string();
    out.replace_range(sb..eb, "");
    out
}

/// Short display form of a worktree path for the picker, trimmed to `avail`.
fn fit_worktree_path(path: &std::path::Path, avail: usize) -> String {
    let text = path.to_string_lossy();
    let n = text.chars().count();
    if n <= avail {
        return text.into_owned();
    }
    if avail == 0 {
        return String::new();
    }
    let tail: String = text.chars().skip(n.saturating_sub(avail - 1)).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, width: u16) -> WireCell {
        WireCell {
            text: text.to_string(),
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

    fn grid() -> Grid {
        Grid { cols: 4, rows: 3, cells: Vec::new(), cursor: None, scroll: None, links: HashMap::new(), rendered: Vec::new(), dirty_rows: HashSet::new() }
    }

    fn test_view() -> View {
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
            keybind_overlay: KeybindOverlay { open: false, scroll: 0 },
            settings: SettingsPanel { open: false, tab: 0, selected: 0 },
            worktree_picker: WorktreePicker { open: false, session: 0, items: Vec::new(), selected: 0, scroll: 0, error: None },
            pane_numbers: None,
            status_msg: None,
            notice: None,
            update_notice: None,
            link_mods: false,
            drag: None,
            sel: None,
            pending_click: None,
            pending_wheel: HashMap::new(),
            copy: None,
            tab_hover: None,
            tab_rects: Vec::new(),
            tab_scroll: 0,
            plus_rect: None,
            dirty: false,
            detach_requested: false,
            status_bar: StatusBarConfig::default(),
            hostname: "testhost".to_string(),
            clock_str: "12:00".to_string(),
            clock_next: Instant::now() + Duration::from_secs(60),
            is_ssh: false,
        }
    }

    fn mouse_moved(x: u16, y: u16) -> crossterm::event::MouseEvent {
        use crossterm::event::MouseEventKind;
        MouseEvent { kind: MouseEventKind::Moved, column: x, row: y, modifiers: KeyModifiers::NONE }
    }

    /// Hovering the MENU dropdown or the context menu must move the selection
    /// AND mark the view dirty, so the primary-accent highlight repaints live.
    #[test]
    fn hovering_menu_repaints_selection() {
        let mut view = test_view();
        view.menu.open = true;
        view.menu.selected = 0;
        // The dropdown sits above the MENU button; item i lives at row 17+i
        // (cols 80, rows 24, mode NORMAL -> MENU at x 9).
        view.on_mouse(mouse_moved(4, 18)).unwrap();
        assert_eq!(view.menu.selected, 1, "hover item 2 selects it");
        assert!(view.dirty(), "hover must trigger a repaint");
        view.dirty = false;
        // Hovering the same item again is a no-op (no repaint churn).
        view.on_mouse(mouse_moved(4, 18)).unwrap();
        assert!(!view.dirty(), "unchanged hover must not repaint");
        // Hovering item 4 updates selection + repaints.
        view.on_mouse(mouse_moved(4, 20)).unwrap();
        assert_eq!(view.menu.selected, 3);
        assert!(view.dirty());
    }

    #[test]
    fn hovering_ctx_menu_repaints_selection() {
        let mut view = test_view();
        view.ctx_menu.open = true;
        view.ctx_menu.x = 30;
        view.ctx_menu.y = 5;
        view.ctx_menu.target = CtxTarget::Pane(1);
        view.ctx_menu.selected = 0;
        // The context menu opens down-right of (30,5): box at (31,6,18,7),
        // item i at row 7+i. Hover item 2.
        view.on_mouse(mouse_moved(32, 8)).unwrap();
        assert_eq!(view.ctx_menu.selected, 1);
        assert!(view.dirty(), "ctx-menu hover must repaint");
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
    fn selection_text_joins_rows_and_skips_continuation() {
        let mut g = grid();
        g.cells = vec![
            vec![cell("a", 1), cell("b", 1), cell("c", 1), cell("d", 1)],
            vec![cell("\u{1f600}", 2), cell(" ", 0), cell("x", 1), cell("y", 1)],
            vec![cell("1", 1), cell("2", 1), cell("3", 1), cell("4", 1)],
        ];
        let mut view = test_view();
        view.grids.insert(1, g);
        let sel = Sel { pane_id: 1, start: (1, 0), end: (1, 1) };
        let text = view.selection_text(&sel);
        assert_eq!(text, "bcd\n\u{1f600}", "wide char is kept, continuation cell skipped: {text:?}");
    }

    #[test]
    fn render_draws_pane_borders_and_status() {
        // One session with a single focused pane, plus a matching grid.
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
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
        let mut g = grid();
        g.cells = vec![vec![cell("hi", 1), cell(" ", 1)], vec![cell("yo", 1), cell(" ", 1)]];
        let mut view = test_view();
        view.layout = Some(layout);
        view.grids.insert(1, g);
        view.recompute_geometry();
        // 80x24 with sidebar+tab bar: the pane area starts at col 26, row 1 (tab bar at row 0).
        assert_eq!(view.rects, vec![(1, Rect::new(26, 1, 54, 22))]);

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();
        // Tab bar at top — rectangular pill, no brackets, x in last cell on hover, name at first cell
        // Pill width for "1" is 6 => pill at x=26..31, name at 26
        assert_eq!(buf.cell((26, 0)).unwrap().symbol(), "1");
        assert!(buf.cell((26, 0)).unwrap().style().bg.is_some(), "tab bar should have distinct bg");
        // Pane border at the top-left of the pane area (below tab bar) — now rounded by default.
        assert_eq!(buf.cell((26, 1)).unwrap().symbol(), "╭");
        // The title chip carries the pane label (pane frame at y=1).
        assert_eq!(buf.cell((27, 1)).unwrap().symbol(), " ");
        assert!(buf.cell((28, 1)).unwrap().symbol() == "s" || buf.cell((28, 1)).unwrap().symbol() == " ");
        // The status bar shows NORMAL + the session name.
        let status_line: String = (0..40).map(|x| buf.cell((x, 23)).unwrap().symbol().to_string()).collect();
        assert!(status_line.contains("NORMAL"), "status chip missing: {status_line:?}");
        assert!(status_line.contains("sess"), "session name missing: {status_line:?}");
    }

    #[test]
    fn recompute_geometry_requests_pane_sizes() {
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
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

    /// The highlight rectangle must cover exactly the cells `selection_text`
    /// copies — a horizontal drag over a table like `top` must not light up a
    /// vertical band (the classic transposed-corner bug).
    #[test]
    fn selection_highlight_matches_copied_text() {
        let mut g = grid();
        g.cells = vec![
            vec![cell("a", 1), cell("b", 1), cell("c", 1), cell("d", 1), cell("e", 1)],
            vec![cell("f", 1), cell("g", 1), cell("h", 1), cell("i", 1), cell("j", 1)],
            vec![cell("k", 1), cell("l", 1), cell("m", 1), cell("n", 1), cell("o", 1)],
        ];
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
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
        view.grids.insert(1, g);
        view.recompute_geometry();
        // Horizontal drag across row 1, cols 1..=3.
        view.sel = Some(Sel { pane_id: 1, start: (1, 1), end: (3, 1) });
        assert_eq!(view.selection_text(&view.sel.unwrap()), "ghi", "copy of a row drag");

        // Render and check exactly cols 1..=3 of row 1 are reversed.
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let inner = PaneGeom { pane_id: 1, rect: view.rects[0].1 }.inner();
        let rev = |c: u16, r: u16| {
            buf.cell((inner.x + c, inner.y + r)).unwrap().modifier.contains(Modifier::REVERSED)
        };
        assert!(!rev(0, 1) && rev(1, 1) && rev(2, 1) && rev(3, 1) && !rev(4, 1),
            "row 1 highlights only cols 1..=3");
        assert!(!rev(1, 0) && !rev(1, 2), "adjacent rows stay clear");
    }

    /// A drag drawn up-left (end before start) still highlights the same cells
    /// the copy extracts.
    #[test]
    fn selection_reversed_drag_normalizes_corners() {
        let sel = Sel { pane_id: 1, start: (3, 1), end: (1, 1) };
        let ((tr, tc), (br, bc)) = sel_corners(&sel);
        assert_eq!(((tr, tc), (br, bc)), ((1, 1), (1, 3)));
    }

    /// Rendering must never panic at any (tiny → large) terminal size, with
    /// every overlay open — the classic TUI's full frame was drawn every
    /// cycle, so the client's draw path has to be total over all geometry.
    #[test]
    fn render_is_total_across_sizes_and_overlays() {
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus: 1, zoom: false, root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane { id: 1, title: " shell ".into(), cwd: std::path::PathBuf::from("/tmp"), is_ai: false, agent: None, mouse_reporting: false, alt_screen: false }))) }],
                focus: 1,
                zoom: false,
                branch: Some(WireBranch { name: "main".into(), ahead: 1, behind: 0 }),
                root: Some(Box::new(LayoutNode::Split {
                    id: 1,
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    a: Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                        id: 1,
                        title: " shell 1 ".into(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        is_ai: false,
                        agent: None,
                        mouse_reporting: false,
                        alt_screen: false,
                    })),
                    b: Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                        id: 2,
                        title: " AI CLI ".into(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        is_ai: true,
                        agent: Some(kumo_protocol::AgentInfo {
                            name: "opencode".into(),
                            status: AgentStatus::Blocked,
                            cpu: 0.0,
                            mem_kb: 0,
                        }),
                        mouse_reporting: true,
                        alt_screen: false,
                    })),
                })),
            }],
        };
        let mut g = grid();
        g.cells = vec![vec![cell("hi", 1), cell(" ", 1), cell("x", 1), cell("y", 1)]];
        for (w, h) in [(2u16, 2u16), (5, 4), (20, 8), (40, 12), (80, 24), (120, 40)] {
            let mut view = test_view();
            view.cols = w;
            view.rows = h;
            view.layout = Some(layout.clone());
            view.grids.insert(1, g.clone());
            view.recompute_geometry();
            // Open every overlay: it must still render without panicking.
            view.menu.open = true;
            view.ctx_menu.open = true;
            view.popup.open = true;
            view.popup.name = "worktree/name".into();
            view.keybind_overlay.open = true;
            view.settings.open = true;
            view.worktree_picker.open = true;
            view.worktree_picker.items = vec![
                WireWorktree { path: std::path::PathBuf::from("/tmp"), branch: Some("main".into()), is_main: true, open: false },
            ];
            view.pane_numbers = Some(Instant::now());
            view.update_notice = Some(("key".into(), "nightly".into()));
            view.sel = Some(Sel { pane_id: 1, start: (0, 0), end: (1, 0) });
            view.link_mods = true;
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            term.draw(|f| view.draw(f)).unwrap();
        }
    }

    #[test]
    fn delete_word_backward_plain() {
        assert_eq!(delete_word_backward("foo bar", 7), ("foo ".to_string(), 4));
    }

    #[test]
    fn delete_word_forward_plain() {
        assert_eq!(delete_word_forward("foo bar", 4), "foo ".to_string());
    }

    #[test]
    fn fit_branch_name_truncates() {
        assert_eq!(fit_branch_name("very/long/feature-branch-name", 8), "very/lo…");
    }
}
