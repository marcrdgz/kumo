//! Command handlers for the daemon.
//!
//! Every [`kumo_protocol::Command`] is executed here against the single source
//! of truth (`App`): sessions, the semantic layout tree, PTYs, and agent
//! metadata. The daemon never renders chrome — it mutates state, resizes
//! panes, and answers commands. This is the one place the CLI, the TUI, and
//! the desktop app all drive.

use anyhow::{Result, anyhow};
use std::path::PathBuf;

use crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};

use kumo_protocol::{
    AgentInfo, AgentStatusLine, SessionInfo, SplitDir, WireKeyEvent, WireNotice, WireWorktree,
};

use super::App;
use kumo_core::layout;
use crate::daemon::pane::Pane;
use crate::daemon::pty::Pty;

/// Fraction of the split width/height a `leader+H/J/K/L` resize nudges per press.
const RESIZE_STEP: f32 = 0.05;

/// The editor used by MENU `config`: `$VISUAL`, then `$EDITOR` (command
/// strings may carry args, e.g. `code --wait`), then `vi`.
fn config_editor() -> (String, Vec<String>) {
    let raw = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    let mut it = raw.split_whitespace();
    let program = it.next().unwrap_or("vi").to_string();
    let args: Vec<String> = it.map(|s| s.to_string()).collect();
    (program, args)
}

impl App {
    /// The focused pane of the active session.
    pub(crate) fn active_focus(&self) -> u64 {
        self.sessions.get(self.active).map(|s| s.tree.focus).unwrap_or(0)
    }

    // ------------------------------------------------------------------
    // Interactive input (attached viewers)
    // ------------------------------------------------------------------

    /// Write a key to the focused pane (passthrough; the daemon interprets no
    /// keys itself — the client owns the keymap and issues commands).
    pub(crate) fn write_key(&mut self, key: KeyEvent) {
        let focus = self.active_focus();
        if let Some(pane) = self.panes.get_mut(&focus) {
            let bytes = crate::daemon::keys::encode(key);
            if !bytes.is_empty() {
                pane.write(&bytes);
            }
        }
    }

    /// Mouse events from a dumb viewport are pane-relative: scroll wheels move
    /// the focused pane's viewport. Selection/drag is a client concern.
    pub(crate) fn on_pane_mouse(&mut self, m: MouseEvent) {
        let focus = self.active_focus();
        if let Some(pane) = self.panes.get_mut(&focus) {
            match m.kind {
                MouseEventKind::ScrollUp => pane.scroll(1),
                MouseEventKind::ScrollDown => pane.scroll(-1),
                _ => {}
            }
        }
    }

