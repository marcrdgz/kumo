use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use anyhow::Result;
use ratatui::buffer::Buffer;

use kumo_core::layout::{LayoutTree, SplitDir};
use kumo_core::theme::{Theme, THEMES};
use kumo_core::Launch;
use crate::daemon::agents::AgentStatus;
use crate::daemon::pane::{Pane, PtyEvent};
use crate::daemon::pty::Pty;
use crate::daemon::state::{self, SavedState};

use self::tasks::BranchInfo;

mod commands;
#[cfg(unix)]
pub(super) mod server;
mod tasks;
pub(crate) mod ui;
pub(crate) mod proc;

// The daemon is the only engine: it owns PTYs, the semantic layout tree, and
// per-pane terminal content, and is driven entirely by commands. Every client
// (TUI, desktop, mobile) draws its own chrome.

/// Default pane size until a client requests a real size via `PaneResize`.
const DEFAULT_PANE_DIMS: (u16, u16) = (80, 24);

struct Tab {
    id: u64,
    name: String,
    tree: LayoutTree,
    zoom: bool,
}

struct Session {
    id: u64,
    name: String,
    tabs: Vec<Tab>,
    active_tab: usize,
    workspace: PathBuf,
}

impl Session {
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }
    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }
    fn tab_index_by_spec(&self, spec: &str) -> Option<usize> {
        // by id (numeric), by name, or by 1-based index
        if let Ok(id) = spec.parse::<u64>() {
            if let Some(idx) = self.tabs.iter().position(|t| t.id == id) {
                return Some(idx);
            }
            // fall through to name / index
        }
        if let Some(idx) = self.tabs.iter().position(|t| t.name == spec) {
            return Some(idx);
        }
        if let Ok(n) = spec.parse::<usize>() {
            if n >= 1 && n <= self.tabs.len() {
                return Some(n - 1);
            }
        }
        None
    }
    fn pane_count(&self) -> usize {
        self.tabs.iter().map(|t| t.tree.pane_count()).sum()
    }
    fn contains_pane(&self, pid: u64) -> bool {
        self.tabs.iter().any(|t| t.tree.contains(pid))
    }
    fn find_tab_containing(&self, pid: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.tree.contains(pid))
    }
}

#[allow(dead_code)]
pub struct App {
    sessions: Vec<Session>,
    active: usize,
    panes: HashMap<u64, Pane>,
    events_tx: mpsc::Sender<PtyEvent>,
    events_rx: mpsc::Receiver<PtyEvent>,
    shell: String,
    ai: (String, Vec<String>),
    workspace: PathBuf,
    /// Cached git branch (name + ahead/behind) per workspace, refreshed periodically.
    branch_cache: HashMap<PathBuf, (Option<BranchInfo>, Instant)>,
    /// When the pane process tree was last scanned for an AI CLI.
    last_ai_scan: Instant,
    /// When the follow-workspace scan last ran (only meaningful in Follow mode).
    last_follow_scan: Instant,
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
    /// Cached (cpu%, rss KiB) per AI pane, sampled during status refresh so
    /// the sidebar's micro-pill metrics render live values.
    agent_proc_cache: HashMap<u64, (f32, u64)>,
    /// Delta state for per-process CPU sampling.
    proc: proc::ProcSampler,
    /// When the last audible agent alert sounded per pane (cooldown, so a
    /// flickering status does not repeat the beep).
    last_agent_sound: HashMap<u64, Instant>,
    /// Rendered cells of each pane's viewport, blitted back when the pane is
    /// unchanged so the frame loop never re-iterates unchanged terminals.
    pane_cache: HashMap<u64, Buffer>,
    /// Client-requested terminal size per pane (`PaneResize`); the daemon
    /// resizes each pane's PTY + emulator to this. Default 80x24.
    pane_sizes: HashMap<u64, (u16, u16)>,
    quit: bool,
    /// Active theme + its index in `THEMES`; switching applies it to all panes.
    theme: Theme,
    theme_idx: usize,
    /// Startup update banner (top-right), when a newer release exists.
    update_notice: Option<kumo_core::update::UpdateNotice>,
    /// Receives the background update check result.
    update_rx: mpsc::Receiver<Option<kumo_core::update::UpdateNotice>>,
    /// Receives background git branch results (workspace -> branch info).
    branch_rx: mpsc::Receiver<(PathBuf, Option<BranchInfo>)>,
    /// Sender for background git branch jobs.
    branch_tx: mpsc::Sender<(PathBuf, Option<BranchInfo>)>,
    /// Receives background AI CLI scan results (snapshot + pane IDs with their process IDs).
    ai_rx: mpsc::Receiver<(Option<crate::daemon::pane::ProcessSnapshot>, Vec<(u64, Option<u32>)>)>,
    /// Sender for background AI CLI scan jobs.
    ai_tx: mpsc::Sender<(Option<crate::daemon::pane::ProcessSnapshot>, Vec<(u64, Option<u32>)>)>,
    /// Workspaces with pending git branch lookups.
    pending_branch_lookups: HashMap<PathBuf, Instant>,
    /// Whether an AI CLI scan is currently in progress.
    ai_scan_in_progress: bool,
    /// Monotonic version bumped on every layout change (sessions, tree, focus,
    /// branch, workspace). Used to avoid deep `Layout` comparisons and clones.
    layout_version: u64,
    /// Cached `Arc<Layout>` for the current `layout_version`.
    cached_layout: Option<std::sync::Arc<kumo_protocol::Layout>>,
    cached_layout_version: u64,
}

