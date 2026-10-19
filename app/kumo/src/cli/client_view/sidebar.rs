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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::client_view::tests::*;

    #[test]
    fn agents_panel_groups_by_state_in_order() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Working),
            (2, AgentStatus::Blocked),
            (3, AgentStatus::Idle),
            (4, AgentStatus::Done),
        ]));
        let rows = view.agents_panel_items();
        let heads: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                SidebarRow::GroupHeader(st, n) => Some((*st, *n)),
                _ => None,
            })
            .collect();
        assert_eq!(
            heads,
            vec![
                (AgentStatus::Blocked, 1),
                (AgentStatus::Done, 1),
                (AgentStatus::Working, 1),
                (AgentStatus::Idle, 1),
            ]
        );
        let entries: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                SidebarRow::AgentName(_, pid, _, _) => Some(*pid),
                _ => None,
            })
            .collect();
        assert_eq!(entries, vec![2, 4, 1, 3], "entries follow group order");
        assert!(
            rows.iter().all(|r| !matches!(r, SidebarRow::AgentDir(..))),
            "grouped panel has no workspace-dir rows"
        );
    }

    #[test]
    fn agents_panel_placeholder_when_no_agents() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[]));
        assert_eq!(view.agents_panel_items(), vec![SidebarRow::Dim("no agents".to_string())]);
    }

    #[test]
    fn project_sidebar_anchors_compact_agent_inbox_below_new_session() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Idle),
            (2, AgentStatus::Blocked),
            (3, AgentStatus::Working),
            (4, AgentStatus::Done),
        ]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();
        let rows = view.sidebar_rows();
        assert!(rows.iter().any(|(y, row)| *y == geometry.new_session_y && matches!(row, SidebarRow::NewSession)));
        assert!(rows.iter().any(|(y, row)| *y == geometry.divider_y && matches!(row, SidebarRow::Divider)));
        assert!(rows.iter().any(|(y, row)| {
            *y == geometry.agents_header_y
                && matches!(row, SidebarRow::AgentInboxHeader { blocked: 1, done: 1, working: 1 })
        }));
        let compact: Vec<_> = rows
            .iter()
            .filter_map(|(_, row)| match row {
                SidebarRow::CompactAgent(_, pid, _, status) => Some((*pid, *status)),
                _ => None,
            })
            .collect();
        assert_eq!(
            compact,
            vec![
                (2, AgentStatus::Blocked),
                (4, AgentStatus::Done),
                (3, AgentStatus::Working),
            ]
        );
        assert!(rows.iter().all(|(_, row)| !matches!(row, SidebarRow::InlineAgent(..))));

        view.sidebar_scroll.0 = u16::MAX;
        let scrolled = view.sidebar_rows();
        assert!(scrolled.iter().any(|(y, row)| *y == geometry.new_session_y && matches!(row, SidebarRow::NewSession)));
        assert!(scrolled.iter().any(|(y, row)| *y == geometry.agents_header_y && matches!(row, SidebarRow::AgentInboxHeader { .. })));
    }

    #[test]
    fn project_sidebar_renders_contextual_shortcuts_without_covering_counters() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Blocked),
            (2, AgentStatus::Done),
            (3, AgentStatus::Working),
        ]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        view.cols = 120;
        view.sidebar_width = 28;
        view.recompute_geometry();

        let geometry = view.project_sidebar_geometry();
        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| {
            let area = f.area();
            view.render_sidebar(f, area);
        }).unwrap();
        let buffer = term.backend().buffer();
        let hint_x = view.effective_sidebar_width() - 2;
        assert_eq!(buffer.cell((hint_x, geometry.new_session_y)).unwrap().symbol(), "c");
        assert_eq!(buffer.cell((hint_x, geometry.agents_header_y)).unwrap().symbol(), "i");
        let header: String = (0..hint_x).map(|x| buffer.cell((x, geometry.agents_header_y)).unwrap().symbol().to_string()).collect();
        assert!(header.contains("!1"), "blocked counter missing: {header:?}");
        assert!(header.contains("✓1"), "done counter missing: {header:?}");
        assert!(view.sidebar_hit(hint_x, geometry.new_session_y));
        assert!(view.popup.open, "clicking the contextual new-session hint keeps the row target");
        view.popup.open = false;
        assert!(view.sidebar_hit(hint_x, geometry.agents_header_y));
        assert_eq!(view.mode, Mode::Inbox, "clicking the contextual inbox hint keeps the row target");
    }

    #[test]
    fn project_sidebar_hides_inbox_shortcut_when_width_is_tight() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Blocked),
            (2, AgentStatus::Done),
            (3, AgentStatus::Working),
        ]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        view.cols = 21;
        view.sidebar_width = 21;
        view.recompute_geometry();

        let geometry = view.project_sidebar_geometry();
        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| {
            let area = f.area();
            view.render_sidebar(f, area);
        }).unwrap();
        let buffer = term.backend().buffer();
        let hint_x = view.effective_sidebar_width() - 2;
        assert_eq!(buffer.cell((hint_x, geometry.new_session_y)).unwrap().symbol(), "c");
        assert_ne!(buffer.cell((hint_x, geometry.agents_header_y)).unwrap().symbol(), "i");
        let header: String = (0..hint_x).map(|x| buffer.cell((x, geometry.agents_header_y)).unwrap().symbol().to_string()).collect();
        assert!(header.contains("!1"), "blocked counter remains visible at narrow width: {header:?}");
        assert!(header.contains("✓1"), "done counter remains visible at narrow width: {header:?}");
    }

    #[test]
    fn short_project_sidebar_reduces_agent_inbox_to_header() {
        let mut view = test_view();
        view.rows = 12;
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked), (2, AgentStatus::Done)]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        assert_eq!(view.project_sidebar_geometry().agent_rows, 0);
        let rows = view.sidebar_rows();
        assert!(rows.iter().any(|(_, row)| matches!(row, SidebarRow::AgentInboxHeader { .. })));
        assert!(rows.iter().all(|(_, row)| !matches!(row, SidebarRow::CompactAgent(..))));
    }

    #[test]
    fn project_sidebar_empty_state_keeps_new_session_fixed_below_two_lines() {
        let mut view = test_view();
        view.layout = Some(Layout { active: None, sessions: Vec::new() });
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();
        assert!(geometry.spaces_h >= 2);

        let rows = view.sidebar_rows();
        assert!(rows.iter().any(|(_, row)| matches!(row, SidebarRow::Dim(label) if label == "No sessions yet")));
        assert!(rows.iter().any(|(_, row)| matches!(row, SidebarRow::Dim(label) if label == "press c to create one")));
        assert!(rows.iter().any(|(y, row)| *y == geometry.new_session_y && matches!(row, SidebarRow::NewSession)));

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buffer = term.backend().buffer();
        let line = |y| (0..view.effective_sidebar_width()).map(|x| buffer.cell((x, y)).unwrap().symbol().to_string()).collect::<String>();
        assert!(line(3).contains("No sessions yet"));
        assert!(line(4).contains("press c to create one"));
    }

    #[test]
    fn project_sidebar_empty_state_falls_back_to_one_line_at_low_height() {
        let mut view = test_view();
        view.rows = 8;
        view.layout = Some(Layout { active: None, sessions: Vec::new() });
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();
        assert_eq!(geometry.spaces_h, 1);

        let rows = view.sidebar_rows();
        assert!(rows.iter().any(|(_, row)| matches!(row, SidebarRow::Dim(label) if label == "No sessions · press c")));
        assert!(rows.iter().any(|(y, row)| *y == geometry.new_session_y && matches!(row, SidebarRow::NewSession)));
        assert!(rows.iter().any(|(y, row)| *y == geometry.agents_header_y && matches!(row, SidebarRow::AgentInboxHeader { .. })));
    }

    #[test]
    fn compact_agent_header_shows_all_clear_for_idle_only_agents() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Idle), (2, AgentStatus::Unknown)]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();
        assert_eq!(view.compact_agent_summary(), "all clear");

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buffer = term.backend().buffer();
        let line: String = (0..view.effective_sidebar_width()).map(|x| buffer.cell((x, geometry.agents_header_y)).unwrap().symbol().to_string()).collect();
        assert!(line.contains("AGENTS"));
        assert!(line.contains("all clear"));
        assert!(!line.contains("no agents"));
    }

    #[test]
    fn compact_agent_header_keeps_attention_counters() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked), (2, AgentStatus::Done), (3, AgentStatus::Working)]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();
        assert_eq!(view.compact_agent_summary(), "!1 ✓1 ⠋1");

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buffer = term.backend().buffer();
        let line: String = (0..view.effective_sidebar_width()).map(|x| buffer.cell((x, geometry.agents_header_y)).unwrap().symbol().to_string()).collect();
        assert!(line.contains("!1 ✓1 ⠋1"));
    }

    #[test]
    fn compact_agent_header_stays_separate_from_title_at_narrow_width() {
        let mut view = test_view();
        view.sidebar_width = MIN_SIDEBAR_WIDTH;
        view.layout = Some(Layout { active: None, sessions: Vec::new() });
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buffer = term.backend().buffer();
        let title: String = (2..8).map(|x| buffer.cell((x, geometry.agents_header_y)).unwrap().symbol().to_string()).collect();
        let summary: String = (10..19).map(|x| buffer.cell((x, geometry.agents_header_y)).unwrap().symbol().to_string()).collect();
        assert_eq!(title, "AGENTS");
        assert_eq!(summary, "no agents");
    }

    #[test]
    fn compact_agent_inbox_caps_rows_and_has_a_fixed_divider() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[
            (1, AgentStatus::Blocked),
            (2, AgentStatus::Blocked),
            (3, AgentStatus::Done),
            (4, AgentStatus::Done),
        ]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let geometry = view.project_sidebar_geometry();
        assert_eq!(geometry.agent_rows, 3);
        assert_eq!(
            view.sidebar_rows().iter().filter(|(_, row)| matches!(row, SidebarRow::CompactAgent(..))).count(),
            3
        );
        assert!(view.sidebar_hit(0, geometry.divider_y));
        assert!(view.sidebar_drag.is_none(), "project inbox divider is not draggable");
    }

    #[test]
    fn working_agents_fill_available_compact_inbox_rows() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Working), (2, AgentStatus::Idle)]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let compact: Vec<_> = view
            .sidebar_rows()
            .into_iter()
            .filter_map(|(_, row)| match row {
                SidebarRow::CompactAgent(_, pid, _, status) => Some((pid, status)),
                _ => None,
            })
            .collect();
        assert_eq!(compact, vec![(1, AgentStatus::Working)]);
    }

    #[test]
    fn clicking_compact_agent_header_opens_full_inbox() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Working)]));
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        let y = view.project_sidebar_geometry().agents_header_y;
        assert!(view.sidebar_hit(2, y));
        assert_eq!(view.mode, Mode::Inbox);
        assert!(view.agent_inbox_rect().is_some());
    }

    #[test]
    fn tiny_project_sidebar_rows_never_overlap() {
        for height in 2..7 {
            let mut view = test_view();
            view.rows = height;
            view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
            view.sidebar_layout_override = Some(SidebarLayout::Project);
            let rows = view.sidebar_rows();
            let mut ys: Vec<_> = rows.iter().map(|(y, _)| *y).collect();
            ys.sort_unstable();
            ys.dedup();
            assert_eq!(ys.len(), rows.len(), "duplicate sidebar rows at height {height}");
        }
    }

    #[test]
    fn one_row_spaces_view_keeps_worktree_visible_as_compact_fallback() {
        let mut view = test_view();
        view.rows = 8;
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].branch = Some(WireBranch { name: "main".into(), ahead: 0, behind: 0 });
        view.layout = Some(layout);
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        assert_eq!(view.project_sidebar_geometry().spaces_h, 1);
        view.sidebar_scroll.0 = 1;
        assert!(view.sidebar_rows().iter().any(|(_, row)| matches!(row, SidebarRow::Worktree(0))));
    }

    #[test]
    fn divided_sidebar_stacks_panels() {
        let mut view = test_view();
        view.sidebar_layout_override = Some(SidebarLayout::Divided);
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        let rows = view.sidebar_rows();
        let labels: Vec<_> = rows
            .iter()
            .filter_map(|(y, r)| match r {
                SidebarRow::PanelLabel(t, right) => Some((*y, t.clone(), right.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(labels.len(), 2, "both panels labeled");
        assert_eq!(labels[0], (2, "spaces".to_string(), None));
        // Content area is 20 rows: default divider sits at the exact middle
        // (spaces 9 rows), so it lands at 12 and the agents label at 13.
        assert_eq!(labels[1], (13, "agents".to_string(), Some("grouped".to_string())));
        assert!(
            rows.iter().any(|(y, r)| *y == 3 && matches!(r, SidebarRow::Session(0))),
            "spaces content first"
        );
        assert!(rows.iter().any(|(_, r)| matches!(r, SidebarRow::NewSession)));
        assert!(
            rows.iter().any(|(y, r)| *y == 12 && matches!(r, SidebarRow::Divider)),
            "divider between the panels"
        );
        assert!(
            rows.iter().any(|(y, r)| *y == 14 && matches!(r, SidebarRow::GroupHeader(AgentStatus::Blocked, 1))),
            "group header under the agents label"
        );
        assert!(
            rows.iter().any(|(y, r)| *y == 15 && matches!(r, SidebarRow::AgentName(0, 1, _, AgentStatus::Blocked))),
            "entry follows its group header"
        );
    }

    #[test]
    fn divided_divider_drag_resizes_panels() {
        let mut view = test_view();
        view.sidebar_layout_override = Some(SidebarLayout::Divided);
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        let down = |x, y| crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        let drag = |x, y| crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Drag(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        let up = |x, y| crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Up(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        // The default split is the exact middle: 9 spaces rows, divider at 12.
        let rows = view.sidebar_rows();
        assert!(rows.iter().any(|(y, r)| *y == 12 && matches!(r, SidebarRow::Divider)));
        // Press on the divider (y=12), drag down 3 rows, release.
        view.on_mouse(down(10, 12)).unwrap();
        assert!(view.sidebar_drag.is_some(), "press on the divider starts a drag");
        view.on_mouse(drag(10, 15)).unwrap();
        view.on_mouse(up(10, 15)).unwrap();
        assert!(view.sidebar_drag.is_none());
        assert_eq!(view.sidebar_split, Some(12), "spaces grew by the drag delta");
        let rows = view.sidebar_rows();
        assert!(
            rows.iter().any(|(y, r)| *y == 15 && matches!(r, SidebarRow::Divider)),
            "divider moves with the drag"
        );
        assert!(
            rows.iter().any(|(y, r)| *y == 18 && matches!(r, SidebarRow::AgentName(0, 1, _, _))),
            "agent entries follow the moved label"
        );
    }

    #[test]
    fn divided_sidebar_render_places_panel_rows() {
        let mut view = test_view();
        view.sidebar_layout_override = Some(SidebarLayout::Divided);
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        // Short kind so the full `kind · space · pane` entry row fits the
        // sidebar budget.
        if let Some(LayoutNode::Pane(p)) = view
            .layout
            .as_mut()
            .and_then(|l| l.sessions.first_mut())
            .and_then(|s| s.tabs.first_mut())
            .and_then(|t| t.root.as_deref_mut())
        {
            p.agent.as_mut().unwrap().name = "ai".into();
        }
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let row_text = |y: u16| -> String {
            (0..24)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect::<String>()
                .trim()
                .to_string()
        };
        assert_eq!(row_text(2), "spaces", "spaces label at the old tabs row");
        assert!(row_text(3).contains("sess"), "session row under spaces");
        let divider = row_text(12);
        assert!(!divider.is_empty() && divider.chars().all(|c| c == '─'), "divider between the panels: {divider:?}");
        let dstyle = buf.cell((5, 12)).unwrap().style();
        assert_eq!(dstyle.fg, Some(view.current_theme().panel_muted), "divider is muted");
        let agents_label = row_text(13);
        assert!(
            agents_label.starts_with("agents") && agents_label.contains("grouped"),
            "agents label after the divider: {agents_label:?}"
        );
        assert_eq!(row_text(14), "blocked (1)", "group header under the agents label");
        assert_eq!(
            row_text(15),
            "◉ ai · sess · 1",
            "grouped entry carries kind · space (session) · pane: {}",
            row_text(15)
        );
        // Panel labels render in the primary color, bold, with no filled chip.
        for y in [2u16, 13u16] {
            let st = buf.cell((2, y)).unwrap().style();
            assert!(
                st.bg.is_none() || st.bg == Some(RColor::Reset),
                "panel labels carry no filled bg at y={y}"
            );
            assert_ne!(st.fg, Some(RColor::Black), "panel labels are not LEADER-chipped at y={y}");
            assert!(st.add_modifier.contains(Modifier::BOLD), "panel labels are bold at y={y}");
        }
    }

    #[test]
    fn agent_entry_label_fits_the_budget() {
        // Full triple when it fits.
        assert_eq!(agent_entry_label("opencode", "kumo", "1", 19), "opencode · kumo · 1");
        // Pane drops first when the triple overflows.
        assert_eq!(agent_entry_label("opencode", "dev/kumo", "1", 19), "opencode · dev/kumo");
        // Then the space.
        assert_eq!(agent_entry_label("opencode", "dev/kumo", "1", 10), "opencode");
        // The kind is truncated as the last resort, never beyond `avail`.
        assert_eq!(agent_entry_label("opencode", "dev/kumo", "1", 5), "open…");
    }

    #[test]
    fn agents_panel_order_toggles_on_label_click() {
        let mut view = test_view();
        view.sidebar_layout_override = Some(SidebarLayout::Divided);
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked)]));
        assert_eq!(view.agents_panel_order, AgentPanelOrder::Grouped);
        assert!(view.agents_panel_items().iter().any(|r| matches!(r, SidebarRow::GroupHeader(..))));
        // Click the sort descriptor on the agents label row (y=13).
        view.on_mouse(mouse_click(20, 13)).unwrap();
        assert_eq!(view.agents_panel_order, AgentPanelOrder::Ranked);
        let rows = view.agents_panel_items();
        assert!(
            rows.iter().all(|r| !matches!(r, SidebarRow::GroupHeader(..))),
            "ranked ordering shows no state headers"
        );
        assert!(
            rows.iter().any(|r| matches!(r, SidebarRow::AgentDir(..))),
            "ranked ordering keeps workspace-dir rows"
        );
        // Ranked name rows carry the same space · pane inline label.
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();
        // Ranked rows: the workspace row leads with `space · pane` (y=14),
        // the name row below stays plain (y=15).
        let dir_row: String = (0..24).map(|x| buf.cell((x, 14)).unwrap().symbol().to_string()).collect();
        assert_eq!(dir_row.trim(), "sess · 1", "ranked workspace row: {dir_row:?}");
        let name_row: String = (0..24).map(|x| buf.cell((x, 15)).unwrap().symbol().to_string()).collect();
        assert!(
            name_row.trim().starts_with("◉ agent1"),
            "ranked name row: {name_row:?}"
        );
        // Click again to return to grouped.
        view.on_mouse(mouse_click(20, 13)).unwrap();
        assert_eq!(view.agents_panel_order, AgentPanelOrder::Grouped);
    }

    #[test]
    fn tabs_layout_rows_keep_legacy_structure() {
        let mut view = test_view();
        view.sidebar_layout_override = Some(SidebarLayout::Tabs);
        view.sidebar_tab = SidebarTab::Agents;
        view.layout = Some(panes_layout(&[(1, AgentStatus::Blocked), (2, AgentStatus::Working)]));
        let rows = view.sidebar_rows();
        assert!(
            rows.iter().all(|(_, r)| !matches!(r, SidebarRow::PanelLabel(..))),
            "tabs layout renders no panel labels"
        );
        let entries: Vec<_> = rows
            .iter()
            .filter_map(|(_, r)| match r {
                SidebarRow::AgentName(_, pid, _, _) => Some(*pid),
                _ => None,
            })
            .collect();
        assert_eq!(entries, vec![1, 2], "rank order (blocked first)");
        assert_eq!(
            rows.iter().filter(|(_, r)| matches!(r, SidebarRow::AgentDir(..))).count(),
            2,
            "workspace-dir rows kept"
        );
    }

    #[test]
    fn project_sidebar_groups_linked_worktrees_under_main_project() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "main".into();
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/kumo");
        layout.sessions[0].project_root = Some(std::path::PathBuf::from("/tmp/kumo"));
        layout.sessions[0].branch = Some(WireBranch { name: "main".into(), ahead: 2, behind: 1 });
        let mut linked = layout.sessions[0].clone();
        linked.name = "sidebar-uiux".into();
        linked.workspace = std::path::PathBuf::from("/tmp/kumo-sidebar-uiux");
        linked.branch = Some(WireBranch { name: "sidebar-uiux".into(), ahead: 1, behind: 0 });
        layout.sessions.push(linked);
        view.layout = Some(layout);
        let rows = view.project_content();
        let headers: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                SidebarRow::ProjectHeader { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(headers, vec!["kumo"]);
        assert_eq!(rows.iter().filter(|r| matches!(r, SidebarRow::Branch(_, _))).count(), 2);
        assert_eq!(rows.iter().filter(|r| matches!(r, SidebarRow::Worktree(_))).count(), 2);
        let worktrees: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                SidebarRow::Worktree(i) => Some(view.session_name(*i)),
                _ => None,
            })
            .collect();
        assert_eq!(worktrees, vec!["main", "sidebar-uiux"]);
    }

    #[test]
    fn project_sidebar_renders_distinct_hierarchy_levels() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "workspace".into();
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/kumo-workspace");
        layout.sessions[0].project_root = Some(std::path::PathBuf::from("/tmp/kumo"));
        layout.sessions[0].branch = Some(WireBranch { name: "feature/ui".into(), ahead: 0, behind: 0 });
        layout.active = Some("workspace".into());
        view.layout = Some(layout);
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        view.cols = 120;
        view.recompute_geometry();

        let rows = view.sidebar_rows();
        let workspace_y = rows.iter().find_map(|(y, row)| matches!(row, SidebarRow::Worktree(0)).then_some(*y)).unwrap();
        let branch_y = rows.iter().find_map(|(y, row)| matches!(row, SidebarRow::Branch(0, _)).then_some(*y)).unwrap();
        let project_y = rows.iter().find_map(|(y, row)| matches!(row, SidebarRow::ProjectHeader { .. }).then_some(*y)).unwrap();

        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();

        assert_eq!(buf.cell((3, project_y)).unwrap().symbol(), "▰");
        assert_eq!(buf.cell((2, workspace_y)).unwrap().symbol(), "└");
        assert_eq!(buf.cell((2, workspace_y)).unwrap().fg, view.current_theme().panel_muted);
        assert_eq!(buf.cell((4, workspace_y)).unwrap().symbol(), "◆");
        assert_eq!(buf.cell((view.effective_sidebar_width() - 2, workspace_y)).unwrap().symbol(), "●");
        assert_eq!(buf.cell((1, workspace_y)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((5, branch_y)).unwrap().symbol(), "⎇");
        assert!(rows.iter().all(|(_, row)| !matches!(row, SidebarRow::InlineAgent(..))));
        assert_eq!(buf.cell((0, workspace_y)).unwrap().symbol(), "┃");
        assert_eq!(buf.cell((0, branch_y)).unwrap().symbol(), "┃");
    }

    #[test]
    fn project_sidebar_highlights_only_the_active_workspace_path() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.active = Some("active".into());
        layout.sessions[0].name = "active".into();
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/project-active");
        layout.sessions[0].project_root = Some(std::path::PathBuf::from("/tmp/project-active"));
        layout.sessions[0].branch = Some(WireBranch { name: "feature/active".into(), ahead: 0, behind: 0 });

        let mut linked = layout.sessions[0].clone();
        linked.name = "linked".into();
        linked.workspace = std::path::PathBuf::from("/tmp/project-active-linked");
        linked.branch = Some(WireBranch { name: "feature/linked".into(), ahead: 0, behind: 0 });

        let mut other = layout.sessions[0].clone();
        other.name = "other".into();
        other.workspace = std::path::PathBuf::from("/tmp/project-other");
        other.project_root = Some(std::path::PathBuf::from("/tmp/project-other"));
        other.branch = Some(WireBranch { name: "main".into(), ahead: 0, behind: 0 });

        let active_project = session_project_key(&layout.sessions[0]);
        let other_project = session_project_key(&other);
        layout.sessions.extend([linked, other]);
        view.project_order = vec![active_project.clone(), other_project.clone()];
        view.layout = Some(layout);
        view.sidebar_layout_override = Some(SidebarLayout::Project);
        view.cols = 120;
        view.recompute_geometry();

        let rows = view.sidebar_rows();
        let row_y = |predicate: &dyn Fn(&SidebarRow) -> bool| {
            rows.iter().find_map(|(y, row)| predicate(row).then_some(*y)).unwrap()
        };
        let active_project_y = row_y(&|row| matches!(row, SidebarRow::ProjectHeader { key, .. } if key == &active_project));
        let active_worktree_y = row_y(&|row| matches!(row, SidebarRow::Worktree(0)));
        let active_branch_y = row_y(&|row| matches!(row, SidebarRow::Branch(0, _)));
        let other_project_y = row_y(&|row| matches!(row, SidebarRow::ProjectHeader { key, .. } if key == &other_project));
        let other_worktree_y = row_y(&|row| matches!(row, SidebarRow::Worktree(2)));
        let other_branch_y = row_y(&|row| matches!(row, SidebarRow::Branch(2, _)));

        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let active_bg = sidebar_active_bg(&view.current_theme());

        for y in [active_project_y, active_worktree_y, active_branch_y] {
            assert_eq!(buf.cell((0, y)).unwrap().bg, active_bg, "active path background at y={y}");
            assert_eq!(buf.cell((0, y)).unwrap().symbol(), "┃", "active path marker at y={y}");
        }
        assert!(buf.cell((5, active_project_y)).unwrap().modifier.contains(Modifier::BOLD));
        assert!(buf.cell((6, active_worktree_y)).unwrap().modifier.contains(Modifier::BOLD));
        assert!(buf.cell((7, active_branch_y)).unwrap().modifier.contains(Modifier::BOLD));

        for y in [other_project_y, other_worktree_y, other_branch_y] {
            assert_eq!(buf.cell((0, y)).unwrap().bg, RColor::Reset, "inactive path background at y={y}");
            assert_ne!(buf.cell((0, y)).unwrap().symbol(), "┃", "inactive path marker at y={y}");
        }
        assert!(!buf.cell((6, other_worktree_y)).unwrap().modifier.contains(Modifier::BOLD));
        assert!(!buf.cell((7, other_branch_y)).unwrap().modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn project_sidebar_collapse_hides_only_that_project() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "main".into();
        let mut linked = layout.sessions[0].clone();
        linked.name = "other".into();
        linked.workspace = std::path::PathBuf::from("/tmp/other");
        layout.sessions.push(linked);
        view.layout = Some(layout);
        let project = kumo_core::ui_state::canonical_project_key(&view.layout.as_ref().unwrap().sessions[0].workspace);
        view.collapsed_projects.insert(project);
        let rows = view.project_content();
        assert_eq!(rows.iter().filter(|r| matches!(r, SidebarRow::Worktree(_))).count(), 1);
        assert!(rows.iter().any(|r| matches!(r, SidebarRow::ProjectHeader { .. })));
    }

    #[test]
    fn project_sidebar_orders_blocked_worktree_first() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "idle".into();
        let mut blocked = panes_layout(&[(9, AgentStatus::Blocked)]).sessions.remove(0);
        blocked.name = "blocked".into();
        blocked.workspace = layout.sessions[0].workspace.clone();
        layout.sessions.push(blocked);
        view.layout = Some(layout);
        let rows = view.project_content();
        let first = rows.iter().find_map(|r| match r { SidebarRow::Worktree(i) => Some(view.session_name(*i)), _ => None });
        assert_eq!(first.as_deref(), Some("blocked"));
    }

    #[test]
    fn project_sidebar_manual_worktree_order_overrides_attention() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "idle".into();
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/project-idle");
        layout.sessions[0].project_root = Some(std::path::PathBuf::from("/tmp/project"));
        let mut blocked = panes_layout(&[(9, AgentStatus::Blocked)]).sessions.remove(0);
        blocked.name = "blocked".into();
        blocked.workspace = std::path::PathBuf::from("/tmp/project-blocked");
        blocked.project_root = Some(std::path::PathBuf::from("/tmp/project"));
        let project = session_project_key(&layout.sessions[0]);
        view.worktree_order.insert(
            project,
            vec![session_worktree_key(&layout.sessions[0]), session_worktree_key(&blocked)],
        );
        layout.sessions.push(blocked);
        view.layout = Some(layout);

        let first = view.project_content().into_iter().find_map(|row| match row {
            SidebarRow::Worktree(i) => Some(view.session_name(i)),
            _ => None,
        });
        assert_eq!(first.as_deref(), Some("idle"));
    }

    #[test]
    fn project_sidebar_manual_project_order_overrides_attention() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "first".into();
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/first");
        let mut urgent = panes_layout(&[(9, AgentStatus::Blocked)]).sessions.remove(0);
        urgent.name = "urgent".into();
        urgent.workspace = std::path::PathBuf::from("/tmp/urgent");
        view.project_order = vec![session_project_key(&layout.sessions[0]), session_project_key(&urgent)];
        layout.sessions.push(urgent);
        view.layout = Some(layout);

        let first = view.project_content().into_iter().find_map(|row| match row {
            SidebarRow::ProjectHeader { key, .. } => Some(key),
            _ => None,
        });
        assert_eq!(first.as_deref(), Some("/tmp/first"));
    }

    #[test]
    fn sidebar_reorder_moves_before_and_after_target() {
        let mut order = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert!(move_sidebar_item(&mut order, "one", "three", true));
        assert_eq!(order, ["two", "three", "one"]);
        assert!(move_sidebar_item(&mut order, "one", "two", false));
        assert_eq!(order, ["one", "two", "three"]);
        assert!(!move_sidebar_item(&mut order, "one", "one", false));
        assert!(!move_sidebar_item(&mut order, "one", "two", false));
    }

    #[test]
    fn sidebar_reorder_rejects_worktree_drop_in_another_project() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/one");
        let mut other = panes_layout(&[(2, AgentStatus::Idle)]).sessions.remove(0);
        other.name = "other".into();
        other.workspace = std::path::PathBuf::from("/tmp/two");
        layout.sessions.push(other);
        view.layout = Some(layout);
        let worktree_rows: Vec<u16> = view
            .sidebar_rows()
            .into_iter()
            .filter_map(|(y, row)| matches!(row, SidebarRow::Worktree(_)).then_some(y))
            .collect();

        assert!(view.begin_sidebar_reorder(worktree_rows[0]));
        assert!(view.update_sidebar_reorder(worktree_rows[1]));
        let drag = view.sidebar_reorder_drag.as_ref().unwrap();
        let SidebarReorderTarget::Worktree { project, .. } = &drag.target else { panic!("worktree target") };
        assert_eq!(project, "/tmp/one");
    }

    #[test]
    fn sidebar_reorder_bar_uses_worktree_boundaries() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].name = "one".into();
        layout.sessions[0].workspace = std::path::PathBuf::from("/tmp/project-one");
        layout.sessions[0].project_root = Some(std::path::PathBuf::from("/tmp/project"));
        let mut two = layout.sessions[0].clone();
        two.name = "two".into();
        two.workspace = std::path::PathBuf::from("/tmp/project-two");
        layout.sessions.push(two);
        view.layout = Some(layout);
        let project = session_project_key(&view.layout.as_ref().unwrap().sessions[0]);
        let first_key = session_worktree_key(&view.layout.as_ref().unwrap().sessions[0]);
        let second_key = session_worktree_key(&view.layout.as_ref().unwrap().sessions[1]);
        let worktree_rows: Vec<u16> = view
            .sidebar_rows()
            .into_iter()
            .filter_map(|(y, row)| matches!(row, SidebarRow::Worktree(_)).then_some(y))
            .collect();
        let drag = SidebarReorderDrag {
            source: SidebarReorderItem::Worktree {
                project: project.clone(),
                worktree: session_worktree_key(&view.layout.as_ref().unwrap().sessions[1]),
                session_idx: 1,
            },
            source_y: worktree_rows[1],
            target: SidebarReorderTarget::Worktree { project: project.clone(), worktree: first_key.clone() },
            after: true,
            moved: true,
        };

        let base_rows = view.sidebar_rows_base();
        assert_eq!(view.sidebar_reorder_insertion_y(&drag, &base_rows), Some(worktree_rows[1]));
        view.sidebar_reorder_drag = Some(drag);
        let visual_rows = view.sidebar_rows();
        assert!(visual_rows.iter().any(|(y, row)| *y == worktree_rows[1] && matches!(row, SidebarRow::DropIndicator)));
        assert!(visual_rows.iter().any(|(y, row)| *y == worktree_rows[1] + 1 && matches!(row, SidebarRow::Worktree(1))));

        let after_last = SidebarReorderDrag {
            source: SidebarReorderItem::Worktree { project: project.clone(), worktree: first_key, session_idx: 0 },
            source_y: worktree_rows[0],
            target: SidebarReorderTarget::Worktree { project, worktree: second_key },
            after: true,
            moved: true,
        };
        let insertion = view.sidebar_reorder_insertion_y(&after_last, &view.sidebar_rows_base()).unwrap();
        assert!(insertion < view.project_sidebar_geometry().new_session_y);
        view.sidebar_reorder_drag = Some(after_last);
        let rows = view.sidebar_rows();
        let new_session_y = rows.iter().find_map(|(y, row)| matches!(row, SidebarRow::NewSession).then_some(*y)).unwrap();
        let drop_y = rows.iter().find_map(|(y, row)| matches!(row, SidebarRow::DropIndicator).then_some(*y)).unwrap();
        assert_ne!(drop_y, new_session_y, "drop indicator must not cover the fixed action");
    }

    #[test]
    fn project_sidebar_filter_matches_branch_and_shows_empty_state() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].branch = Some(WireBranch { name: "feature/search".into(), ahead: 0, behind: 0 });
        view.layout = Some(layout);
        view.sidebar_filter = "search".into();
        assert_eq!(view.project_content().iter().filter(|r| matches!(r, SidebarRow::Worktree(_))).count(), 1);
        view.sidebar_filter = "does-not-exist".into();
        assert!(view.project_content().iter().any(|r| matches!(r, SidebarRow::Dim(s) if s.contains("no matches"))));
    }

    #[test]
    fn project_sidebar_filter_reveals_matches_in_collapsed_project() {
        let mut view = test_view();
        let mut layout = panes_layout(&[(1, AgentStatus::Idle)]);
        layout.sessions[0].branch = Some(WireBranch { name: "feature/search".into(), ahead: 0, behind: 0 });
        let project = session_project_key(&layout.sessions[0]);
        view.layout = Some(layout);
        view.collapsed_projects.insert(project);

        assert!(!view.project_content().iter().any(|row| matches!(row, SidebarRow::Worktree(_))));
        view.sidebar_filter = "search".into();
        let filtered = view.project_content();
        assert!(filtered.iter().any(|row| matches!(row, SidebarRow::Worktree(_))));
        assert!(!filtered.iter().any(|row| matches!(row, SidebarRow::Dim(s) if s.contains("no matches"))));
    }

    #[test]
    fn project_sidebar_filter_backspace_and_escape_are_local() {
        let mut view = test_view();
        view.sidebar_filter = "abc".into();
        view.sidebar_filter_active = true;
        view.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        assert_eq!(view.sidebar_filter, "ab");
        view.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(!view.sidebar_filter_active);
        assert!(view.sidebar_filter.is_empty());
    }
}
