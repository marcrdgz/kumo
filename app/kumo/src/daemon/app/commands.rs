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
    AgentExplainReport, AgentIdleReason, AgentInfo, AgentMarkerMatch, AgentStatusLine,
    EvidenceRegion, PaneInfo, SessionInfo, SplitDir, WireKeyEvent, WireNotice, WireWorktree,
};

use super::App;
use kumo_core::layout;
use crate::daemon::agents::{self, AgentStatus, Snapshot};
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
    /// The focused pane of the active session's active tab.
    pub(crate) fn active_focus(&self) -> u64 {
        self.sessions.get(self.active).map(|s| s.active_tab().tree.focus).unwrap_or(0)
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

    /// Close a pane in the named session (default: the focused pane of the active tab).
    pub(crate) fn close_pane_in_session(&mut self, session: &str, pane_id: Option<u64>) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let pid = match pane_id {
            Some(pid) => {
                if !self.sessions[idx].contains_pane(pid) {
                    return Ok(format!("no pane {pid} in {session:?}"));
                }
                pid
            }
            None => self.sessions[idx].active_tab().tree.focus,
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
    /// neighbor in `dir` (active tab).
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
        let focus = self.sessions[idx].active_tab().tree.focus;
        let t = self.sessions[idx].active_tab;
        if !self.sessions[idx].tabs[t].tree.resize_pane(focus, dir, RESIZE_STEP) {
            return Ok(format!("nothing to resize in that direction in {session:?}"));
        }
        self.bump_layout_version();
        Ok(format!("resized split in {session:?}"))
    }

    /// Set the ratio of a specific split (identified by the id shipped in the
    /// semantic layout) to an absolute value — the daemon-side half of a
    /// desktop drag on the divider (active tab).
    pub(crate) fn set_split_ratio_in_session(
        &mut self,
        session: &str,
        split_id: u64,
        ratio: f32,
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let t = self.sessions[idx].active_tab;
        self.sessions[idx].tabs[t].tree.set_ratio(split_id, ratio);
        self.bump_layout_version();
        Ok(format!("set split {split_id} ratio in {session:?}"))
    }

    /// Swap the focused pane with its sibling in the named session's active tab.
    pub(crate) fn swap_focused(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let focus = self.sessions[idx].active_tab().tree.focus;
        let t = self.sessions[idx].active_tab;
        if self.sessions[idx].tabs[t].tree.swap_with_sibling(focus) {
            self.bump_layout_version();
            Ok(format!("swapped pane in {session:?}"))
        } else {
            Ok(format!("no sibling to swap in {session:?}"))
        }
    }

    /// Mirror (rotate) the layout tree of the named session's active tab.
    pub(crate) fn rotate_layout(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let t = self.sessions[idx].active_tab;
        self.sessions[idx].tabs[t].tree.mirror();
        self.bump_layout_version();
        Ok(format!("rotated layout in {session:?}"))
    }

    /// Toggle the active tab's zoom in the named session.
    pub(crate) fn zoom_session(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let t = self.sessions[idx].active_tab;
        self.sessions[idx].tabs[t].zoom = !self.sessions[idx].tabs[t].zoom;
        self.bump_layout_version();
        Ok(format!("toggled zoom in {session:?}"))
    }

    /// Send key events to a pane in the named session (default: focused of active tab).
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
        let s_idx = idx;
        let t_idx = self.sessions[s_idx].active_tab;
        let prev_focus = self.sessions[s_idx].tabs[t_idx].tree.focus;
        self.active = idx;
        let pid = match pane_id {
            Some(pid) => {
                if !self.sessions[idx].contains_pane(pid) {
                    return Ok(format!("no pane {pid} in {session:?}"));
                }
                // focus the tab containing that pane
                if let Some(t) = self.sessions[idx].find_tab_containing(pid) {
                    self.sessions[idx].active_tab = t;
                    self.sessions[idx].tabs[t].tree.focus = pid;
                }
                pid
            }
            None => self.sessions[idx].active_tab().tree.focus,
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
            .map(|(i, s)| {
                let zoomed = s.active_tab().zoom;
                let focus = s.active_tab().tree.pane_count().checked_sub(0).and_then(|_| {
                    if s.active_tab().tree.pane_count() > 0 { Some(s.active_tab().tree.focus) } else { None }
                });
                // collect pane ids across all tabs for agent scan
                let all_pids: Vec<u64> = s.tabs.iter().flat_map(|t| t.tree.pane_ids()).collect();
                SessionInfo {
                    name: s.name.clone(),
                    workspace: s.workspace.clone(),
                    tab_count: s.tabs.len(),
                    pane_count: s.pane_count(),
                    zoomed,
                    active: i == self.active,
                    active_tab: Some(s.active_tab().name.clone()),
                    focus,
                    tabs: s.tabs.iter().enumerate().map(|(ti, t)| kumo_protocol::TabInfo {
                        id: t.id,
                        name: t.name.clone(),
                        pane_count: t.tree.pane_count(),
                        zoomed: t.zoom,
                        active: ti == s.active_tab,
                        focus: (t.tree.pane_count()>0).then_some(t.tree.focus),
                        panes: t.tree
                            .pane_ids()
                            .into_iter()
                            .map(|pid| {
                                let active = t.tree.pane_count() > 0 && t.tree.focus == pid;
                                let label = self
                                    .panes
                                    .get(&pid)
                                    .map(|p| self.pane_info_label(p))
                                    .unwrap_or_default();
                                PaneInfo { id: pid, label, active }
                            })
                            .collect(),
                    })                        .collect(),
                    agents: all_pids
                        .into_iter()
                        .filter_map(|pid| {
                            let pane = self.panes.get(&pid)?;
                            if !pane.is_ai_cli() { return None; }
                            let (cpu, mem_kb) = self.agent_proc_cache.get(&pid).copied().unwrap_or((0.0, 0));
                            let (_, tab_index, pane_index) =
                                self.pane_position(pid).unwrap_or((0, 0, 0));
                            Some(AgentInfo {
                                name: self.agent_label(pid),
                                status: self.agent_status_cache.get(&pid).copied().unwrap_or(AgentStatus::Idle).into(),
                                cpu, mem_kb,
                                pane_id: pid,
                                pane_index: pane_index as u64,
                                tab_index: tab_index as u64,
                            })
                        })
                        .collect(),
                }
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

    /// `kumo agent start --kind <kind> --pane <id> [-- <args>]`: launches an
    /// agent program in an existing shell pane. Returns once the pane's
    /// detection is not immediately blocked (`agent_not_ready` on blocked).
    pub(crate) fn agent_start(
        &mut self,
        session: &str,
        pane_id: u64,
        kind: &str,
        args: &[String],
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        if !self.sessions[idx].contains_pane(pane_id) {
            return Ok(format!("no pane {pane_id} in {session:?}"));
        }
        let kind = kind.trim();
        if kind.is_empty() {
            return Ok("kind cannot be empty".to_string());
        }
        // Build command line: `kind` + args, shell-escaped naively (args with spaces quoted)
        let mut cmd = kind.to_string();
        for a in args {
            cmd.push(' ');
            if a.contains(' ') || a.contains('"') {
                cmd.push('"');
                cmd.push_str(&a.replace('"', "\\\""));
                cmd.push('"');
            } else {
                cmd.push_str(a);
            }
        }
        cmd.push('\r');
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.write(cmd.as_bytes());
        }
        // Check immediately whether pane went blocked (e.g. permission prompt on launch)
        if let Some(pane) = self.panes.get(&pane_id) {
            let st = pane.agent_status();
            if st == crate::daemon::agents::AgentStatus::Blocked {
                return Ok("error: agent_not_ready (pane is blocked immediately after start)".to_string());
            }
        }
        Ok(format!("started {kind} in pane {pane_id}"))
    }

    /// `kumo agent rename <pane> <name>`: live alias so scripts reference agents
    /// by name. No persistence — ephemeral per daemon.
    pub(crate) fn agent_rename(&mut self, session: &str, pane_id: u64, name: &str) -> Result<String> {
        let Some(s) = self.sessions.iter().find(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        if !s.contains_pane(pane_id) {
            return Ok(format!("no pane {pane_id} in {session:?}"));
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            return Ok("name cannot be empty".to_string());
        }
        if name.len() > 64 {
            return Ok("name too long (max 64)".to_string());
        }
        self.agent_aliases.insert(pane_id, name.clone());
        self.bump_layout_version();
        Ok(format!("renamed pane {pane_id} to {name:?}"))
    }

    /// `kumo agent broadcast <text> [--filter status]`: fan one prompt out to
    /// every AI pane in the session, filtered by status when provided.
    pub(crate) fn agent_broadcast(
        &mut self,
        session: &str,
        text: &str,
        filter: Option<kumo_protocol::AgentStatus>,
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let pids: Vec<u64> = self.sessions[idx]
            .tabs
            .iter()
            .flat_map(|t| t.tree.pane_ids())
            .collect();
        let mut sent = 0usize;
        for pid in pids {
            let pane = match self.panes.get(&pid) {
                Some(p) if p.is_ai_cli() => p,
                _ => continue,
            };
            if let Some(f) = filter {
                let st: kumo_protocol::AgentStatus = self
                    .current_agent_status(pid)
                    .unwrap_or(crate::daemon::agents::AgentStatus::Idle)
                    .into();
                if st != f {
                    continue;
                }
            }
            // Bracketed-paste aware inject without the blocked guard (broadcast
            // intentionally sends even if blocked — the receiver will queue).
            let bracketed = pane.vt.mode_get(crate::daemon::vt::MODE_BRACKETED_PASTE);
            let payload = if bracketed {
                format!("\x1b[200~{}\x1b[201~\r", text)
            } else {
                format!("{}\r", text)
            };
            if let Some(p) = self.panes.get_mut(&pid) {
                p.write(payload.as_bytes());
                sent += 1;
            }
        }
        Ok(format!("broadcast to {sent} agent(s) in {session:?}"))
    }

    /// One status line per running AI CLI, for `kumo agent status`.
    pub(crate) fn agent_status_lines(&self) -> Vec<AgentStatusLine> {
        let mut out = Vec::new();
        for s in &self.sessions {
            for tab in &s.tabs {
                for pid in tab.tree.pane_ids() {
                    let Some(pane) = self.panes.get(&pid) else { continue };
                    if !pane.is_ai_cli() { continue; }
                    let (_, tab_index, pane_index) = self.pane_position(pid).unwrap_or((0, 0, 0));
                    out.push(AgentStatusLine {
                        session: s.name.clone(),
                        pane_id: pid,
                        name: self.agent_label(pid),
                        status: self.agent_status_cache.get(&pid).copied().unwrap_or(AgentStatus::Idle).into(),
                        pane_index: pane_index as u64,
                        tab_index: tab_index as u64,
                    });
                }
            }
        }
        out
    }

    /// Diagnostic report for `kumo agent explain`: why this pane reads the
    /// state it does — matched markers, evidence region, and the idle verdict
    /// reason — evaluated live against the pane's terminal buffer and the
    /// daemon's cached state.
    pub(crate) fn agent_explain(&self, session: &str, pane_id: u64) -> Result<AgentExplainReport> {
        let Some(s) = self.sessions.iter().find(|s| s.name == session) else {
            anyhow::bail!("no session {session:?}");
        };
        if !s.contains_pane(pane_id) {
            anyhow::bail!("no pane {pane_id} in session {session:?}");
        }
        let Some(pane) = self.panes.get(&pane_id) else {
            anyhow::bail!("no pane {pane_id}");
        };
        let exp = agents::explain(&Snapshot::capture(&pane.vt));
        // Detection only runs for AI panes; a dead or plain shell pane reads
        // the default Idle (matching what the UI displays via the cache).
        let raw = if pane.dead || !pane.is_ai_cli() {
            AgentStatus::Idle
        } else {
            exp.status
        };
        let prev = self.last_agent_status.get(&pane_id).copied();
        let focused = self.pane_is_focused(pane_id);
        let status = super::tasks::apply_seen(raw, prev, focused);
        let idle_reason = if !pane.is_ai_cli() {
            AgentIdleReason::NotAnAgent
        } else if pane.dead {
            AgentIdleReason::DeadPane
        } else if status == AgentStatus::Done {
            AgentIdleReason::UnseenFinish
        } else if status == AgentStatus::Idle && prev == Some(AgentStatus::Done) {
            AgentIdleReason::SeenAfterFocus
        } else if exp.status == AgentStatus::Idle {
            AgentIdleReason::IdleMarkers
        } else if exp.status == AgentStatus::Unknown {
            AgentIdleReason::UnknownFallback
        } else {
            AgentIdleReason::Active
        };
        let mut markers = Vec::new();
        for ev in exp.blocked {
            let agent = ev.agent;
            for m in ev.blocked {
                markers.push(marker_wire(&agent, "blocked", m));
            }
        }
        for ev in exp.working {
            let agent = ev.agent;
            for m in ev.working {
                markers.push(marker_wire(&agent, "working", m));
            }
        }
        for ev in exp.idle {
            let agent = ev.agent;
            for m in ev.idle {
                markers.push(marker_wire(&agent, "idle", m));
            }
        }
        let (cpu, mem_kb) = self.agent_proc_cache.get(&pane_id).copied().unwrap_or((0.0, 0));
        let (_, tab_index, pane_index) = self.pane_position(pane_id).unwrap_or((0, 0, 0));
        let cli = pane
            .custom_name
            .clone()
            .unwrap_or_else(|| if pane.is_ai_cli() { self.agent_label(pane_id) } else { "shell".to_string() });
        Ok(AgentExplainReport {
            pane_id,
            session: s.name.clone(),
            cli,
            os_pid: pane.pty.process_id().map(u64::from).unwrap_or(0),
            is_ai_cli: pane.is_ai_cli(),
            dead: pane.dead,
            focused,
            raw_status: raw.into(),
            status: status.into(),
            prev_status: prev.map(Into::into),
            idle_reason,
            markers,
            precedence: agents::PRECEDENCE.to_string(),
            last_output_age_ms: pane.last_output_age().as_millis() as u64,
            cpu,
            mem_kb,
            pane_index: pane_index as u64,
            tab_index: tab_index as u64,
        })
    }

    /// Wire label of a pane in the `session list` views: custom name, then the
    /// AI CLI name, then `shell`. (The chrome's `shell N` numbering is a
    /// focused-tab rendering concern, not list data.)
    fn pane_info_label(&self, pane: &Pane) -> String {
        if let Some(name) = &pane.custom_name {
            return name.clone();
        }
        if pane.is_ai_cli() {
            return self.agent_label(pane.id);
        }
        "shell".to_string()
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
        if !self.sessions[idx].contains_pane(pane_id) {
            return Ok(format!("no pane {pane_id} in {session:?}"));
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            return Ok("name cannot be empty".to_string());
        }
        let all_ids: Vec<u64> = self.sessions[idx].tabs.iter().flat_map(|t| t.tree.pane_ids()).collect();
        let taken = all_ids.into_iter().filter(|id| *id != pane_id).map(|id| self.pane_label(id)).any(|l| l.trim() == name);
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
    /// the kumo-side flags the picker renders (main tree, already-open) plus checkpoint fields.
    pub(crate) fn worktree_list(&self, session: &str) -> Result<Vec<WireWorktree>> {
        let Some(ws) = self.sessions.iter().find(|s| s.name == session).map(|s| s.workspace.clone()) else {
            return Ok(Vec::new());
        };
        let items = kumo_core::worktrees::list_worktrees(&ws)
            .map_err(|e| anyhow!("{e}"))?;
        let rows = items
            .into_iter()
            .enumerate()
            .map(|(idx, info)| {
                // git worktree list --porcelain returns main first, so index 0 is main.
                let is_main = idx == 0;
                let open = self.session_for_workspace(&info.path).is_some();
                let meta = kumo_core::worktree_meta::get(&info.path);
                WireWorktree {
                    path: info.path,
                    branch: info.branch.clone().or_else(|| meta.as_ref().and_then(|m| m.branch.clone())),
                    is_main,
                    open,
                    comment: meta.as_ref().and_then(|m| m.comment.clone()),
                    status: meta.as_ref().and_then(|m| m.status.clone()),
                    is_ephemeral: meta.as_ref().map(|m| m.is_ephemeral).unwrap_or(false),
                }
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

    /// Extended creator for isolated `--ai` worktrees (no `kumo/` prefix).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn worktree_create_full(
        &mut self,
        session: &str,
        branch: &str,
        from: Option<&str>,
        note: Option<&str>,
        agent: Option<&str>,
        is_ai: bool,
        name: Option<&str>,
    ) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let branch_override = if branch.trim().is_empty() { None } else { Some(branch.trim()) };
        let from = from.map(|s| s.trim()).filter(|s| !s.is_empty());
        let note = note.map(|s| s.trim()).filter(|s| !s.is_empty());
        let agent = agent.map(|s| s.trim()).filter(|s| !s.is_empty());
        let name = name.map(|s| s.trim()).filter(|s| !s.is_empty());
        // Generic path (no is_ai, no from/note/agent) with explicit branch -> keep old fast path
        let use_ext = is_ai || from.is_some() || note.is_some() || agent.is_some() || name.is_some() || branch_override.is_none();
        let res = if use_ext {
            self.new_worktree_session_ext(idx, branch_override, from, note, agent, is_ai, name)
        } else {
            let b = branch_override.unwrap().to_string();
            self.new_worktree_session(idx, &b).map(|_| b)
        };
        match res {
            Ok(b) => Ok(format!("created worktree {b:?}")),
            Err(e) => Ok(format!("error: {e}")),
        }
    }

    /// Remove a worktree directory and optionally its branch (ephemeral preview when `!force`).
    pub(crate) fn worktree_remove(&mut self, session: &str, path: &std::path::Path, force: bool) -> Result<String> {
        if !self.sessions.iter().any(|s| s.name == session) {
            return Ok(format!("no session {session:?}"));
        }
        let ws = self.sessions.iter().find(|s| s.name == session).map(|s| s.workspace.clone());
        match self.remove_worktree_at(path, force, ws.as_deref()) {
            Ok(msg) => Ok(msg),
            Err(e) => Ok(format!("error: {e}")),
        }
    }

    /// Set checkpoint comment/status for a worktree.
    pub(crate) fn worktree_set(
        &mut self,
        session: &str,
        path: &std::path::Path,
        comment: Option<&str>,
        status: Option<&str>,
    ) -> Result<String> {
        if !self.sessions.iter().any(|s| s.name == session) {
            return Ok(format!("no session {session:?}"));
        }
        // Validate status via protocol helper — "—", "–", "-" and empty all mean clear
        let is_clear = |t: &str| t.is_empty() || t == "—" || t == "–" || t == "-";
        let status_norm = if let Some(s) = status {
            let trimmed = s.trim();
            if is_clear(trimmed) { Some(None) } else {
                if kumo_protocol::WorktreeStatus::parse(trimmed).is_none() {
                    return Ok(format!("invalid status {trimmed:?} (use todo|in-progress|in-review|completed)"));
                }
                Some(Some(trimmed.to_ascii_lowercase()))
            }
        } else { None }; // None means no change; Some(None) means clear — caller uses Option<Option>
        let comment_norm = comment.map(|c| { let t=c.trim(); if is_clear(t) {None} else {Some(t.to_string())} });
        // Distinguish no-change vs clear: here comment == None means no --comment flag; Some("") means clear
        // Our caller passes None for absent flag; empty string means clear.
        let c_arg = if comment.is_some() { Some(comment_norm.flatten()) } else { None };
        let s_arg = if status.is_some() { status_norm } else { None };
        // When both flag-absent, just query
        if c_arg.is_none() && s_arg.is_none() {
            return Ok(format!("no change for {}", path.display()));
        }
        match kumo_core::worktree_meta::set(path, c_arg, s_arg, None, None) {
            Ok(cp) => {
                self.bump_layout_version();
                let msg = format!("set {} comment={:?} status={:?}", path.display(), cp.comment, cp.status);
                Ok(msg)
            }
            Err(e) => Ok(format!("error: {e}")),
        }
    }

    /// Query the checkpoint for a session's workspace (or explicit `path`).
    pub(crate) fn worktree_current(&self, session: &str, path: Option<&std::path::Path>) -> Result<Option<WireWorktree>> {
        let Some(ws) = self.sessions.iter().find(|s| s.name == session).map(|s| s.workspace.clone()) else {
            return Ok(None);
        };
        let target = path.map(|p| p.to_path_buf()).unwrap_or(ws.clone());
        let branch = kumo_core::worktrees::list_worktrees(&target).ok().and_then(|list| {
            let canon = std::fs::canonicalize(&target).ok();
            list.into_iter().find(|w| {
                let c = std::fs::canonicalize(&w.path).ok();
                match (&c, &canon) {
                    (Some(a), Some(b)) => a == b,
                    _ => w.path == target,
                }
            }).and_then(|w| w.branch)
        });
        // is_main: first entry of `git worktree list` is the main checkout
        let list_for_main = kumo_core::worktrees::list_worktrees(&ws).ok().unwrap_or_default();
        let main_path = list_for_main.first().map(|w| w.path.clone());
        let canon_target = std::fs::canonicalize(&target).ok();
        let canon_main = main_path.as_ref().and_then(|p| std::fs::canonicalize(p).ok());
        let is_main = match (&canon_target, &canon_main) {
            (Some(t), Some(m)) => t == m,
            _ => main_path.as_ref().map(|p| p == &target).unwrap_or(false),
        };
        let open = self.session_for_workspace(&target).is_some();
        let meta = kumo_core::worktree_meta::get(&target);
        Ok(Some(WireWorktree {
            path: target,
            branch: branch.or_else(|| meta.as_ref().and_then(|m| m.branch.clone())),
            is_main,
            open,
            comment: meta.as_ref().and_then(|m| m.comment.clone()),
            status: meta.as_ref().and_then(|m| m.status.clone()),
            is_ephemeral: meta.as_ref().map(|m| m.is_ephemeral).unwrap_or(false),
        }))
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
    /// with the new ANSI palette and record it as the active theme. Supports
    /// the built-ins plus an optional `[theme.custom]` entry at the end.
    pub(crate) fn set_theme(&mut self, idx: usize) -> Result<String> {
        let all = kumo_core::theme::all_themes(kumo_core::config::custom_theme());
        if idx >= all.len() {
            return Ok(format!("no such theme #{idx}"));
        }
        let theme = all[idx].clone();
        for pane in self.panes.values_mut() {
            pane.apply_theme_owned(&theme);
        }
        let name = theme.name.clone();
        self.theme = theme;
        self.theme_idx = idx;
        Ok(format!("theme: {}", name))
    }

    /// MENU `config`: open the config file in an editor pane inside the named
    /// session's active tab (vertical split). Uses `$VISUAL`, then `$EDITOR`, then `vi`.
    pub(crate) fn open_config_in_session(&mut self, session: &str) -> Result<String> {
        let Some(idx) = self.sessions.iter().position(|s| s.name == session) else {
            return Ok(format!("no session {session:?}"));
        };
        let (prog, mut args) = config_editor();
        let path = kumo_core::config::config_file_toml();
        let path = if path.is_file() { path } else { kumo_core::config::config_file() };
        args.push(path.to_string_lossy().into_owned());
        let focus = self.sessions[idx].active_tab().tree.focus;
        let sid = self.sessions[idx].id;
        let t = self.sessions[idx].active_tab;
        let pid = Pty::next_pane_id();
        let (cols, rows) = self.pane_sizes.get(&focus).copied().unwrap_or(super::DEFAULT_PANE_DIMS);
        let pane = Pane::spawn(sid, pid, self.shell.clone(), Some((prog, args)), Some(self.sessions[idx].workspace.clone()), cols, rows, false, self.events_tx.clone(), &self.theme)?;
        self.panes.insert(pid, pane);
        if !self.sessions[idx].tabs[t].tree.split(focus, pid, kumo_core::layout::SplitDir::V) {
            if let Some(mut p) = self.panes.remove(&pid) { p.pty.kill(); }
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

    /// Scroll a pane's viewport by an arbitrary delta (copy-mode).
    pub(crate) fn copy_scroll(&mut self, pane_id: u64, delta: i32) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.scroll(delta);
        }
    }

    /// Scroll a pane so that `row` (screen coordinate) is at the top.
    pub(crate) fn copy_scroll_to(&mut self, pane_id: u64, row: u32) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.scroll_to_row(row);
        }
    }

    pub(crate) fn copy_search(&self, pane_id: u64, query: &str) -> Vec<kumo_protocol::CopyHit> {
        let Some(pane) = self.panes.get(&pane_id) else { return Vec::new(); };
        pane.search(query)
            .into_iter()
            .map(|h| kumo_protocol::CopyHit { row: h.row, start_col: h.start_col, end_col: h.end_col })
            .collect()
    }

    pub(crate) fn copy_set_selection(&mut self, pane_id: u64, start: (u16, u16), end: (u16, u16)) -> bool {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            return pane.set_selection(start, end);
        }
        false
    }

    pub(crate) fn copy_clear_selection(&mut self, pane_id: u64) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.clear_selection();
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

/// Convert a detected marker match into its wire form for `agent explain`.
fn marker_wire(agent: &str, kind: &str, m: agents::MarkerMatch) -> AgentMarkerMatch {
    let region = match m.region {
        agents::Region::Screen => EvidenceRegion::Screen,
        agents::Region::Form => EvidenceRegion::Form,
        agents::Region::Footer => EvidenceRegion::Footer,
        agents::Region::Title => EvidenceRegion::Title,
    };
    AgentMarkerMatch {
        agent: agent.to_string(),
        kind: kind.to_string(),
        marker: m.marker.to_string(),
        region,
    }
}
