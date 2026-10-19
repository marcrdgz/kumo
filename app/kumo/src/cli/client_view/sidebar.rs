//! Sidebar model: content rows per tab/layout, scrolling, reordering and hit-testing.

use super::*;

impl View {
    // ------------------------------------------------------------------
    // Sidebar hit-testing
    // ------------------------------------------------------------------

    pub(super) fn sidebar_footer_y(&self) -> u16 {
        self.rows.saturating_sub(self.status_h() + 1)
    }

    pub(super) fn content_region_h(&self) -> u16 {
        if self.sidebar_layout() == SidebarLayout::Project {
            self.project_sidebar_geometry().spaces_h as u16
        } else {
            self.sidebar_footer_y().saturating_sub(2)
        }
    }

    pub(super) fn agent_counts(&self) -> (usize, usize, usize) {
        let mut blocked = 0;
        let mut done = 0;
        let mut working = 0;
        for (_, _, _, status, _) in self.all_agent_entries() {
            match status {
                AgentStatus::Blocked => blocked += 1,
                AgentStatus::Done => done += 1,
                AgentStatus::Working => working += 1,
                AgentStatus::Idle | AgentStatus::Unknown => {}
            }
        }
        (blocked, done, working)
    }

    pub(super) fn compact_agent_summary(&self) -> String {
        let (blocked, done, working) = self.agent_counts();
        if blocked + done + working > 0 {
            format!("!{blocked} ✓{done} {}{working}", self.spinner_char())
        } else if self.all_agent_entries().is_empty() {
            "no agents".to_string()
        } else {
            "all clear".to_string()
        }
    }

    /// Attention rows lead the compact inbox; working agents fill any
    /// remaining slots. Idle and unknown agents stay in the full inbox.
    fn compact_agent_entries(&self) -> Vec<(usize, u64, String, AgentStatus)> {
        self.all_agent_entries()
            .into_iter()
            .filter(|(_, _, _, status, _)| {
                matches!(status, AgentStatus::Blocked | AgentStatus::Done | AgentStatus::Working)
            })
            .map(|(_, session, pane, status, name)| (session, pane, name, status))
            .collect()
    }

    pub(super) fn project_sidebar_geometry(&self) -> ProjectSidebarGeometry {
        let footer = self.sidebar_footer_y();
        if footer < 5 {
            return ProjectSidebarGeometry {
                tiny: true,
                spaces_h: 0,
                new_session_y: footer.saturating_sub(1),
                divider_y: footer,
                agents_header_y: footer,
                agent_rows: 0,
            };
        }
        let desired = if self.rows < 14 { 0 } else if self.rows < 20 { 2 } else { 3 };
        let agent_rows = desired
            .min(self.compact_agent_entries().len())
            .min(footer.saturating_sub(5) as usize);
        let agents_header_y = footer.saturating_sub(agent_rows as u16);
        let divider_y = agents_header_y.saturating_sub(1);
        let new_session_y = divider_y.saturating_sub(1);
        ProjectSidebarGeometry {
            tiny: false,
            spaces_h: new_session_y.saturating_sub(3) as usize,
            new_session_y,
            divider_y,
            agents_header_y,
            agent_rows,
        }
    }

