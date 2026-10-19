//! Frame rendering: the top-level `draw` pass, pane frames/content and the
//! shared modal chrome. Each chrome area renders from its own submodule.

mod copy_mode;
mod overlays;
mod settings;
mod sidebar;
mod status;
mod tabs;
mod worktree;

use super::*;

impl View {
    // ------------------------------------------------------------------
    // Rendering
    // ------------------------------------------------------------------

    pub fn on_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.cols = cols.max(2);
        self.rows = rows.max(2);
        self.recompute_geometry();
        // Clamp copy cursor inside new dims
        let pid = self.copy.as_ref().map(|c| c.pane_id);
        if let Some(pid) = pid {
            if let Some((w, h)) = self.copy_pane_dims(pid) {
                if let Some(cs) = self.copy.as_mut() {
                    cs.cursor.0 = cs.cursor.0.min(w.saturating_sub(1));
                    cs.cursor.1 = cs.cursor.1.min(h.saturating_sub(1));
                    if let Some(a) = cs.anchor.as_mut() {
                        a.0 = a.0.min(w.saturating_sub(1));
                        a.1 = a.1.min(h.saturating_sub(1));
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn expire_timers(&mut self) {
        let now = Instant::now();
        if let Some(t) = self.pane_numbers {
            if now.duration_since(t) > PANE_NUMBERS_TIMEOUT {
                self.pane_numbers = None;
            }
        }
        if let Some((_, t)) = &self.status_msg {
            if now.duration_since(*t) > TOAST_TIMEOUT {
                self.status_msg = None;
            }
        }
        if let Some((_, t)) = &self.notice {
            if now.duration_since(*t) > TOAST_TIMEOUT {
                self.notice = None;
            }
        }
        self.agent_toasts.retain(|t| now.duration_since(t.born) < AGENT_TOAST_TIMEOUT);
    }

    /// Redraw the whole frame into `terminal`. Uses ratatui's diffing, so the
    /// host terminal only receives the changed cells.
    pub fn render_now(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        self.expire_timers();
        terminal.draw(|f| self.draw(f))?;
        self.place_cursor(terminal)?;
        self.dirty = false;
        Ok(())
    }

    pub(super) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let link_mods = self.link_mods;
        let selected = self.sel;
        let theme = self.current_theme();
        // Pane frames (borders + titles + content).
        for &(pid, rect) in &self.rects {
            let focused = self.active_tab().map(|t| t.focus == pid).unwrap_or(false);
            let title = self.pane_title(pid, focused, rect);
            self.render_pane_frame(f, rect, focused, &title);
            if let Some(grid) = self.grids.get_mut(&pid) {
                render_pane_content(f, pid, rect, grid, selected, link_mods, &theme);
            }
            // Copy-mode overlay (cursor + search hits + selection fallback)
            if let Some(cs) = self.copy.as_ref() {
                if cs.pane_id == pid {
                    self.render_copy_overlay(f, pid, rect);
                }
            }
        }
        self.render_tab_bar(f);
        self.render_pane_numbers(f);
        if self.sidebar_open {
            self.render_sidebar(f, area);
        }
        self.render_status(f);
        self.render_agent_toasts(f);
        self.render_copy_search_bar(f);
        self.render_menu(f);
        self.render_ctx_menu(f);
        self.render_name_popup(f);
        self.render_update_notice(f);
        self.render_keybind_overlay(f);
        self.render_settings(f);
        self.render_worktree_picker(f);
        self.render_worktree_create(f);
        self.render_session_close_confirm(f);
        self.render_finder(f);
        self.render_agent_inbox(f);
    }

    pub(super) fn pane_label(&self, pid: u64) -> String {
        self.active_session()
            .and_then(|s| find_pane_in_session(s, pid))
            .map(|p| {
                if p.is_ai {
                    if let Some(agent) = p.agent.as_ref() {
                        let name = agent.name.trim();
                        if !name.is_empty() {
                            return format!(" AI · {name} ");
                        }
                    }
                }
                let title = p.title.trim();
                if p.is_ai {
                    if title.is_empty() { " AI ".to_string() } else { format!(" AI · {title} ") }
                } else if title.is_empty() {
                    " ".to_string()
                } else {
                    format!(" {title} ")
                }
            })
            .unwrap_or_else(|| " pane ".to_string())
    }

    fn pane_title(&self, pid: u64, focused: bool, rect: Rect) -> String {
        let base = self.pane_label(pid);
        if focused && self.session_zoom() {
            // The right-aligned badge marks zoom; only fall back to a title
            // suffix when the pane is too narrow to fit both.
            let badge_w = ZOOM_BADGE.chars().count() as u16;
            if rect.width < base.chars().count() as u16 + badge_w + 2 {
                return format!("{base}(zoom) ");
            }
        }
        base
    }

    pub(super) fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, title: &str) {
        if rect.width < 3 || rect.height < 3 {
            return;
        }
        let theme = self.current_theme();
        // Attention states own the border color even when the pane does not
        // have focus, so a blocked or finished agent remains visible.
        let agent_status = self.rects.iter().find_map(|(pid, r)| {
            (*r == rect).then(|| {
                self.active_session()
                    .and_then(|s| find_pane_in_session(s, *pid))
                    .and_then(|p| p.agent.as_ref().map(|agent| agent.status))
            })
        }).flatten();
        let blocked = agent_status == Some(AgentStatus::Blocked);
        let done = agent_status == Some(AgentStatus::Done);
        let border = if blocked {
            theme.orange
        } else if done {
            theme.green
        } else if focused {
            theme.accent
        } else {
            // Idle borders recede toward the terminal background so the
            // focused pane's accent border is the clear bright edge.
            dim_toward(theme.border_idle, theme.term_bg, &theme.palette, 1, 2)
        };
        let style_cfg = kumo_core::config::sidebar_borders().style;
        if style_cfg == kumo_core::config::BorderStyle::Hidden {
            // Hidden: no border, just title chip at top-left inset.
            let max = rect.width.saturating_sub(2) as usize;
            let chip = if focused {
                Style::default().fg(theme.accent).bg(RColor::Reset).add_modifier(Modifier::BOLD)
            } else if blocked {
                Style::default().fg(theme.orange).bg(RColor::Reset).add_modifier(Modifier::BOLD)
            } else if done {
                Style::default().fg(theme.green).bg(RColor::Reset).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg).bg(RColor::Reset)
            };
            for (i, ch) in title.chars().take(max).enumerate() {
                put(f, rect.x + 1 + i as u16, rect.y, &ch.to_string(), chip);
            }
            if focused && self.session_zoom() {
                let badge_w = ZOOM_BADGE.chars().count() as u16;
                if rect.width >= title.chars().count() as u16 + badge_w + 2 {
                    let bx = rect.right() - badge_w;
                    let bstyle = Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD);
                    for (i, ch) in ZOOM_BADGE.chars().enumerate() {
                        put(f, bx + i as u16, rect.y, &ch.to_string(), bstyle);
                    }
                }
            }
            return;
        }
        let border_style = Style::default().fg(border).bg(RColor::Reset);
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
        let (tl, tr, bl, br, h, v) = border_chars(style_cfg);
        put(f, x0, y0, tl, border_style);
        put(f, x1, y0, tr, border_style);
        put(f, x0, y1, bl, border_style);
        put(f, x1, y1, br, border_style);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, h, border_style);
            put(f, x, y1, h, border_style);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, v, border_style);
            put(f, x1, y, v, border_style);
        }
        // The outline owns focus. Keeping the title on the terminal surface
        // avoids a second saturated chip competing with the same border.
        let max = rect.width.saturating_sub(2) as usize;
        let chip = if focused {
            Style::default().fg(theme.accent).bg(RColor::Reset).add_modifier(Modifier::BOLD)
        } else if blocked {
            Style::default().fg(theme.orange).bg(RColor::Reset).add_modifier(Modifier::BOLD)
        } else if done {
            Style::default().fg(theme.green).bg(RColor::Reset).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg).bg(RColor::Reset)
        };
        for (i, ch) in title.chars().take(max).enumerate() {
            put(f, x0 + 1 + i as u16, y0, &ch.to_string(), chip);
        }
        // Zoom badge, right-aligned on the top border row.
        if focused && self.session_zoom() {
            let badge_w = ZOOM_BADGE.chars().count() as u16;
            if rect.width >= title.chars().count() as u16 + badge_w + 2 {
                let bx = x1 + 1 - badge_w;
                let bstyle = Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD);
                for (i, ch) in ZOOM_BADGE.chars().enumerate() {
                    put(f, bx + i as u16, y0, &ch.to_string(), bstyle);
                }
            }
        }
    }

    fn render_pane_numbers(&self, f: &mut Frame) {
        let Some(started) = self.pane_numbers else { return };
        if started.elapsed() > PANE_NUMBERS_TIMEOUT {
            return;
        }
        let theme = self.current_theme();
        if self.rects.len() < 2 {
            return;
        }
        let style = Style::default().fg(RColor::Black).bg(theme.accent).add_modifier(Modifier::BOLD);
        for (i, (_, rect)) in self.rects.iter().enumerate() {
            let Some(digit) = char::from_digit((i + 1) as u32, 10) else { continue };
            put(f, rect.x + rect.width / 2, rect.y + rect.height / 2, &digit.to_string(), style);
        }
    }

    /// Place the host-terminal cursor: the popup's input field when open, else
    /// the focused pane's terminal cursor.
    fn place_cursor<B: ratatui::backend::Backend>(&self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        if self.keybind_overlay.open {
            if let Some(rect) = self.keybind_overlay_rect() {
                let field_w = rect.width.saturating_sub(4) as usize;
                let start = (self.keybind_overlay.cursor + 1).saturating_sub(field_w.saturating_sub(2));
                let x = rect.x + 4 + self.keybind_overlay.cursor.saturating_sub(start) as u16;
                if x < rect.right().saturating_sub(2) {
                    terminal.set_cursor_position((x, rect.y + 2))?;
                    terminal.show_cursor()?;
                    return Ok(());
                }
            }
        }
        if self.popup.open {
            if let Some((x, y)) = self.name_popup_input_cursor() {
                terminal.set_cursor_position((x, y))?;
                terminal.show_cursor()?;
                return Ok(());
            }
        }
        let focus = self.active_tab().map(|t| t.focus);
        if let Some(focus) = focus {
            if let Some((_, rect)) = self.rects.iter().find(|(pid, _)| *pid == focus) {
                let inner = PaneGeom { pane_id: focus, rect: *rect }.inner();
                if let Some(grid) = self.grids.get(&focus) {
                    if let Some((cx, cy)) = grid.cursor {
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
        }
        terminal.hide_cursor()?;
        Ok(())
    }
}

fn render_pane_content(f: &mut Frame, pid: u64, rect: Rect, grid: &mut Grid, selected: Option<Sel>, link_mods: bool, theme: &OwnedTheme) {
    let inner = PaneGeom { pane_id: pid, rect }.inner();
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    // Pre-compute selection corners once per frame (not per cell)
    // Only apply selection if this pane is the one being selected
    let sel_corners = selected.filter(|sel| sel.pane_id == pid).map(|sel| sel_corners(&sel));
    for r in 0..grid.rows {
        if r as u16 >= inner.height {
            break;
        }
        // Get link ranges for this row (if link mods are active)
        // Clone before calling get_rendered_row() to avoid borrow conflicts
        let row_links = if link_mods {
            grid.links.get(&(r as u16)).cloned()
        } else {
            None
        };
        // Get the cached rendered row, rebuilding only if dirty
        let Some(rendered_row) = grid.get_rendered_row(r) else {
            continue;
        };
        for (c, cell) in rendered_row.iter().enumerate() {
            if c as u16 >= inner.width {
                break;
            }
            // Skip continuation cells (wide character tails). Wide glyphs at
            // the pane's right edge are already clipped by the daemon, so the
            // grid never carries a wide glyph in the last column.
            if matches!(cell.diff_option, ratatui::buffer::CellDiffOption::Skip) {
                continue;
            }
            let x = inner.x + c as u16;
            let y = inner.y + r as u16;
            if let Some(target) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                // Clone the cached cell
                *target = cell.clone();
                
                // Apply selection highlight (client-side overlay, not cached)
                if let Some(((tr, tc), (br, bc))) = sel_corners {
                    if (r as u16, c as u16) >= (tr, tc) && (r as u16, c as u16) <= (br, bc) {
                        target.set_style(target.style().add_modifier(Modifier::REVERSED));
                    }
                }
                
                // Apply link underline (client-side overlay, not cached)
                if let Some(links) = row_links.as_ref() {
                    if links.iter().any(|l| (c as u16) >= l.start && (c as u16) < l.end) {
                        target.set_style(target.style().add_modifier(Modifier::UNDERLINED));
                    }
                }
            }
        }
    }
    // Scrollbar on the last content column when the pane has scrollback.
    if let Some(scroll) = grid.scroll {
        render_scrollbar(f, inner, scroll, theme);
    }
}

fn render_scrollbar(f: &mut Frame, inner: Rect, sb: ScrollState, theme: &OwnedTheme) {
    let total = sb.total as usize;
    let screen = sb.screen as usize;
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
            put(f, x, y, "▐", Style::default().fg(theme.secondary));
        } else {
            put(f, x, y, "░", Style::default().fg(theme.panel_sep));
        }
    }
}