/// Foreground TUI loop, used only on non-unix (fallback until daemon parity
/// lands); on unix the daemon drives `App` directly and the thin client renders.
#[cfg_attr(unix, allow(dead_code))]
impl App {
    fn new(launch: Launch) -> Result<App> {
        let shell = kumo_core::config::default_shell();
        let (ai_prog, ai_args) = kumo_core::config::ai_command();
        let ai_prog = kumo_core::config::resolve_program(&ai_prog);
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
        let (branch_tx, branch_rx) = mpsc::channel::<(PathBuf, Option<BranchInfo>)>();
        let (ai_tx, ai_rx) = mpsc::channel::<(Option<crate::daemon::pane::ProcessSnapshot>, Vec<(u64, Option<u32>)>)>();
        let _ = std::thread::Builder::new()
            .name("kumo-update-check".into())
            .spawn(move || {
                let notice = kumo_core::update::poll_update_notice();
                let _ = update_tx.send(notice);
            });
        let mut app = App {
            sessions: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            events_tx,
            events_rx,
            shell,
            ai: (ai_prog, ai_args),
            workspace,
            branch_cache: HashMap::new(),
            last_ai_scan: Instant::now(),
            last_follow_scan: Instant::now(),
            last_agent_debug: Instant::now(),
            last_status_refresh: Instant::now(),
            agent_status_cache: HashMap::new(),
            last_agent_status: HashMap::new(),
            agent_proc_cache: HashMap::new(),
            proc: proc::ProcSampler::default(),
            last_agent_sound: HashMap::new(),
            pane_cache: HashMap::new(),
            pane_sizes: HashMap::new(),
            quit: false,
            theme: THEMES[kumo_core::theme::DEFAULT_THEME_IDX],
            theme_idx: kumo_core::theme::DEFAULT_THEME_IDX,
            update_notice: None,
            update_rx,
            branch_rx,
            branch_tx,
            pending_branch_lookups: HashMap::new(),
            ai_rx,
            ai_tx,
            ai_scan_in_progress: false,
            layout_version: 0,
            cached_layout: None,
            cached_layout_version: 0,
        };

        match launch {
            Launch::Auto | Launch::Attach => {
                if let Some(state) = state::load(&kumo_core::config::state_file())? {
                    app.restore(state)?;
                } else if matches!(launch, Launch::Attach) {
                    anyhow::bail!("no saved state to attach to (start with `kumo` or `kumo new`)");
                } else {
                    app.new_session()?;
                }
            }
            Launch::New(Some(p)) if p.is_dir() => app.new_session_in(p)?,
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
            for tab in &session.tabs {
                let mut ids = Vec::new();
                state::tree_pane_ids(&tab.tree, &mut ids);
                for old in ids {
                    map.entry(old).or_insert_with(Pty::next_pane_id);
                }
            }
        }
        state::remap_pane_ids(&mut state, &map);

        self.panes.clear();
        self.sessions.clear();
        let (cols, rows) = DEFAULT_PANE_DIMS;
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
                    &self.theme,
                )?;
                pane.custom_name = sp.custom_name;
                self.panes.insert(sp.id, pane);
            }
            let mut tabs = Vec::new();
            for saved_tab in saved.tabs {
                let mut tree = LayoutTree::from_node(state::to_layout_node(&saved_tab.tree), saved_tab.focus);
                if !tree.contains(tree.focus) {
                    if let Some(&first) = tree.pane_ids().first() {
                        tree.focus = first;
                    }
                }
                tabs.push(Tab { id: saved_tab.id, name: saved_tab.name, tree, zoom: saved_tab.zoom });
            }
            // If the state had no tabs (should not happen after v2), create a default.
            if tabs.is_empty() {
                if let Some(pid) = self.panes.keys().next().copied() {
                    tabs.push(Tab { id: 1, name: "1".to_string(), tree: LayoutTree::new(pid), zoom: false });
                }
            }
            let active_tab = saved.active_tab.min(tabs.len().saturating_sub(1));
            self.sessions.push(Session { id: sid, name: saved.name, tabs, active_tab, workspace: saved.workspace });
            self.active = i;
        }
        if self.sessions.is_empty() {
            // Saved state without any surviving pane (or empty): fall back to a
            // fresh session rather than rendering a broken tree.
            self.new_session()?;
        } else {
            self.active = saved_active.min(self.sessions.len() - 1);
        }
        self.bump_layout_version();
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
            for tab in &session.tabs {
                let mut ids = Vec::new();
                state::tree_pane_ids(&tab.tree, &mut ids);
                for old in ids {
                    map.entry(old).or_insert_with(Pty::next_pane_id);
                }
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
                    sp.mouse_tracking,
                    sp.snapshot,
                    self.events_tx.clone(),
                    &self.theme,
                )?;
                pane.custom_name = sp.custom_name;
                self.panes.insert(sp.id, pane);
            }
            let mut tabs = Vec::new();
            for saved_tab in saved.tabs {
                let mut tree = LayoutTree::from_node(state::to_layout_node(&saved_tab.tree), saved_tab.focus);
                for pid in &missing { tree.remove_pane(*pid); }
                if !tree.contains(tree.focus) {
                    if let Some(&first) = tree.pane_ids().first() {
                        tree.focus = first;
                    }
                }
                if tree.pane_count() == 0 {
                    continue;
                }
                tabs.push(Tab { id: saved_tab.id, name: saved_tab.name, tree, zoom: saved_tab.zoom });
            }
            if tabs.is_empty() && !self.panes.is_empty() {
                // Should not happen, but keep daemon alive
                continue;
            }
            if tabs.is_empty() {
                continue;
            }
            let active_tab = saved.active_tab.min(tabs.len().saturating_sub(1));
            self.sessions.push(Session { id: sid, name: saved.name, tabs, active_tab, workspace: saved.workspace });
            self.active = i;
        }
        if self.sessions.is_empty() {
            return Ok(false);
        }
        self.active = saved_active.min(self.sessions.len() - 1);
        self.workspace = self.sessions[self.active].workspace.clone();
        self.bump_layout_version();
        Ok(true)
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
            let mut panes = Vec::new();
            let mut pane_ids = Vec::new();
            for tab in &session.tabs {
                pane_ids.extend(tab.tree.pane_ids());
            }
            for pid in pane_ids {
                let Some(pane) = self.panes.get(&pid) else { continue };
                let snapshot = pane.vt.snapshot_encode();
                if snapshot.is_none() {
                    log::warn!("kumo: snapshot encode failed for pane {pid}");
                }
                // dedup if a pane id somehow appears in multiple tabs (should not happen)
                if panes.iter().any(|p: &state::SavedPane| p.id == pid) {
                    continue;
                }
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
                    mouse_tracking: pane.has_mouse_reporting(),
                    snapshot,
                });
            }
            let tabs = session.tabs.iter().filter_map(|tab| {
                let root = tab.tree.root.as_ref()?;
                Some(state::SavedTab {
                    id: tab.id,
                    name: tab.name.clone(),
                    zoom: tab.zoom,
                    focus: tab.tree.focus,
                    tree: state::from_layout_node(root),
                })
            }).collect::<Vec<_>>();
            if tabs.is_empty() {
                continue;
            }
            sessions.push(state::SavedSession {
                name: session.name.clone(),
                workspace: session.workspace.clone(),
                active_tab: session.active_tab,
                tabs,
                panes,
            });
        }
        if sessions.is_empty() {
            return None;
        }
        Some(state::SavedState { version: state::STATE_VERSION, active: self.active, sessions })
    }

    // ----- lifecycle -----

    /// Create a session (used for the initial session at startup).
    fn new_session(&mut self) -> Result<()> {
        self.new_session_with_name(self.default_session_name())
    }

    /// Create a fresh session in `workspace` and focus it. The workspace is the
    /// `kumo new [WORKSPACE]` dir, or (against a running daemon) the client's
    /// cwd sent over the wire; an explicit dir always wins over the
    /// `[terminal] new-cwd` policy.
    fn new_session_in(&mut self, workspace: PathBuf) -> Result<()> {
        self.workspace = self.resolve_workspace(Some(&workspace));
        self.new_session()
    }

    /// Resolve where a session's panes should open, applying the `[terminal]
    /// new-cwd` policy. An explicit directory (CLI arg / client cwd) always
    /// wins; otherwise `Follow`/`Current` use the launch directory, `Home`
    /// uses `$HOME`, and `Fixed(path)` uses the configured path.
    fn resolve_workspace(&self, explicit: Option<&PathBuf>) -> PathBuf {
        if let Some(p) = explicit {
            if p.is_dir() {
                return p.clone();
            }
        }
        match kumo_core::config::new_cwd() {
            kumo_core::config::NewCwd::Follow | kumo_core::config::NewCwd::Current => self.workspace.clone(),
            kumo_core::config::NewCwd::Home => std::env::var("HOME")
                .ok()
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| self.workspace.clone()),
            kumo_core::config::NewCwd::Fixed(p) if p.is_dir() => p,
            _ => self.workspace.clone(),
        }
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
        let workspace = self.resolve_workspace(None);
        let (cols, rows) = DEFAULT_PANE_DIMS;
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
            &self.theme,
        )?;
        self.panes.insert(pid, pane);
        let tab = Tab { id: self.next_tab_id(), name: "1".to_string(), tree: LayoutTree::new(pid), zoom: false };
        self.sessions.push(Session { id: sid, name, tabs: vec![tab], active_tab: 0, workspace });
        self.active = self.sessions.len() - 1;
        self.bump_layout_version();
        Ok(())
    }

    /// Create a session with an explicit name and workspace, and focus it. The
    /// workspace is used verbatim (no `new-cwd` policy): this is the shared
    /// tail for worktree sessions, whose path is chosen by git, not kumo.
    fn new_session_in_workspace(&mut self, name: String, workspace: PathBuf) -> Result<()> {
        let name = self.unique_session_name(&name);
        let sid = self.next_session_id();
        let pid = Pty::next_pane_id();
        let (cols, rows) = DEFAULT_PANE_DIMS;
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
            &self.theme,
        )?;
        self.panes.insert(pid, pane);
        let tab = Tab { id: self.next_tab_id(), name: "1".to_string(), tree: LayoutTree::new(pid), zoom: false };
        self.sessions.push(Session { id: sid, name, tabs: vec![tab], active_tab: 0, workspace });
        self.active = self.sessions.len() - 1;
        self.bump_layout_version();
        Ok(())
    }

    /// `base` unless a session already uses it, then `base-2`, `base-3`, …
    fn unique_session_name(&self, base: &str) -> String {
        if !self.sessions.iter().any(|s| s.name == base) {
            return base.to_string();
        }
        let mut n = 2;
        loop {
            let cand = format!("{base}-{n}");
            if !self.sessions.iter().any(|s| s.name == cand) {
                return cand;
            }
            n += 1;
        }
    }

    /// Create a git worktree from the repo of the session at `idx` and open a
    /// new session inside it. `branch` is a new branch created from the
    /// current HEAD; the worktree lands at git's default sibling path. Returns
    /// a displayable error (kept in the popup) when the workspace is not a git
    /// repository or git rejects the branch/path.
    fn new_worktree_session(&mut self, idx: usize, branch: &str) -> Result<(), String> {
        let Some(session) = self.sessions.get(idx) else {
            return Err("no such session".to_string());
        };
        let Some(root) = kumo_core::worktrees::repo_root(&session.workspace) else {
            return Err(format!("{} is not a git repository", session.workspace.display()));
        };
        let path = kumo_core::worktrees::worktree_path(&root, branch);
        kumo_core::worktrees::add_worktree(&root, &path, branch)?;
        self.new_session_in_workspace(branch.to_string(), path)
            .map_err(|e| format!("{e:#}"))
    }

    /// Open the session already working in `path` (matching the exact path or
    /// its canonicalized form), or create a new one named after `branch` (or
    /// the directory name) with that workspace. Used by the worktree picker.
    pub(super) fn open_session_in_worktree(
        &mut self,
        path: &std::path::Path,
        branch: Option<&str>,
    ) -> Result<()> {
        if let Some(i) = self.session_for_workspace(path) {
            self.active = i;
            self.bump_layout_version();
            return Ok(());
        }
        let name = branch.map(str::to_string).unwrap_or_else(|| {
            path.file_name()
                .map(|b| b.to_string_lossy().into_owned())
                .unwrap_or_else(|| "worktree".to_string())
        });
        self.new_session_in_workspace(name, path.to_path_buf())
    }

    /// Index of the session already using `path` as its workspace, if any.
    fn session_for_workspace(&self, path: &std::path::Path) -> Option<usize> {
        let canon = std::fs::canonicalize(path).ok();
        self.sessions.iter().position(|s| {
            if s.workspace == path {
                return true;
            }
            match (&canon, std::fs::canonicalize(&s.workspace).ok()) {
                (Some(a), Some(b)) => *a == b,
                _ => false,
            }
        })
    }

    /// Re-apply the config to live state (`kumo reload` / client MENU `reload`).
    /// `shell` and `ai-cmd` are cached at startup, so they refresh here;
    /// `new-cwd` and `agent-sound` are read live from the config on each use.
    /// Applies to panes spawned from now on — existing panes keep their PTY.
    pub(super) fn reload_config(&mut self) {
        kumo_core::config::invalidate_cache();
        let shell = kumo_core::config::default_shell();
        let (ai_prog, ai_args) = kumo_core::config::ai_command();
        let ai_prog = kumo_core::config::resolve_program(&ai_prog);
        self.shell = shell;
        self.ai = (ai_prog, ai_args);
    }

    fn next_session_id(&mut self) -> u64 {
        let max = self.sessions.iter().map(|s| s.id).max().unwrap_or(0);
        max + 1
    }

    fn next_tab_id(&self) -> u64 {
        self.sessions.iter().flat_map(|s| s.tabs.iter().map(|t| t.id)).max().unwrap_or(0) + 1
    }

    fn unique_tab_name(&self, session_idx: usize, base: &str) -> String {
        let session = &self.sessions[session_idx];
        if !session.tabs.iter().any(|t| t.name == base) {
            return base.to_string();
        }
        let mut n = 2;
        loop {
            let cand = format!("{base}-{n}");
            if !session.tabs.iter().any(|t| t.name == cand) {
                return cand;
            }
            n += 1;
        }
    }

    fn default_tab_name(&self, session_idx: usize) -> String {
        // smallest free numeric name
        let mut n = 1;
        loop {
            let cand = n.to_string();
            if !self.sessions[session_idx].tabs.iter().any(|t| t.name == cand) {
                return cand;
            }
            n += 1;
        }
    }

    pub(crate) fn new_tab_in_session(&mut self, session_name: &str, name: Option<&str>, workspace: Option<PathBuf>) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session_name) else {
            return Ok(format!("no session {session_name:?}"));
        };
        let ws = workspace.unwrap_or_else(|| self.sessions[idx].workspace.clone());
        let tab_name = name.map(|n| self.unique_tab_name(idx, n)).unwrap_or_else(|| self.default_tab_name(idx));
        let sid = self.sessions[idx].id;
        let pid = Pty::next_pane_id();
        let (cols, rows) = DEFAULT_PANE_DIMS;
        let pane = Pane::spawn(sid, pid, self.shell.clone(), None, Some(ws), cols, rows, false, self.events_tx.clone(), &self.theme)?;
        self.panes.insert(pid, pane);
        let tab_id = self.next_tab_id();
        let tab = Tab { id: tab_id, name: tab_name.clone(), tree: LayoutTree::new(pid), zoom: false };
        self.sessions[idx].tabs.push(tab);
        self.sessions[idx].active_tab = self.sessions[idx].tabs.len() - 1;
        self.bump_layout_version();
        Ok(format!("created tab {tab_name:?} in {session_name:?}"))
    }

    pub(crate) fn close_tab_in_session(&mut self, session_name: &str, tab_spec: Option<&str>) -> Result<String> {
        let Some(s_idx) = self.sessions.iter().position(|s| s.name == session_name) else {
            return Ok(format!("no session {session_name:?}"));
        };
        let t_idx = match tab_spec {
            Some(spec) => match self.sessions[s_idx].tab_index_by_spec(spec) {
                Some(i) => i,
                None => return Ok(format!("no tab {spec:?} in {session_name:?}")),
            },
            None => self.sessions[s_idx].active_tab,
        };
        if self.sessions[s_idx].tabs.len() <= 1 {
            // last tab: close the whole session (mirrors pane close that empties session)
            self.close_session(s_idx);
            return Ok(format!("closed tab and session {session_name:?}"));
        }
        let tab = self.sessions[s_idx].tabs.remove(t_idx);
        for pid in tab.tree.pane_ids() {
            let os_pid = self.panes.get(&pid).and_then(|p| p.pty.process_id());
            if let Some(mut pane) = self.panes.remove(&pid) { pane.pty.kill(); }
            self.pane_cache.remove(&pid);
            self.pane_sizes.remove(&pid);
            self.agent_status_cache.remove(&pid);
            self.last_agent_status.remove(&pid);
            self.last_agent_sound.remove(&pid);
            self.agent_proc_cache.remove(&pid);
            if let Some(os_pid) = os_pid { self.proc.forget(os_pid); }
        }
        // fix active_tab
        if t_idx < self.sessions[s_idx].active_tab {
            self.sessions[s_idx].active_tab -= 1;
        } else if t_idx == self.sessions[s_idx].active_tab {
            self.sessions[s_idx].active_tab = self.sessions[s_idx].active_tab.min(self.sessions[s_idx].tabs.len() - 1);
        }
        self.bump_layout_version();
        Ok(format!("closed tab {:?} in {:?}", tab.name, session_name))
    }

    pub(crate) fn focus_tab_in_session_named(&mut self, session_name: &str, tab_spec: &str) -> bool {
        let Some(s_idx) = self.sessions.iter().position(|s| s.name == session_name) else { return false };
        if let Some(t_idx) = self.sessions[s_idx].tab_index_by_spec(tab_spec) {
            self.sessions[s_idx].active_tab = t_idx;
            self.bump_layout_version();
            return true;
        }
        false
    }

    pub(crate) fn rename_tab_in_session(&mut self, session: &str, tab: &str, new_name: &str) -> Result<String> {
        let Some(s_idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let Some(t_idx) = self.sessions[s_idx].tab_index_by_spec(tab) else {
            return Ok(format!("no tab {tab:?} in {session:?}"));
        };
        let name = new_name.trim().to_string();
        if name.is_empty() { return Ok("name cannot be empty".to_string()); }
        if self.sessions[s_idx].tabs.iter().enumerate().any(|(i,t)| i!=t_idx && t.name==name) {
            return Ok(format!("a tab named '{name}' already exists"));
        }
        self.sessions[s_idx].tabs[t_idx].name = name.clone();
        self.bump_layout_version();
        Ok(format!("renamed tab to {name:?}"))
    }

    pub(crate) fn cycle_tab_in_session(&mut self, session: &str, delta: isize) -> Result<String> {
        let Some(s_idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let len = self.sessions[s_idx].tabs.len();
        if len < 2 { return Ok(format!("only one tab in {session:?}")); }
        let cur = self.sessions[s_idx].active_tab as isize;
        let next = ((cur + delta).rem_euclid(len as isize)) as usize;
        self.sessions[s_idx].active_tab = next;
        self.bump_layout_version();
        Ok(format!("focused tab {}", self.sessions[s_idx].tabs[next].name))
    }

    pub(crate) fn jump_tab_in_session(&mut self, session: &str, n: usize) -> Result<String> {
        let Some(s_idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        if n == 0 || n > self.sessions[s_idx].tabs.len() {
            return Ok(format!("no tab {n} in {session:?}"));
        }
        self.sessions[s_idx].active_tab = n - 1;
        self.bump_layout_version();
        Ok(format!("focused tab {}", self.sessions[s_idx].tabs[n-1].name))
    }

    fn bump_layout_version(&mut self) {
        self.layout_version = self.layout_version.wrapping_add(1);
        self.cached_layout = None;
    }

    fn split_active(&mut self, dir: SplitDir, is_ai: bool) -> Result<()> {
        let (focus, sid) = {
            let sess = &self.sessions[self.active];
            (sess.active_tab().tree.focus, sess.id)
        };
        let pid = Pty::next_pane_id();
        let (cols, rows) = self.pane_sizes.get(&focus).copied().unwrap_or(DEFAULT_PANE_DIMS);
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
            &self.theme,
        )?;
        self.panes.insert(pid, pane);
        let active_tab = self.sessions[self.active].active_tab;
        if !self.sessions[self.active].tabs[active_tab].tree.split(focus, pid, dir) {
            if let Some(mut p) = self.panes.remove(&pid) {
                p.pty.kill();
            }
        } else {
            self.bump_layout_version();
        }
        Ok(())
    }

    fn close_pane(&mut self, pid: u64) {
        let os_pid = self.panes.get(&pid).and_then(|p| p.pty.process_id());
        if let Some(mut pane) = self.panes.remove(&pid) {
            pane.pty.kill();
        }
        self.pane_cache.remove(&pid);
        self.pane_sizes.remove(&pid);
        self.agent_status_cache.remove(&pid);
        self.last_agent_status.remove(&pid);
        self.last_agent_sound.remove(&pid);
        self.agent_proc_cache.remove(&pid);
        if let Some(os_pid) = os_pid {
            self.proc.forget(os_pid);
        }

        let s_idx = self.active;
        let t_idx = self.sessions[s_idx].find_tab_containing(pid).unwrap_or(self.sessions[s_idx].active_tab);
        let empty_tab = self.sessions[s_idx].tabs[t_idx].tree.remove_pane(pid);
        if empty_tab {
            self.sessions[s_idx].tabs.remove(t_idx);
            if self.sessions[s_idx].tabs.is_empty() {
                self.sessions.remove(s_idx);
                if self.sessions.is_empty() {
                    self.quit = true;
                    self.bump_layout_version();
                    return;
                }
                self.active = self.active.min(self.sessions.len() - 1);
                self.bump_layout_version();
                return;
            }
            // adjust active_tab
            if t_idx < self.sessions[s_idx].active_tab {
                self.sessions[s_idx].active_tab -= 1;
            } else if t_idx == self.sessions[s_idx].active_tab {
                self.sessions[s_idx].active_tab = self.sessions[s_idx].active_tab.min(self.sessions[s_idx].tabs.len() - 1);
            } else if self.sessions[s_idx].active_tab >= self.sessions[s_idx].tabs.len() {
                self.sessions[s_idx].active_tab = self.sessions[s_idx].tabs.len() - 1;
            }
        }
        self.bump_layout_version();
    }

    /// Close the session at `idx` and all of its panes.
    fn close_session(&mut self, idx: usize) {
        if self.sessions.get(idx).is_none() {
            return;
        }
        for tab in &self.sessions[idx].tabs {
            for pid in tab.tree.pane_ids() {
                let os_pid = self.panes.get(&pid).and_then(|p| p.pty.process_id());
                if let Some(mut pane) = self.panes.remove(&pid) {
                    pane.pty.kill();
                }
                self.pane_cache.remove(&pid);
                self.pane_sizes.remove(&pid);
                self.agent_status_cache.remove(&pid);
                self.last_agent_status.remove(&pid);
                self.last_agent_sound.remove(&pid);
                self.agent_proc_cache.remove(&pid);
                if let Some(os_pid) = os_pid {
                    self.proc.forget(os_pid);
                }
            }
        }
        self.sessions.remove(idx);
        if self.sessions.is_empty() {
            self.quit = true;
            self.bump_layout_version();
            return;
        }
        if idx <= self.active {
            self.active = self.active.saturating_sub(1);
        }
        self.active = self.active.min(self.sessions.len() - 1);
        self.bump_layout_version();
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
                if s.contains_pane(pid) {
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
        let os_pid = self.panes.get(&pid).and_then(|p| p.pty.process_id());
        if let Some(mut pane) = self.panes.remove(&pid) {
            pane.pty.kill();
        }
        self.pane_cache.remove(&pid);
        self.pane_sizes.remove(&pid);
        self.agent_status_cache.remove(&pid);
        self.last_agent_status.remove(&pid);
        self.last_agent_sound.remove(&pid);
        self.agent_proc_cache.remove(&pid);
        if let Some(os_pid) = os_pid {
            self.proc.forget(os_pid);
        }
        let t_idx_opt = self.sessions[idx].find_tab_containing(pid);
        let Some(t_idx) = t_idx_opt else {
            self.bump_layout_version();
            return;
        };
        let empty_tab = self.sessions[idx].tabs[t_idx].tree.remove_pane(pid);
        if empty_tab {
            self.sessions[idx].tabs.remove(t_idx);
            if self.sessions[idx].tabs.is_empty() {
                self.sessions.remove(idx);
                if self.sessions.is_empty() {
                    self.quit = true;
                    self.bump_layout_version();
                    return;
                }
                self.active = self.active.saturating_sub(1).min(self.sessions.len() - 1);
                self.bump_layout_version();
                return;
            }
            if t_idx < self.sessions[idx].active_tab {
                self.sessions[idx].active_tab -= 1;
            } else if t_idx == self.sessions[idx].active_tab {
                self.sessions[idx].active_tab = self.sessions[idx].active_tab.min(self.sessions[idx].tabs.len() - 1);
            }
        }
        self.bump_layout_version();
    }

    // ----- events -----

    fn on_pty_event(&mut self, ev: PtyEvent) {
        let PtyEvent::Output { pane_id, data } = ev;
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.feed(&data);
            // Check if the terminal bell was rung and trigger an alert
            if pane.vt.take_bell_count() > 0 {
                crate::daemon::alert::play(crate::daemon::alert::AlertKind::Finished);
            }
            // Check if there's a clipboard write request (OSC 52)
            if let Some(text) = pane.vt.take_clipboard_text() {
                // Copy to system clipboard using pbcopy on macOS, xclip/xsel on Linux
                #[cfg(target_os = "macos")]
                {
                    use std::io::Write;
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
                #[cfg(target_os = "linux")]
                {
                    use std::io::Write;
                    use std::process::{Command, Stdio};
                    for cmd in ["xclip", "xsel"] {
                        if let Ok(mut child) = Command::new(cmd)
                            .args(["-selection", "clipboard"])
                            .stdin(Stdio::piped())
                            .spawn()
                        {
                            if let Some(mut stdin) = child.stdin.take() {
                                let _ = stdin.write_all(text.as_bytes());
                            }
                            let _ = child.wait();
                            break;
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn focus_session_named(&mut self, name: &str) -> bool {
        if let Some(i) = self.sessions.iter().position(|s| s.name == name) {
            self.active = i;
            self.bump_layout_version();
            true
        } else {
            false
        }
    }

    /// Focus a specific pane inside a named session (desktop pane click). Also
    /// activates the session and its tab. Returns whether the pane exists in it.
    pub(crate) fn focus_pane_in_session(&mut self, name: &str, pane_id: u64) -> bool {
        let Some(i) = self.sessions.iter().position(|s| s.name == name) else {
            return false;
        };
        let Some(t_idx) = self.sessions[i].find_tab_containing(pane_id) else {
            return false;
        };
        self.active = i;
        self.sessions[i].active_tab = t_idx;
        self.sessions[i].tabs[t_idx].tree.focus = pane_id;
        self.bump_layout_version();
        true
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Restore env vars on drop so tests never leak mutations.
    struct EnvGuard(Vec<(&'static str, Option<String>)>);
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            EnvGuard(vec![(key, prev)])
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kumo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_workspace_applies_new_cwd_policy() {
        let _lock = kumo_core::config::TEST_ENV_LOCK.lock().unwrap();
        let cfg = scratch("ws-cfg");
        let home = scratch("ws-home");
        let work = scratch("ws-work");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("KUMO_NO_UPDATE", "1"),
        );
        std::fs::write(cfg.join("config"), "shell = /bin/sh\n").unwrap();

        let launch = std::env::current_dir().unwrap();
        let app = App::new(Launch::New(None)).unwrap();
        assert_eq!(app.resolve_workspace(None), launch, "follow/current defaults to the launch dir");
        assert_eq!(app.resolve_workspace(Some(&work)), work, "explicit dir always wins");
        drop(app);

        std::fs::write(cfg.join("config.toml"), "[terminal]\nnew-cwd = \"home\"\n").unwrap();
        let app = App::new(Launch::New(None)).unwrap();
        assert_eq!(app.resolve_workspace(None), home, "new-cwd = home resolves to $HOME");
        drop(app);

        std::fs::write(
            cfg.join("config.toml"),
            format!("[terminal]\nnew-cwd = \"fixed\"\nfixed-cwd = \"{}\"\n", work.display()),
        )
        .unwrap();
        let app = App::new(Launch::New(None)).unwrap();
        assert_eq!(app.resolve_workspace(None), work, "new-cwd = fixed resolves to fixed-cwd");
        drop(app);

        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn reload_config_refreshes_shell_and_ai() {
        let _lock = kumo_core::config::TEST_ENV_LOCK.lock().unwrap();
        let cfg = scratch("reload-cfg");
        let home = scratch("reload-home");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("KUMO_NO_UPDATE", "1"),
        );
        std::fs::write(cfg.join("config"), "shell = /bin/sh\n").unwrap();
        let mut app = App::new(Launch::New(None)).unwrap();
        assert_eq!(app.shell, "/bin/sh");
        assert_ne!(app.ai.0, "/usr/bin/true", "precondition: the ai-cmd is not already the test value");
        std::fs::write(
            cfg.join("config.toml"),
            "ai-cmd = \"/usr/bin/true\"\n[terminal]\nnew-cwd = \"home\"\n",
        )
        .unwrap();
        app.reload_config();
        assert_eq!(app.shell, "/bin/sh", "reload keeps the current shell");
        assert_eq!(app.ai.0, "/usr/bin/true", "reload picks up a new ai-cmd");
        drop(app);
        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Make a temp git repo on branch `main`. Returns the working tree path.
    fn temp_git_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kumo-wt-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).status().unwrap().success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "main", dir.to_str().unwrap()]);
        run(&["-C", dir.to_str().unwrap(), "config", "user.email", "t@t"]);
        run(&["-C", dir.to_str().unwrap(), "config", "user.name", "t"]);
        run(&["-C", dir.to_str().unwrap(), "commit", "-q", "--allow-empty", "-m", "x"]);
        dir
    }

    #[test]
    fn worktree_session_creates_and_reuses() {
        let _lock = kumo_core::config::TEST_ENV_LOCK.lock().unwrap();
        let cfg = scratch("wt-app-cfg");
        let home = scratch("wt-app-home");
        let _guards = (
            EnvGuard::set("KUMO_CONFIG_DIR", &cfg.to_string_lossy()),
            EnvGuard::set("HOME", &home.to_string_lossy()),
            EnvGuard::set("KUMO_NO_UPDATE", "1"),
        );
        std::fs::write(cfg.join("config"), "shell = /bin/sh\n").unwrap();
        let repo = temp_git_repo();
        let mut app = App::new(Launch::New(Some(repo.clone()))).unwrap();
        assert_eq!(app.sessions.len(), 1);

        // Creating a worktree opens a new session in it, named after the branch.
        app.new_worktree_session(0, "feat/test").unwrap();
        assert_eq!(app.sessions.len(), 2, "a worktree creates a new session");
        assert_eq!(app.active, 1, "the new worktree session is focused");
        assert_eq!(app.sessions[1].name, "feat/test");
        let wt_path = app.sessions[1].workspace.clone();
        assert!(wt_path.to_string_lossy().ends_with("feat/test"), "sibling path: {wt_path:?}");

        // Re-opening the same worktree reuses the existing session instead of
        // duplicating it, and refocuses it from any other session.
        app.open_session_in_worktree(&wt_path, Some("feat/test")).unwrap();
        assert_eq!(app.sessions.len(), 2, "reuse, not duplicate");
        assert_eq!(app.active, 1);

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&home);
    }
}