    /// Bracketed-paste text: write it to the focused pane with trailing
    /// newlines stripped and interior newlines translated to `\r`.
    pub(crate) fn paste(&mut self, text: &str) {
        let text = text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            return;
        }
        let bytes = text.replace('\n', "\r");
        let focus = self.active_focus();
        if let Some(pane) = self.panes.get_mut(&focus) {
            pane.write(bytes.as_bytes());
        }
    }

    // ------------------------------------------------------------------
    // Panes
    // ------------------------------------------------------------------

    /// Set a pane's terminal size (clients compute geometry from the semantic
    /// tree and request sizes; the daemon resizes the PTY + emulator).
    pub(crate) fn resize_pane(&mut self, pane_id: u64, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        self.pane_sizes.insert(pane_id, (cols, rows));
        self.pane_cache.remove(&pane_id);
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.resize(cols, rows);
            pane.dirty = true;
            pane.full_redraw = true;
        }
    }

    /// Split a pane in the named session (default: the focused pane).
    pub(crate) fn split_in_session(
        &mut self,
        session: &str,
        dir: SplitDir,
        is_ai: bool,
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let dir = match dir {
            SplitDir::Vertical => layout::SplitDir::V,
            SplitDir::Horizontal => layout::SplitDir::H,
        };
        let prev = self.active;
        self.active = idx;
        let result = self.split_active(dir, is_ai);
        self.active = prev;
        result?;
        Ok(format!("split {session:?}"))
    }

    /// Close a pane in the named session (default: the focused pane).
    pub(crate) fn close_pane_in_session(&mut self, session: &str, pane_id: Option<u64>) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let pid = match pane_id {
            Some(pid) => {
                if !self.sessions[idx].tree.contains(pid) {
                    return Ok(format!("no pane {pid} in {session:?}"));
                }
                pid
            }
            None => self.sessions[idx].tree.focus,
        };
        // Route through the active-session close logic by temporarily focusing
        // the target session; restore the previous active session unless the
        // close removed a session.
        let prev = self.active;
        let before = self.sessions.len();
        self.active = idx;
        self.close_pane(pid);
        if self.sessions.len() == before {
            self.active = prev;
        }
        Ok(format!("closed pane {pid}"))
    }

    /// Nudge the ratio of the split separating the focused pane from its
    /// neighbor in `dir`.
    pub(crate) fn resize_split_in_session(&mut self, session: &str, dir: kumo_protocol::ResizeDir) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let dir = match dir {
            kumo_protocol::ResizeDir::Left => kumo_core::layout::ResizeDir::Left,
            kumo_protocol::ResizeDir::Down => kumo_core::layout::ResizeDir::Down,
            kumo_protocol::ResizeDir::Up => kumo_core::layout::ResizeDir::Up,
            kumo_protocol::ResizeDir::Right => kumo_core::layout::ResizeDir::Right,
        };
        let focus = self.sessions[idx].tree.focus;
        if !self.sessions[idx].tree.resize_pane(focus, dir, RESIZE_STEP) {
            return Ok(format!("nothing to resize in that direction in {session:?}"));
        }
        self.bump_layout_version();
        Ok(format!("resized split in {session:?}"))
    }

    /// Set the ratio of a specific split (identified by the id shipped in the
    /// semantic layout) to an absolute value — the daemon-side half of a
    /// desktop drag on the divider.
    pub(crate) fn set_split_ratio_in_session(
        &mut self,
        session: &str,
        split_id: u64,
        ratio: f32,
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        self.sessions[idx].tree.set_ratio(split_id, ratio);
        self.bump_layout_version();
        Ok(format!("set split {split_id} ratio in {session:?}"))
    }

    /// Swap the focused pane with its sibling in the named session.
    pub(crate) fn swap_focused(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let focus = self.sessions[idx].tree.focus;
        if self.sessions[idx].tree.swap_with_sibling(focus) {
            self.bump_layout_version();
            Ok(format!("swapped pane in {session:?}"))
        } else {
            Ok(format!("no sibling to swap in {session:?}"))
        }
    }

    /// Mirror (rotate) the layout tree of the named session.
    pub(crate) fn rotate_layout(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        self.sessions[idx].tree.mirror();
        self.bump_layout_version();
        Ok(format!("rotated layout in {session:?}"))
    }

    /// Toggle the named session's zoom.
    pub(crate) fn zoom_session(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        self.sessions[idx].zoom = !self.sessions[idx].zoom;
        self.bump_layout_version();
        Ok(format!("toggled zoom in {session:?}"))
    }

    /// Send key events to a pane in the named session (default: focused).
    pub(crate) fn send_keys(
        &mut self,
        session: &str,
        pane_id: Option<u64>,
        keys: &[WireKeyEvent],
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let prev_active = self.active;
        let prev_focus = self.sessions[idx].tree.focus;
        self.active = idx;
        let pid = match pane_id {
            Some(pid) => {
                if !self.sessions[idx].tree.contains(pid) {
                    return Ok(format!("no pane {pid} in {session:?}"));
                }
                self.sessions[idx].tree.focus = pid;
                pid
            }
            None => self.sessions[idx].tree.focus,
        };
        if prev_active != idx || prev_focus != pid {
            self.bump_layout_version();
        }
        let mut sent = 0usize;
        if let Some(pane) = self.panes.get_mut(&pid) {
            for key in keys {
                let bytes = crate::daemon::keys::encode(key.to_crossterm());
                if !bytes.is_empty() {
                    pane.write(&bytes);
                    sent += 1;
                }
            }
        }
        Ok(format!("sent {sent} key(s) to pane {pid}"))
    }

    // ------------------------------------------------------------------
    // Sessions
    // ------------------------------------------------------------------

    /// Create a session (with an explicit name/workspace, or defaults) and
    /// focus it. Returns a human-readable outcome.
    pub(crate) fn new_session_command(
        &mut self,
        name: Option<&str>,
        workspace: Option<&PathBuf>,
    ) -> Result<String> {
        let name = name
            .map(|n| n.to_string())
            .unwrap_or_else(|| self.default_session_name());
        let name = self.unique_session_name(&name);
        match workspace {
            Some(ws) => self.new_session_in_workspace(name.clone(), ws.clone())?,
            None => self.new_session_with_name(name.clone())?,
        }
        Ok(format!("created session {name:?}"))
    }

    /// Close a session by name, killing its panes.
    pub(crate) fn kill_session_named(&mut self, name: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == name) else {
            return Ok(format!("no session {name:?}"));
        };
        self.close_session(idx);
        Ok(format!("killed session {name:?}"))
    }

    /// Metadata list for `kumo session list` (the full semantic tree travels
    /// via `DaemonEvent::Layout`).
    pub(crate) fn session_info_list(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .enumerate()
            .map(|(i, s)| SessionInfo {
                name: s.name.clone(),
                workspace: s.workspace.clone(),
                pane_count: s.tree.pane_count(),
                zoomed: s.zoom,
                active: i == self.active,
                focus: (s.tree.pane_count() > 0).then_some(s.tree.focus),
                agents: s
                    .tree
                    .pane_ids()
                    .into_iter()
                    .filter_map(|pid| {
                        let pane = self.panes.get(&pid)?;
                        if !pane.is_ai_cli() {
                            return None;
                        }
                        let (cpu, mem_kb) = self.agent_proc_cache.get(&pid).copied().unwrap_or((0.0, 0));
                        Some(AgentInfo {
                            name: self.agent_label(pid),
                            status: self
                                .agent_status_cache
                                .get(&pid)
                                .copied()
                                .unwrap_or(crate::daemon::agents::AgentStatus::Idle)
                                .into(),
                            cpu,
                            mem_kb,
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Agents
    // ------------------------------------------------------------------

    /// Spawn an AI CLI in a new pane of the named session (default program =
    /// the configured AI command).
    pub(crate) fn agent_spawn(&mut self, session: &str, program: Option<&str>) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let prev = self.active;
        self.active = idx;
        let saved = self.ai.clone();
        if let Some(prog) = program {
            self.ai = (prog.to_string(), Vec::new());
        }
        let result = self.split_active(layout::SplitDir::V, true);
        self.ai = saved;
        self.active = prev;
        result.map_err(|e| anyhow!("agent spawn failed: {e:#}"))?;
        Ok(format!("spawned agent in {session:?}"))
    }

    /// One status line per running AI CLI, for `kumo agent status`.
    pub(crate) fn agent_status_lines(&self) -> Vec<AgentStatusLine> {
        let mut out = Vec::new();
        for s in &self.sessions {
            for pid in s.tree.pane_ids() {
                let Some(pane) = self.panes.get(&pid) else { continue };
                if !pane.is_ai_cli() {
                    continue;
                }
                out.push(AgentStatusLine {
                    session: s.name.clone(),
                    pane_id: pid,
                    name: self.agent_label(pid),
                    status: self
                        .agent_status_cache
                        .get(&pid)
                        .copied()
                        .unwrap_or(crate::daemon::agents::AgentStatus::Idle)
                        .into(),
                });
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // Chrome actions (clients draw all chrome; these mutate daemon state)
    // ------------------------------------------------------------------

    /// Rename a pane in the named session (the name popup's commit).
    pub(crate) fn rename_pane_in_session(
        &mut self,
        session: &str,
        pane_id: u64,
        name: &str,
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        if !self.sessions[idx].tree.contains(pane_id) {
            return Ok(format!("no pane {pane_id} in {session:?}"));
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            return Ok("name cannot be empty".to_string());
        }
        let taken = self
            .sessions[idx]
            .tree
            .pane_ids()
            .into_iter()
            .filter(|id| *id != pane_id)
            .map(|id| self.pane_label(id))
            .any(|l| l.trim() == name);
        if taken {
            return Ok(format!("a pane named '{name}' already exists"));
        }
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.custom_name = Some(name.clone());
        }
        self.bump_layout_version();
        Ok(format!("renamed pane {pane_id} to {name:?}"))
    }

    /// Rename a session.
    pub(crate) fn rename_session(&mut self, session: &str, new_name: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let name = new_name.trim().to_string();
        if name.is_empty() {
            return Ok("name cannot be empty".to_string());
        }
        let taken = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .any(|(_, s)| s.name == name);
        if taken {
            return Ok(format!("a session named '{name}' already exists"));
        }
        self.sessions[idx].name = name.clone();
        self.bump_layout_version();
        Ok(format!("renamed session to {name:?}"))
    }

    /// List the git worktrees of a session's repository (main + linked), with
    /// the kumo-side flags the picker renders (main tree, already-open).
    pub(crate) fn worktree_list(&self, session: &str) -> Result<Vec<WireWorktree>> {
        let Some(ws) = self.sessions.iter().find(|s| s.name == session).map(|s| s.workspace.clone()) else {
            return Ok(Vec::new());
        };
        let items = kumo_core::worktrees::list_worktrees(&ws)
            .map_err(|e| anyhow!("{e}"))?;
        let root = kumo_core::worktrees::repo_root(&ws);
        let rows = items
            .into_iter()
            .map(|info| {
                let canon = std::fs::canonicalize(&info.path).ok();
                let is_main = match (&root, &canon) {
                    (Some(r), Some(c)) => *r == *c,
                    _ => false,
                };
                let open = self.session_for_workspace(&info.path).is_some();
                WireWorktree { path: info.path, branch: info.branch, is_main, open }
            })
            .collect();
        Ok(rows)
    }

    /// Create a git worktree (new branch from the repo HEAD) and open a fresh
    /// session inside it, named after the branch.
    pub(crate) fn worktree_create(&mut self, session: &str, branch: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let branch = branch.trim().to_string();
        if branch.is_empty() {
            return Ok("branch name cannot be empty".to_string());
        }
        match self.new_worktree_session(idx, &branch) {
            Ok(()) => Ok(format!("created worktree {branch:?}")),
            Err(e) => Ok(format!("error: {e}")),
        }
    }

    /// Open the session already working in `path`, or create a new one there
    /// (the worktree picker's confirm).
    pub(crate) fn worktree_open(&mut self, session: &str, path: &std::path::Path) -> Result<String> {
        if !self.sessions.iter().any(|s| s.name == session) {
            return Ok(format!("no session {session:?}"));
        }
        self.open_session_in_worktree(path, None)?;
        Ok(format!("opened {path:?}"))
    }

    /// Apply theme `idx` daemon-side: re-color every pane's terminal emulator
    /// with the new ANSI palette and record it as the active theme.
    pub(crate) fn set_theme(&mut self, idx: usize) -> Result<String> {
        if idx >= kumo_core::theme::THEMES.len() {
            return Ok(format!("no such theme #{idx}"));
        }
        let theme = kumo_core::theme::THEMES[idx];
        for pane in self.panes.values_mut() {
            pane.apply_theme(&theme);
        }
        self.theme = theme;
        self.theme_idx = idx;
        Ok(format!("theme: {}", theme.name))
    }

    /// MENU `config`: open the config file in an editor pane inside the named
    /// session (vertical split). Uses `$VISUAL`, then `$EDITOR`, then `vi`.
    pub(crate) fn open_config_in_session(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let (prog, mut args) = config_editor();
        let path = kumo_core::config::config_file_toml();
        let path = if path.is_file() { path } else { kumo_core::config::config_file() };
        args.push(path.to_string_lossy().into_owned());
        let focus = self.sessions[idx].tree.focus;
        let sid = self.sessions[idx].id;
        let pid = Pty::next_pane_id();
        let (cols, rows) = self.pane_sizes.get(&focus).copied().unwrap_or(super::DEFAULT_PANE_DIMS);
        let pane = Pane::spawn(
            sid,
            pid,
            self.shell.clone(),
            Some((prog, args)),
            Some(self.sessions[idx].workspace.clone()),
            cols,
            rows,
            false,
            self.events_tx.clone(),
            &self.theme,
        )?;
        self.panes.insert(pid, pane);
        if !self.sessions[idx].tree.split(focus, pid, kumo_core::layout::SplitDir::V) {
            if let Some(mut p) = self.panes.remove(&pid) {
                p.pty.kill();
            }
            return Ok(format!("no room to open the editor in {session:?}"));
        }
        self.bump_layout_version();
        Ok(format!("opened the config in {session:?}"))
    }

    /// Write raw bytes into a specific pane (mouse-reporting forwarding, where
    /// the client knows exactly which pane the pointer is over).
    pub(crate) fn pane_write(&mut self, pane_id: u64, bytes: &[u8]) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.write(bytes);
        }
    }

    /// Scroll a specific pane's viewport by one wheel step.
    pub(crate) fn scroll_pane(&mut self, pane_id: u64, up: bool) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.scroll(if up { -3 } else { 3 });
        }
    }

    /// The active startup update notice, if any.
    pub(crate) fn update_status(&self) -> Option<WireNotice> {
        self.update_notice.as_ref().map(|n| WireNotice {
            key: n.key.clone(),
            display: n.display.clone(),
        })
    }

    /// Dismiss the startup update banner (persisted so it stays gone).
    pub(crate) fn dismiss_update(&mut self, key: &str) {
        kumo_core::update::dismiss(key);
        self.update_notice = None;
    }
}