/// Shared modal chrome: panel fill, rounded accent border, and a one-cell
/// drop shadow along the bottom/right edges (clamped to the screen). Every
/// overlay renders through this so popups read as one design system.
///
/// `floor` is the first row of bottom chrome (the status bar): shadow strips
/// stop above it so a popup anchored to the bar never repaints it.
fn draw_modal(f: &mut Frame, dd: Rect, theme: &OwnedTheme, floor: u16) {    fill(f, dd, theme.panel_sep);
    let border = Style::default().fg(theme.accent).bg(theme.panel_sep);
    let (x0, y0, x1, y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
    put(f, x0, y0, "╭", border);
    put(f, x1, y0, "╮", border);
    put(f, x0, y1, "╰", border);
    put(f, x1, y1, "╯", border);
    for x in (x0 + 1)..x1 {
        put(f, x, y0, "─", border);
        put(f, x, y1, "─", border);
    }
    for y in (y0 + 1)..y1 {
        put(f, x0, y, "│", border);
        put(f, x1, y, "│", border);
    }
    let shadow_bg = dim_toward(theme.panel_sep, ColorRgb::new(0, 0, 0), &theme.palette, 2, 5);
    let scr = f.area();
    // Last row shadows may occupy: screen bottom, but never chrome.
    let max_row = scr.height.saturating_sub(1).min(floor.saturating_sub(1));
    if dd.right() < scr.width && max_row > dd.y {
        let y_end = dd.bottom().min(max_row);
        fill(f, Rect::new(dd.right(), dd.y + 1, 1, y_end - dd.y), shadow_bg);
    }
    if dd.bottom() <= max_row {
        let x_end = dd.right().min(scr.width.saturating_sub(1));
        if x_end > dd.x {
            fill(f, Rect::new(dd.x + 1, dd.bottom(), x_end - dd.x, 1), shadow_bg);
        }
    }
}

/// Overpaint a modal's border with `color` (same rounded frame draw_modal
/// draws, in an accent color). Used by corner toasts so the kind reads at a
/// glance without changing the shared modal look.
fn paint_border(f: &mut Frame, dd: Rect, color: RColor, bg: RColor) {
    let border = Style::default().fg(color).bg(bg);
    let Some(x1) = dd.right().checked_sub(1) else { return };
    let Some(y1) = dd.bottom().checked_sub(1) else { return };
    put(f, dd.x, dd.y, "╭", border);
    put(f, x1, dd.y, "╮", border);
    put(f, dd.x, y1, "╰", border);
    put(f, x1, y1, "╯", border);
    for x in (dd.x + 1)..x1 {
        put(f, x, dd.y, "─", border);
        put(f, x, y1, "─", border);
    }
    for y in (dd.y + 1)..y1 {
        put(f, dd.x, y, "│", border);
        put(f, x1, y, "│", border);
    }
}

fn render_item_row(f: &mut Frame, x0: u16, y: u16, width: u16, item: &str, sel: bool, theme: &OwnedTheme) {
    let bg = if sel { theme.accent } else { theme.panel_sep };
    for cx in (x0 + 1)..(x0 + 1 + width) {
        put(f, cx, y, " ", Style::default().bg(bg));
    }
    let (marker, marker_style, label_style) = if sel {
        (
            "▸",
            Style::default().fg(RColor::Black).bg(bg).add_modifier(Modifier::BOLD),
            Style::default().fg(RColor::Black).bg(bg).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ",
            Style::default().fg(theme.accent).bg(bg),
            Style::default().fg(theme.fg).bg(bg),
        )
    };
    put(f, x0 + 1, y, marker, marker_style);
    text(f, x0 + 3, y, item, label_style, width.saturating_sub(2));
}

fn draw_scrollbar(f: &mut Frame, x: u16, y_top: u16, region_h: u16, offset: usize, total: usize, theme: &OwnedTheme) {
    if total <= region_h as usize || region_h == 0 {
        return;
    }
    let bar_h = region_h as usize;
    let thumb = ((region_h as usize * bar_h) / total).max(1).min(bar_h);
    let hist = total - region_h as usize;
    let y_max = bar_h.saturating_sub(thumb);
    let y_start = offset.saturating_mul(y_max) / hist.max(1);
    for i in 0..bar_h {
        let y = y_top + i as u16;
        let cell = f.buffer_mut().cell_mut((x, y)).unwrap();
        if i >= y_start && i < y_start + thumb {
            cell.set_symbol("▐").set_fg(theme.secondary);
        } else {
            cell.set_symbol("░").set_fg(theme.panel_sep);
        }
    }
}
