use anyhow::Result;
use ratatui::Frame;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::bindings::leader_hint;
use super::overlays::MENU_BTN;
use super::{App, Mode};
use crate::layout::TreeGeom;
use crate::agents::AgentStatus;
use crate::vt;

impl App {
    pub(super) fn frame<B: Backend>(&mut self, terminal: &mut ratatui::Terminal<B>) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        self.poll_exits();
        if self.quit {
            return Ok(());
        }
        let size = terminal.size()?;
        self.term_size = (size.width, size.height);
        self.refresh_branches();
        self.refresh_workspace_follow();
        self.refresh_ai_cli();
        self.refresh_agent_statuses();
        self.log_agent_statuses();
        let area = Rect::new(0, 0, size.width, size.height);
        let geom = self.active_geom();

        for pg in &geom.panes {
            if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                let inner = pg.inner();
                let key = (inner.width, inner.height);
                if self.last_sizes.get(&pg.pane_id) != Some(&key) {
                    pane.resize(inner.width, inner.height);
                    self.last_sizes.insert(pg.pane_id, key);
                }
            }
        }

        let focused = self.sessions[self.active].tree.focus;
        // When focus moves, re-render the old and new panes so the cursor
        // highlight is drawn/cleared even if neither produced output.
        if self.last_focused != Some(focused) {
            if let Some(old) = self.last_focused {
                if let Some(p) = self.panes.get_mut(&old) {
                    p.dirty = true;
                }
            }
            if let Some(p) = self.panes.get_mut(&focused) {
                p.dirty = true;
            }
            self.last_focused = Some(focused);
        }
        let geom_ref = &geom;
        terminal.draw(|f| self.render(f, area, geom_ref, focused))?;
        self.place_cursor(terminal, &geom, focused)?;
        Ok(())
    }

    fn render(&mut self, f: &mut Frame, size: Rect, geom: &TreeGeom, focused: u64) {
        // Note: no global fill over the pane area, so unchanged (non-dirty)
        // panes keep the cells ratatui retains from their last render.
        for pg in &geom.panes {
            let title = self.pane_title(pg.pane_id, pg.pane_id == focused);
            let blocked = self
                .panes
                .get(&pg.pane_id)
                .map(|p| p.is_ai_cli())
                .unwrap_or(false)
                && self.agent_status_cache.get(&pg.pane_id).copied() == Some(AgentStatus::Blocked);
            let title = if blocked { format!("{title}· blocked ") } else { title };
            self.render_pane_frame(f, pg.rect, pg.pane_id == focused, blocked, &title);
        }
        for pg in &geom.panes {
            if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                let inner = pg.inner();
                if inner.width > 0 && inner.height > 0 {
                    // Whether a link modifier is held right now drives the
                    // underline of links in the next render.
                    pane.link_mods = self.link_mods;
                    // Re-render dirty rows into the pane's retained cache;
                    // unchanged rows are kept and blitted back (no FFI scan).
                    if pane.dirty {
                        // Keep the previous cache unless it was for a different
                        // rect (moved/resized), so clean rows survive.
                        let recreate = self
                            .pane_cache
                            .get(&pg.pane_id)
                            .map(|c| c.area != inner)
                            .unwrap_or(true);
                        if recreate {
                            pane.full_redraw = true;
                            self.pane_cache.insert(pg.pane_id, Buffer::empty(inner));
                        }
                        if let Some(cached) = self.pane_cache.get_mut(&pg.pane_id) {
                            let status = pane.render_dirty(inner, pg.pane_id == focused, cached);
                            if let Some(status) = status {
                                self.agent_status_cache.insert(pg.pane_id, status);
                            }
                        }
                    }
                    if let Some(cached) = self.pane_cache.get(&pg.pane_id) {
                        let dst = f.buffer_mut();
                        for (i, src) in cached.content.iter().enumerate() {
                            let (x, y) = cached.pos_of(i);
                            if let Some(dst_cell) = dst.cell_mut((x, y)) {
                                *dst_cell = src.clone();
                            }
                        }
                    }
                    let sb = pane.scrollbar_data();
                    self.render_scrollbar(f, &sb, inner);
                }
            }
        }

        self.render_pane_numbers(f);

        if self.sidebar_open {
            self.render_sidebar(f, size);
        }

        self.render_status(f, size);
        self.render_menu(f);
        self.render_ctx_menu(f);
        self.render_name_popup(f);
        self.render_update_notice(f);
        self.render_keybind_overlay(f);
        self.render_settings(f);
        self.render_worktree_picker(f);
        self.render_toast(f);
    }

    /// Draw the `leader+q` pane-number overlay: a numbered badge on each pane.
    /// Expires after `PANE_NUMBERS_TIMEOUT` even without a keypress.
    fn render_pane_numbers(&mut self, f: &mut Frame) {
        let Some(started) = self.pane_numbers else { return };
        if started.elapsed() > super::PANE_NUMBERS_TIMEOUT {
            self.pane_numbers = None;
            return;
        }
        let ids = self.sessions[self.active].tree.pane_ids();
        if ids.len() < 2 {
            return;
        }
        let style = Style::default().fg(RColor::Black).bg(self.theme.accent).add_modifier(Modifier::BOLD);
        let geom = self.active_geom();
        for (i, pid) in ids.iter().enumerate() {
            let Some(digit) = char::from_digit((i + 1) as u32, 10) else { continue };
            let Some(pg) = geom.panes.iter().find(|p| p.pane_id == *pid) else { continue };
            let inner = pg.inner();
            put(f, inner.x + inner.width / 2, inner.y + inner.height / 2, &digit.to_string(), style);
        }
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

    fn pane_title(&self, pid: u64, focused: bool) -> String {
        let base = self.pane_label(pid);
        if focused && self.sessions[self.active].zoom {
            format!("{base}(zoom) ")
        } else {
            base
        }
    }

    fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, blocked: bool, title: &str) {
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        let border = if blocked {
            // A blocked AI pane glows orange even when it does not have focus.
            self.theme.orange
        } else if focused {
            self.theme.accent
        } else {
            self.theme.border_idle
        };
        // Native background: the frame is just line glyphs over the host
        // terminal's background, matching the pane content.
        let border_style = Style::default().fg(border).bg(RColor::Reset);
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        put(f, x0, y0, "┌", border_style);
        put(f, x1, y0, "┐", border_style);
        put(f, x0, y1, "└", border_style);
        put(f, x1, y1, "┘", border_style);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border_style);
            put(f, x, y1, "─", border_style);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border_style);
            put(f, x1, y, "│", border_style);
        }
        // Title chip: filled accent when focused, orange when a blocked AI
        // pane, plain otherwise.
        let max = rect.width.saturating_sub(2) as usize;
        let chip = if focused {
            Style::default()
                .fg(RColor::Black)
                .bg(self.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else if blocked {
            Style::default()
                .fg(RColor::Black)
                .bg(self.theme.orange)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.fg).bg(RColor::Reset)
        };
        for (i, ch) in title.chars().take(max).enumerate() {
            put(f, x0 + 1 + i as u16, y0, &ch.to_string(), chip);
        }
    }

    fn render_scrollbar(&self, f: &mut Frame, sb: &vt::TerminalScrollbar, inner: Rect) {
        let total = sb.total as usize;
        let screen = sb.len as usize;
        if total <= screen || screen == 0 {
            return;
        }
        let hist = total - screen;
        let bar_h = inner.height as usize;
        let thumb = ((screen * bar_h) / total).max(1).min(bar_h);
        let off = sb.offset as usize;
        let y_max = bar_h.saturating_sub(thumb);
        let y_start = off.saturating_mul(y_max) / hist.max(1);
        let x = inner.x + inner.width.saturating_sub(1);
        for i in 0..bar_h {
            let y = inner.y + i as u16;
            if i >= y_start && i < y_start + thumb {
                put(f, x, y, "▐", Style::default().fg(self.theme.secondary));
            } else {
                put(f, x, y, "░", Style::default().fg(self.theme.panel_sep));
            }
        }
    }

    fn render_status(&self, f: &mut Frame, size: Rect) {
        let area = Rect::new(0, size.height.saturating_sub(1), size.width, 1);
        fill(f, area, RColor::Reset);
        let session = &self.sessions[self.active];
        let n = session.tree.pane_count();
        let mode = if self.mode == Mode::Leader { "LEADER" } else { "NORMAL" };
        let mode_style = if self.mode == Mode::Leader {
            Style::default().fg(RColor::Black).bg(self.theme.secondary).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(RColor::Black).bg(self.theme.accent)
        };

        // Mode chip at the left edge.
        let chip = format!(" {} ", mode);
        let chip_w = chip.chars().count() as u16;
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(chip, mode_style)])),
            Rect::new(0, area.y, chip_w, 1),
        );

        // MENU button right after the chip, then the remaining spans.
        let btn_w = MENU_BTN.chars().count() as u16;
        let btn_x = self.menu_btn_x();
        let btn_style = if self.menu.open {
            Style::default().fg(RColor::Black).bg(self.theme.secondary).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.fg).bg(RColor::Reset).add_modifier(Modifier::BOLD)
        };
        text(f, btn_x, area.y, MENU_BTN, btn_style, btn_w);

        let mut spans: Vec<Span> = vec![
            Span::raw(" "),
            Span::styled(session.name.clone(), Style::default().fg(self.theme.fg).bg(RColor::Reset)),
            Span::styled(format!(" · {n} panes"), Style::default().fg(self.theme.panel_muted).bg(RColor::Reset)),
        ];
        if session.zoom {
            spans.push(Span::styled(
                " · zoomed",
                Style::default().fg(self.theme.secondary).bg(RColor::Reset),
            ));
        }
        if !self.sidebar_open {
            spans.push(Span::styled(
                " · sidebar hidden",
                Style::default().fg(self.theme.panel_muted).bg(RColor::Reset),
            ));
        }
        if let Some((msg, t)) = &self.notice {
            if t.elapsed() < std::time::Duration::from_secs(2) {
                spans.push(Span::styled(
                    format!(" ⚠ {msg} "),
                    Style::default().fg(self.theme.secondary).bg(RColor::Reset),
                ));
            }
        }

        let start = btn_x + btn_w;
        let left_w = spans
            .iter()
            .map(|s| s.content.chars().count() as u16)
            .sum::<u16>()
            .min(area.width.saturating_sub(start));
        if left_w > 0 {
            f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(start, area.y, left_w, 1));
        }

        if self.mode == Mode::Leader {
            let hint = leader_hint(&self.keymap);
            let avail = area.width.saturating_sub(start.saturating_add(left_w));
            if avail > 0 {
                // Clip the hint to the available width instead of hiding it: on
                // narrow terminals the head ("?: help · …") still shows.
                let hint: String = hint.chars().take(avail as usize).collect();
                let hint_w = hint.chars().count() as u16;
                let x = area.width.saturating_sub(hint_w);
                let hint_style = Style::default()
                    .fg(RColor::Black)
                    .bg(self.theme.secondary)
                    .add_modifier(Modifier::BOLD);
                f.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(hint, hint_style)])),
                    Rect::new(x, area.y, hint_w, 1),
                );
            }
        }
    }

    /// Draw the startup update banner (top-right, two lines) with a red ✕
    /// close button.
    fn render_update_notice(&self, f: &mut Frame) {
        let Some(rect) = self.update_notice_rect() else { return };
        let Some((line1, line2)) = self.update_notice_lines() else { return };
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        let border = Style::default().fg(self.theme.panel_muted).bg(self.theme.panel_sep);
        fill(f, rect, self.theme.panel_sep);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y1, "└", border);
        put(f, x1, y1, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y1, "─", border);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border);
            put(f, x1, y, "│", border);
        }
        put(
            f,
            x0 + 2,
            y0 + 1,
            "✕",
            Style::default().fg(self.theme.red).bg(self.theme.panel_sep).add_modifier(Modifier::BOLD),
        );
        let inner_w = rect.width.saturating_sub(2);
        text(
            f,
            x0 + 5,
            y0 + 1,
            &line1,
            Style::default().fg(self.theme.fg).bg(self.theme.panel_sep),
            inner_w.saturating_sub(6),
        );
        text(
            f,
            x0 + 5,
            y0 + 2,
            &line2,
            Style::default().fg(self.theme.fg).bg(self.theme.panel_sep),
            inner_w.saturating_sub(5),
        );
    }

    /// Draw the transient toast (e.g. "copied to clipboard") as a small
    /// bordered popup centered horizontally near the top, that fades out after
    /// a short time. Non-interactive.
    fn render_toast(&self, f: &mut Frame) {
        let Some((msg, t)) = &self.toast else { return };
        if t.elapsed() > std::time::Duration::from_millis(1600) {
            return;
        }
        let area = f.area();
        let w = (msg.chars().count() as u16 + 4).min(area.width.saturating_sub(2));
        if w < 4 || area.height < 3 {
            return;
        }
        let x0 = area.width.saturating_sub(w) / 2;
        let y0 = 1;
        let x1 = x0 + w - 1;
        let border = Style::default().fg(self.theme.secondary).bg(self.theme.panel_sep);
        fill(f, Rect::new(x0, y0, w, 3), self.theme.panel_sep);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y0 + 2, "└", border);
        put(f, x1, y0 + 2, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y0 + 2, "─", border);
        }
        put(f, x0, y0 + 1, "│", border);
        put(f, x1, y0 + 1, "│", border);
        let st = Style::default()
            .fg(self.theme.fg)
            .bg(self.theme.panel_sep)
            .add_modifier(Modifier::BOLD);
        text(f, x0 + 2, y0 + 1, msg, st, w - 2);
    }

    fn place_cursor<B: Backend>(
        &mut self,
        terminal: &mut ratatui::Terminal<B>,
        geom: &TreeGeom,
        focused: u64,
    ) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        if self.popup.open {
            if let Some((x, y)) = self.name_popup_input_cursor() {
                terminal.set_cursor_position((x, y))?;
                terminal.show_cursor()?;
                return Ok(());
            }
        }
        if let Some(pg) = geom.panes.iter().find(|p| p.pane_id == focused) {
            if let Some(pane) = self.panes.get(&pg.pane_id) {
                let inner = pg.inner();
                if let Some((cx, cy)) = pane.cursor_pos() {
                    let x = inner.x + cx;
                    let y = inner.y + cy;
                    if x < inner.x + inner.width && y < inner.y + inner.height {
                        terminal.set_cursor_position((x, y))?;
                        terminal.show_cursor()?;
                        return Ok(());
                    }
                }
            }
        }
        terminal.hide_cursor()?;
        Ok(())
    }
}

pub(super) fn put(f: &mut Frame, x: u16, y: u16, ch: &str, style: Style) {
    let a = f.area();
    if x >= a.width || y >= a.height {
        return;
    }
    let c = f.buffer_mut().cell_mut((x, y)).unwrap();
    c.set_symbol(ch).set_style(style);
}

pub(super) fn text(f: &mut Frame, x: u16, y: u16, s: &str, style: Style, max_width: u16) {
    for (i, ch) in s.chars().take(max_width as usize).enumerate() {
        put(f, x + i as u16, y, &ch.to_string(), style);
    }
}

pub(super) fn fill(f: &mut Frame, area: Rect, color: RColor) {
    for y in area.y..(area.y + area.height) {
        for x in area.x..(area.x + area.width) {
            let a = f.area();
            if x >= a.width || y >= a.height {
                continue;
            }
            let c = f.buffer_mut().cell_mut((x, y)).unwrap();
            c.set_symbol(" ").set_style(Style::default().bg(color));
        }
    }
}
