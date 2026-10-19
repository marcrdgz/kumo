use super::*;

impl View {
    pub(in crate::cli::client_view) fn render_sidebar(&self, f: &mut Frame, size: Rect) {
        let w = self.effective_sidebar_width().min(size.width);
        let theme = self.current_theme();
        let area = Rect::new(0, 0, w, size.height.saturating_sub(self.status_h()));
        fill(f, area, RColor::Reset);
        let bstyle = kumo_core::config::sidebar_borders().style;
        let sep_hidden = bstyle == kumo_core::config::BorderStyle::Hidden;
        if !sep_hidden {
            let (_, _, _, _, _, v) = border_chars(bstyle);
            // Sidebar separator follows pane border vertical style (rounded/single -> │)
            let sep = if bstyle == kumo_core::config::BorderStyle::Double { "║" }
                      else if bstyle == kumo_core::config::BorderStyle::Heavy { "┃" }
                      else { "│" };
            let _ = v; // keep helper unified
            for y in area.y..(area.y + area.height) {
                put(f, area.x + area.width, y, sep, Style::default().fg(theme.panel_sep));
            }
        }
        if self.sidebar_layout() == SidebarLayout::Tabs {
            self.render_tabs(f, area, w);
        }
        let divided = self.sidebar_layout() == SidebarLayout::Divided;
        for (y, row) in self.sidebar_rows() {
            if y > area.y + area.height {
                break;
            }
            let x = area.x;
            let max = w.saturating_sub(2);
            match row {
                SidebarRow::Header(t) => {
                    let style = Style::default().fg(theme.accent).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    let pad = max.saturating_sub(t.chars().count() as u16) / 2;
                    text(f, x + pad, y, &t, style, max);
                }
                SidebarRow::Spacer => {
                    put(f, x, y, " ", Style::default().bg(RColor::Reset));
                }
                SidebarRow::PanelLabel(title, right) => {
                    let style = Style::default().fg(theme.accent).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    text(f, x + 2, y, &title, style, max.saturating_sub(2));
                    if let Some(right) = right {
                        let rw = right.chars().count() as u16;
                        let max_r = w.saturating_sub(1).max(1);
                        if rw <= max_r.saturating_sub(2) {
                            text(f, x + max_r.saturating_sub(rw), y, &right, style, rw);
                        }
                    }
                }
                SidebarRow::Divider => {
                    // Plain muted divider. Only the legacy divided layout makes
                    // it draggable; the project inbox divider stays fixed.
                    let style = Style::default().fg(theme.panel_muted);
                    let line: String = "─".repeat(w.saturating_sub(1).max(1) as usize);
                    if w > 1 {
                        text(f, x, y, &line, style, w.saturating_sub(1));
                    }
                }
                SidebarRow::DropIndicator => {
                    let style = Style::default().fg(theme.accent).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    let line = "─".repeat(w.saturating_sub(4).max(1) as usize);
                    text(f, x + 2, y, &line, style, w.saturating_sub(4).max(1));
                }
                SidebarRow::GroupHeader(status, count) => {
                    let dim = matches!(status, AgentStatus::Idle | AgentStatus::Unknown);
                    let (r, g, b) = kumo_core::theme::agent_status_color(status);
                    let style = if dim {
                        Style::default().fg(theme.panel_muted).bg(RColor::Reset)
                    } else {
                        Style::default().fg(RColor::Rgb(r, g, b)).bg(RColor::Reset).add_modifier(Modifier::BOLD)
                    };
                    let label = format!("{} ({count})", status.label());
                    text(f, x + 2, y, &label, style, max.saturating_sub(2));
                }
                SidebarRow::Dim(s) => {
                    text(f, x + 2, y, &s, Style::default().fg(theme.panel_muted).bg(RColor::Reset), max.saturating_sub(2));
                }
                SidebarRow::Session(i) => {
                    let active = self.layout.as_ref().map(|l| l.active.as_deref() == Some(&self.session_name(i))).unwrap_or(false);
                    let bg = if active { sidebar_active_bg(&theme) } else { RColor::Reset };
                    let name = self.session_name(i);
                    if active {
                        fill(f, Rect::new(x, y, w, 1), bg);
                        put(f, x + 1, y, "▸", Style::default().fg(theme.accent).bg(bg));
                        text(f, x + 3, y, &name, Style::default().fg(theme.fg).bg(bg), max.saturating_sub(3));
                    } else {
                        put(f, x + 1, y, " ", Style::default().bg(bg));
                        text(f, x + 3, y, &name, Style::default().fg(theme.panel_muted).bg(bg), max.saturating_sub(3));
                    }
                }
                SidebarRow::Branch(i, b) => {
                    let active = self.layout.as_ref().map(|l| l.active.as_deref() == Some(&self.session_name(i))).unwrap_or(false);
                    let project_layout = self.sidebar_layout() == SidebarLayout::Project;
                    let is_hover = project_layout && self.sidebar_hover == Some(i);
                    let bg = if active {
                        sidebar_active_bg(&theme)
                    } else if is_hover {
                        lighten(theme.panel_sep, 18)
                    } else {
                        RColor::Reset
                    };
                    let name_color = if active || is_hover { theme.fg } else { theme.panel_muted };
                    if bg != RColor::Reset {
                        fill(f, Rect::new(x, y, w, 1), bg);
                    }
                    if active && project_layout {
                        put(f, x, y, "┃", Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD));
                    }
                    let branch_x = if project_layout { x + 7 } else { x + 4 };
                    let avail = max.saturating_sub(if project_layout { 7 } else { 4 }) as usize;
                    let suffix = match (b.ahead, b.behind) {
                        (0, 0) => String::new(),
                        (a, 0) => format!(" \u{2191}{}", a),
                        (0, be) => format!(" ~{}", be),
                        (a, be) => format!(" \u{2191}{}~{}", a, be),
                    };
                    let suffix_w = suffix.chars().count().min(avail);
                    let name_avail = avail.saturating_sub(suffix_w);
                    let shown = fit_branch_name(&b.name, name_avail);
                    let branch_style = Style::default()
                        .fg(name_color)
                        .bg(bg)
                        .add_modifier(Modifier::DIM)
                        .add_modifier(if active { Modifier::BOLD } else { Modifier::empty() });
                    if project_layout {
                        put(
                            f,
                            x + 2,
                            y,
                            if self.is_last_project_worktree(i) { " " } else { "│" },
                            Style::default().fg(theme.panel_muted).bg(bg),
                        );
                        put(f, x + 5, y, "⎇", Style::default().fg(theme.panel_muted).bg(bg));
                    }
                    text(f, branch_x, y, &shown, branch_style, avail as u16);
                    let mut cx = branch_x + shown.chars().count() as u16;
                    let mut remaining = (avail as u16).saturating_sub(shown.chars().count() as u16);
                    if b.ahead > 0 && remaining > 1 {
                        put(f, cx, y, " ", Style::default().bg(bg));
                        cx += 1;
                        remaining -= 1;
                        let s = format!("\u{2191}{}", b.ahead);
                        let wd = (s.chars().count() as u16).min(remaining);
                        text(f, cx, y, &s, Style::default().fg(theme.green).bg(bg), remaining);
                        cx += wd;
                        remaining = remaining.saturating_sub(wd);
                    }
                    if b.behind > 0 && remaining > 1 {
                        put(f, cx, y, " ", Style::default().bg(bg));
                        cx += 1;
                        remaining -= 1;
                        let s = format!("~{}", b.behind);
                        text(f, cx, y, &s, Style::default().fg(theme.orange).bg(bg), remaining);
                    }
                }
                SidebarRow::AgentDir(..) | SidebarRow::AgentName(..) => {
                    let (i, pid, third, status, is_dir) = match &row {
                        SidebarRow::AgentDir(i, pid, third, status) => (i, pid, third, status, true),
                        SidebarRow::AgentName(i, pid, third, status) => (i, pid, third, status, false),
                        _ => unreachable!(),
                    };
                    let session_active = self
                        .layout
                        .as_ref()
                        .map(|l| l.active.as_deref() == Some(&self.session_name(*i)))
                        .unwrap_or(false);
                    let pane_focused = self
                        .layout
                        .as_ref()
                        .and_then(|l| l.sessions.get(*i))
                        .map(|s| s.tabs.get(s.active_tab).map(|t| t.focus == *pid).unwrap_or(false))
                        .unwrap_or(false);
                    let focused = (session_active && pane_focused) || self.inbox_selected(*i, *pid);
                    let bg = if focused { sidebar_active_bg(&theme) } else { RColor::Reset };
                    if focused {
                        fill(f, Rect::new(x, y, w, 1), bg);
                    }
                    if self.inbox_selected(*i, *pid) {
                        put(f, x + 1, y, "▸", Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD));
                    }
                    let status_color = {
                        let (r, g, b) = kumo_core::theme::agent_status_color(*status);
                        RColor::Rgb(r, g, b)
                    };
                    // The grouped panel dims the idle/unknown tail; the ranked
                    // panel (and the tabs layout) keep their look.
                    let dim = divided
                        && self.agents_panel_order == AgentPanelOrder::Grouped
                        && matches!(status, AgentStatus::Idle | AgentStatus::Unknown);
                    let dot = match status {
                        AgentStatus::Blocked => "◉",
                        AgentStatus::Done => "✓",
                        AgentStatus::Working => self.spinner_char(),
                        AgentStatus::Idle => "●",
                        AgentStatus::Unknown => "?",
                    };
                    let entry_color = if dim { theme.panel_muted } else { status_color };
                    let name_style = if *status == AgentStatus::Blocked {
                        Style::default().fg(entry_color).bg(bg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(entry_color).bg(bg)
                    };
                    if !is_dir {
                        let dot_style = if dim {
                            Style::default().fg(theme.panel_muted).bg(bg)
                        } else if *status == AgentStatus::Idle {
                            Style::default().fg(status_color).bg(bg)
                        } else {
                            Style::default().fg(status_color).bg(bg).add_modifier(Modifier::BOLD)
                        };
                        put(f, x + 2, y, dot, dot_style);
                    }
                    if is_dir {
                        let path_color = if focused { theme.fg } else { theme.panel_muted };
                        // Ranked workspace rows lead with `space · pane`
                        // instead of the workspace path.
                        let label = if divided && self.agents_panel_order == AgentPanelOrder::Ranked {
                            let (space, pane) = Self::agent_pane_location(self.layout.as_ref(), *i, *pid);
                            agent_space_pane_label(&space, &pane, max.saturating_sub(4) as usize)
                        } else {
                            third.clone()
                        };
                        text(f, x + 4, y, &label, Style::default().fg(path_color).bg(bg), max.saturating_sub(4));
                    } else {
                        let avail = max.saturating_sub(4) as usize;
                        // Grouped entries fit `kind · space · pane` inline (the
                        // session and tab of the agent pane) when there is
                        // room; the status suffix joins when it still fits.
                        let label = if divided && self.agents_panel_order == AgentPanelOrder::Grouped {
                            let (space, pane) = Self::agent_pane_location(self.layout.as_ref(), *i, *pid);
                            let mut label = agent_entry_label(third, &space, &pane, avail);
                            let suffix = match status {
                                AgentStatus::Blocked => Some(" ·blocked"),
                                AgentStatus::Done => Some(" ·done"),
                                _ => None,
                            };
                            if let Some(sfx) = suffix {
                                if label.chars().count() + sfx.chars().count() <= avail {
                                    label.push_str(sfx);
                                }
                            }
                            label
                        } else {
                            let label = if *status == AgentStatus::Blocked
                                && third.chars().count() + " ·blocked".len() <= avail
                            {
                                format!("{third} ·blocked")
                            } else if *status == AgentStatus::Done
                                && third.chars().count() + " ·done".len() <= avail
                            {
                                format!("{third} ·done")
                            } else {
                                third.clone()
                            };
                            label
                        };
                        text(f, x + 4, y, &label, name_style, max.saturating_sub(4));
                    }
                }
                SidebarRow::NewSession => {
                    let style = Style::default().fg(theme.fg).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    let hint_style = Style::default().fg(theme.panel_muted).bg(RColor::Reset);
                    let show_hint = self.sidebar_layout() == SidebarLayout::Project && max >= 5;
                    let hint_w = 1u16;
                    let max_r = w.saturating_sub(1).max(1);
                    let label_w = if show_hint { max.saturating_sub(hint_w + 2) } else { max };
                    text(f, x, y, "  + NEW SESSION", style, label_w);
                    if show_hint {
                        text(f, x + max_r.saturating_sub(hint_w), y, "c", hint_style, hint_w);
                    }
                }
                SidebarRow::AgentInboxHeader { .. } => {
                    let style = Style::default().fg(theme.panel_muted).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    text(f, x + 2, y, "AGENTS", style, max.saturating_sub(2));
                    let summary = self.compact_agent_summary();
                    let summary_w = summary.chars().count() as u16;
                    let show_hint = self.sidebar_layout() == SidebarLayout::Project && summary_w + 12 < w;
                    if summary_w + 10 < w {
                        let hint_w = 1u16;
                        let max_r = w.saturating_sub(1).max(1);
                        let summary_x = if show_hint {
                            x + max_r.saturating_sub(hint_w + 2 + summary_w)
                        } else {
                            x + max_r.saturating_sub(summary_w)
                        };
                        text(
                            f,
                            summary_x,
                            y,
                            &summary,
                            Style::default().fg(theme.panel_muted).bg(RColor::Reset),
                            summary_w,
                        );
                        if show_hint {
                            text(
                                f,
                                x + max_r.saturating_sub(hint_w),
                                y,
                                "i",
                                Style::default().fg(theme.panel_muted).bg(RColor::Reset),
                                hint_w,
                            );
                        }
                    }
                }
                SidebarRow::CompactAgent(i, _pid, name, status) => {
                    let (r, g, b) = kumo_core::theme::agent_status_color(status);
                    let marker = match status {
                        AgentStatus::Blocked => "!",
                        AgentStatus::Done => "✓",
                        AgentStatus::Working => self.spinner_char(),
                        AgentStatus::Idle | AgentStatus::Unknown => "·",
                    };
                    put(
                        f,
                        x + 2,
                        y,
                        marker,
                        Style::default().fg(RColor::Rgb(r, g, b)).bg(RColor::Reset).add_modifier(Modifier::BOLD),
                    );
                    let workspace = self.session_name(i);
                    let label = format!("{name} · {workspace}");
                    let avail = max.saturating_sub(4) as usize;
                    let shown = fit_branch_name(&label, avail);
                    text(
                        f,
                        x + 4,
                        y,
                        &shown,
                        Style::default().fg(theme.fg).bg(RColor::Reset),
                        avail as u16,
                    );
                }
                SidebarRow::Search => {
                    let style = Style::default().fg(if self.sidebar_filter_active { theme.fg } else { theme.panel_muted }).bg(RColor::Reset);
                    let label = if self.sidebar_filter_active {
                        format!("⌕ {}▏", self.sidebar_filter)
                    } else if self.sidebar_filter.is_empty() {
                        "⌕ Filter…".to_string()
                    } else {
                        format!("⌕ {}", self.sidebar_filter)
                    };
                    text(f, x + 2, y, &label, style, max.saturating_sub(2));
                    let count = self.project_content().iter().filter(|r| matches!(r, SidebarRow::Worktree(_))).count();
                    let hint = if self.sidebar_filter_active {
                        format!("{count} match{}", if count == 1 { "" } else { "es" })
                    } else if self.sidebar_filter.is_empty() {
                        "f".to_string()
                    } else {
                        String::new()
                    };
                    let rw = hint.chars().count() as u16;
                    let max_r = w.saturating_sub(1).max(1);
                    if rw + 4 <= max {
                        let hs = Style::default().fg(theme.panel_muted).bg(RColor::Reset);
                        text(f, x + max_r.saturating_sub(rw), y, &hint, hs, rw);
                    }
                }
                SidebarRow::SectionLabel(title, right) => {
                    let style = Style::default().fg(theme.panel_muted).bg(RColor::Reset).add_modifier(Modifier::BOLD);
                    text(f, x + 2, y, &title, style, max.saturating_sub(2));
                    if let Some(right) = right {
                        let rw = right.chars().count() as u16;
                        let max_r = w.saturating_sub(1).max(1);
                        if rw <= max_r.saturating_sub(2) {
                            text(f, x + max_r.saturating_sub(rw), y, &right, style, rw);
                        }
                    }
                }
                SidebarRow::ProjectHeader { key, label: name } => {
                    let collapsed = self.collapsed_projects.contains(&key);
                    let active = {
                        // header is active if any worktree in its group hosts the active session
                        let mut is_active = false;
                        if let Some(layout) = &self.layout {
                            for s in &layout.sessions {
                                if session_project_key(s) == key && layout.active.as_deref() == Some(&s.name) {
                                    is_active = true;
                                    break;
                                }
                            }
                        }
                        is_active
                    };
                    // Keep the complete active path visually connected: the
                    // project, its active worktree, and that worktree's
                    // branch all share the same selection background and
                    // accent path marker.
                    let bg = if active { sidebar_active_bg(&theme) } else { RColor::Reset };
                    let fg = if active { theme.fg } else { theme.panel_muted };
                    if active {
                        fill(f, Rect::new(x, y, w, 1), bg);
                    }
                    // Folder glyph + name, with a stable repo-key color. The
                    // visible label can change as attention counts change.
                    let hash = {
                        let mut h: u32 = 0;
                        for b in key.bytes() { h = h.wrapping_mul(31).wrapping_add(b as u32); }
                        h
                    };
                    let palette = [theme.accent, theme.secondary, theme.green, theme.orange, theme.panel_muted];
                    let dot_col = palette[(hash as usize) % palette.len()];
                    if active {
                        put(f, x, y, "┃", Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD));
                    }
                    put(f, x + 1, y, if collapsed { "▸" } else { "▾" }, Style::default().fg(dot_col).bg(bg));
                    put(f, x + 3, y, "▰", Style::default().fg(dot_col).bg(bg));
                    let shown = fit_middle_label(&name, max.saturating_sub(5) as usize);
                    text(f, x + 5, y, &shown, Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD), max.saturating_sub(5));
                }
                SidebarRow::Worktree(i) => {
                    let active = self.layout.as_ref().map(|l| l.active.as_deref() == Some(&self.session_name(i))).unwrap_or(false);
                    let is_hover = self.sidebar_hover == Some(i);
                    let bg = if active { sidebar_active_bg(&theme) } else if is_hover { lighten(theme.panel_sep, 18) } else { RColor::Reset };
                    let fg = if active || is_hover { theme.fg } else { theme.panel_muted };
                    if bg != RColor::Reset {
                        fill(f, Rect::new(x, y, w, 1), bg);
                    }
                    if active {
                        put(f, x, y, "┃", Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD));
                    }
                    // Tree guide + workspace glyph make the hierarchy legible
                    // even when project, session and branch share a name.
                    put(
                        f,
                        x + 2,
                        y,
                        if self.is_last_project_worktree(i) { "└" } else { "├" },
                        Style::default().fg(theme.panel_muted).bg(bg),
                    );
                    put(f, x + 3, y, "─", Style::default().fg(theme.panel_muted).bg(bg));
                    put(
                        f,
                        x + 4,
                        y,
                        if active { "◆" } else { "◇" },
                        Style::default().fg(if active { theme.accent } else { theme.panel_muted }).bg(bg),
                    );
                    // Keep hierarchy on the left and align the aggregate AI
                    // state in a stable metadata column on the right. The
                    // final sidebar column remains reserved for the scrollbar.
                    let dot_status = self.worktree_primary_status(i);
                    if let Some(st) = dot_status {
                        let (r,g,b) = kumo_core::theme::agent_status_color(st);
                        let dot = match st {
                            AgentStatus::Blocked => "◉",
                            AgentStatus::Done => "✓",
                            AgentStatus::Working => self.spinner_char(),
                            AgentStatus::Idle => "●",
                            AgentStatus::Unknown => "·",
                        };
                        let dot_fg = RColor::Rgb(r,g,b);
                        let dot_style = if matches!(st, AgentStatus::Idle | AgentStatus::Unknown) {
                            Style::default().fg(theme.panel_muted).bg(bg)
                        } else {
                            Style::default().fg(dot_fg).bg(bg).add_modifier(Modifier::BOLD)
                        };
                        put(f, x + w.saturating_sub(2), y, dot, dot_style);
                    }
                    let name = self.session_name(i);
                    let name_avail = max.saturating_sub(6).max(1);
                    let shown = fit_middle_label(&name, name_avail as usize);
                    let name_style = if active { Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD) } else { Style::default().fg(fg).bg(bg) };
                    text(f, x + 6, y, &shown, name_style, name_avail);
                }
                SidebarRow::InlineAgent(i, pid, name, status) => {
                    let session_active = self.layout.as_ref().map(|l| l.active.as_deref() == Some(&self.session_name(i))).unwrap_or(false);
                    let pane_focused = self.layout.as_ref().and_then(|l| l.sessions.get(i)).map(|s| s.tabs.get(s.active_tab).map(|t| t.focus == pid).unwrap_or(false)).unwrap_or(false);
                    let focused = (session_active && pane_focused) || self.inbox_selected(i, pid);
                    // Only the exact focused agent receives a selection fill;
                    // the parent workspace row already communicates session focus.
                    let bg = if focused { agent_active_bg(&theme) } else { RColor::Reset };
                    if bg != RColor::Reset { fill(f, Rect::new(x, y, w, 1), bg); }
                    if self.inbox_selected(i, pid) {
                        put(f, x + 1, y, "▸", Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD));
                    }
                    let (r,g,b) = kumo_core::theme::agent_status_color(status);
                    let dot = match status {
                        AgentStatus::Blocked => "◉",
                        AgentStatus::Done => "✓",
                        AgentStatus::Working => self.spinner_char(),
                        AgentStatus::Idle => "●",
                        AgentStatus::Unknown => "·",
                    };
                    let dot_fg = RColor::Rgb(r,g,b);
                    let dot_style = if matches!(status, AgentStatus::Idle | AgentStatus::Unknown) {
                        Style::default().fg(theme.panel_muted).bg(bg)
                    } else {
                        Style::default().fg(dot_fg).bg(bg).add_modifier(Modifier::BOLD)
                    };
                    put(f, x + 2, y, "│", Style::default().fg(theme.panel_sep).bg(bg));
                    put(f, x + 4, y, "└", Style::default().fg(theme.panel_sep).bg(bg));
                    put(f, x + 5, y, dot, dot_style);
                    let avail = max.saturating_sub(7) as usize;
                    let label = if name.chars().count() > avail {
                        let mut s = name.clone();
                        while s.chars().count() > avail.saturating_sub(1) && !s.is_empty() { s.pop(); }
                        format!("{s}…")
                    } else { name.clone() };
                    let name_style = if status == AgentStatus::Blocked {
                        Style::default().fg(dot_fg).bg(bg).add_modifier(Modifier::BOLD)
                    } else if matches!(status, AgentStatus::Idle | AgentStatus::Unknown) {
                        Style::default().fg(theme.panel_muted).bg(bg)
                    } else {
                        Style::default().fg(dot_fg).bg(bg)
                    };
                    text(f, x + 7, y, &label, name_style, avail as u16);
                }
            }
        }
        // Scrollbars (rightmost sidebar column): one per overflowing panel.
        let scroll_x = w.saturating_sub(1);
        match self.sidebar_layout() {
            SidebarLayout::Tabs => {
                let region_h = self.content_region_h();
                let items = self.active_tab_items();
                if items.len() > region_h as usize {
                    let offset = (self.active_scroll() as usize).min(items.len() - region_h as usize);
                    draw_scrollbar(f, scroll_x, 3, region_h, offset, items.len(), &theme);
                }
            }
            SidebarLayout::Divided => {
                let p = self.divided_panels();
                if p.spaces_visible && p.spaces_items.len() > p.spaces_h {
                    let off = (self.sidebar_scroll.0 as usize).min(p.spaces_items.len() - p.spaces_h);
                    draw_scrollbar(f, scroll_x, 3, p.spaces_h as u16, off, p.spaces_items.len(), &theme);
                }
                let entries_shown = p.agents_h;
                if p.agents_visible && p.agents_items.len() > entries_shown {
                    let off = (self.sidebar_scroll.1 as usize).min(p.agents_items.len() - entries_shown);
                    draw_scrollbar(f, scroll_x, p.agents_label_y + 1, entries_shown as u16, off, p.agents_items.len(), &theme);
                }
            }
            SidebarLayout::Project => {
                let items = self.project_content();
                let region_h = self.content_region_h() as usize;
                if items.len() > region_h {
                    let off = (self.sidebar_scroll.0 as usize).min(items.len() - region_h);
                    draw_scrollbar(f, scroll_x, 3, region_h as u16, off, items.len(), &theme);
                }
            }
        }
    }
}
