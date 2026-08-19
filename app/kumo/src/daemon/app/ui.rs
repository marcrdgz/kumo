//! Daemon-side content rendering and semantic layout export.
//!
//! The daemon is the "smart renderer" for **pane content only**: it refreshes
//! metadata, renders each pane's terminal into its retained cache, and exports
//! the semantic layout tree (sessions → splits in ratios → panes with metadata).
//! It never draws chrome — no borders, box-drawing characters, sidebar, or
//! status bar. Every client computes its own geometry and draws all chrome.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use kumo_protocol::Layout;

use super::App;
use crate::daemon::vt;

impl App {
    /// Refresh metadata (git branches, cwd follow, AI detection) and render the
    /// content of every dirty pane into its retained cache. Returns the ids of
    /// panes whose content changed (candidates for a `PaneFrame`).
    pub(super) fn tick(&mut self) -> Vec<u64> {
        self.poll_exits();
        self.refresh_branches();
        self.refresh_workspace_follow();
        self.refresh_ai_cli();
        self.refresh_agent_statuses();

        let mut changed = Vec::new();
        let ids: Vec<u64> = self.panes.keys().copied().collect();
        for pid in ids {
            let (cols, rows) = self.pane_size(pid);
            let Some(pane) = self.panes.get_mut(&pid) else { continue };
            if !pane.dirty && !pane.full_redraw {
                continue;
            }
            let rect = Rect::new(0, 0, cols.max(1), rows.max(1));
            let recreate = self.pane_cache.get(&pid).map(|c| c.area != rect).unwrap_or(true);
            if recreate {
                pane.resize(cols.max(1), rows.max(1));
                pane.full_redraw = true;
                self.pane_cache.insert(pid, Buffer::empty(rect));
            }
            if let Some(cached) = self.pane_cache.get_mut(&pid) {
                // `focused = false`: the cursor is streamed via `PaneFrame` and
                // drawn by the client, so it is never baked into the grid.
                let _ = pane.render_dirty(rect, false, cached);
                changed.push(pid);
            }
        }
        changed
    }

    /// The requested terminal size for `pid` (set by clients via `PaneResize`).
    pub(super) fn pane_size(&self, pid: u64) -> (u16, u16) {
        self.pane_sizes.get(&pid).copied().unwrap_or(DEFAULT_PANE_SIZE)
    }

    /// Display label of a pane in the active session, without the focus/zoom
    /// suffix. A custom name wins; otherwise the AI CLI marker or `shell N`.
    pub(super) fn pane_label(&self, pid: u64) -> String {
        let Some(pane) = self.panes.get(&pid) else {
            return " pane ".to_string();
        };
        if let Some(name) = &pane.custom_name {
            return format!(" {name} ");
        }
        if pane.is_ai_cli() {
            return " AI CLI ".to_string();
        }
        if self.sessions[self.active].tree.pane_count() > 1 {
            let n = self
                .sessions[self.active]
                .tree
                .pane_ids()
                .into_iter()
                .filter(|id| self.panes.get(id).is_some_and(|p| !p.is_ai_cli()))
                .position(|id| id == pid)
                .map(|i| i + 1)
                .unwrap_or(pid as usize);
            format!(" shell {n} ")
        } else {
            " shell ".to_string()
        }
    }

    /// Export the full semantic layout tree: sessions → splits (ratios) →
    /// panes (title, cwd, agent status, terminal flags). Pushed to layout
    /// subscribers; clients derive geometry and draw all chrome. Cached via
    /// `layout_version` so repeated `tick` calls without layout changes are
    /// cheap (`Arc` clone).
    pub(super) fn layout(&mut self) -> std::sync::Arc<Layout> {
        if let Some(cached) = &self.cached_layout {
            if self.cached_layout_version == self.layout_version {
                return cached.clone();
            }
        }
        let active = self.sessions.get(self.active).map(|s| s.name.clone());
        let sessions = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| kumo_protocol::SessionLayout {
                name: s.name.clone(),
                workspace: s.workspace.clone(),
                focus: s.tree.focus,
                zoom: s.zoom,
                root: s.tree.root.as_ref().map(|r| Box::new(self.layout_node(r))),
                branch: self.session_branch(i).map(Into::into),
            })
            .collect();
        let layout = Layout { active, sessions };
        let arc = std::sync::Arc::new(layout);
        self.cached_layout = Some(arc.clone());
        self.cached_layout_version = self.layout_version;
        arc
    }

    fn layout_node(&self, node: &kumo_core::layout::Node) -> kumo_protocol::LayoutNode {
        use kumo_protocol::LayoutNode as LN;
        match node {
            kumo_core::layout::Node::Pane { id } => LN::Pane(self.layout_pane(*id)),
            kumo_core::layout::Node::Split { id, dir, ratio, a, b } => LN::Split {
                id: *id,
                dir: match dir {
                    kumo_core::layout::SplitDir::V => kumo_protocol::SplitDir::Vertical,
                    kumo_core::layout::SplitDir::H => kumo_protocol::SplitDir::Horizontal,
                },
                ratio: *ratio,
                a: Box::new(self.layout_node(a)),
                b: Box::new(self.layout_node(b)),
            },
        }
    }

    fn layout_pane(&self, pid: u64) -> kumo_protocol::LayoutPane {
        let pane = self.panes.get(&pid);
        let is_ai = pane.map(|p| p.is_ai_cli()).unwrap_or(false);
        let agent = if is_ai {
            let (cpu, mem_kb) = self.agent_proc_cache.get(&pid).copied().unwrap_or((0.0, 0));
            Some(kumo_protocol::AgentInfo {
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
        } else {
            None
        };
        kumo_protocol::LayoutPane {
            id: pid,
            title: self.pane_label(pid),
            cwd: pane.map(|p| p.cwd.clone()).unwrap_or_default(),
            is_ai,
            agent,
            mouse_reporting: pane.map(|p| p.has_mouse_reporting()).unwrap_or(false),
            alt_screen: pane.map(|p| p.in_alt_screen()).unwrap_or(false),
        }
    }
}

/// Default terminal size for a pane until a client resizes it via `PaneResize`.
const DEFAULT_PANE_SIZE: (u16, u16) = (80, 24);

impl From<crate::daemon::agents::AgentStatus> for kumo_protocol::AgentStatus {
    fn from(status: crate::daemon::agents::AgentStatus) -> Self {
        match status {
            crate::daemon::agents::AgentStatus::Working => kumo_protocol::AgentStatus::Working,
            crate::daemon::agents::AgentStatus::Blocked => kumo_protocol::AgentStatus::Blocked,
            crate::daemon::agents::AgentStatus::Idle => kumo_protocol::AgentStatus::Idle,
        }
    }
}

impl From<crate::daemon::app::tasks::BranchInfo> for kumo_protocol::WireBranch {
    fn from(b: crate::daemon::app::tasks::BranchInfo) -> Self {
        kumo_protocol::WireBranch { name: b.name, ahead: b.ahead, behind: b.behind }
    }
}

/// Pack a scrollbar into the wire `ScrollState` (offset / total / screen).
pub(super) fn scroll_state(sb: vt::TerminalScrollbar) -> kumo_protocol::ScrollState {
    kumo_protocol::ScrollState {
        offset: sb.offset.min(u16::MAX as u64) as u16,
        total: sb.total.min(u16::MAX as u64) as u16,
        screen: sb.len.min(u16::MAX as u64) as u16,
    }
}
