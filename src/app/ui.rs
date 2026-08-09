use anyhow::Result;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::overlays::MENU_BTN;
use super::{App, BORDER_IDLE, Mode, PANEL_MUTED, PANEL_SEP, Term, YELLOW};
use crate::layout::TreeGeom;
use crate::pane::{ACCENT, FG};
use crate::vt;

impl App {
    pub(super) fn frame(&mut self, terminal: &mut Term) -> Result<()> {
        self.poll_exits();
        if self.quit {
            return Ok(());
        }
        let size = terminal.size()?;
        self.term_size = (size.width, size.height);
        self.refresh_branches();
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
            self.render_pane_frame(f, pg.rect, pg.pane_id == focused, &title);
        }
        for pg in &geom.panes {
            if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                let inner = pg.inner();
                if inner.width > 0 && inner.height > 0 {
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

        if self.sidebar_open {
            self.render_sidebar(f, size);
        }

        self.render_status(f, size);
        self.render_menu(f);
        self.render_name_popup(f);
    }

    fn pane_title(&self, pid: u64, focused: bool) -> String {
        let base = match self.panes.get(&pid) {
            Some(p) if p.is_ai_cli() => " AI CLI ".to_string(),
            Some(_) => {
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
            None => " pane ".to_string(),
        };
        if focused && self.sessions[self.active].zoom {
            format!("{base}(zoom) ")
        } else {
            base
        }
    }

    fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, title: &str) {
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        let border = if focused { ACCENT } else { BORDER_IDLE };
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
        // Title chip: filled accent when focused, plain otherwise.
        let max = rect.width.saturating_sub(2) as usize;
        let chip = if focused {
            Style::default()
                .fg(RColor::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG).bg(RColor::Reset)
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
                put(f, x, y, "▐", Style::default().fg(ACCENT));
            } else {
                put(f, x, y, "░", Style::default().fg(PANEL_SEP));
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
            Style::default().fg(RColor::Black).bg(YELLOW).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(RColor::Black).bg(ACCENT)
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
            Style::default().fg(RColor::Black).bg(YELLOW).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG).bg(RColor::Reset).add_modifier(Modifier::BOLD)
        };
        text(f, btn_x, area.y, MENU_BTN, btn_style, btn_w);

        let mut spans: Vec<Span> = vec![
            Span::raw(" "),
            Span::styled(session.name.clone(), Style::default().fg(FG).bg(RColor::Reset)),
            Span::styled(format!(" · {n} panes"), Style::default().fg(PANEL_MUTED).bg(RColor::Reset)),
        ];
        if session.zoom {
            spans.push(Span::styled(
                " · zoomed",
                Style::default().fg(YELLOW).bg(RColor::Reset),
            ));
        }
        if !self.sidebar_open {
            spans.push(Span::styled(
                " · sidebar hidden",
                Style::default().fg(PANEL_MUTED).bg(RColor::Reset),
            ));
        }
        if let Some((msg, t)) = &self.notice {
            if t.elapsed() < std::time::Duration::from_secs(2) {
                spans.push(Span::styled(
                    format!(" ⚠ {msg} "),
                    Style::default().fg(YELLOW).bg(RColor::Reset),
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
            let hint = " v: v-split · -: h-split · a: AI · c: new · x: close · z: zoom · h/j/k/l: focus · n/p: session · tab: pane · b: sidebar · d: detach · esc: exit ";
            let hint_w = hint.chars().count() as u16;
            let used = start.saturating_add(left_w);
            if hint_w <= area.width.saturating_sub(used) {
                let x = area.width.saturating_sub(hint_w);
                let hint_style = Style::default()
                    .fg(RColor::Black)
                    .bg(YELLOW)
                    .add_modifier(Modifier::BOLD);
                f.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(hint, hint_style)])),
                    Rect::new(x, area.y, hint_w, 1),
                );
            }
        }
    }

    fn place_cursor(&mut self, terminal: &mut Term, geom: &TreeGeom, focused: u64) -> Result<()> {
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
