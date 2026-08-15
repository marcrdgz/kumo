//! Daemon-side content rendering and semantic layout export.
//!
//! The daemon is the "smart renderer" but it **never draws chrome**: no borders,
//! box-drawing characters, sidebar, or status bar. `tick` refreshes metadata
//! and renders each pane's terminal content into its retained cache; `layout`
//! exports the semantic split tree (ratios, not pixels) for clients to draw.
//!
//! Clients are "dumb viewports": they compute geometry from the semantic tree,
//! request pane sizes via `PaneResize`, and draw their own borders/chrome.

use ratatui::buffer::Buffer;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Style};

use kumo_protocol::{AgentInfo, Layout, LayoutNode, LayoutPane, SessionLayout, SplitDir};

use super::App;
use crate::layout;

/// Legacy chrome-drawing helpers, kept so the (dead) sidebar/overlay renderers
/// still compile; the daemon never calls them.
#[allow(dead_code)]
pub(super) fn fill(f: &mut Frame, area: Rect, color: RColor) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = f.buffer_mut().cell_mut((x, y)) {
                c.reset();
                c.set_bg(color);
            }
        }
    }
}

#[allow(dead_code)]
pub(super) fn put(f: &mut Frame, x: u16, y: u16, s: &str, style: Style) {
    if let Some(c) = f.buffer_mut().cell_mut((x, y)) {
        c.set_symbol(s);
        c.set_style(style);
    }
}

#[allow(dead_code)]
pub(super) fn text(f: &mut Frame, x: u16, y: u16, s: &str, style: Style, max: u16) {
    for (i, ch) in s.chars().take(max as usize).enumerate() {
        put(f, x + i as u16, y, &ch.to_string(), style);
    }
}

impl From<crate::agents::AgentStatus> for kumo_protocol::AgentStatus {
    fn from(status: crate::agents::AgentStatus) -> Self {
        match status {
            crate::agents::AgentStatus::Working => kumo_protocol::AgentStatus::Working,
            crate::agents::AgentStatus::Blocked => kumo_protocol::AgentStatus::Blocked,
            crate::agents::AgentStatus::Idle => kumo_protocol::AgentStatus::Idle,
        }
    }
}

/// Default terminal size for a pane until a client resizes it via `PaneResize`.
const DEFAULT_PANE_SIZE: (u16, u16) = (80, 24);

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
                // `focused = false`: the cursor is streamed via `PaneFrame`
                // and drawn by the client, so it is never baked into the grid.
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

    /// Export the full semantic layout tree: sessions → splits (ratios) →
    /// panes (title, cwd, agent status). Pushed to layout subscribers on any
    /// change; clients derive geometry and draw all chrome.
    pub(super) fn layout(&self) -> Layout {
        let active = self.sessions.get(self.active).map(|s| s.name.clone());
        let sessions = self
            .sessions
            .iter()
            .map(|s| SessionLayout {
                name: s.name.clone(),
                workspace: s.workspace.clone(),
                focus: s.tree.focus,
                zoom: s.zoom,
                root: s.tree.root.as_ref().map(|r| Box::new(self.layout_node(r))),
            })
            .collect();
        Layout { active, sessions }
    }

    fn layout_node(&self, node: &layout::Node) -> LayoutNode {
        match node {
            layout::Node::Pane { id } => LayoutNode::Pane(self.layout_pane(*id)),
            layout::Node::Split { dir, ratio, a, b, .. } => LayoutNode::Split {
                dir: match dir {
                    layout::SplitDir::V => SplitDir::Vertical,
                    layout::SplitDir::H => SplitDir::Horizontal,
                },
                ratio: *ratio,
                a: Box::new(self.layout_node(a)),
                b: Box::new(self.layout_node(b)),
            },
        }
    }

    fn layout_pane(&self, pid: u64) -> LayoutPane {
        let pane = self.panes.get(&pid);
        let is_ai = pane.map(|p| p.is_ai_cli()).unwrap_or(false);
        let agent = if is_ai {
            Some(AgentInfo {
                name: self.agent_label(pid),
                status: self
                    .agent_status_cache
                    .get(&pid)
                    .copied()
                    .unwrap_or(crate::agents::AgentStatus::Idle)
                    .into(),
            })
        } else {
            None
        };
        LayoutPane {
            id: pid,
            title: self.pane_label(pid),
            cwd: pane.map(|p| p.cwd.clone()).unwrap_or_default(),
            is_ai,
            agent,
        }
    }

    /// Display label of a pane (no focus/zoom suffix). A custom name wins;
    /// otherwise the AI CLI marker or `shell N`.
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
}
