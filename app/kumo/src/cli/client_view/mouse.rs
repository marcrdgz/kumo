//! Mouse input: dispatch of clicks, drags, wheel and hover across chrome and panes.

use super::*;

impl View {
    // ------------------------------------------------------------------
    // Mouse
    // ------------------------------------------------------------------

    pub fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        // Clicks and drags write pane bytes themselves: flush the pending wheel
        // batch first so ordering against them is preserved. Hover moves and
        // further scrolling never write bytes, so they can keep coalescing.
        if !matches!(
            m.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp | MouseEventKind::Moved
        ) {
            self.flush_wheel()?;
        }
        self.set_link_mods(m.modifiers.intersects(link_modifiers()));
        if self.mode == Mode::Copy {
            return self.on_copy_mouse(m);
        }
        if self.mode == Mode::Inbox {
            return self.on_inbox_mouse(m);
        }
        let x = m.column;
        let y = m.row;
        // Tab bar hover tracking (y==0) — update before click handling so close "x" appears
        if m.kind == MouseEventKind::Moved {
            let new_hover = self.tab_hit(x, y).map(|(idx, _)| idx);
            if new_hover != self.tab_hover {
                self.tab_hover = new_hover;
                self.update_tab_rects();
                self.mark_dirty();
            }
            // Sidebar close hover for project worktrees
            let w = self.effective_sidebar_width();
            let mut new_side_hover: Option<usize> = None;
            if self.sidebar_open && self.sidebar_layout() == SidebarLayout::Project && x < w && y >= 3 && y <= self.sidebar_footer_y() {
                for (ry, row) in self.sidebar_rows() {
                    if ry == y {
                        match row {
                            SidebarRow::Worktree(i) | SidebarRow::Branch(i, _) => new_side_hover = Some(i),
                            _ => {}
                        }
                        break;
                    }
                }
            }
            if new_side_hover != self.sidebar_hover {
                self.sidebar_hover = new_side_hover;
                self.mark_dirty();
            }
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) && self.update_notice_close_at(x, y) {
            if let Some((key, _)) = self.update_notice.clone() {
                let _ = self.send(&Command::UpdateDismiss { key });
            }
            self.update_notice = None;
            return Ok(());
        }
        if self.keybind_overlay.open {
            match m.kind {
                MouseEventKind::ScrollDown => {
                    let len = command_palette_matches(&self.keymap, &self.keybind_overlay.input).len();
                    if len > 0 {
                        self.keybind_overlay.selected = (self.keybind_overlay.selected + 1).min(len - 1);
                        self.normalize_command_palette_scroll();
                    }
                }
                MouseEventKind::ScrollUp => {
                    self.keybind_overlay.selected = self.keybind_overlay.selected.saturating_sub(1);
                    self.normalize_command_palette_scroll();
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(item) = self.command_palette_item_at(x, y) {
                        let matches = command_palette_matches(&self.keymap, &self.keybind_overlay.input);
                        if let Some(keymap_idx) = matches.get(item).copied() {
                            let action = self.keymap[keymap_idx].action;
                            self.keybind_overlay.open = false;
                            self.run_action(action)?;
                        }
                    } else if !self.keybind_overlay_rect().map(|rect| rect.contains(Position::new(x, y))).unwrap_or(false) {
                        self.keybind_overlay.open = false;
                    }
                }
                _ => {}
            }
            self.mark_dirty();
            return Ok(());
        }
        if self.session_close_confirm.open {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                if let Some(rect) = self.session_close_confirm_button_rect(true) {
                    if rect.contains(Position::new(x, y)) {
                        self.submit_worktree_removal();
                        self.mark_dirty();
                        return Ok(());
                    }
                }
                if let Some(rect) = self.session_close_confirm_button_rect(false) {
                    if rect.contains(Position::new(x, y)) {
                        self.session_close_confirm.open = false;
                        self.mark_dirty();
                        return Ok(());
                    }
                }
                if self.session_close_confirm_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                    return Ok(());
                }
                self.session_close_confirm.open = false;
                self.mark_dirty();
            }
            return Ok(());
        }
        if self.worktree_create.open {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                if let Some(tab) = self.worktree_create_tab_at(x, y) {
                    if tab != WorktreeCreateTab::Jira {
                        self.worktree_create.tab = tab;
                        self.worktree_create.focus = WorktreeCreateFocus::CreateFrom;
                        self.mark_dirty();
                    }
                    return Ok(());
                }
                if let Some(rect) = self.worktree_create_input_rect(WorktreeCreateFocus::CreateFrom) {
                    if rect.contains(Position::new(x, y)) {
                        self.worktree_create.focus = WorktreeCreateFocus::CreateFrom;
                        let len = self.worktree_create.create_from.chars().count();
                        let col = (x.saturating_sub(rect.x)) as usize;
                        self.worktree_create.cursor = col.min(len);
                        self.mark_dirty();
                        return Ok(());
                    }
                }
                if self.worktree_create.advanced {
                    for focus in [WorktreeCreateFocus::BranchOverride, WorktreeCreateFocus::Note, WorktreeCreateFocus::Agent] {
                        if let Some(rect) = self.worktree_create_input_rect(focus) {
                            if rect.contains(Position::new(x, y)) {
                                self.worktree_create.focus = focus;
                                let len = match focus {
                                    WorktreeCreateFocus::BranchOverride => self.worktree_create.branch_override.chars().count(),
                                    WorktreeCreateFocus::Note => self.worktree_create.note.chars().count(),
                                    WorktreeCreateFocus::Agent => self.worktree_create.agent.chars().count(),
                                    _ => 0,
                                };
                                let col = (x.saturating_sub(rect.x)) as usize;
                                match focus {
                                    WorktreeCreateFocus::BranchOverride => self.worktree_create.branch_cursor = col.min(len),
                                    WorktreeCreateFocus::Note => self.worktree_create.note_cursor = col.min(len),
                                    WorktreeCreateFocus::Agent => self.worktree_create.agent_cursor = col.min(len),
                                    _ => {}
                                }
                                self.mark_dirty();
                                return Ok(());
                            }
                        }
                    }
                }
                if let Some(rect) = self.worktree_create_advanced_toggle_rect() {
                    if rect.contains(Position::new(x, y)) {
                        self.worktree_create.advanced = !self.worktree_create.advanced;
                        if !self.worktree_create.advanced {
                            self.worktree_create.focus = WorktreeCreateFocus::CreateFrom;
                        }
                        self.mark_dirty();
                        return Ok(());
                    }
                }
                if let Some(rect) = self.worktree_create_button_rect(true) {
                    if rect.contains(Position::new(x, y)) {
                        self.commit_worktree_create();
                        return Ok(());
                    }
                }
                if let Some(rect) = self.worktree_create_button_rect(false) {
                    if rect.contains(Position::new(x, y)) {
                        self.worktree_create.open = false;
                        self.mark_dirty();
                        return Ok(());
                    }
                }
                if self.worktree_create_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                    return Ok(());
                }
                self.worktree_create.open = false;
                self.mark_dirty();
            }
            return Ok(());
        }
        if self.worktree_picker.open {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(i) = self.worktree_picker_item_at(x, y) {
                        self.worktree_picker.selected = i;
                        self.pick_worktree(i);
                    } else if self.worktree_picker_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        // Inside the picker but off a row: modal no-op.
                    } else {
                        self.worktree_picker.open = false;
                    }
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.worktree_picker_item_at(x, y) {
                        if self.worktree_picker.selected != i {
                            self.worktree_picker.selected = i;
                            self.mark_dirty();
                        }
                    }
                }
                MouseEventKind::ScrollDown => self.worktree_picker_move(1),
                MouseEventKind::ScrollUp => self.worktree_picker_move(-1),
                _ => {}
            }
            return Ok(());
        }
        if self.settings.open {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(tab) = self.settings_tab_at(x, y) {
                        self.settings_set_tab(tab);
                        return Ok(());
                    }
                    if let Some(i) = self.settings_item_at(x, y) {
                        self.settings.selected = i;
                        self.settings_activate_item(i);
                        return Ok(());
                    }
                    if self.settings_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        return Ok(());
                    }
                    self.settings.open = false;
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.settings_item_at(x, y) {
                        if self.settings.selected != i {
                            self.settings.selected = i;
                            self.mark_dirty();
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        if self.finder.open {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(i) = self.finder_item_at(x, y) {
                        self.finder.selected = i;
                        self.finder_commit();
                    } else if self.finder_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        // click inside chrome but off a row: no-op
                    } else {
                        self.finder.open = false;
                        self.mark_dirty();
                    }
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.finder_item_at(x, y) {
                        if self.finder.selected != i { self.finder.selected = i; self.mark_dirty(); }
                    }
                }
                MouseEventKind::ScrollDown => {
                    if self.finder.selected + 1 < self.finder.filtered.len() {
                        self.finder.selected += 1;
                        let max_vis = 8usize;
                        if self.finder.selected >= self.finder.scroll + max_vis { self.finder.scroll += 1; }
                        self.mark_dirty();
                    }
                }
                MouseEventKind::ScrollUp if self.finder.selected > 0 => {
                    self.finder.selected -= 1;
                    if self.finder.selected < self.finder.scroll { self.finder.scroll = self.finder.selected; }
                    self.mark_dirty();
                }
                _ => {}
            }
            return Ok(());
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.popup.open {
                    if let Some(btn) = self.name_popup_button_at(x, y) {
                        match btn {
                            PopupBtn::Enter => self.commit_name(),
                            PopupBtn::Cancel => self.popup.open = false,
                        }
                        return Ok(());
                    }
                    if self.name_popup_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false) {
                        return Ok(());
                    }
                    self.popup.open = false;
                }
                if self.menu.open {
                    if let Some(i) = self.menu_item_at(x, y) {
                        self.menu_select(i)?;
                        return Ok(());
                    }
                    if self.menu_btn_at(x, y) {
                        self.menu.open = false;
                        return Ok(());
                    }
                    self.menu.open = false;
                }
                if self.ctx_menu.open {
                    if let Some(i) = self.ctx_menu_item_at(x, y) {
                        self.ctx_menu_select(i)?;
                        return Ok(());
                    }
                    if self.ctx_menu_at(x, y) {
                        return Ok(());
                    }
                    self.ctx_menu.open = false;
                }
                // A corner toast click focuses the agent's pane and dismisses
                // the toast (checked after the modals above so an open popup
                // keeps priority over transient chrome).
                if let Some(i) = self.agent_toast_at(x, y) {
                    let t = self.agent_toasts.remove(i);
                    if let Some(session) = self
                        .layout
                        .as_ref()
                        .and_then(|l| {
                            l.sessions
                                .iter()
                                .find(|s| s.tabs.iter().any(|tab| Self::find_layout_pane_in_tab(tab, t.pane_id).is_some()))
                                .map(|s| s.name.clone())
                        })
                    {
                        let _ = self.send(&Command::PaneFocus { session, pane_id: t.pane_id });
                    }
                    self.mark_dirty();
                    return Ok(());
                }
                if self.menu_btn_at(x, y) {
                    self.menu.open = !self.menu.open;
                    self.menu.selected = 0;
                    return Ok(());
                }
                if y == 0 && self.tabs_area().contains(Position::new(x, y)) {
                    if let Some(r) = self.tab_left_arrow_rect() {
                        if r.contains(Position::new(x, y)) {
                            self.tab_scroll = self.tab_scroll.saturating_sub(1);
                            self.update_tab_rects();
                            self.mark_dirty();
                            return Ok(());
                        }
                    }
                    if let Some(r) = self.tab_right_arrow_rect() {
                        if r.contains(Position::new(x, y)) {
                            if let Some(sess) = self.active_session() {
                                self.tab_scroll = (self.tab_scroll + 1).min(sess.tabs.len().saturating_sub(1));
                                self.update_tab_rects();
                                self.mark_dirty();
                            }
                            return Ok(());
                        }
                    }
                    if let Some(pr) = self.plus_rect {
                        if pr.contains(Position::new(x, y)) {
                            if let Some(sess) = self.active_session().map(|s| s.name.clone()) {
                                let _ = self.send(&Command::TabNew { session: sess, name: None, workspace: None });
                            }
                            return Ok(());
                        }
                    }
                }
                if let Some((idx, is_close)) = self.tab_hit(x, y) {
                    if is_close {
                        if let Some(sess) = self.active_session().cloned() {
                            let tab_name = sess.tabs.get(idx).map(|t| t.name.clone()).unwrap_or_default();
                            let _ = self.send(&Command::TabClose { session: sess.name, tab: Some(tab_name) });
                        }
                    } else {
                        self.begin_tab_reorder(idx);
                    }
                    return Ok(());
                }
                let w = self.effective_sidebar_width();
                if self.sidebar_open && x == w && y < self.rows.saturating_sub(self.status_h()) {
                    self.sidebar_width_drag = Some(SidebarWidthDrag { start_x: x, start_width: w });
                    return Ok(());
                }
                if self.sidebar_open && x < w && self.begin_sidebar_reorder(y) {
                    return Ok(());
                }
                if self.sidebar_open && x < w && self.sidebar_hit(x, y) {
                    return Ok(());
                }
                if let Some((split_id, dir, area)) = self.splitter_at(x, y) {
                    self.drag = Some(SplitDrag { split_id, dir, area });
                    return Ok(());
                }
                if m.modifiers.intersects(link_modifiers()) {
                    if let Some((pid, rect)) = self.pane_at(x, y) {
                        let inner = PaneGeom { pane_id: pid, rect }.inner();
                        let col = x.saturating_sub(inner.x);
                        let row = y.saturating_sub(inner.y);
                        if let Some(url) = self.link_at(pid, col, row) {
                            let session = self.active_session().map(|s| s.name.clone());
                            if let Some(session) = session {
                                let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                            }
                            crate::cli::util::open_url(&url);
                            return Ok(());
                        }
                    }
                }
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                    let inner = PaneGeom { pane_id: pid, rect }.inner();
                    let col = x.saturating_sub(inner.x);
                    let row = y.saturating_sub(inner.y);
                    let reporting = self.pane_mouse_reporting(pid);
                    if reporting {
                        self.pending_click = Some(PendingClick { pane_id: pid, col, row });
                        let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                        let bytes = sgr_mouse(b, col + 1, row + 1, false);
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    } else {
                        if let Some(old) = self.sel {
                            if old.pane_id != pid {
                                self.sel = None;
                            }
                        }
                        self.sel = Some(Sel { pane_id: pid, start: (col, row), end: (col, row) });
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.popup.open || self.menu.open {
                    return Ok(());
                }
                if self.ctx_menu_at(x, y) {
                    self.ctx_menu.open = false;
                    return Ok(());
                }
                if y == 0 && self.tabs_area().contains(Position::new(x, y)) {
                    if let Some((tab_idx, _)) = self.tab_hit(x, y) {
                        if let Some(s_idx) = self.layout.as_ref().and_then(|l| l.sessions.iter().position(|s| Some(&s.name)==l.active.as_ref())) {
                            self.open_ctx_menu(x, y, CtxTarget::Tab(s_idx, tab_idx));
                            return Ok(());
                        }
                    } else {
                        // Right-click on empty tab bar: new tab menu
                        if let Some(s_idx) = self.layout.as_ref().and_then(|l| l.sessions.iter().position(|s| Some(&s.name)==l.active.as_ref())) {
                            let t_idx = self.active_session().map(|s| s.active_tab).unwrap_or(0);
                            self.open_ctx_menu(x, y, CtxTarget::Tab(s_idx, t_idx));
                            return Ok(());
                        }
                    }
                }
                if let Some(idx) = self.sidebar_session_at(x, y) {
                    self.open_ctx_menu(x, y, CtxTarget::Session(idx));
                    return Ok(());
                }
                if let Some((pid, _)) = self.pane_at(x, y) {
                    self.open_ctx_menu(x, y, CtxTarget::Pane(pid));
                } else {
                    self.ctx_menu.open = false;
                }
                return Ok(());
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.update_tab_reorder(x) {
                    return Ok(());
                }
                if self.update_sidebar_reorder(y) {
                    return Ok(());
                }
                if let Some(d) = self.sidebar_width_drag {
                    let delta = x as i32 - d.start_x as i32;
                    let w = (d.start_width as i32 + delta).clamp(MIN_SIDEBAR_WIDTH as i32, MAX_SIDEBAR_WIDTH as i32) as u16;
                    let clamped = w.min(self.cols.saturating_sub(5).max(MIN_SIDEBAR_WIDTH));
                    self.sidebar_width = clamped;
                    self.recompute_geometry();
                    self.mark_dirty();
                    return Ok(());
                }
                if let Some(d) = self.sidebar_drag {
                    let avail = self.content_region_h() as usize;
                    let max_spaces = Self::spaces_split_max(avail);
                    let delta = y as i32 - d.start_y as i32;
                    let h = (d.start_split as i32 + delta).clamp(0, max_spaces as i32) as u16;
                    self.sidebar_split = Some(h);
                    self.mark_dirty();
                    return Ok(());
                }
                if let Some(drag) = self.drag {
                    let ratio = match drag.dir {
                        SplitDir::Vertical => {
                            (x.saturating_sub(drag.area.x)) as f32 / (drag.area.width.max(1)) as f32
                        }
                        SplitDir::Horizontal => {
                            (y.saturating_sub(drag.area.y)) as f32 / (drag.area.height.max(1)) as f32
                        }
                    };
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneResizeTo {
                            session,
                            split_id: drag.split_id,
                            ratio: ratio.clamp(0.05, 0.95),
                        });
                    }
                    return Ok(());
                }
                let sel = self.sel;
                if let Some(sel) = sel {
                    if let Some((pid, rect)) = self.pane_at(x, y) {
                        if pid == sel.pane_id {
                            let inner = PaneGeom { pane_id: pid, rect }.inner();
                            let c = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let r = y.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                            self.sel = Some(Sel { pane_id: pid, start: sel.start, end: (c, r) });
                            // Repaint live so the highlight follows the pointer.
                            self.mark_dirty();
                        }
                    }
                    return Ok(());
                }
                if let Some(pc) = self.pending_click {
                    let pos = self
                        .pane_at(x, y)
                        .filter(|(pid, _)| *pid == pc.pane_id)
                        .map(|(pid, rect)| {
                            let i = PaneGeom { pane_id: pid, rect }.inner();
                            (x.saturating_sub(i.x) + 1, y.saturating_sub(i.y) + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    let bytes = sgr_mouse(b + 32, pos.0, pos.1, false);
                    let _ = self.send(&Command::PaneWrite { pane_id: pc.pane_id, bytes });
                    return Ok(());
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.finish_tab_reorder() {
                    return Ok(());
                }
                if self.finish_sidebar_reorder() {
                    return Ok(());
                }
                self.drag = None;
                self.sidebar_drag = None;
                self.sidebar_width_drag = None;
                if let Some(pc) = self.pending_click.take() {
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    let up = self
                        .pane_at(x, y)
                        .filter(|(pid, _)| *pid == pc.pane_id)
                        .map(|(pid, rect)| {
                            let i = PaneGeom { pane_id: pid, rect }.inner();
                            (x.saturating_sub(i.x) + 1, y.saturating_sub(i.y) + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    let bytes = sgr_mouse(b, up.0, up.1, true);
                    let _ = self.send(&Command::PaneWrite { pane_id: pc.pane_id, bytes });
                } else if let Some(sel) = self.sel {
                    if sel.start != sel.end {
                        let text = self.selection_text(&sel);
                        if !text.is_empty() {
                            crate::cli::util::copy_to_clipboard(&text);
                            self.status_msg = Some(("copied to clipboard".to_string(), Instant::now()));
                        }
                    } else {
                        self.sel = None;
                    }
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if self.sidebar_open && self.sidebar_wheel(x, y, up) {
                    return Ok(());
                }
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    let session = self.active_session().map(|s| s.name.clone());
                    if let Some(session) = session {
                        let _ = self.send(&Command::PaneFocus { session, pane_id: pid });
                    }
                    let inner = PaneGeom { pane_id: pid, rect }.inner();
                    let col = x.saturating_sub(inner.x) + 1;
                    let row = y.saturating_sub(inner.y) + 1;
                    if self.pane_mouse_reporting(pid) {
                        let b = if up { 64 } else { 65 };
                        let bytes = sgr_mouse(b, col, row, false);
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    } else if self.pane_alt_screen(pid) {
                        let bytes: Vec<u8> = if up { b"\x1b[A".to_vec() } else { b"\x1b[B".to_vec() };
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    } else {
                        let _ = self.send(&Command::PaneScroll { pane_id: pid, up });
                    }
                }
            }
            MouseEventKind::Moved => {
                if self.popup.open {
                    // Hover highlights a popup button (repaint on change).
                    let hover = self.name_popup_button_at(x, y);
                    if self.popup.hover != hover {
                        self.popup.hover = hover;
                        self.mark_dirty();
                    }
                    return Ok(());
                }
                if self.menu.open {
                    // Modal menu: hovering moves the selection like j/k.
                    if let Some(i) = self.menu_item_at(x, y) {
                        if self.menu.selected != i {
                            self.menu.selected = i;
                            self.mark_dirty();
                        }
                    }
                    return Ok(());
                }
                if self.ctx_menu.open {
                    if let Some(i) = self.ctx_menu_item_at(x, y) {
                        if self.ctx_menu.selected != i {
                            self.ctx_menu.selected = i;
                            self.mark_dirty();
                        }
                    }
                    return Ok(());
                }
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    if self.pane_mouse_reporting(pid) {
                        let inner = PaneGeom { pane_id: pid, rect }.inner();
                        let col = x.saturating_sub(inner.x) + 1;
                        let row = y.saturating_sub(inner.y) + 1;
                        let bytes = sgr_mouse(35, col, row, false);
                        let _ = self.send(&Command::PaneWrite { pane_id: pid, bytes });
                    }
                }
            }
            _ => {}
        }
        self.mark_dirty();
        Ok(())
    }

    fn pane_mouse_reporting(&self, pid: u64) -> bool {
        self.active_session()
            .and_then(|s| find_pane_in_session(s, pid))
            .map(|p| p.mouse_reporting)
            .unwrap_or(false)
    }

    fn pane_alt_screen(&self, pid: u64) -> bool {
        self.active_session()
            .and_then(|s| find_pane_in_session(s, pid))
            .map(|p| p.alt_screen)
            .unwrap_or(false)
    }

    fn link_at(&self, pid: u64, col: u16, row: u16) -> Option<String> {
        let grid = self.grids.get(&pid)?;
        let links = grid.links.get(&row)?;
        links.iter().find(|l| col >= l.start && col < l.end).map(|l| l.url.clone())
    }

    pub(super) fn selection_text(&self, sel: &Sel) -> String {
        let Some(grid) = self.grids.get(&sel.pane_id) else { return String::new() };
        let (mut r0, mut c0, mut r1, mut c1) = (sel.start.1, sel.start.0, sel.end.1, sel.end.0);
        if r1 < r0 || (r1 == r0 && c1 < c0) {
            std::mem::swap(&mut r0, &mut r1);
            std::mem::swap(&mut c0, &mut c1);
        }
        let mut lines = Vec::new();
        for row in r0..=r1 {
            let Some(cells) = grid.cells.get(row as usize) else { continue };
            let mut line = String::new();
            let start = if row == r0 { c0 } else { 0 };
            // Inclusive of the end column on the last row (both drag corners
            // select the cell under the pointer), like a terminal's selection.
            let end = if row == r1 { c1.saturating_add(1) } else { cells.len() as u16 };
            for (i, cell) in cells.iter().enumerate() {
                let ci = i as u16;
                if ci < start || ci >= end || cell.cell_width == 0 {
                    continue;
                }
                line.push_str(&cell.text);
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }
}
