use super::*;

impl View {
    pub(super) fn render_menu(&self, f: &mut Frame) {
        if !self.menu.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.menu_dropdown_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            render_item_row(f, dd.x, dd.y + 1 + i as u16, dd.width.saturating_sub(2), item, i == self.menu.selected, &theme);
        }
    }

    pub(super) fn render_ctx_menu(&self, f: &mut Frame) {
        if !self.ctx_menu.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.ctx_menu_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        for (i, item) in self.ctx_items().iter().enumerate() {
            render_item_row(f, dd.x, dd.y + 1 + i as u16, dd.width.saturating_sub(2), item, i == self.ctx_menu.selected, &theme);
        }
    }

    pub(super) fn render_name_popup(&self, f: &mut Frame) {
        if !self.popup.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.name_popup_rect() else { return };
        let (x0, y0, _x1, _y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
        draw_modal(f, dd, &theme, self.shadow_floor());
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        let title_text = match self.popup.target {
            Some(PopupTarget::RenamePane(_)) => "rename pane",
            Some(PopupTarget::RenameSession(_)) => "rename session",
            Some(PopupTarget::NewWorktree(_)) => "new worktree",
            _ => "new session",
        };
        text(f, x0 + 2, y0 + 1, title_text, title, dd.width.saturating_sub(4));
        let label = Style::default().fg(theme.fg).bg(theme.panel_sep);
        let label_text = match self.popup.target {
            Some(PopupTarget::NewWorktree(_)) => "branch:",
            _ => "name:",
        };
        text(f, x0 + 2, y0 + 2, label_text, label, dd.width.saturating_sub(4));
        let field = input_style(&theme);
        let field_w = dd.width.saturating_sub(4);
        for cx in (x0 + 2)..(x0 + 2 + field_w) {
            put(f, cx, y0 + 3, " ", field);
        }
        let text_w = field_w as usize - 1;
        let name = &self.popup.name;
        let cursor = self.popup.cursor.min(name.chars().count());
        let end = cursor + 1;
        let start = end.saturating_sub(text_w);
        let mut col = x0 + 2;
        for (i, ch) in name.chars().enumerate() {
            if i < start {
                continue;
            }
            if i - start >= text_w {
                break;
            }
            put(f, col, y0 + 3, &ch.to_string(), field);
            col += 1;
        }
        for btn in [PopupBtn::Enter, PopupBtn::Cancel] {
            let Some(rect) = self.name_popup_button_rect(btn) else { continue };
            let label = match btn {
                PopupBtn::Enter => "⏎ enter ",
                PopupBtn::Cancel => " esc cancel ",
            };
            let hovered = self.popup.hover == Some(btn);
            let st = if hovered {
                Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD)
            };
            text(f, rect.x, rect.y, label, st, rect.width);
        }
        if let Some(err) = &self.popup.error {
            text(f, x0 + 2, y0 + 5, err, Style::default().fg(theme.orange).bg(theme.panel_sep), dd.width.saturating_sub(4));
        }
    }

    pub(in crate::cli::client_view) fn render_agent_inbox(&self, f: &mut Frame) {
        let Some(rect) = self.agent_inbox_rect() else { return };
        let theme = self.current_theme();
        draw_modal(f, rect, &theme, self.shadow_floor());
        let inner_w = rect.width.saturating_sub(4);
        let entries = self.all_agent_entries();
        let (blocked, done, working) = self.agent_counts();
        let title = format!("agent inbox · !{blocked}  ✓{done}  {}{working}", self.spinner_char());
        text(
            f,
            rect.x + 2,
            rect.y + 1,
            &title,
            Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD),
            inner_w,
        );

        if entries.is_empty() {
            text(
                f,
                rect.x + 2,
                rect.y + 3,
                "no agents",
                Style::default().fg(theme.panel_muted).bg(theme.panel_sep),
                inner_w,
            );
        } else {
            let visible = self.agent_inbox_visible_rows();
            let start = self.agent_inbox_view_start();
            let selected = self.inbox.as_ref().map(|inbox| inbox.sel).unwrap_or(0).min(entries.len() - 1);
            for (row, (_, session, pane, status, name)) in entries.iter().skip(start).take(visible).enumerate() {
                let idx = start + row;
                let y = rect.y + 3 + row as u16;
                let active = idx == selected;
                let bg = if active { sidebar_active_bg(&theme) } else { theme.panel_sep };
                fill(f, Rect::new(rect.x + 1, y, rect.width.saturating_sub(2), 1), bg);
                let (r, g, b) = kumo_core::theme::agent_status_color(*status);
                let marker = match status {
                    AgentStatus::Blocked => "!",
                    AgentStatus::Done => "✓",
                    AgentStatus::Working => self.spinner_char(),
                    AgentStatus::Idle => "●",
                    AgentStatus::Unknown => "·",
                };
                put(
                    f,
                    rect.x + 2,
                    y,
                    if active { "▸" } else { " " },
                    Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD),
                );
                put(
                    f,
                    rect.x + 4,
                    y,
                    marker,
                    Style::default().fg(RColor::Rgb(r, g, b)).bg(bg).add_modifier(Modifier::BOLD),
                );
                let (_, tab) = Self::agent_pane_location(self.layout.as_ref(), *session, *pane);
                let location = self
                    .layout
                    .as_ref()
                    .and_then(|layout| layout.sessions.get(*session))
                    .map(|session| agent_inbox_location_label(session, &tab))
                    .unwrap_or_else(|| self.session_name(*session));
                let label = format!("{name} · {location}");
                let shown = fit_branch_name(&label, inner_w.saturating_sub(5) as usize);
                text(
                    f,
                    rect.x + 6,
                    y,
                    &shown,
                    Style::default().fg(if active { theme.fg } else { theme.panel_muted }).bg(bg),
                    inner_w.saturating_sub(5),
                );
            }

            let (_, session, pane, status, name) = &entries[selected];
            let session_info = self.layout.as_ref().and_then(|layout| layout.sessions.get(*session));
            let workspace = session_info.map(|session| session.workspace.display().to_string()).unwrap_or_default();
            let (_, tab) = Self::agent_pane_location(self.layout.as_ref(), *session, *pane);
            let location = session_info
                .map(|session| agent_inbox_location_label(session, &tab))
                .unwrap_or_else(|| self.session_name(*session));
            let detail_y = rect.bottom().saturating_sub(4);
            let separator = "─".repeat(inner_w as usize);
            text(f, rect.x + 2, detail_y.saturating_sub(1), &separator, Style::default().fg(theme.panel_muted).bg(theme.panel_sep), inner_w);
            let detail = format!("{name} · {} · {location}", status.label());
            text(f, rect.x + 2, detail_y, &fit_branch_name(&detail, inner_w as usize), Style::default().fg(theme.fg).bg(theme.panel_sep), inner_w);
            text(f, rect.x + 2, detail_y + 1, &fit_branch_name(&workspace, inner_w as usize), Style::default().fg(theme.panel_muted).bg(theme.panel_sep), inner_w);
        }
        let footer = "j/k navigate · enter open · esc close";
        text(
            f,
            rect.x + 2,
            rect.bottom().saturating_sub(2),
            footer,
            Style::default().fg(theme.panel_muted).bg(theme.panel_sep),
            inner_w,
        );
    }

    pub(super) fn render_keybind_overlay(&self, f: &mut Frame) {
        if !self.keybind_overlay.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.keybind_overlay_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        let matches = command_palette_matches(&self.keymap, &self.keybind_overlay.input);
        text(f, dd.x + 2, dd.y + 1, &format!("commands · {}", matches.len()), title, inner_w);

        let input = Rect::new(dd.x + 2, dd.y + 2, inner_w, 1);
        fill(f, input, theme.input_bg);
        let query_style = input_style(&theme);
        put(f, input.x, input.y, "›", query_style.add_modifier(Modifier::BOLD));
        if self.keybind_overlay.input.is_empty() {
            text(f, input.x + 2, input.y, "type to filter actions…", input_hint_style(&theme), input.width.saturating_sub(2));
        } else {
            let field_w = input.width.saturating_sub(2) as usize;
            let start = (self.keybind_overlay.cursor + 1).saturating_sub(field_w);
            let visible: String = self.keybind_overlay.input.chars().skip(start).take(field_w).collect();
            text(f, input.x + 2, input.y, &visible, query_style, field_w as u16);
        }
        let field_w = input.width.saturating_sub(2) as usize;
        let start = (self.keybind_overlay.cursor + 1).saturating_sub(field_w);
        let cursor_x = input.x + 2 + self.keybind_overlay.cursor.saturating_sub(start) as u16;
        if cursor_x < input.right() {
            let symbol = self.keybind_overlay.input.chars().nth(self.keybind_overlay.cursor).unwrap_or(' ').to_string();
            put(f, cursor_x, input.y, &symbol, query_style.add_modifier(Modifier::REVERSED));
        }

        let max_keys = self.keymap.iter().map(|b| bindings::chord_display(b.key).chars().count()).max().unwrap_or(4) as u16;
        let body_top = dd.y + 3;
        let body_bottom = dd.bottom() - 2;
        if matches.is_empty() {
            text(f, dd.x + 2, body_top, "no matching commands", Style::default().fg(theme.panel_muted).bg(theme.panel_sep), inner_w);
        }
        for (row, keymap_idx) in matches.iter().copied().skip(self.keybind_overlay.scroll).enumerate() {
            let i = self.keybind_overlay.scroll + row;
            let b = &self.keymap[keymap_idx];
            let y = body_top + row as u16;
            if y >= body_bottom {
                break;
            }
            let selected = i == self.keybind_overlay.selected;
            let bg = if selected { lighten(theme.panel_sep, 28) } else { theme.panel_sep };
            fill(f, Rect::new(dd.x + 1, y, dd.width.saturating_sub(2), 1), bg);
            if selected {
                put(f, dd.x + 1, y, "▸", Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD));
            }
            let keys = Style::default().fg(if selected { theme.secondary } else { theme.accent }).bg(bg).add_modifier(Modifier::BOLD);
            let desc = Style::default().fg(if selected { theme.fg } else { theme.panel_muted }).bg(bg);
            let chord = bindings::chord_display(b.key);
            text(f, dd.x + 2, y, &chord, keys, max_keys);
            text(f, dd.x + 2 + max_keys + 2, y, palette_action_desc(b.action), desc, inner_w.saturating_sub(max_keys + 2));
        }
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2, dd.bottom() - 2, "↑/↓ select · enter run · esc close", footer, inner_w);
    }

    pub(super) fn render_session_close_confirm(&self, f: &mut Frame) {
        if !self.session_close_confirm.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.session_close_confirm_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        let force = self.session_close_confirm.mode == WorktreeRemovalMode::Force;
        let title_text = if force { "force remove worktree" } else { "remove worktree" };
        text(f, dd.x + 2, dd.y + 1, title_text, title, inner_w);
        let msg = if force {
            "This permanently removes local changes and the worktree branch. Continue?"
        } else {
            "Remove this worktree after closing its session?"
        };
        let msg_style = Style::default().fg(theme.fg).bg(theme.panel_sep);
        text(f, dd.x + 2, dd.y + 2, msg, msg_style, inner_w);
        let path = self.session_close_confirm.path.display().to_string();
        let path_style = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        text(f, dd.x + 2, dd.y + 3, &path, path_style, inner_w);
        let hint = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        let hint_text = if force {
            "Force bypasses dirty checks and deletes the branch · No cancels"
        } else {
            "Safe removal refuses dirty or untracked files · No cancels"
        };
        text(f, dd.x + 2, dd.y + 4, hint_text, hint, inner_w);
        for (is_yes, label) in [(true, " Remove (y) "), (false, " Cancel (n) ")] {
            if let Some(rect) = self.session_close_confirm_button_rect(is_yes) {
                let st = if is_yes {
                    Style::default().fg(RColor::Black).bg(theme.green).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD)
                };
                text(f, rect.x, rect.y, label, st, rect.width);
            }
        }
    }

    pub(super) fn render_finder(&self, f: &mut Frame) {
        if !self.finder.open { return; }
        let theme = self.current_theme();
        let Some(dd) = self.finder_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, dd.x + 2, dd.y + 1, "workspace finder", title, inner_w);
        // input line
        let input_bg = theme.input_bg;
        let field_style = input_style(&theme);
        let input_area = Rect::new(dd.x + 2, dd.y + 2, inner_w, 1);
        fill(f, input_area, input_bg);
        let cursor = self.finder.cursor.min(self.finder.input.chars().count());
        let mut before = String::new();
        let mut after = String::new();
        for (i,c) in self.finder.input.chars().enumerate() {
            if i < cursor { before.push(c); } else { after.push(c); }
        }
        let prompt = "⌕ ";
        text(f, input_area.x, input_area.y, prompt, input_hint_style(&theme), 2);
        text(f, input_area.x + 2, input_area.y, &before, field_style, inner_w.saturating_sub(3));
        // cursor
        let cur_x = input_area.x + 2 + before.chars().count() as u16;
        if cur_x < input_area.x + input_area.width {
            let ch = after.chars().next().map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
            let cur_style = field_style.add_modifier(Modifier::REVERSED);
            put(f, cur_x, input_area.y, &ch, cur_style);
            if !after.is_empty() {
                text(f, cur_x + 1, input_area.y, &after[after.chars().next().unwrap().len_utf8()..], field_style, inner_w.saturating_sub(3 + before.chars().count() as u16 + 1));
            }
        }
        let body_top = dd.y + 3;
        let body_bottom = dd.bottom() - 1;
        for (i, fidx) in self.finder.filtered.iter().enumerate().skip(self.finder.scroll) {
            let y = body_top + (i - self.finder.scroll) as u16;
            if y >= body_bottom { break; }
            let Some(item) = self.finder.items.get(*fidx) else { continue };
            let sel = i == self.finder.selected;
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in (dd.x + 1)..(dd.x + 1 + inner_w + 2) { put(f, cx, y, " ", Style::default().bg(bg)); }
            let fg = if sel { RColor::Black } else { theme.fg };
            let marker = if sel { "▸" } else { " " };
            put(f, dd.x + 2, y, marker, Style::default().fg(if sel { RColor::Black } else { theme.accent }).bg(bg));
            let label = match item {
                FinderItem::Session { name, .. } => name.clone(),
                FinderItem::Tab { session_name, tab_name, .. } => format!("{session_name} / {tab_name}"),
            };
            text(f, dd.x + 4, y, &label, Style::default().fg(fg).bg(bg).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() }), inner_w.saturating_sub(4));
        }
        if self.finder.filtered.is_empty() {
            let st = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
            text(f, dd.x + 4, body_top, "no matches", st, inner_w);
        }
    }
}
