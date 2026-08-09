use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color as RColor;
use ratatui::Terminal;

use crate::layout::{self, LayoutTree, SplitDir};
use crate::pane::{AgentStatus, Pane, PtyEvent};
use crate::pty::Pty;

use self::mouse::{Drag, PendingClick, Sel};
use self::overlays::{is_leader, CtxMenu, CtxTarget, Menu, NamePopup};
use self::sidebar::SidebarScroll;

mod mouse;
mod overlays;
mod sidebar;
mod tasks;
mod ui;
mod util;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Catppuccin mocha chrome colors (sidebars, status bar, chrome borders).
const PANEL_SEP: RColor = RColor::Rgb(0x31, 0x32, 0x44); // surface0
const PANEL_MUTED: RColor = RColor::Rgb(0x6c, 0x70, 0x86); // overlay0
const BORDER_IDLE: RColor = RColor::Rgb(0x6c, 0x70, 0x86); // overlay0, visible on any terminal bg
const YELLOW: RColor = RColor::Rgb(0xf9, 0xe2, 0xaf); // yellow
const GREEN: RColor = RColor::Rgb(0xa6, 0xe3, 0xa1); // green
const ORANGE: RColor = RColor::Rgb(0xfa, 0xb3, 0x87); // peach

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

#[derive(Clone, Copy)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
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
    /// When agent status was last recomputed from the terminal buffer (so a
    /// finished, quiet agent falls back to Idle without new output).
    last_status_refresh: Instant,
    /// Cached agent status per AI pane, refreshed during pane rendering.
    agent_status_cache: HashMap<u64, AgentStatus>,
    /// Previously focused pane, so focus changes re-render the two panes (cursor).
    last_focused: Option<u64>,
    /// Rendered cells of each pane's viewport, blitted back when the pane is
    /// unchanged so the frame loop never re-iterates unchanged terminals.
    pane_cache: HashMap<u64, Buffer>,
    quit: bool,
    /// Status-bar menu (MENU button + dropdown).
    menu: Menu,
    /// Right-click context menu inside a pane.
    ctx_menu: CtxMenu,
    /// Scroll offsets for the sidebar sessions / AGENTS sections.
    sidebar_scroll: SidebarScroll,
    /// Modal popup for naming a new session.
    popup: NamePopup,
    /// Transient status-bar notice, e.g. "config: coming soon".
    notice: Option<(String, Instant)>,
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
            last_status_refresh: Instant::now(),
            agent_status_cache: HashMap::new(),
            last_focused: None,
            pane_cache: HashMap::new(),
            quit: false,
            menu: Menu { open: false, selected: 0 },
            ctx_menu: CtxMenu { open: false, x: 0, y: 0, selected: 0, target: CtxTarget::Pane(0) },
            // AGENTS defaults to the bottom of its region (live list), so the
            // newest agents are visible without scrolling.
            sidebar_scroll: SidebarScroll { sessions: 0, agents: u16::MAX },
            popup: NamePopup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            notice: None,
        };
        app.new_session()?;
        Ok(app)
    }

    // ----- lifecycle -----

    /// Create a session (used for the initial session at startup).
    fn new_session(&mut self) -> Result<()> {
        self.new_session_with_name(self.default_session_name())
    }

    /// Smallest free `session-N` name (N = 1, 2, ...).
    fn default_session_name(&self) -> String {
        let mut n = 1;
        loop {
            let cand = format!("session-{n}");
            if !self.sessions.iter().any(|s| s.name == cand) {
                return cand;
            }
            n += 1;
        }
    }

    /// Create a session with an explicit name and focus it.
    fn new_session_with_name(&mut self, name: String) -> Result<()> {
        let sid = self.next_session_id();
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
        if self.popup.open {
            self.on_popup_key(key);
            return Ok(());
        }
        if self.menu.open {
            self.on_menu_key(key);
            return Ok(());
        }
        if self.ctx_menu.open {
            self.on_ctx_menu_key(key);
            return Ok(());
        }

        let leader = is_leader(key);
        match self.mode {
            Mode::Normal => {
                if leader {
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
                if leader || key.code == KeyCode::Esc {
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
            KeyCode::Char('c') => self.open_session_popup(),
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
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                // leader + 1-9 jumps to the session at that list position.
                let n = c.to_digit(10).unwrap_or(0) as usize;
                if n <= self.sessions.len() {
                    self.active = n - 1;
                }
            }
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
    fn tree_geom(&self) -> layout::TreeGeom {
        let mut geom = layout::TreeGeom::default();
        if let Some(root) = &self.sessions[self.active].tree.root {
            layout::compute_geometry(root, self.panes_area(), &mut geom);
        }
        geom
    }

    fn active_geom(&self) -> layout::TreeGeom {
        let mut geom = layout::TreeGeom::default();
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
}
