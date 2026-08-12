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
use crate::agents::AgentStatus;
use crate::pane::{Pane, PtyEvent};
use crate::pty::Pty;
use crate::state::{self, SavedState};

use self::mouse::{Drag, PendingClick, Sel};
use self::overlays::{is_leader, CtxMenu, CtxTarget, KeybindOverlay, Menu, NamePopup};
use self::sidebar::SidebarScroll;
use self::tasks::BranchInfo;

mod bindings;
mod mouse;
mod overlays;
#[cfg(unix)]
pub(super) mod server;
mod sidebar;
mod tasks;
mod ui;
mod util;

/// Foreground TUI terminal backend, used only by the non-unix fallback path
/// (`App::run`); the daemon renders to a `TestBackend` instead.
#[cfg_attr(unix, allow(dead_code))]
type Term = Terminal<CrosstermBackend<Stdout>>;

/// Catppuccin mocha chrome colors (sidebars, status bar, chrome borders).
const PANEL_SEP: RColor = RColor::Rgb(0x17, 0x18, 0x26); // surface0, dark navy
const PANEL_MUTED: RColor = RColor::Rgb(0x6c, 0x70, 0x86); // overlay0
const BORDER_IDLE: RColor = RColor::Rgb(0x6c, 0x70, 0x86); // overlay0, visible on any terminal bg
const MAUVE: RColor = RColor::Rgb(0xcb, 0xa6, 0xf7); // mauve
const GREEN: RColor = RColor::Rgb(0xa6, 0xe3, 0xa1); // green
const ORANGE: RColor = RColor::Rgb(0xfa, 0xb3, 0x87); // peach
const RED: RColor = RColor::Rgb(0xf3, 0x8b, 0xa8); // red

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

/// How to start kumo (0.3.0 tmux-style CLI).
pub enum Launch {
    /// `kumo`: attach to the last saved state if present, else fresh in cwd.
    Auto,
    /// `kumo attach`: restore the saved state; error if none exists.
    Attach,
    /// `kumo new [WORKSPACE]` / `kumo [WORKSPACE]`: start fresh, never attach.
    New(Option<PathBuf>),
    /// Daemon restarted by `kumo update` (`daemon --resume <file>`): adopt the
    /// inherited PTY masters recorded in the resume file.
    Resume(PathBuf),
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
    /// Cached git branch (name + ahead/behind) per workspace, refreshed periodically.
    branch_cache: HashMap<PathBuf, (Option<BranchInfo>, Instant)>,
    /// When the pane process tree was last scanned for an AI CLI.
    last_ai_scan: Instant,
    /// When the agent-status debug log was last written (throttle).
    last_agent_debug: Instant,
    /// When agent status was last recomputed from the terminal buffer (so a
    /// finished, quiet agent falls back to Idle without new output).
    last_status_refresh: Instant,
    /// Cached agent status per AI pane, refreshed during pane rendering.
    agent_status_cache: HashMap<u64, AgentStatus>,
    /// Last observed agent status per AI pane, for lifecycle transition
    /// detection (unlike `agent_status_cache`, never touched by rendering).
    last_agent_status: HashMap<u64, AgentStatus>,
    /// When the last audible agent alert sounded per pane (cooldown, so a
    /// flickering status does not repeat the beep).
    last_agent_sound: HashMap<u64, Instant>,
    /// Previously focused pane, so focus changes re-render the two panes (cursor).
    last_focused: Option<u64>,
    /// Rendered cells of each pane's viewport, blitted back when the pane is
    /// unchanged so the frame loop never re-iterates unchanged terminals.
    pane_cache: HashMap<u64, Buffer>,
    quit: bool,
    /// True when the user asked to detach (`leader+d` / MENU `detach`): the
    /// loop exits and the state is persisted before returning.
    detach_requested: bool,
    /// Status-bar menu (MENU button + dropdown).
    menu: Menu,
    /// Right-click context menu inside a pane.
    ctx_menu: CtxMenu,
    /// Scroll offsets for the sidebar sessions / AGENTS sections.
    sidebar_scroll: SidebarScroll,
    /// Modal popup for naming a new session.
    popup: NamePopup,
    /// `leader+?` keybind showcase.
    keybind_overlay: KeybindOverlay,
    /// Transient status-bar notice, e.g. "config: coming soon".
    notice: Option<(String, Instant)>,
    /// Startup update banner (top-right), when a newer release exists.
    update_notice: Option<crate::update::UpdateNotice>,
    /// Receives the background update check result.
    update_rx: mpsc::Receiver<Option<crate::update::UpdateNotice>>,
}

