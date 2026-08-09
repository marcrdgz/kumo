use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};

use crate::layout::{self, LayoutTree, SplitDir, TreeGeom};
use crate::pane::{sgr_mouse, AgentStatus, Pane, PtyEvent};
use crate::pty::Pty;
use crate::vt;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Catppuccin mocha chrome colors (sidebars, status bar, chrome borders).
const PANEL_SEP: RColor = RColor::Rgb(0x31, 0x32, 0x44); // surface0
const PANEL_MUTED: RColor = RColor::Rgb(0x6c, 0x70, 0x86); // overlay0
const BORDER_IDLE: RColor = RColor::Rgb(0x6c, 0x70, 0x86); // overlay0, visible on any terminal bg
const YELLOW: RColor = RColor::Rgb(0xf9, 0xe2, 0xaf); // yellow
const GREEN: RColor = RColor::Rgb(0xa6, 0xe3, 0xa1); // green
const ORANGE: RColor = RColor::Rgb(0xfa, 0xb3, 0x87); // peach

/// How often the sidebar re-reads the git branch of each session's workspace.
const BRANCH_REFRESH: Duration = Duration::from_secs(3);
/// How often to re-scan pane process trees for an AI CLI (opencode/claude).
const AI_SCAN_INTERVAL: Duration = Duration::from_secs(2);

struct Session {
    id: u64,
    name: String,
    tree: LayoutTree,
    zoom: bool,
    workspace: PathBuf,
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Leader,
}

enum Drag {
    Splitter { split_id: u64 },
}

/// Mouse text selection inside a pane (viewport-relative coordinates).
#[derive(Clone, Copy, PartialEq)]
struct Sel {
    pane_id: u64,
    start: (u16, u16),
    end: (u16, u16),
}

/// A left-click in a mouse-reporting pane, waiting to be resolved as either a
/// click (forwarded to the app) or the start of a text drag (selection).
#[derive(Clone, Copy)]
struct PendingClick {
    pane_id: u64,
    col: u16,
    row: u16,
}

#[derive(Clone, Copy)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Stable rows of the left sidebar, shared by rendering and mouse hit-testing.
#[derive(Clone)]
enum SidebarRow {
    Header(String),
    Spacer,
    Section(String),
    Session(usize),
    Branch(String),
    AgentDir(usize, u64),
    AgentName(usize, u64),
    NewSession,
}

pub struct App {
    sessions: Vec<Session>,
    active: usize,
    panes: HashMap<u64, Pane>,
    mode: Mode,
    drag: Option<Drag>,
    sel: Option<Sel>,
    pending_click: Option<PendingClick>,
    events_tx: mpsc::Sender<PtyEvent>,
    events_rx: mpsc::Receiver<PtyEvent>,
    shell: String,
    ai: (String, Vec<String>),
    workspace: PathBuf,
    term_size: (u16, u16),
    last_sizes: HashMap<u64, (u16, u16)>,
    sidebar_open: bool,
    sidebar_width: u16,
    /// Cached git branch per workspace, refreshed periodically.
    branch_cache: HashMap<PathBuf, (Option<String>, Instant)>,
    /// When the pane process tree was last scanned for an AI CLI.
    last_ai_scan: Instant,
    /// When the agent-status debug log was last written (throttle).
    last_agent_debug: Instant,
    /// Cached agent status per AI pane, refreshed during pane rendering.
    agent_status_cache: HashMap<u64, AgentStatus>,
    /// Previously focused pane, so focus changes re-render the two panes (cursor).
    last_focused: Option<u64>,
    /// Rendered cells of each pane's viewport, blitted back when the pane is
    /// unchanged so the frame loop never re-iterates unchanged terminals.
    pane_cache: HashMap<u64, Buffer>,
    quit: bool,
}