    pub(super) fn visible_sidebar_tabs(&self) -> Vec<SidebarTab> {
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

    pub(super) fn ensure_sidebar_tab_visible(&mut self) {
        let visible = self.visible_sidebar_tabs();
        if !visible.contains(&self.sidebar_tab) {
            if let Some(first) = visible.first().copied() {
                self.sidebar_tab = first;
            }
        }
    }

    fn tab_at(&self, x: u16, y: u16) -> Option<SidebarTab> {
        if self.sidebar_layout() == SidebarLayout::Divided {
            return None;
        }
        if self.sidebar_layout() == SidebarLayout::Project {
            return None;
        }
        let w = self.effective_sidebar_width();
        if y != 2 || x >= w {
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
        let half = (w / 2).max(4);
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

    #[allow(dead_code)]
    fn project_name_for_session(&self, idx: usize) -> String {
        self.layout
            .as_ref()
            .and_then(|l| l.sessions.get(idx))
            .map(|s| short_workspace(&s.workspace))
            .unwrap_or_else(|| format!("session-{}", idx + 1))
    }

    #[allow(dead_code)]
    fn project_groups(&self) -> Vec<(String, String, Vec<usize>)> {
        let mut map: std::collections::HashMap<String, (String, Vec<usize>)> = std::collections::HashMap::new();
        let mut order: Vec<String> = Vec::new();
        if let Some(layout) = &self.layout {
            for (i, s) in layout.sessions.iter().enumerate() {
                let key = session_project_key(s);
                if !map.contains_key(&key) {
                    order.push(key.clone());
                }
                let label = project_label(s);
                map.entry(key).or_insert_with(|| (label, Vec::new())).1.push(i);
            }
        }
        order.into_iter().map(|k| {
            let (label, indices) = map.remove(&k).unwrap_or_default();
            (k, label, indices)
        }).collect()
    }

    pub(super) fn project_content(&self) -> Vec<SidebarRow> {
        let mut out = Vec::new();
        let layout = match &self.layout {
            Some(l) if !l.sessions.is_empty() => l,
            _ => {
                out.push(SidebarRow::Dim("No sessions yet".to_string()));
                out.push(SidebarRow::Dim("press c to create one".to_string()));
                return out;
            }
        };
        let mut projects = self.project_groups();
        self.sort_project_groups(&mut projects);
        for (project_key, project, mut indices) in projects {
            indices.retain(|idx| self.sidebar_filter_matches(*idx, &project));
            if indices.is_empty() { continue; }
            self.sort_project_worktrees(&project_key, &mut indices);
            out.push(SidebarRow::ProjectHeader { key: project_key.clone(), label: project });
            // Filtering temporarily reveals matching worktrees inside collapsed
            // projects. Otherwise the matching header would be followed by a
            // misleading "no matches" row with the result still hidden.
            if self.sidebar_filter.trim().is_empty() && self.collapsed_projects.contains(&project_key) { continue; }
            for idx in indices {
                out.push(SidebarRow::Worktree(idx));
                if let Some(branch) = layout.sessions.get(idx).and_then(|s| s.branch.clone()) {
                    out.push(SidebarRow::Branch(idx, branch));
                }
            }
        }
        if !self.sidebar_filter.trim().is_empty() && !out.iter().any(|row| matches!(row, SidebarRow::Worktree(_))) {
            out.push(SidebarRow::Dim(format!("no matches for \"{}\"", self.sidebar_filter)));
        }
        out
    }

    fn sort_project_groups(&self, projects: &mut [(String, String, Vec<usize>)]) {
        if self.project_order.is_empty() {
            projects.sort_by_key(|(_, _, indices)| {
                indices
                    .iter()
                    .map(|idx| self.project_attention(*idx))
                    .min()
                    .unwrap_or(u8::MAX)
            });
        } else {
            projects.sort_by_key(|(key, _, _)| self.project_order.iter().position(|saved| saved == key).unwrap_or(usize::MAX));
        }
    }

    fn sort_project_worktrees(&self, project_key: &str, indices: &mut [usize]) {
        if let Some(order) = self.worktree_order.get(project_key).filter(|order| !order.is_empty()) {
            indices.sort_by_key(|idx| {
                self.layout
                    .as_ref()
                    .and_then(|layout| layout.sessions.get(*idx))
                    .map(session_worktree_key)
                    .and_then(|key| order.iter().position(|saved| saved == &key))
                    .unwrap_or(usize::MAX)
            });
        } else {
            indices.sort_by_key(|idx| self.project_attention(*idx));
        }
    }

    pub(super) fn is_last_project_worktree(&self, idx: usize) -> bool {
        let Some(session) = self.layout.as_ref().and_then(|layout| layout.sessions.get(idx)) else { return false };
        let project_key = session_project_key(session);
        let Some((_, _, mut indices)) = self.project_groups().into_iter().find(|(key, _, _)| *key == project_key) else {
            return false;
        };
        self.sort_project_worktrees(&project_key, &mut indices);
        indices.last().copied() == Some(idx)
    }

    fn sidebar_filter_matches(&self, idx: usize, project: &str) -> bool {
        let query = self.sidebar_filter.trim().to_lowercase();
        if query.is_empty() { return true; }
        let Some(session) = self.layout.as_ref().and_then(|l| l.sessions.get(idx)) else { return false; };
        let mut haystack = format!("{} {} {}", project, session.name, session.workspace.display()).to_lowercase();
        if let Some(branch) = &session.branch { haystack.push_str(&format!(" {}", branch.name)); }
        for (_, pane) in session_panes_all(session) {
            if let Some(agent) = &pane.agent { haystack.push_str(&format!(" {}", agent.name)); }
        }
        haystack.contains(&query)
    }

    fn project_attention(&self, idx: usize) -> u8 {
        let status = self.worktree_primary_status(idx);
        if matches!(status, Some(AgentStatus::Blocked)) { return 0; }
        if matches!(status, Some(AgentStatus::Done)) { return 1; }
        match status {
            Some(AgentStatus::Working) => 2,
            _ if matches!(status, Some(AgentStatus::Idle)) => 3,
            _ => 4,
        }
    }

    pub(super) fn agent_rank(status: AgentStatus) -> u8 {
        match status {
            AgentStatus::Blocked => 0,
            AgentStatus::Done => 1,
            AgentStatus::Working => 2,
            AgentStatus::Idle => 3,
            AgentStatus::Unknown => 4,
        }
    }

    /// `(session name, tab name)` of the pane `pid` in session `i`, for the
    /// inline agent row labels.
    pub(super) fn agent_pane_location(layout: Option<&Layout>, i: usize, pid: u64) -> (String, String) {
        layout
            .and_then(|l| l.sessions.get(i))
            .map(|s| {
                let space = s.name.clone();
                let pane = s
                    .tabs
                    .iter()
                    .find(|t| Self::find_layout_pane_in_tab(t, pid).is_some())
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                (space, pane)
            })
            .unwrap_or_default()
    }

    /// Every AI-pane entry across all sessions, with its canonical sort key:
    /// `(rank, session_idx, pane_id, status, agent_name)`, ordered blocked →
    /// done → running → idle → unknown, then session, then pane.
    pub(super) fn all_agent_entries(&self) -> Vec<(u8, usize, u64, AgentStatus, String)> {
        let mut out = Vec::new();
        if let Some(layout) = &self.layout {
            for (i, s) in layout.sessions.iter().enumerate() {
                for (_, pane) in session_panes_all(s) {
                    if let Some(agent) = pane.agent.as_ref() {
                        if pane.is_ai {
                            out.push((Self::agent_rank(agent.status), i, pane.id, agent.status, agent.name.clone()));
                        }
                    }
                }
            }
        }
        out.sort_by_key(|(rank, i, pid, _, _)| (*rank, *i, *pid));
        out
    }

    fn agents_content(&self) -> Vec<SidebarRow> {
        let mut out = Vec::new();
        for (_, i, pid, status, name) in self.all_agent_entries() {
            let ws = self
                .layout
                .as_ref()
                .and_then(|l| l.sessions.get(i))
                .map(|s| short_workspace(&s.workspace))
                .unwrap_or_default();
            out.push(SidebarRow::AgentDir(i, pid, ws, status));
            out.push(SidebarRow::AgentName(i, pid, name, status));
        }
        out
    }

    /// The [sidebar] layout the sidebar currently renders with.
    pub(super) fn sidebar_layout(&self) -> SidebarLayout {
        self.sidebar_layout_override.unwrap_or_else(|| kumo_core::config::sidebar().layout)
    }

    /// Valid range of the spaces-panel height in rows: the divider, the agents
    /// label, and at least one agent row must stay visible.
    pub(super) fn spaces_split_max(avail: usize) -> usize {
        avail.saturating_sub(3)
    }

    /// Divided-layout panel heights: the user divider (`split`) sets the
    /// spaces height, clamped to keep at least one agent row; `None` puts the
    /// divider in the exact middle of the content area.
    fn divided_panel_heights(split: Option<u16>, avail: usize) -> (usize, usize) {
        let max_spaces = Self::spaces_split_max(avail);
        let auto = (avail.saturating_sub(2) / 2).min(max_spaces);
        let spaces_h = split.map(|s| (s as usize).min(max_spaces)).unwrap_or(auto);
        (spaces_h, avail.saturating_sub(spaces_h + 2))
    }

    /// Divided-layout sidebar content: both panels' items and their geometry.
    pub(super) fn divided_panels(&self) -> DividedPanels {
        let cfg = kumo_core::config::sidebar();
        let spaces_visible = cfg.sections.sessions;
        let agents_visible = cfg.sections.agents;
        let spaces_items = if spaces_visible { self.sessions_content() } else { Vec::new() };
        let agents_items = if agents_visible { self.agents_panel_items() } else { Vec::new() };
        let avail = self.content_region_h() as usize;
        let (spaces_h, agents_h) = Self::divided_panel_heights(self.sidebar_split, avail);
        let divider_y = if spaces_visible && agents_visible { (3 + spaces_h) as u16 } else { u16::MAX };
        let agents_label_y = if spaces_visible { divider_y + 1 } else { 2 };
        DividedPanels { spaces_visible, agents_visible, spaces_items, agents_items, spaces_h, agents_h, divider_y, agents_label_y }
    }

    /// Agent rows for the divided layout. Grouped: state sections with counts
    /// (blocked · done · running) and a dimmed idle · unknown tail. Ranked:
    /// the classic rank-sorted workspace rows. Both fall back to a placeholder
    /// line when no agent exists.
    pub(super) fn agents_panel_items(&self) -> Vec<SidebarRow> {
        if self.agents_panel_order == AgentPanelOrder::Ranked {
            return self.agents_content();
        }
        type AgentGroup = (AgentStatus, Vec<(usize, u64, String)>);
        let mut out = Vec::new();
        let mut groups: Vec<AgentGroup> = Vec::new();
        for (_, i, pid, status, name) in self.all_agent_entries() {
            match groups.last_mut() {
                Some((st, v)) if *st == status => v.push((i, pid, name)),
                _ => groups.push((status, vec![(i, pid, name)])),
            }
        }
        for (status, entries) in groups {
            out.push(SidebarRow::GroupHeader(status, entries.len()));
            for (i, pid, name) in entries {
                out.push(SidebarRow::AgentName(i, pid, name, status));
            }
        }
        if out.is_empty() {
            out.push(SidebarRow::Dim("no agents".to_string()));
        }
        out
    }

    fn effective_sidebar_tab(&self) -> SidebarTab {
        let visible = self.visible_sidebar_tabs();
        if visible.contains(&self.sidebar_tab) { self.sidebar_tab } else { visible[0] }
    }

    pub(super) fn active_tab_items(&self) -> Vec<SidebarRow> {
        match self.effective_sidebar_tab() {
            SidebarTab::Sessions => self.sessions_content(),
            SidebarTab::Agents => self.agents_content(),
        }
    }

    pub(super) fn active_scroll(&self) -> u16 {
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

    pub(super) fn sidebar_rows_base(&self) -> Vec<(u16, SidebarRow)> {
        let mut out = vec![
            (0, SidebarRow::Header("kumo".into())),
            (1, SidebarRow::Spacer),
        ];
        let region_h = self.content_region_h() as usize;
        match self.sidebar_layout() {
            SidebarLayout::Tabs => {
                let items = self.active_tab_items();
                let offset = (self.active_scroll() as usize).min(items.len().saturating_sub(region_h));
                for (i, item) in items.iter().skip(offset).take(region_h).enumerate() {
                    out.push((3 + i as u16, item.clone()));
                }
            }
            SidebarLayout::Divided => {
                let p = self.divided_panels();
                if p.spaces_visible {
                    out.push((2, SidebarRow::PanelLabel("spaces".to_string(), None)));
                    let max = p.spaces_items.len().saturating_sub(p.spaces_h);
                    let off = (self.sidebar_scroll.0 as usize).min(max);
                    for (i, item) in p.spaces_items.iter().skip(off).take(p.spaces_h).enumerate() {
                        out.push((3 + i as u16, item.clone()));
                    }
                }
                if p.spaces_visible && p.agents_visible {
                    out.push((p.divider_y, SidebarRow::Divider));
                }
                if p.agents_visible {
                    let desc = match self.agents_panel_order {
                        AgentPanelOrder::Grouped => "grouped",
                        AgentPanelOrder::Ranked => "ranked",
                    };
                    out.push((
                        p.agents_label_y,
                        SidebarRow::PanelLabel("agents".to_string(), Some(desc.to_string())),
                    ));
                    let max = p.agents_items.len().saturating_sub(p.agents_h);
                    let off = (self.sidebar_scroll.1 as usize).min(max);
                    for (i, item) in p.agents_items.iter().skip(off).take(p.agents_h).enumerate() {
                        out.push((p.agents_label_y + 1 + i as u16, item.clone()));
                    }
                }
            }
            SidebarLayout::Project => {
                // Search and project label stay at the top. The independently
                // scrolling project tree ends above the fixed New Session and
                // bottom-anchored compact agent inbox.
                out[1] = (1, SidebarRow::Search);
                let geometry = self.project_sidebar_geometry();
                let no_sessions = self.layout.as_ref().map(|layout| layout.sessions.is_empty()).unwrap_or(true);
                let items = if no_sessions && geometry.spaces_h < 2 {
                    vec![SidebarRow::Dim("No sessions · press c".to_string())]
                } else {
                    self.project_content()
                };
                let space_count = items.iter().filter(|row| matches!(row, SidebarRow::Worktree(..))).count();
                if geometry.tiny {
                    out.clear();
                    if geometry.new_session_y > 0 {
                        out.push((0, SidebarRow::Header("kumo".into())));
                    }
                    if geometry.agents_header_y > 0 {
                        out.push((geometry.new_session_y, SidebarRow::NewSession));
                    }
                    let (blocked, done, working) = self.agent_counts();
                    out.push((geometry.agents_header_y, SidebarRow::AgentInboxHeader { blocked, done, working }));
                    return out;
                }
                out.push((2, SidebarRow::SectionLabel("SPACES".to_string(), Some(space_count.to_string()))));
                let mut offset = (self.sidebar_scroll.0 as usize).min(items.len().saturating_sub(geometry.spaces_h));
                if offset > 0 && matches!(items.get(offset), Some(SidebarRow::Branch(..))) {
                    offset -= 1;
                }
                let mut end = (offset + geometry.spaces_h).min(items.len());
                if geometry.spaces_h >= 2
                    && end < items.len()
                    && matches!(items.get(end.saturating_sub(1)), Some(SidebarRow::Worktree(..)))
                    && matches!(items.get(end), Some(SidebarRow::Branch(..)))
                {
                    end = end.saturating_sub(1);
                }
                for (i, item) in items[offset..end].iter().enumerate() {
                    out.push((3 + i as u16, item.clone()));
                }
                out.push((geometry.new_session_y, SidebarRow::NewSession));
                out.push((geometry.divider_y, SidebarRow::Divider));
                let (blocked, done, working) = self.agent_counts();
                out.push((geometry.agents_header_y, SidebarRow::AgentInboxHeader { blocked, done, working }));
                for (i, (session, pane, name, status)) in self
                    .compact_agent_entries()
                    .into_iter()
                    .take(geometry.agent_rows)
                    .enumerate()
                {
                    out.push((
                        geometry.agents_header_y + 1 + i as u16,
                        SidebarRow::CompactAgent(session, pane, name, status),
                    ));
                }
            }
        }
        out
    }

    pub(super) fn sidebar_rows(&self) -> Vec<(u16, SidebarRow)> {
        let mut rows = self.sidebar_rows_base();
        let insertion_y = self
            .sidebar_reorder_drag
            .as_ref()
            .filter(|drag| drag.moved)
            .and_then(|drag| self.sidebar_reorder_insertion_y(drag, &rows));
        if let Some(insertion_y) = insertion_y {
            let project_footer = (self.sidebar_layout() == SidebarLayout::Project)
                .then(|| self.project_sidebar_geometry().new_session_y);
            for (y, _) in &mut rows {
                if *y >= insertion_y && project_footer.map(|footer| *y < footer).unwrap_or(true) {
                    *y = y.saturating_add(1);
                }
            }
            if let Some(footer) = project_footer {
                rows.retain(|(y, row)| {
                    *y < footer
                        || matches!(
                            row,
                            SidebarRow::NewSession
                                | SidebarRow::Divider
                                | SidebarRow::AgentInboxHeader { .. }
                                | SidebarRow::CompactAgent(..)
                        )
                });
            }
            rows.push((insertion_y, SidebarRow::DropIndicator));
            rows.sort_by_key(|(y, _)| *y);
        }
        rows
    }

    pub(super) fn sidebar_wheel(&mut self, x: u16, y: u16, up: bool) -> bool {
        let w = self.effective_sidebar_width();
        if !self.sidebar_open || x >= w {
            return false;
        }
        if y < 3 || y > self.sidebar_footer_y() {
            return false;
        }
        const STEP: usize = 3;
        match self.sidebar_layout() {
            SidebarLayout::Tabs => {
                let max = self.active_scroll_max() as usize;
                let scroll = if up {
                    (self.active_scroll() as usize).saturating_sub(STEP)
                } else {
                    (self.active_scroll() as usize).saturating_add(STEP).min(max)
                };
                self.set_active_scroll(scroll as u16);
                self.mark_dirty();
                true
            }
            SidebarLayout::Divided => {
                let p = self.divided_panels();
                if p.agents_visible && y >= p.agents_label_y {
                    let shown = p.agents_h;
                    let max = p.agents_items.len().saturating_sub(shown);
                    let next = if up {
                        (self.sidebar_scroll.1 as usize).saturating_sub(STEP)
                    } else {
                        (self.sidebar_scroll.1 as usize).saturating_add(STEP).min(max)
                    };
                    self.sidebar_scroll.1 = next as u16;
                } else if p.spaces_visible {
                    let max = p.spaces_items.len().saturating_sub(p.spaces_h);
                    let next = if up {
                        (self.sidebar_scroll.0 as usize).saturating_sub(STEP)
                    } else {
                        (self.sidebar_scroll.0 as usize).saturating_add(STEP).min(max)
                    };
                    self.sidebar_scroll.0 = next as u16;
                } else {
                    return false;
                }
                self.mark_dirty();
                true
            }
            SidebarLayout::Project => {
                if y >= self.project_sidebar_geometry().new_session_y {
                    return false;
                }
                let items = self.project_content();
                let region_h = self.content_region_h() as usize;
                let max = items.len().saturating_sub(region_h);
                let next = if up {
                    (self.sidebar_scroll.0 as usize).saturating_sub(STEP)
                } else {
                    (self.sidebar_scroll.0 as usize).saturating_add(STEP).min(max)
                };
                self.sidebar_scroll.0 = next as u16;
                self.mark_dirty();
                true
            }
        }
    }

    pub(super) fn begin_sidebar_reorder(&mut self, y: u16) -> bool {
        if self.sidebar_layout() != SidebarLayout::Project
            || self.sidebar_filter_active
            || !self.sidebar_filter.trim().is_empty()
        {
            return false;
        }
        let row = self.sidebar_rows().into_iter().find(|(ry, _)| *ry == y).map(|(_, row)| row);
        let source = match row {
            Some(SidebarRow::ProjectHeader { key, .. }) => SidebarReorderItem::Project(key),
            Some(SidebarRow::Worktree(session_idx)) => {
                let Some(session) = self.layout.as_ref().and_then(|layout| layout.sessions.get(session_idx)) else {
                    return false;
                };
                SidebarReorderItem::Worktree {
                    project: session_project_key(session),
                    worktree: session_worktree_key(session),
                    session_idx,
                }
            }
            _ => return false,
        };
        let target = match &source {
            SidebarReorderItem::Project(key) => SidebarReorderTarget::Project(key.clone()),
            SidebarReorderItem::Worktree { project, worktree, .. } => SidebarReorderTarget::Worktree {
                project: project.clone(),
                worktree: worktree.clone(),
            },
        };
        self.sidebar_reorder_drag = Some(SidebarReorderDrag {
            source,
            source_y: y,
            target,
            after: false,
            moved: false,
        });
        self.mark_dirty();
        true
    }

    fn sidebar_reorder_target_at(&self, source: &SidebarReorderItem, y: u16) -> Option<(SidebarReorderTarget, u16)> {
        let mut project: Option<(String, u16)> = None;
        let mut worktree: Option<(String, String, u16)> = None;
        let rows = self.sidebar_rows();
        if rows.iter().any(|(row_y, row)| *row_y == y && matches!(row, SidebarRow::DropIndicator)) {
            return None;
        }
        for (row_y, row) in rows {
            if row_y > y { break; }
            match row {
                SidebarRow::ProjectHeader { key, .. } => {
                    project = Some((key, row_y));
                    worktree = None;
                }
                SidebarRow::Worktree(idx) => {
                    if let Some(session) = self.layout.as_ref().and_then(|layout| layout.sessions.get(idx)) {
                        worktree = Some((session_project_key(session), session_worktree_key(session), row_y));
                    }
                }
                _ => {}
            }
        }
        match source {
            SidebarReorderItem::Project(_) => {
                project.map(|(key, row_y)| (SidebarReorderTarget::Project(key), row_y))
            }
            SidebarReorderItem::Worktree { project: source_project, .. } => {
                worktree
                    .filter(|(project, _, _)| project == source_project)
                    .map(|(project, worktree, row_y)| {
                        (SidebarReorderTarget::Worktree { project, worktree }, row_y)
                    })
            }
        }
    }

    pub(super) fn update_sidebar_reorder(&mut self, y: u16) -> bool {
        let Some(source) = self.sidebar_reorder_drag.as_ref().map(|drag| drag.source.clone()) else {
            return false;
        };
        let target = self.sidebar_reorder_target_at(&source, y);
        if let Some(drag) = self.sidebar_reorder_drag.as_mut() {
            drag.moved = true;
            if let Some((target, target_y)) = target {
                drag.after = target_y > drag.source_y;
                drag.target = target;
            }
        }
        self.mark_dirty();
        true
    }

    pub(super) fn finish_sidebar_reorder(&mut self) -> bool {
        let Some(drag) = self.sidebar_reorder_drag.take() else { return false };
        if !drag.moved {
            match drag.source {
                SidebarReorderItem::Project(key) => self.toggle_project_collapsed(&key),
                SidebarReorderItem::Worktree { session_idx, .. } => {
                    if let Some(name) = self.layout.as_ref().and_then(|layout| layout.sessions.get(session_idx)).map(|s| s.name.clone()) {
                        let _ = self.send(&Command::SessionFocus { name });
                    }
                }
            }
            self.mark_dirty();
            return true;
        }

        let changed = match (drag.source, drag.target) {
            (SidebarReorderItem::Project(source), SidebarReorderTarget::Project(target)) => {
                let mut projects = self.project_groups();
                self.sort_project_groups(&mut projects);
                let mut order: Vec<String> = projects.into_iter().map(|(key, _, _)| key).collect();
                let changed = move_sidebar_item(&mut order, &source, &target, drag.after);
                if changed { self.project_order = order; }
                changed
            }
            (
                SidebarReorderItem::Worktree { project, worktree: source, .. },
                SidebarReorderTarget::Worktree { project: target_project, worktree: target },
            ) if project == target_project => {
                let mut indices = self
                    .project_groups()
                    .into_iter()
                    .find(|(key, _, _)| key == &project)
                    .map(|(_, _, indices)| indices)
                    .unwrap_or_default();
                self.sort_project_worktrees(&project, &mut indices);
                let mut order: Vec<String> = indices
                    .into_iter()
                    .filter_map(|idx| self.layout.as_ref()?.sessions.get(idx).map(session_worktree_key))
                    .collect();
                let changed = move_sidebar_item(&mut order, &source, &target, drag.after);
                if changed { self.worktree_order.insert(project, order); }
                changed
            }
            _ => false,
        };
        if changed {
            if let Err(e) = kumo_core::ui_state::set_sidebar_order(self.project_order.clone(), self.worktree_order.clone()) {
                self.notice = Some((format!("could not save sidebar order: {e}"), Instant::now()));
            }
        }
        self.mark_dirty();
        true
    }

    pub(super) fn sidebar_reorder_insertion_y(&self, drag: &SidebarReorderDrag, rows: &[(u16, SidebarRow)]) -> Option<u16> {
        let target_idx = rows.iter().position(|(_, row)| match (&drag.target, row) {
            (SidebarReorderTarget::Project(target), SidebarRow::ProjectHeader { key, .. }) => target == key,
            (SidebarReorderTarget::Worktree { project, worktree }, SidebarRow::Worktree(idx)) => self
                .layout
                .as_ref()
                .and_then(|layout| layout.sessions.get(*idx))
                .map(|session| session_project_key(session) == *project && session_worktree_key(session) == *worktree)
                .unwrap_or(false),
            _ => false,
        })?;
        if !drag.after {
            return Some(rows[target_idx].0);
        }
        for (y, row) in rows.iter().skip(target_idx + 1) {
            let boundary = match &drag.target {
                SidebarReorderTarget::Project(_) => {
                    matches!(row, SidebarRow::ProjectHeader { .. } | SidebarRow::NewSession)
                }
                SidebarReorderTarget::Worktree { project, .. } => match row {
                    SidebarRow::Worktree(idx) => self
                        .layout
                        .as_ref()
                        .and_then(|layout| layout.sessions.get(*idx))
                        .map(|session| session_project_key(session) == *project)
                        .unwrap_or(false),
                    SidebarRow::ProjectHeader { .. } | SidebarRow::NewSession => true,
                    _ => false,
                },
            };
            if boundary {
                return Some(if self.sidebar_layout() == SidebarLayout::Project && matches!(row, SidebarRow::NewSession) {
                    y.saturating_sub(1)
                } else {
                    *y
                });
            }
        }
        Some(if self.sidebar_layout() == SidebarLayout::Project {
            self.project_sidebar_geometry().new_session_y.saturating_sub(1)
        } else {
            self.sidebar_footer_y()
        })
    }

    fn toggle_project_collapsed(&mut self, key: &str) {
        let collapsed = !self.collapsed_projects.contains(key);
        if collapsed { self.collapsed_projects.insert(key.to_string()); } else { self.collapsed_projects.remove(key); }
        if let Some(path) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.sessions.iter().find(|session| session_project_key(session) == key))
            .map(|session| session.project_root.clone().unwrap_or_else(|| session.workspace.clone()))
        {
            if let Err(e) = kumo_core::ui_state::set_project_collapsed(&path, collapsed) {
                self.notice = Some((format!("could not save sidebar state: {e}"), Instant::now()));
            }
        }
    }

    pub(super) fn sidebar_hit(&mut self, x: u16, y: u16) -> bool {
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
                    // A click while the inbox owns the keyboard ends the focus
                    // mode (the jump replaces the cursor).
                    if self.mode == Mode::Inbox {
                        self.close_inbox();
                    }
                    return true;
                }
                SidebarRow::PanelLabel(title, Some(right)) if title == "agents" => {
                    // Clicking the right-hand sort descriptor toggles the
                    // agent panel between grouped and ranked ordering.
                    let rw = right.chars().count() as u16;
                    let w = self.effective_sidebar_width();
                    let max_r = w.saturating_sub(1).max(1);
                    if x >= max_r.saturating_sub(rw) && x < max_r {
                        self.agents_panel_order = match self.agents_panel_order {
                            AgentPanelOrder::Grouped => AgentPanelOrder::Ranked,
                            AgentPanelOrder::Ranked => AgentPanelOrder::Grouped,
                        };
                        self.mark_dirty();
                        return true;
                    }
                    return false;
                }
                SidebarRow::NewSession => {
                    self.open_session_popup();
                    return true;
                }
                SidebarRow::Divider => {
                    if self.sidebar_layout() == SidebarLayout::Divided {
                        // The legacy divided layout keeps its adjustable split;
                        // the project inbox divider is deliberately fixed.
                        let spaces_h = self.divided_panels().spaces_h as u16;
                        self.sidebar_drag =
                            Some(SidebarDrag { start_y: y, start_split: self.sidebar_split.unwrap_or(spaces_h) });
                        self.mark_dirty();
                    }
                    return true;
                }
                SidebarRow::Search => {
                    self.sidebar_filter_active = true;
                    self.mark_dirty();
                    return true;
                }
                SidebarRow::Worktree(i) => {
                    let name = self.layout.as_ref().and_then(|l| l.sessions.get(i)).map(|s| s.name.clone());
                    if let Some(name) = name {
                        let _ = self.send(&Command::SessionFocus { name });
                    }
                    return true;
                }
                SidebarRow::ProjectHeader { key, .. } => {
                    self.toggle_project_collapsed(&key);
                    self.mark_dirty();
                    return true;
                }
                SidebarRow::InlineAgent(i, pid, _, _) => {
                    let name = self.layout.as_ref().and_then(|l| l.sessions.get(i)).map(|s| s.name.clone());
                    if let Some(name) = name {
                        let _ = self.send(&Command::PaneFocus { session: name, pane_id: pid });
                    }
                    if self.mode == Mode::Inbox {
                        self.close_inbox();
                    }
                    return true;
                }
                SidebarRow::AgentInboxHeader { .. } => {
                    self.open_inbox();
                    return true;
                }
                SidebarRow::CompactAgent(i, pid, _, _) => {
                    let name = self.layout.as_ref().and_then(|l| l.sessions.get(i)).map(|s| s.name.clone());
                    if let Some(name) = name {
                        let _ = self.send(&Command::PaneFocus { session: name, pane_id: pid });
                    }
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    pub(super) fn sidebar_session_at(&self, x: u16, y: u16) -> Option<usize> {
        let w = self.effective_sidebar_width();
        if !self.sidebar_open || x >= w {
            return None;
        }
        self.sidebar_rows()
            .into_iter()
            .find(|(ry, _)| *ry == y)
            .and_then(|(_, row)| match row {
                SidebarRow::Session(i) => Some(i),
                SidebarRow::Branch(i, _) => Some(i),
                SidebarRow::Worktree(i) => Some(i),
                _ => None,
            })
    }
}