/// Foreground TUI loop, used only on non-unix (fallback until daemon parity
/// lands); on unix the daemon drives `App` directly and the thin client renders.
#[cfg_attr(unix, allow(dead_code))]
pub fn run(terminal: &mut Term, launch: Launch) -> Result<()> {
    let mut app = App::new(launch)?;
    while !app.quit {
        while let Ok(ev) = app.events_rx.try_recv() {
            app.on_pty_event(ev);
        }
        while let Ok(notice) = app.update_rx.try_recv() {
            app.update_notice = notice;
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
    app.on_exit();
    Ok(())
}

impl App {
    fn new(launch: Launch) -> Result<App> {
        let shell = crate::config::default_shell();
        let (ai_prog, ai_args) = crate::config::ai_command();
        let ai_prog = crate::config::resolve_program(&ai_prog);
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let cwd = std::env::current_dir().ok();
        // Workspace for a fresh session: the explicit `kumo new [dir]` arg, else
        // the directory kumo was launched from, else $HOME.
        let workspace = match &launch {
            Launch::New(Some(p)) if p.is_dir() => p.clone(),
            _ => cwd.clone().unwrap_or_else(|| home.clone()),
        };

        let (events_tx, events_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let notice = crate::update::poll_update_notice();
            let _ = update_tx.send(notice);
        });
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
            last_agent_status: HashMap::new(),
            last_agent_sound: HashMap::new(),
            last_focused: None,
            pane_cache: HashMap::new(),
            quit: false,
            detach_requested: false,
            menu: Menu { open: false, selected: 0 },
            ctx_menu: CtxMenu { open: false, x: 0, y: 0, selected: 0, target: CtxTarget::Pane(0) },
            // AGENTS defaults to the bottom of its region (live list), so the
            // newest agents are visible without scrolling.
            sidebar_scroll: SidebarScroll { sessions: 0, agents: u16::MAX },
            popup: NamePopup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            keybind_overlay: KeybindOverlay { open: false, scroll: 0 },
            notice: None,
            update_notice: None,
            update_rx,
        };

        match launch {
            Launch::Auto | Launch::Attach => {
                if let Some(state) = state::load(&crate::config::state_file())? {
                    app.restore(state)?;
                } else if matches!(launch, Launch::Attach) {
                    anyhow::bail!("no saved state to attach to (start with `kumo` or `kumo new`)");
                } else {
                    app.new_session()?;
                }
            }
            Launch::New(_) => app.new_session()?,
            #[cfg(unix)]
            Launch::Resume(path) => {
                let resumed = match state::load(&path)? {
                    Some(state) => app.resume(state),
                    None => Ok(false),
                };
                // A missing/corrupt resume file (or an unadoptable pane) must
                // never take the daemon down mid-update: fall back to a fresh
                // session so the daemon still comes up.
                match resumed {
                    Ok(true) => {
                        let _ = std::fs::remove_file(&path);
                    }
                    Ok(false) => {
                        log::warn!("kumo: resume had nothing to adopt; starting fresh");
                        let _ = std::fs::remove_file(&path);
                        app.new_session()?;
                    }
                    Err(e) => {
                        log::warn!("kumo: resume failed ({e:#}); starting fresh");
                        let _ = std::fs::remove_file(&path);
                        app.new_session()?;
                    }
                }
            }
            // PTY master adoption is unix-only; the daemon never passes a
            // resume file on other platforms.
            #[cfg(not(unix))]
            Launch::Resume(_) => app.new_session()?,
        }
        Ok(app)
    }

    /// Rebuild sessions and panes from a saved state. Saved pane ids are
    /// remapped to fresh process-local ids; every pane is respawned via the
    /// same `Pane::spawn` path a fresh session uses (and that 0.4.0's daemon
    /// will own).
    fn restore(&mut self, mut state: SavedState) -> Result<()> {
        // Assign a fresh pane id per saved id, consistently across sessions.
        let mut map = std::collections::HashMap::new();
        for session in &state.sessions {
            let mut ids = Vec::new();
            state::tree_pane_ids(&session.tree, &mut ids);
            for old in ids {
                map.entry(old).or_insert_with(Pty::next_pane_id);
            }
        }
        state::remap_pane_ids(&mut state, &map);

        self.panes.clear();
        self.sessions.clear();
        let (cols, rows) = self.pane_dims();
        let saved_active = state.active;
        for (i, saved) in state.sessions.into_iter().enumerate() {
            let sid = self.next_session_id();
            for sp in saved.panes {
                let mut pane = Pane::spawn(
                    sid,
                    sp.id,
                    sp.shell,
                    sp.program,
                    Some(sp.cwd.clone()),
                    cols,
                    rows,
                    sp.is_ai,
                    self.events_tx.clone(),
                )?;
                pane.custom_name = sp.custom_name;
                self.panes.insert(sp.id, pane);
            }
            let mut tree = LayoutTree::from_node(state::to_layout_node(&saved.tree), saved.focus);
            if !tree.contains(tree.focus) {
                if let Some(&first) = tree.pane_ids().first() {
                    tree.focus = first;
                }
            }
            self.sessions.push(Session {
                id: sid,
                name: saved.name,
                tree,
                zoom: saved.zoom,
                workspace: saved.workspace,
            });
            self.active = i;
        }
        if self.sessions.is_empty() {
            // Saved state without any surviving pane (or empty): fall back to a
            // fresh session rather than rendering a broken tree.
            self.new_session()?;
        } else {
            self.active = saved_active.min(self.sessions.len() - 1);
        }
        Ok(())
    }

    /// Rebuild sessions/panes from a resume file (daemon restart for `kumo
    /// update`), adopting each pane's inherited PTY master descriptor. Terminal
    /// screens come back fresh — the live child processes keep running inside
    /// the PTYs. Returns whether any pane/session was actually resumed.
    #[cfg(unix)]
    fn resume(&mut self, mut state: SavedState) -> Result<bool> {
        // Assign a fresh pane id per saved id, consistently across sessions.
        let mut map = std::collections::HashMap::new();
        for session in &state.sessions {
            let mut ids = Vec::new();
            state::tree_pane_ids(&session.tree, &mut ids);
            for old in ids {
                map.entry(old).or_insert_with(Pty::next_pane_id);
            }
        }
        state::remap_pane_ids(&mut state, &map);

        self.panes.clear();
        self.sessions.clear();
        let saved_active = state.active;
        for (i, saved) in state.sessions.into_iter().enumerate() {
            let sid = self.next_session_id();
            let mut missing = Vec::new();
            for sp in saved.panes {
                let Some(fd) = sp.master_fd else {
                    missing.push(sp.id);
                    continue;
                };
                let mut pane = Pane::resume(
                    sid,
                    sp.id,
                    sp.shell,
                    sp.program,
                    sp.cwd.clone(),
                    sp.cols,
                    sp.rows,
                    sp.is_ai,
                    fd as i32,
                    sp.child_pid.map(|p| p as i32),
                    self.events_tx.clone(),
                )?;
                pane.custom_name = sp.custom_name;
                self.panes.insert(sp.id, pane);
            }
            let mut tree = LayoutTree::from_node(state::to_layout_node(&saved.tree), saved.focus);
            // A pane with no recordable master fd was skipped: drop it from the
            // tree so no dangling pane id is ever rendered.
            for pid in missing {
                tree.remove_pane(pid);
            }
            if !tree.contains(tree.focus) {
                if let Some(&first) = tree.pane_ids().first() {
                    tree.focus = first;
                }
            }
            self.sessions.push(Session {
                id: sid,
                name: saved.name,
                tree,
                zoom: saved.zoom,
                workspace: saved.workspace,
            });
            self.active = i;
        }
        if self.sessions.is_empty() {
            return Ok(false);
        }
        self.active = saved_active.min(self.sessions.len() - 1);
        self.workspace = self.sessions[self.active].workspace.clone();
        Ok(true)
    }

    /// Serialize the current sessions/panes for `state::save`. Dormant until
    /// 0.5.0 persistence revives it on the daemon side.
    #[cfg_attr(unix, allow(dead_code))]
    fn to_saved_state(&self) -> Option<SavedState> {
        if self.sessions.is_empty() {
            return None;
        }
        let mut sessions = Vec::new();
        for session in &self.sessions {
            let root = session.tree.root.as_ref()?;
            let mut panes = Vec::new();
            for pid in session.tree.pane_ids() {
                let Some(pane) = self.panes.get(&pid) else { continue };
                panes.push(state::SavedPane {
                    id: pid,
                    is_ai: pane.is_ai,
                    shell: pane.pty.shell.clone(),
                    program: pane.program.clone(),
                    cwd: pane.cwd.clone(),
                    custom_name: pane.custom_name.clone(),
                    master_fd: None,
                    child_pid: None,
                    cols: 0,
                    rows: 0,
                });
            }
            sessions.push(state::SavedSession {
                name: session.name.clone(),
                workspace: session.workspace.clone(),
                zoom: session.zoom,
                focus: session.tree.focus,
                tree: state::from_layout_node(root),
                panes,
            });
        }
        if sessions.is_empty() {
            return None;
        }
        Some(state::SavedState { version: state::STATE_VERSION, active: self.active, sessions })
    }

    /// Serialize the current sessions/panes into a resume file, recording each
    /// pane's raw PTY master descriptor + child pid so a restarted daemon can
    /// adopt the live terminals (`kumo update`).
    #[cfg(unix)]
    fn to_resume_state(&self) -> Option<SavedState> {
        if self.sessions.is_empty() {
            return None;
        }
        let mut sessions = Vec::new();
        for session in &self.sessions {
            let root = session.tree.root.as_ref()?;
            let mut panes = Vec::new();
            for pid in session.tree.pane_ids() {
                let Some(pane) = self.panes.get(&pid) else { continue };
                panes.push(state::SavedPane {
                    id: pid,
                    is_ai: pane.is_ai,
                    shell: pane.pty.shell.clone(),
                    program: pane.program.clone(),
                    cwd: pane.cwd.clone(),
                    custom_name: pane.custom_name.clone(),
                    master_fd: pane.pty.raw_fd().map(|fd| fd as i64),
                    child_pid: pane.pty.process_id().map(|p| p as i64),
                    cols: pane.pty.cols,
                    rows: pane.pty.rows,
                });
            }
            sessions.push(state::SavedSession {
                name: session.name.clone(),
                workspace: session.workspace.clone(),
                zoom: session.zoom,
                focus: session.tree.focus,
                tree: state::from_layout_node(root),
                panes,
            });
        }
        if sessions.is_empty() {
            return None;
        }
        Some(state::SavedState { version: state::STATE_VERSION, active: self.active, sessions })
    }

    /// Persist (or clear) the state file once the loop exits. Dormant until
    /// 0.5.0 persistence revives it on the daemon side.
    #[cfg_attr(unix, allow(dead_code))]
    fn on_exit(&mut self) {
        let path = crate::config::state_file();
        if self.detach_requested {
            match self.to_saved_state() {
                Some(state) => {
                    if let Err(e) = state::save(&path, &state) {
                        log::warn!("kumo: failed to save state: {e:#}");
                    }
                }
                None => {
                    // Detached with every session closed: a resume would be
                    // pointless, so clear any previous state.
                    let _ = std::fs::remove_file(&path);
                }
            }
        } else if self.sessions.is_empty() {
            // Exited by closing every session: don't let a stale state resume.
            let _ = std::fs::remove_file(&path);
        }
    }

    // ----- lifecycle -----

    /// Create a session (used for the initial session at startup).
    fn new_session(&mut self) -> Result<()> {
        self.new_session_with_name(self.default_session_name())
    }

    /// Create a fresh session in `workspace` and focus it. The workspace is the
    /// `kumo new [WORKSPACE]` dir, or (against a running daemon) the client's
    /// cwd sent over the wire.
    fn new_session_in(&mut self, workspace: PathBuf) -> Result<()> {
        self.workspace = workspace;
        self.new_session()
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
        self.agent_status_cache.remove(&pid);
        self.last_agent_status.remove(&pid);
        self.last_agent_sound.remove(&pid);

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

    /// Close the session at `idx` and all of its panes.
    fn close_session(&mut self, idx: usize) {
        if self.sessions.get(idx).is_none() {
            return;
        }
        for pid in self.sessions[idx].tree.pane_ids() {
            if let Some(mut pane) = self.panes.remove(&pid) {
                pane.pty.kill();
            }
            self.last_sizes.remove(&pid);
            self.pane_cache.remove(&pid);
            self.agent_status_cache.remove(&pid);
            self.last_agent_status.remove(&pid);
            self.last_agent_sound.remove(&pid);
        }
        self.sessions.remove(idx);
        if self.sessions.is_empty() {
            self.quit = true;
            return;
        }
        if idx <= self.active {
            self.active = self.active.saturating_sub(1);
        }
        self.active = self.active.min(self.sessions.len() - 1);
    }

    /// Remove panes whose child process has exited, collapsing the layout.
    fn poll_exits(&mut self) {
        let mut exited: Vec<u64> = Vec::new();
        for (pid, pane) in self.panes.iter_mut() {
            if !pane.dead && matches!(pane.pty.try_wait(), Ok(Some(_))) {
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
        self.agent_status_cache.remove(&pid);
        self.last_agent_status.remove(&pid);
        self.last_agent_sound.remove(&pid);
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
            self.on_ctx_menu_key(key)?;
            return Ok(());
        }
        if self.keybind_overlay.open {
            self.on_overlay_key(key);
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
            KeyCode::Char('d') => {
                // detach: save the session state and exit (light restore for
                // now; 0.4.0's daemon turns this into a real client detach).
                self.detach_requested = true;
                self.quit = true;
            }
            KeyCode::Char('n') => self.cycle_session(1),
            KeyCode::Char('p') => self.cycle_session(-1),
            KeyCode::Char('?') => self.open_keybind_overlay(),
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

    /// Lines of the update banner: the headline (next to the close button) and
    /// the action hint. Kept on two lines so the banner stays narrow.
    fn update_notice_lines(&self) -> Option<(String, String)> {
        let notice = self.update_notice.as_ref()?;
        Some((
            format!("New version {} available", notice.display),
            "run 'kumo update'".to_string(),
        ))
    }

    /// Rect of the update banner, anchored to the top-right corner.
    pub(super) fn update_notice_rect(&self) -> Option<Rect> {
        let (line1, line2) = self.update_notice_lines()?;
        let (w, h) = self.term_size;
        let inner_w = line1.chars().count().max(line2.chars().count()) as u16 + 6;
        let width = inner_w + 2;
        if w < width + 1 || h < 4 {
            return None;
        }
        Some(Rect::new(w - width - 1, 0, width, 4))
    }

    /// Whether `(x, y)` hits the banner's close button.
    pub(super) fn update_notice_close_at(&self, x: u16, y: u16) -> bool {
        let Some(r) = self.update_notice_rect() else { return false };
        x == r.x + 2 && y == r.y + 1
    }
}
