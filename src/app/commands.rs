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

use kumo_protocol::{AgentInfo, AgentStatusLine, SessionInfo, SplitDir, WireKeyEvent};

use super::App;
use crate::layout;

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
            let bytes = crate::keys::encode(key);
            if !bytes.is_empty() {
                pane.write(&bytes);
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
            kumo_protocol::ResizeDir::Left => crate::layout::ResizeDir::Left,
            kumo_protocol::ResizeDir::Down => crate::layout::ResizeDir::Down,
            kumo_protocol::ResizeDir::Up => crate::layout::ResizeDir::Up,
            kumo_protocol::ResizeDir::Right => crate::layout::ResizeDir::Right,
        };
        let prev = self.active;
        self.active = idx;
        self.resize_focused(dir);
        self.active = prev;
        Ok(format!("resized split in {session:?}"))
    }

    /// Swap the focused pane with its sibling in the named session.
    pub(crate) fn swap_focused(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let focus = self.sessions[idx].tree.focus;
        if self.sessions[idx].tree.swap_with_sibling(focus) {
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
        Ok(format!("rotated layout in {session:?}"))
    }

    /// Toggle the named session's zoom.
    pub(crate) fn zoom_session(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        self.sessions[idx].zoom = !self.sessions[idx].zoom;
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
        let mut sent = 0usize;
        if let Some(pane) = self.panes.get_mut(&pid) {
            for key in keys {
                let bytes = crate::keys::encode(key.to_crossterm());
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
                        Some(AgentInfo {
                            name: self.agent_label(pid),
                            status: self
                                .agent_status_cache
                                .get(&pid)
                                .copied()
                                .unwrap_or(crate::agents::AgentStatus::Idle)
                                .into(),
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
                        .unwrap_or(crate::agents::AgentStatus::Idle)
                        .into(),
                });
            }
        }
        out
    }
}