pub fn run(terminal: &mut Term, workspace: Option<&str>) -> Result<()> {
    let mut app = App::new(workspace)?;
    while !app.quit {
        while let Ok(ev) = app.events_rx.try_recv() {
            app.on_pty_event(ev);
        }
        if event::poll(Duration::from_millis(16))? {
            // Drain the whole input burst before rendering: a fast trackpad
            // scroll can enqueue many events, and processing one per frame
            // would trickle them into the pane and feel extremely laggy.
            loop {
                match event::read()? {
                    crossterm::event::Event::Key(k) => app.on_key(k)?,
                    crossterm::event::Event::Mouse(m) => app.on_mouse(m)?,
                    crossterm::event::Event::Resize(w, h) => {
                        app.term_size = (w, h);
                    }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        app.frame(terminal)?;
    }
    Ok(())
}

impl App {
    fn new(workspace: Option<&str>) -> Result<App> {
        let shell = crate::config::default_shell();
        let (ai_prog, ai_args) = crate::config::ai_command();
        let ai_prog = crate::config::resolve_program(&ai_prog);
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        // Workspace precedence: explicit argument, then the directory kumo
        // was launched from, then $HOME.
        let workspace = workspace
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or(home);

        let (events_tx, events_rx) = mpsc::channel();
        let mut app = App {
            sessions: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            mode: Mode::Normal,
            drag: None,
            sel: None,
            pending_click: None,
            events_tx,
            events_rx,
            shell,
            ai: (ai_prog, ai_args),
            workspace,
            term_size: (80, 24),
            last_sizes: HashMap::new(),
            sidebar_open: true,
            sidebar_width: 26,
            branch_cache: HashMap::new(),
            last_ai_scan: Instant::now(),
            last_agent_debug: Instant::now(),
            agent_status_cache: HashMap::new(),
            last_focused: None,
            pane_cache: HashMap::new(),
            quit: false,
        };
        app.new_session()?;
        Ok(app)
    }

    // ----- lifecycle -----

    fn new_session(&mut self) -> Result<()> {
        let sid = self.next_session_id();
        let name = format!("session-{}", self.sessions.len() + 1);
        let pid = Pty::next_pane_id();
        let workspace = self.workspace.clone();
        let (cols, rows) = self.pane_dims();
        let pane = Pane::spawn(
            sid,
            pid,
            self.shell.clone(),
            None,
            Some(workspace.clone()),
            cols,
            rows,
            false,
            self.events_tx.clone(),
        )?;
        self.panes.insert(pid, pane);
        self.sessions.push(Session {
            id: sid,
            name,
            tree: LayoutTree::new(pid),
            zoom: false,
            workspace,
        });
        self.active = self.sessions.len() - 1;
        Ok(())
    }

    fn next_session_id(&mut self) -> u64 {
        let max = self.sessions.iter().map(|s| s.id).max().unwrap_or(0);
        max + 1
    }

    fn split_active(&mut self, dir: SplitDir, is_ai: bool) -> Result<()> {
        let focus = self.sessions[self.active].tree.focus;
        let sid = self.sessions[self.active].id;
        let pid = Pty::next_pane_id();
        let (cols, rows) = self.active_pane_dims(focus).unwrap_or(self.pane_dims());
        let (program, shell) = if is_ai {
            (Some((self.ai.0.clone(), self.ai.1.clone())), self.shell.clone())
        } else {
            (None, self.shell.clone())
        };
        let pane = Pane::spawn(
            sid,
            pid,
            shell,
            program,
            Some(self.sessions[self.active].workspace.clone()),
            cols,
            rows,
            is_ai,
            self.events_tx.clone(),
        )?;
        self.panes.insert(pid, pane);
        if !self.sessions[self.active].tree.split(focus, pid, dir) {
            if let Some(mut p) = self.panes.remove(&pid) {
                p.pty.kill();
            }
        }
        Ok(())
    }

    fn close_focused(&mut self) {
        let focus = self.sessions[self.active].tree.focus;
        self.close_pane(focus);
    }

    fn close_pane(&mut self, pid: u64) {
        if let Some(mut pane) = self.panes.remove(&pid) {
            pane.pty.kill();
        }
        self.last_sizes.remove(&pid);
        self.pane_cache.remove(&pid);

        let empty = self.sessions[self.active].tree.remove_pane(pid);
        if empty {
            self.sessions.remove(self.active);
            if self.sessions.is_empty() {
                self.quit = true;
                return;
            }
            self.active = self.active.min(self.sessions.len() - 1);
        }
    }

    /// Remove panes whose child process has exited, collapsing the layout.
    fn poll_exits(&mut self) {
        let mut exited: Vec<u64> = Vec::new();
        for (pid, pane) in self.panes.iter_mut() {
            if !pane.dead && matches!(pane.pty.child.try_wait(), Ok(Some(_))) {
                pane.dead = true;
                exited.push(*pid);
            }
        }
        for pid in exited {
            if !self.panes.contains_key(&pid) {
                continue;
            }
            let mut idx = None;
            for (i, s) in self.sessions.iter().enumerate() {
                if s.tree.contains(pid) {
                    idx = Some(i);
                    break;
                }
            }
            if let Some(idx) = idx {
                self.close_pane_from_session(idx, pid);
            }
        }
    }

    fn close_pane_from_session(&mut self, idx: usize, pid: u64) {
        if let Some(mut pane) = self.panes.remove(&pid) {
            pane.pty.kill();
        }
        self.last_sizes.remove(&pid);
        self.pane_cache.remove(&pid);
        let empty = self.sessions[idx].tree.remove_pane(pid);
        if empty {
            self.sessions.remove(idx);
            if self.sessions.is_empty() {
                self.quit = true;
                return;
            }
            self.active = self.active.saturating_sub(1).min(self.sessions.len() - 1);
        }
    }

    // ----- events -----

    fn on_pty_event(&mut self, ev: PtyEvent) {
        let PtyEvent::Output { pane_id, data } = ev;
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.feed(&data);
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Leader key: Ctrl+Space. Terminals report it as NUL, space-with-ctrl,
        // or a literal space in the enhanced keyboard protocol.
        let is_leader = ctrl
            && matches!(key.code, KeyCode::Char(' ') | KeyCode::Char('\0') | KeyCode::Null);

        match self.mode {
            Mode::Normal => {
                if is_leader {
                    self.mode = Mode::Leader;
                    return Ok(());
                }
                let focus = self.sessions[self.active].tree.focus;
                if let Some(pane) = self.panes.get_mut(&focus) {
                    let bytes = crate::keys::encode(key);
                    if !bytes.is_empty() {
                        pane.write(&bytes);
                    }
                }
            }
            Mode::Leader => {
                if is_leader || key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                self.leader_command(key)?;
            }
        }
        Ok(())
    }

    fn leader_command(&mut self, key: KeyEvent) -> Result<()> {
        self.mode = Mode::Normal;
        match key.code {
            KeyCode::Char('v') => self.split_active(SplitDir::V, false)?,
            KeyCode::Char('-') => self.split_active(SplitDir::H, false)?,
            KeyCode::Char('a') => self.split_active(SplitDir::V, true)?,
            KeyCode::Char('c') => self.new_session()?,
            KeyCode::Char('x') => self.close_focused(),
            KeyCode::Char('z') => {
                self.sessions[self.active].zoom = !self.sessions[self.active].zoom;
            }
            KeyCode::Char('h') => self.focus_dir(Dir::Left),
            KeyCode::Char('j') => self.focus_dir(Dir::Down),
            KeyCode::Char('k') => self.focus_dir(Dir::Up),
            KeyCode::Char('l') => self.focus_dir(Dir::Right),
            KeyCode::Char('b') => self.sidebar_open = !self.sidebar_open,
            KeyCode::Char('d') => self.quit = true, // detach (exit the TUI)
            KeyCode::Char('n') => self.cycle_session(1),
            KeyCode::Char('p') => self.cycle_session(-1),
            KeyCode::Tab => self.cycle_pane(),
            _ => {}
        }
        Ok(())
    }

    fn cycle_session(&mut self, delta: isize) {
        let n = self.sessions.len();
        if n > 1 {
            self.active = ((self.active as isize + delta).rem_euclid(n as isize)) as usize;
        }
    }

    fn cycle_pane(&mut self) {
        let ids = self.sessions[self.active].tree.pane_ids();
        if ids.len() < 2 {
            return;
        }
        let cur = self.sessions[self.active].tree.focus;
        if let Some(pos) = ids.iter().position(|p| *p == cur) {
            self.sessions[self.active].tree.focus = ids[(pos + 1) % ids.len()];
        }
    }

    fn focus_dir(&mut self, dir: Dir) {
        self.sessions[self.active].zoom = false;
        let geom = self.tree_geom();
        let focus = self.sessions[self.active].tree.focus;
        let cur = match geom.panes.iter().find(|p| p.pane_id == focus) {
            Some(p) => p,
            None => return,
        };
        let best = geom
            .panes
            .iter()
            .filter(|p| p.pane_id != focus)
            .filter(|p| match dir {
                Dir::Left => p.rect.right() <= cur.rect.left(),
                Dir::Right => p.rect.left() >= cur.rect.right(),
                Dir::Up => p.rect.bottom() <= cur.rect.top(),
                Dir::Down => p.rect.top() >= cur.rect.bottom(),
            })
            .min_by(|a, b| {
                let da = (a.rect.right() - a.rect.left()).abs_diff(cur.rect.left())
                    + (a.rect.top() as i32 - cur.rect.top() as i32).unsigned_abs() as u16;
                let db = (b.rect.right() - b.rect.left()).abs_diff(cur.rect.left())
                    + (b.rect.top() as i32 - cur.rect.top() as i32).unsigned_abs() as u16;
                da.cmp(&db)
            });
        if let Some(p) = best {
            self.sessions[self.active].tree.focus = p.pane_id;
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        let x = m.column;
        let y = m.row;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.sidebar_open && x < self.sidebar_width {
                    if self.sidebar_hit(x, y) {
                        return Ok(());
                    }
                }
                if let Some(sg) = self.splitter_at(x, y) {
                    self.drag = Some(Drag::Splitter { split_id: sg.split_id });
                    return Ok(());
                }
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    let inner = pg.inner();
                    let col = x.saturating_sub(inner.x);
                    let row = y.saturating_sub(inner.y);
                    let reporting = self
                        .panes
                        .get(&pg.pane_id)
                        .map(|p| p.has_mouse_reporting())
                        .unwrap_or(false);
                    if reporting {
                        // Hold the press until it resolves as a click (sent to
                        // the app on release) or a drag (kumo text selection),
                        // so drag-select works even when the app owns the mouse.
                        self.pending_click = Some(PendingClick { pane_id: pg.pane_id, col, row });
                    } else {
                        self.sel = Some(Sel {
                            pane_id: pg.pane_id,
                            start: (col, row),
                            end: (col, row),
                        });
                        if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                            pane.set_selection((col, row), (col, row));
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(Drag::Splitter { split_id }) = self.drag {
                    let geom = self.active_geom();
                    if let Some(sg) = geom.splitters.iter().find(|s| s.split_id == split_id) {
                        let ratio = match sg.dir {
                            SplitDir::V => {
                                (x.saturating_sub(sg.area.x)) as f32 / (sg.area.width - 1) as f32
                            }
                            SplitDir::H => {
                                (y.saturating_sub(sg.area.y)) as f32 / (sg.area.height - 1) as f32
                            }
                        };
                        self.sessions[self.active].tree.set_ratio(split_id, ratio);
                    }
                    return Ok(());
                }
                let sel = self.sel;
                if let Some(sel) = sel {
                    if let Some(pg) = self.pane_at(x, y) {
                        if pg.pane_id == sel.pane_id {
                            let inner = pg.inner();
                            let c = x
                                .saturating_sub(inner.x)
                                .min(inner.width.saturating_sub(1));
                            let r = y
                                .saturating_sub(inner.y)
                                .min(inner.height.saturating_sub(1));
                            self.sel.as_mut().unwrap().end = (c, r);
                            if let Some(pane) = self.panes.get_mut(&sel.pane_id) {
                                pane.set_selection(sel.start, (c, r));
                            }
                        }
                    }
                    return Ok(());
                }
                // A press that moves enough becomes a selection instead of a click.
                if let Some(pc) = self.pending_click {
                    let crossed = self
                        .pane_at(x, y)
                        .filter(|pg| pg.pane_id == pc.pane_id)
                        .map(|pg| {
                            let i = pg.inner();
                            let c = x.saturating_sub(i.x);
                            let r = y.saturating_sub(i.y);
                            c.abs_diff(pc.col) >= 2 || r.abs_diff(pc.row) >= 2
                        })
                        .unwrap_or(false);
                    if crossed {
                        let start = (pc.col, pc.row);
                        let end = self
                            .pane_at(x, y)
                            .map(|pg| {
                                let i = pg.inner();
                                let c = x.saturating_sub(i.x).min(i.width.saturating_sub(1));
                                let r = y.saturating_sub(i.y).min(i.height.saturating_sub(1));
                                (c, r)
                            })
                            .unwrap_or(start);
                        self.pending_click = None;
                        self.sel = Some(Sel { pane_id: pc.pane_id, start, end });
                        if let Some(pane) = self.panes.get_mut(&pc.pane_id) {
                            pane.set_selection(start, end);
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                if let Some(pc) = self.pending_click.take() {
                    // It was a click: deliver press+release to the app.
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    let up = self
                        .pane_at(x, y)
                        .filter(|pg| pg.pane_id == pc.pane_id)
                        .map(|pg| {
                            let i = pg.inner();
                            (x.saturating_sub(i.x) + 1, y.saturating_sub(i.y) + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    if let Some(pane) = self.panes.get_mut(&pc.pane_id) {
                        pane.write(&sgr_mouse(b, pc.col + 1, pc.row + 1, false));
                        pane.write(&sgr_mouse(b, up.0, up.1, true));
                    }
                } else if let Some(sel) = self.sel.take() {
                    // A plain click without drag copies nothing, like a normal
                    // terminal; only an actual drag copies.
                    if sel.start != sel.end {
                        self.copy_selection(&sel);
                    } else if let Some(pane) = self.panes.get_mut(&sel.pane_id) {
                        pane.clear_selection();
                    }
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    let inner = pg.inner();
                    let col = x - inner.x + 1;
                    let row = y - inner.y + 1;
                    if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                        if pane.has_mouse_reporting() {
                            let b = if up { 64 } else { 65 };
                            pane.write(&sgr_mouse(b, col, row, false));
                        } else if pane.in_alt_screen() {
                            pane.write(if up { b"\x1b[A" } else { b"\x1b[B" });
                        } else {
                            pane.scroll(if up { -3 } else { 3 });
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ----- geometry / focus -----

    /// Rect covered by the pane grid (excludes the status bar).
    fn panes_area(&self) -> Rect {
        let (w, h) = self.term_size;
        let x = if self.sidebar_open {
            (self.sidebar_width + 1).min(w.saturating_sub(1))
        } else {
            0
        };
        Rect::new(x, 0, w.saturating_sub(x), h.saturating_sub(1))
    }

    /// Geometry without zoom applied (used for navigation).
    fn tree_geom(&self) -> TreeGeom {
        let mut geom = TreeGeom::default();
        if let Some(root) = &self.sessions[self.active].tree.root {
            layout::compute_geometry(root, self.panes_area(), &mut geom);
        }
        geom
    }

    fn active_geom(&self) -> TreeGeom {
        let mut geom = TreeGeom::default();
        let session = &self.sessions[self.active];
        if let Some(root) = &session.tree.root {
            if session.zoom {
                geom.panes.push(layout::PaneGeom {
                    pane_id: session.tree.focus,
                    rect: self.panes_area(),
                });
            } else {
                layout::compute_geometry(root, self.panes_area(), &mut geom);
            }
        }
        geom
    }

    fn active_pane_dims(&self, pid: u64) -> Option<(u16, u16)> {
        self.active_geom()
            .panes
            .iter()
            .find(|p| p.pane_id == pid)
            .map(|p| {
                let inner = p.inner();
                (inner.width, inner.height)
            })
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<layout::PaneGeom> {
        self.active_geom()
            .panes
            .into_iter()
            .find(|p| p.rect.contains(Position::new(x, y)))
    }

    fn splitter_at(&self, x: u16, y: u16) -> Option<layout::SplitGeom> {
        self.active_geom()
            .splitters
            .into_iter()
            .find(|s| s.rect.contains(Position::new(x, y)))
    }

    fn set_focus(&mut self, pid: u64) {
        if self.sessions[self.active].tree.contains(pid) {
            self.sessions[self.active].tree.focus = pid;
        }
    }

    fn copy_selection(&mut self, sel: &Sel) {
        if let Some(pane) = self.panes.get_mut(&sel.pane_id) {
            if let Some(text) = pane.selection_text(sel.start, sel.end) {
                if !text.is_empty() {
                    copy_to_clipboard(&text);
                }
            }
            pane.clear_selection();
        }
    }

    /// Short label of the AI CLI running in `pid` (e.g. "opencode"), read from
    /// the cached process scan. Falls back to "AI CLI".
    fn agent_label(&self, pid: u64) -> String {
        self.panes
            .get(&pid)
            .and_then(|p| p.detected_ai_name.clone())
            .map(|name| name.rsplit('/').next().unwrap_or(&name).to_string())
            .unwrap_or_else(|| "AI CLI".to_string())
    }

    fn pane_dims(&self) -> (u16, u16) {
        let r = self.panes_area();
        (r.width.max(1), r.height.max(1))
    }

    // ----- git branch -----

    /// Refresh cached git branches for all session workspaces (every
    /// `BRANCH_REFRESH`). Runs `git` off the hot frame path (once per frame).
    fn refresh_branches(&mut self) {
        let now = Instant::now();
        let live: Vec<PathBuf> = self.sessions.iter().map(|s| s.workspace.clone()).collect();
        for ws in &live {
            let stale = match self.branch_cache.get(ws) {
                Some((_, t)) => now.duration_since(*t) >= BRANCH_REFRESH,
                None => true,
            };
            if stale {
                let branch = git_branch(ws);
                self.branch_cache.insert(ws.clone(), (branch, now));
            }
        }
        self.branch_cache.retain(|ws, _| live.contains(ws));
    }

    /// Cached git branch for a session's workspace.
    fn session_branch(&self, idx: usize) -> Option<String> {
        let ws = &self.sessions[idx].workspace;
        self.branch_cache.get(ws).and_then(|(b, _)| b.clone())
    }

    /// Mark plain shell panes as AI CLI panes when opencode/claude is running
    /// inside them, and clear the flag once the process exits. Runs at most
    /// every `AI_SCAN_INTERVAL`.
    fn refresh_ai_cli(&mut self) {
        if self.last_ai_scan.elapsed() < AI_SCAN_INTERVAL {
            return;
        }
        self.last_ai_scan = Instant::now();
        for pane in self.panes.values_mut() {
            let name = pane.ai_cli_name();
            pane.detected_ai_name = name.clone();
            if !pane.is_ai {
                pane.detected_ai = name.is_some();
            }
        }
    }

    /// Append the per-pane agent status, output age, and detected CLI to
    /// `/tmp/kumo_agent.log` (throttled to 1/s, capped at 512 KiB). Gated
    /// behind `DEBUG_AGENT=1` so it is inert in production but stays in the
    /// codebase for diagnostics.
    fn log_agent_statuses(&mut self) {
        if std::env::var("DEBUG_AGENT").is_err() {
            return;
        }
        if self.last_agent_debug.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_agent_debug = Instant::now();
        use std::io::Write;
        const PATH: &str = "/tmp/kumo_agent.log";
        if std::fs::metadata(PATH).map(|m| m.len()).unwrap_or(0) > 512 * 1024 {
            let _ = std::fs::write(PATH, b"");
        }
        if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(PATH) {
            for (pid, pane) in self.panes.iter() {
                if !pane.is_ai_cli() {
                    continue;
                }
                let tail = pane.recent_text_tail(200).replace('\n', "\\n");
                let _ = writeln!(
                    log,
                    "pid={} cli={} status={:?} age_ms={} recent={}",
                    pid,
                    pane.detected_ai_name.as_deref().unwrap_or("?"),
                    pane.agent_status(),
                    pane.last_output_age().as_millis(),
                    tail,
                );
            }
        }
    }

    /// Static rows of the sidebar (shared by render + mouse hit-testing).
    ///
    /// Top half: sessions with their git branch. Bottom half (starting at the
    /// vertical midpoint) holds the AGENTS list.
    fn sidebar_rows(&self) -> Vec<(u16, SidebarRow)> {
        let mut out = Vec::new();
        let mut y: u16 = 0;
        out.push((y, SidebarRow::Header("kumo".into())));
        y += 1;
        out.push((y, SidebarRow::Spacer));
        y += 1;
        out.push((y, SidebarRow::Section("sessions".into())));
        y += 1;
        for (i, _s) in self.sessions.iter().enumerate() {
            out.push((y, SidebarRow::Session(i)));
            y += 1;
            if let Some(branch) = self.session_branch(i) {
                out.push((y, SidebarRow::Branch(branch)));
                y += 1;
            }
        }

        // AGENTS starts exactly at the screen midpoint (or below the sessions
        // if they overflow it).
        let footer_y = self.term_size.1.saturating_sub(2);
        let agents_y = (self.term_size.1 / 2).max(y).min(footer_y);
        out.push((agents_y, SidebarRow::Section("agents".into())));
        let mut ay = agents_y + 1;
        for (i, s) in self.sessions.iter().enumerate() {
            for pid in s.tree.pane_ids() {
                if !self.panes.get(&pid).map(|p| p.is_ai_cli()).unwrap_or(false) {
                    continue;
                }
                // Each agent takes two rows: workspace folder + agent name.
                if ay + 1 >= footer_y {
                    break;
                }
                out.push((ay, SidebarRow::AgentDir(i, pid)));
                out.push((ay + 1, SidebarRow::AgentName(i, pid)));
                ay += 2;
            }
        }
        out.push((footer_y, SidebarRow::NewSession));
        out
    }

    fn sidebar_hit(&mut self, _x: u16, y: u16) -> bool {
        for (ry, row) in self.sidebar_rows() {
            if ry != y {
                continue;
            }
            match row {
                SidebarRow::Session(i) => {
                    self.active = i;
                    return true;
                }
                SidebarRow::AgentDir(i, pid) | SidebarRow::AgentName(i, pid) => {
                    self.active = i;
                    self.sessions[i].tree.focus = pid;
                    return true;
                }
                SidebarRow::NewSession => {
                    let _ = self.new_session();
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    // ----- rendering -----

    fn frame(&mut self, terminal: &mut Term) -> Result<()> {
        self.poll_exits();
        if self.quit {
            return Ok(());
        }
        let size = terminal.size()?;
        self.term_size = (size.width, size.height);
        self.refresh_branches();
        self.refresh_ai_cli();
        self.log_agent_statuses();
        let area = Rect::new(0, 0, size.width, size.height);
        let geom = self.active_geom();

        for pg in &geom.panes {
            if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                let inner = pg.inner();
                let key = (inner.width, inner.height);
                if self.last_sizes.get(&pg.pane_id) != Some(&key) {
                    pane.resize(inner.width, inner.height);
                    self.last_sizes.insert(pg.pane_id, key);
                }
            }
        }

        let focused = self.sessions[self.active].tree.focus;
        // When focus moves, re-render the old and new panes so the cursor
        // highlight is drawn/cleared even if neither produced output.
        if self.last_focused != Some(focused) {
            if let Some(old) = self.last_focused {
                if let Some(p) = self.panes.get_mut(&old) {
                    p.dirty = true;
                }
            }
            if let Some(p) = self.panes.get_mut(&focused) {
                p.dirty = true;
            }
            self.last_focused = Some(focused);
        }
        let geom_ref = &geom;
        terminal.draw(|f| self.render(f, area, geom_ref, focused))?;
        self.place_cursor(terminal, &geom, focused)?;
        Ok(())
    }

    fn render(&mut self, f: &mut Frame, size: Rect, geom: &TreeGeom, focused: u64) {
        // Note: no global fill over the pane area, so unchanged (non-dirty)
        // panes keep the cells ratatui retains from their last render.
        for pg in &geom.panes {
            let title = self.pane_title(pg.pane_id, pg.pane_id == focused);
            self.render_pane_frame(f, pg.rect, pg.pane_id == focused, &title);
        }
        for pg in &geom.panes {
            if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                let inner = pg.inner();
                if inner.width > 0 && inner.height > 0 {
                    // Re-render dirty rows into the pane's retained cache;
                    // unchanged rows are kept and blitted back (no FFI scan).
                    if pane.dirty {
                        // Keep the previous cache unless it was for a different
                        // rect (moved/resized), so clean rows survive.
                        let recreate = self
                            .pane_cache
                            .get(&pg.pane_id)
                            .map(|c| c.area != inner)
                            .unwrap_or(true);
                        if recreate {
                            pane.full_redraw = true;
                            self.pane_cache.insert(pg.pane_id, Buffer::empty(inner));
                        }
                        if let Some(cached) = self.pane_cache.get_mut(&pg.pane_id) {
                            let status = pane.render_dirty(inner, pg.pane_id == focused, cached);
                            if let Some(status) = status {
                                self.agent_status_cache.insert(pg.pane_id, status);
                            }
                        }
                    }
                    if let Some(cached) = self.pane_cache.get(&pg.pane_id) {
                        let dst = f.buffer_mut();
                        for (i, src) in cached.content.iter().enumerate() {
                            let (x, y) = cached.pos_of(i);
                            if let Some(dst_cell) = dst.cell_mut((x, y)) {
                                *dst_cell = src.clone();
                            }
                        }
                    }
                    let sb = pane.scrollbar_data();
                    self.render_scrollbar(f, &sb, inner);
                }
            }
        }

        if self.sidebar_open {
            self.render_sidebar(f, size);
        }

        self.render_status(f, size);
    }

    fn pane_title(&self, pid: u64, focused: bool) -> String {
        let base = match self.panes.get(&pid) {
            Some(p) if p.is_ai_cli() => " AI CLI ".to_string(),
            Some(_) => {
                if self.sessions[self.active].tree.pane_count() > 1 {
                    format!(" shell {} ", pid)
                } else {
                    " shell ".to_string()
                }
            }
            None => " pane ".to_string(),
        };
        if focused && self.sessions[self.active].zoom {
            format!("{base}(zoom) ")
        } else {
            base
        }
    }

    fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, title: &str) {
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        let border = if focused { crate::pane::ACCENT } else { BORDER_IDLE };
        // Native background: the frame is just line glyphs over the host
        // terminal's background, matching the pane content.
        let border_style = Style::default().fg(border).bg(RColor::Reset);
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        put(f, x0, y0, "┌", border_style);
        put(f, x1, y0, "┐", border_style);
        put(f, x0, y1, "└", border_style);
        put(f, x1, y1, "┘", border_style);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border_style);
            put(f, x, y1, "─", border_style);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border_style);
            put(f, x1, y, "│", border_style);
        }
        // Title chip: filled accent when focused, plain otherwise.
        let max = rect.width.saturating_sub(2) as usize;
        let chip = if focused {
            Style::default()
                .fg(RColor::Black)
                .bg(crate::pane::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(crate::pane::FG).bg(RColor::Reset)
        };
        for (i, ch) in title.chars().take(max).enumerate() {
            put(f, x0 + 1 + i as u16, y0, &ch.to_string(), chip);
        }
    }

    fn render_scrollbar(&self, f: &mut Frame, sb: &vt::TerminalScrollbar, inner: Rect) {
        let total = sb.total as usize;
        let screen = sb.len as usize;
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
                put(f, x, y, "▐", Style::default().fg(crate::pane::ACCENT));
            } else {
                put(f, x, y, "░", Style::default().fg(PANEL_SEP));
            }
        }
    }

    fn render_sidebar(&self, f: &mut Frame, size: Rect) {
        let w = self.sidebar_width.min(size.width);
        let area = Rect::new(0, 0, w, size.height.saturating_sub(1));
        fill(f, area, RColor::Reset);
        // Separator between sidebar and panes.
        for y in area.y..(area.y + area.height) {
            put(f, area.x + area.width, y, "│", Style::default().fg(PANEL_SEP));
        }
        for (y, row) in self.sidebar_rows() {
            if y > area.y + area.height {
                break;
            }
            let x = area.x;
            let max = w.saturating_sub(1);
            match row {
                SidebarRow::Header(t) => {
                    let style = Style::default()
                        .fg(crate::pane::ACCENT)
                        .bg(RColor::Reset)
                        .add_modifier(Modifier::BOLD);
                    text(f, x, y, &format!("  {}", t), style, max);
                }
                SidebarRow::Spacer => {
                    put(f, x, y, " ", Style::default().bg(RColor::Reset));
                }
                SidebarRow::Section(t) => {
                    let style = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
                    text(f, x, y, &format!("  {}", t.to_uppercase()), style, max);
                }
                SidebarRow::Session(i) => {
                    let active = i == self.active;
                    let name = &self.sessions[i].name;
                    let (marker, fg) = if active {
                        ("▸", crate::pane::ACCENT)
                    } else {
                        (" ", PANEL_MUTED)
                    };
                    let line = format!(" {marker} {}", name);
                    text(f, x, y, &line, Style::default().fg(fg).bg(RColor::Reset), max);
                }
                SidebarRow::Branch(b) => {
                    let style = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
                    text(f, x, y, &format!("    {}", b), style, max);
                }
                SidebarRow::AgentDir(i, pid) | SidebarRow::AgentName(i, pid) => {
                    let focused =
                        i == self.active && self.sessions[self.active].tree.focus == pid;
                    let bg = if focused { PANEL_SEP } else { RColor::Reset };
                    // Light up the whole sidebar row when this agent pane is focused.
                    if focused {
                        fill(f, Rect::new(x, y, max + 1, 1), bg);
                    }
                    let status = self.agent_status_cache.get(&pid).copied().unwrap_or(AgentStatus::Idle);
                    let status_color = match status {
                        AgentStatus::Working => GREEN,
                        AgentStatus::Blocked => ORANGE,
                        AgentStatus::Idle => PANEL_MUTED,
                    };
                    match row {
                        SidebarRow::AgentDir(_, _) => {
                            put(f, x + 2, y, "●", Style::default().fg(status_color).bg(bg));
                            let path = short_workspace(&self.sessions[i].workspace);
                            let path_color = if focused { crate::pane::FG } else { PANEL_MUTED };
                            text(f, x + 4, y, &path, Style::default().fg(path_color).bg(bg), max.saturating_sub(4));
                        }
                        SidebarRow::AgentName(_, _) => {
                            let name = self.agent_label(pid);
                            text(f, x + 4, y, &name, Style::default().fg(status_color).bg(bg), max.saturating_sub(4));
                        }
                        _ => {}
                    }
                }
                SidebarRow::NewSession => {
                    let style = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
                    text(f, x, y, "  + new session", style, max);
                }
            }
        }
    }

    fn render_status(&self, f: &mut Frame, size: Rect) {
        let area = Rect::new(0, size.height.saturating_sub(1), size.width, 1);
        fill(f, area, RColor::Reset);
        let session = &self.sessions[self.active];
        let n = session.tree.pane_count();
        let mode = if self.mode == Mode::Leader { "LEADER" } else { "NORMAL" };
        let mode_style = if self.mode == Mode::Leader {
            Style::default().fg(RColor::Black).bg(YELLOW).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(RColor::Black).bg(crate::pane::ACCENT)
        };

        let mut spans: Vec<Span> = vec![
            Span::styled(format!(" {} ", mode), mode_style),
            Span::raw(" "),
            Span::styled(session.name.clone(), Style::default().fg(crate::pane::FG).bg(RColor::Reset)),
            Span::styled(format!(" · {n} panes"), Style::default().fg(PANEL_MUTED).bg(RColor::Reset)),
        ];
        if session.zoom {
            spans.push(Span::styled(
                " · zoomed",
                Style::default().fg(YELLOW).bg(RColor::Reset),
            ));
        }
        if !self.sidebar_open {
            spans.push(Span::styled(
                " · sidebar hidden",
                Style::default().fg(PANEL_MUTED).bg(RColor::Reset),
            ));
        }

        let right = if self.mode == Mode::Leader {
            Some(Span::styled(
                " v: v-split · -: h-split · a: AI · c: new · x: close · z: zoom · h/j/k/l: focus · n/p: session · tab: pane · b: sidebar · d: detach · esc: exit ",
                Style::default().fg(RColor::Black).bg(YELLOW).add_modifier(Modifier::BOLD),
            ))
        } else {
            None
        };
        if let Some(right) = right {
            let left_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let right_w = right.content.chars().count();
            let avail = area.width as usize;
            if left_w + right_w <= avail {
                spans.push(Span::raw(" ".repeat(avail - left_w - right_w)));
                spans.push(right);
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn place_cursor(&mut self, terminal: &mut Term, geom: &TreeGeom, focused: u64) -> Result<()> {
        if let Some(pg) = geom.panes.iter().find(|p| p.pane_id == focused) {
            if let Some(pane) = self.panes.get(&pg.pane_id) {
                let inner = pg.inner();
                if let Some((cx, cy)) = pane.cursor_pos() {
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
        terminal.hide_cursor()?;
        Ok(())
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

/// Current git branch of `ws`, if it is a git repository.
fn git_branch(ws: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C", ws.to_str().unwrap_or_default(), "branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch)
    }
}

fn put(f: &mut Frame, x: u16, y: u16, ch: &str, style: Style) {
    let a = f.area();
    if x >= a.width || y >= a.height {
        return;
    }
    let c = f.buffer_mut().cell_mut((x, y)).unwrap();
    c.set_symbol(ch).set_style(style);
}

fn text(f: &mut Frame, x: u16, y: u16, s: &str, style: Style, max_width: u16) {
    for (i, ch) in s.chars().take(max_width as usize).enumerate() {
        put(f, x + i as u16, y, &ch.to_string(), style);
    }
}

fn fill(f: &mut Frame, area: Rect, color: RColor) {
    for y in area.y..(area.y + area.height) {
        for x in area.x..(area.x + area.width) {
            let a = f.area();
            if x >= a.width || y >= a.height {
                continue;
            }
            let c = f.buffer_mut().cell_mut((x, y)).unwrap();
            c.set_symbol(" ").set_style(Style::default().bg(color));
        }
    }
}

/// Copy `text` to the clipboard: OSC 52 to the outer terminal, plus `pbcopy`
/// on macOS as a fallback.
fn copy_to_clipboard(text: &str) {
    let b64 = base64_encode(text.as_bytes());
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(format!("\x1b]52;c;{b64}\x07").as_bytes());
    let _ = stdout.flush();
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}
