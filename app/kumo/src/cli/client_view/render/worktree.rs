use super::*;

impl View {
    pub(super) fn render_worktree_picker(&self, f: &mut Frame) {
        if !self.worktree_picker.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.worktree_picker_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        let count = self.worktree_picker.items.len();
        let title_text = if count == 0 { "worktrees".to_string() } else { format!("worktrees · {count}") };
        text(f, dd.x + 2, dd.y + 1, &title_text, title, inner_w);
        if let Some(err) = &self.worktree_picker.error {
            let st = Style::default().fg(theme.orange).bg(theme.panel_sep);
            text(f, dd.x + 2, dd.y + 2, err, st, inner_w);
            let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
            text(f, dd.x + 2, dd.bottom() - 2, "esc: close", footer, inner_w);
            return;
        }
        const BRANCH_COL: u16 = 24;
        let branch_x = dd.x + 3;
        let path_x = branch_x + BRANCH_COL + 1;
        let path_w = inner_w.saturating_sub(path_x - dd.x + 1);
        let header = Style::default().fg(theme.panel_muted).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, branch_x, dd.y + 2, "branch", header, BRANCH_COL.saturating_sub(2));
        if path_w > 0 {
            text(f, path_x, dd.y + 2, "path", header, path_w);
        }
        let body_top = dd.y + 3;
        let body_bottom = dd.bottom() - 2;
        let scroll = self.worktree_picker.scroll as usize;
        for (i, row) in self.worktree_picker.items.iter().enumerate().skip(scroll) {
            let y = body_top + (i - scroll) as u16;
            if y >= body_bottom {
                break;
            }
            let sel = i == self.worktree_picker.selected;
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in (dd.x + 1)..(dd.x + 1 + inner_w) {
                put(f, cx, y, " ", Style::default().bg(bg));
            }
            let fg = if sel { contrast_fg(&theme, bg) } else { theme.fg };
            put(f, dd.x + 1, y, if sel { "▸" } else { " " }, Style::default().fg(if sel { fg } else { theme.accent }).bg(bg).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() }));
            if row.open {
                put(f, dd.x + 2, y, "●", Style::default().fg(if sel { fg } else { theme.green }).bg(bg));
            }
            let mut branch_disp = row.branch.as_deref().unwrap_or("(detached)").to_string();
            if row.is_ephemeral {
                branch_disp.push_str(" ◉");
            }
            if let Some(st) = &row.status {
                branch_disp.push_str(&format!(" [{}]", st));
            }
            if let Some(c) = &row.comment {
                if !c.is_empty() && !sel {
                    // show comment suffix dimmed in path column when not selected
                }
            }
            let branch_style = Style::default().fg(fg).bg(bg).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() });
            text(f, branch_x, y, &branch_disp, branch_style, BRANCH_COL.saturating_sub(2));
            if path_w > 0 {
                let mut path = fit_worktree_path(&row.path, path_w as usize);
                if let Some(c) = &row.comment {
                    if sel && !c.is_empty() {
                        // when selected, show comment tail after path if space
                        let avail = path_w as usize;
                        let suffix = format!(" · {}", c.lines().next().unwrap_or(""));
                        if path.len() + suffix.len() < avail {
                            path.push_str(&suffix);
                        }
                    }
                }
                let path_style = Style::default().fg(if row.is_main { fg } else { theme.panel_muted }).bg(bg);
                text(f, path_x, y, &path, path_style, path_w);
            }
        }
        let items = self.worktree_picker.items.len();
        let visible = self.worktree_picker_visible_rows() as usize;
        if items > visible {
            let bar_h = body_bottom.saturating_sub(body_top);
            let thumb = ((visible * bar_h as usize) / items).max(1).min(bar_h as usize);
            let y_max = (bar_h as usize).saturating_sub(thumb);
            let y_start = self.worktree_picker.scroll as usize * y_max / (items - visible);
            for i in 0..bar_h as usize {
                let y = body_top + i as u16;
                let ch = if i >= y_start && i < y_start + thumb { "▐" } else { "░" };
                put(f, dd.right() - 2, y, ch, Style::default().fg(if i >= y_start && i < y_start + thumb { theme.secondary } else { theme.panel_sep }));
            }
        }
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2, body_bottom, "j/k: move · enter: open · x: clear checkpoint · esc: close", footer, inner_w);
    }

    pub(super) fn render_worktree_create(&self, f: &mut Frame) {
        if !self.worktree_create.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.worktree_create_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        let inner_w = dd.width.saturating_sub(4);
        // Title
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, dd.x + 2, dd.y + 1, "new worktree", title, inner_w);
        // Tabs row
        let tabs_y = dd.y + 2;
        let mut cur_x = dd.x + 2;
        for tab in [
            WorktreeCreateTab::Smart,
            WorktreeCreateTab::Github,
            WorktreeCreateTab::Branch,
            WorktreeCreateTab::Name,
        ] {
            let is_sel = tab == self.worktree_create.tab;
            let label = tab.label();
            let w = (label.chars().count() as u16) + 2;
            let style = if is_sel {
                Style::default().fg(contrast_fg(&theme, theme.accent)).bg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg).bg(theme.panel_sep)
            };
            // Add separator dot between tabs
            text(f, cur_x, tabs_y, &format!(" {} ", label), style, w);
            cur_x += w + 1;
            if cur_x >= dd.right() - 2 {
                break;
            }
        }
        // Create from label + input
        let input_label_style = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        let focus = self.worktree_create.focus;
        let is_create_focused = focus == WorktreeCreateFocus::CreateFrom;
        text(f, dd.x + 2, dd.y + 3, "Create from", input_label_style, inner_w);
        let input_rect = Rect::new(dd.x + 2, dd.y + 4, inner_w, 1);
        let input_bg = if is_create_focused { theme.accent } else { theme.input_bg };
        let field_fg = contrast_fg(&theme, input_bg);
        fill(f, input_rect, input_bg);
        // Placeholder / content
        let cf = &self.worktree_create.create_from;
        let cursor = self.worktree_create.cursor;
        let placeholder = match self.worktree_create.tab {
            WorktreeCreateTab::Smart => "name, #1234, branch, or https://github.com/.../pull/123",
            WorktreeCreateTab::Github => "#1234 or GitHub URL",
            WorktreeCreateTab::Jira => "Jira issue",
            WorktreeCreateTab::Branch => "branch name (e.g. feat/login)",
            WorktreeCreateTab::Name => "task name (e.g. fix login)",
        };
        let display = if cf.is_empty() && !is_create_focused { placeholder } else { cf.as_str() };
        let text_style = if cf.is_empty() && !is_create_focused {
            input_hint_style(&theme)
        } else {
            Style::default().fg(field_fg).bg(input_bg)
        };
        // Render with cursor handling (scroll if needed)
        let field_w = inner_w as usize;
        let len = display.chars().count();
        let cur = cursor.min(len);
        // Simple: render substring window around cursor
        let start = if len <= field_w { 0 } else { cur.saturating_sub(field_w / 2).min(len - field_w) };
        let mut col = input_rect.x;
        for (i, ch) in display.chars().enumerate().skip(start).take(field_w) {
            let is_cursor = is_create_focused && i == cur;
            let mut st = text_style;
            if is_cursor {
                st = st.add_modifier(Modifier::REVERSED);
            }
            put(f, col, input_rect.y, &ch.to_string(), st);
            col += 1;
        }
        if is_create_focused && cur == len && col < input_rect.right() {
            put(f, col, input_rect.y, " ", text_style.add_modifier(Modifier::REVERSED));
        }
        // Advanced toggle row
        let adv_y = dd.y + 6;
        let adv_style = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        let adv_label = if self.worktree_create.advanced { "▼ Advanced" } else { "▶ Advanced (branch, note, agent)" };
        let adv_focused = false; // not focusable via Tab, but clickable
        let adv_text_style = if adv_focused { Style::default().fg(contrast_fg(&theme, theme.accent)).bg(theme.accent) } else { adv_style };
        text(f, dd.x + 2, adv_y, adv_label, adv_text_style, inner_w);
        let hint = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2 + adv_label.chars().count() as u16 + 2, adv_y, "· ctrl+a", hint, inner_w);
        if self.worktree_create.advanced {
            // Branch override
            let branch_focused = focus == WorktreeCreateFocus::BranchOverride;
            text(f, dd.x + 2, dd.y + 7, "Branch name override", input_label_style, inner_w);
            let br_rect = Rect::new(dd.x + 2, dd.y + 8, inner_w, 1);
            let bg = if branch_focused { theme.accent } else { theme.input_bg };
            let fg = contrast_fg(&theme, bg);
            fill(f, br_rect, bg);
            let b = &self.worktree_create.branch_override;
            let bc = self.worktree_create.branch_cursor;
            let bph = "e.g. feat/login (leave empty to derive)";
            let bdisplay = if b.is_empty() && !branch_focused { bph } else { b.as_str() };
            let bstyle = if b.is_empty() && !branch_focused { input_hint_style(&theme) } else { Style::default().fg(fg).bg(bg) };
            let blen = bdisplay.chars().count();
            let bcur = bc.min(blen);
            let bstart = if blen <= inner_w as usize { 0 } else { bcur.saturating_sub(inner_w as usize / 2).min(blen - inner_w as usize) };
            let mut bcol = br_rect.x;
            for (i, ch) in bdisplay.chars().enumerate().skip(bstart).take(inner_w as usize) {
                let is_cur = branch_focused && i == bcur;
                let mut st = bstyle;
                if is_cur { st = st.add_modifier(Modifier::REVERSED); }
                put(f, bcol, br_rect.y, &ch.to_string(), st);
                bcol += 1;
            }
            if branch_focused && bcur == blen && bcol < br_rect.right() {
                put(f, bcol, br_rect.y, " ", bstyle.add_modifier(Modifier::REVERSED));
            }
            // Note
            let note_focused = focus == WorktreeCreateFocus::Note;
            text(f, dd.x + 2, dd.y + 9, "Note", input_label_style, inner_w);
            let note_rect = Rect::new(dd.x + 2, dd.y + 10, inner_w, 1);
            let nbg = if note_focused { theme.accent } else { theme.input_bg };
            let nfg = contrast_fg(&theme, nbg);
            fill(f, note_rect, nbg);
            let n = &self.worktree_create.note;
            let nc = self.worktree_create.note_cursor;
            let nph = "optional checkpoint note";
            let ndisp = if n.is_empty() && !note_focused { nph } else { n.as_str() };
            let nstyle = if n.is_empty() && !note_focused { input_hint_style(&theme) } else { Style::default().fg(nfg).bg(nbg) };
            let nlen = ndisp.chars().count();
            let ncur = nc.min(nlen);
            let nstart = if nlen <= inner_w as usize { 0 } else { ncur.saturating_sub(inner_w as usize / 2).min(nlen - inner_w as usize) };
            let mut ncol = note_rect.x;
            for (i, ch) in ndisp.chars().enumerate().skip(nstart).take(inner_w as usize) {
                let is_cur = note_focused && i == ncur;
                let mut st = nstyle;
                if is_cur { st = st.add_modifier(Modifier::REVERSED); }
                put(f, ncol, note_rect.y, &ch.to_string(), st);
                ncol += 1;
            }
            if note_focused && ncur == nlen && ncol < note_rect.right() {
                put(f, ncol, note_rect.y, " ", nstyle.add_modifier(Modifier::REVERSED));
            }
            // Agent
            let agent_focused = focus == WorktreeCreateFocus::Agent;
            text(f, dd.x + 2, dd.y + 11, "Agent", input_label_style, inner_w);
            let ag_rect = Rect::new(dd.x + 2, dd.y + 12, inner_w, 1);
            let abg = if agent_focused { theme.accent } else { theme.input_bg };
            let afg = contrast_fg(&theme, abg);
            fill(f, ag_rect, abg);
            let ag = &self.worktree_create.agent;
            let agc = self.worktree_create.agent_cursor;
            let agph = "e.g. claude, codex, opencode (leave empty for none)";
            let agdisp = if ag.is_empty() && !agent_focused { agph } else { ag.as_str() };
            let agstyle = if ag.is_empty() && !agent_focused { input_hint_style(&theme) } else { Style::default().fg(afg).bg(abg) };
            let aglen = agdisp.chars().count();
            let agcur = agc.min(aglen);
            let agstart = if aglen <= inner_w as usize { 0 } else { agcur.saturating_sub(inner_w as usize / 2).min(aglen - inner_w as usize) };
            let mut agcol = ag_rect.x;
            for (i, ch) in agdisp.chars().enumerate().skip(agstart).take(inner_w as usize) {
                let is_cur = agent_focused && i == agcur;
                let mut st = agstyle;
                if is_cur { st = st.add_modifier(Modifier::REVERSED); }
                put(f, agcol, ag_rect.y, &ch.to_string(), st);
                agcol += 1;
            }
            if agent_focused && agcur == aglen && agcol < ag_rect.right() {
                put(f, agcol, ag_rect.y, " ", agstyle.add_modifier(Modifier::REVERSED));
            }
        }
        // Error
        if let Some(err) = &self.worktree_create.error {
            let err_style = Style::default().fg(theme.orange).bg(theme.panel_sep);
            let err_y = if self.worktree_create.advanced { dd.y + 13 } else { dd.y + 7 };
            text(f, dd.x + 2, err_y, err, err_style, inner_w);
        }
        // Footer + buttons — footer must not overlap the buttons.
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        let footer_y = dd.bottom().saturating_sub(2);
        let footer_text = "tab: next field · enter: create · esc: close";
        let button_start_x = self.worktree_create_button_rect(true).map(|r| r.x).unwrap_or(dd.right());
        let footer_avail = button_start_x.saturating_sub(dd.x + 3);
        text(f, dd.x + 2, footer_y, footer_text, footer, footer_avail);
        for (is_create, label) in [(true, " create "), (false, " cancel ")] {
            if let Some(rect) = self.worktree_create_button_rect(is_create) {
                let is_hover = false;
                let st = if is_create {
                    Style::default().fg(contrast_fg(&theme, theme.green)).bg(theme.green).add_modifier(Modifier::BOLD)
                } else if is_hover {
                    Style::default().fg(contrast_fg(&theme, theme.secondary)).bg(theme.secondary)
                } else {
                    Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD)
                };
                text(f, rect.x, rect.y, label, st, rect.width);
            }
        }
    }
}

/// Short display form of a worktree path for the picker, trimmed to `avail`.
fn fit_worktree_path(path: &std::path::Path, avail: usize) -> String {
    let text = path.to_string_lossy();
    let n = text.chars().count();
    if n <= avail {
        return text.into_owned();
    }
    if avail == 0 {
        return String::new();
    }
    let tail: String = text.chars().skip(n.saturating_sub(avail - 1)).collect();
    format!("…{tail}")
}
