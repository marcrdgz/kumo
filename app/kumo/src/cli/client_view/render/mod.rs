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
        self.render_confirm(f);
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

    fn render_pane_frame(&self, f: &mut Frame, rect: Rect, focused: bool, title: &str) {
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
                    let bstyle = Style::default().fg(contrast_fg(&theme, theme.secondary)).bg(theme.secondary).add_modifier(Modifier::BOLD);
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
                let bstyle = Style::default().fg(contrast_fg(&theme, theme.secondary)).bg(theme.secondary).add_modifier(Modifier::BOLD);
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
        let style = Style::default().fg(contrast_fg(&theme, theme.accent)).bg(theme.accent).add_modifier(Modifier::BOLD);
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
    // ratatui resets its back buffer on every draw, so every visible cell is
    // blitted each frame; only the cached rows' rebuild is incremental.
    let rows = grid.rows.min(inner.height as usize);
    for r in 0..rows {
        grid.get_rendered_row(r);
    }
    let grid = &*grid;
    let buf = f.buffer_mut();
    // Clip to the frame so each row maps onto one contiguous buffer slice.
    let clip = inner.intersection(buf.area);
    for r in 0..rows {
        let y = inner.y + r as u16;
        if clip.is_empty() || y < clip.y || y >= clip.bottom() {
            continue;
        }
        let Some(Some(rendered_row)) = grid.rendered.get(r) else {
            continue;
        };
        let row_links = if link_mods { grid.links.get(&(r as u16)) } else { None };
        let start = buf.index_of(clip.x, y);
        let skip = (clip.x - inner.x) as usize;
        let width = rendered_row.len().saturating_sub(skip).min(clip.width as usize);
        let targets = &mut buf.content[start..start + width];
        let cells = rendered_row.get(skip..).unwrap_or_default();
        for (i, (target, cell)) in targets.iter_mut().zip(cells).enumerate() {
            // Skip continuation cells (wide character tails). Wide glyphs at
            // the pane's right edge are already clipped by the daemon, so the
            // grid never carries a wide glyph in the last column.
            if matches!(cell.diff_option, ratatui::buffer::CellDiffOption::Skip) {
                continue;
            }
            let c = (skip + i) as u16;
            target.clone_from(cell);

            // Apply selection highlight (client-side overlay, not cached)
            if let Some(((tr, tc), (br, bc))) = sel_corners {
                if (r as u16, c) >= (tr, tc) && (r as u16, c) <= (br, bc) {
                    target.set_style(target.style().add_modifier(Modifier::REVERSED));
                }
            }

            // Apply link underline (client-side overlay, not cached)
            if let Some(links) = row_links {
                if links.iter().any(|l| c >= l.start && c < l.end) {
                    target.set_style(target.style().add_modifier(Modifier::UNDERLINED));
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
            Style::default().fg(contrast_fg(theme, bg)).bg(bg).add_modifier(Modifier::BOLD),
            Style::default().fg(contrast_fg(theme, bg)).bg(bg).add_modifier(Modifier::BOLD),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::client_view::tests::*;

    #[test]
    fn pane_blit_places_cells_with_selection_and_link_overlays() {
        let mut view = test_view();
        view.cols = 40;
        view.rows = 10;
        view.sidebar_open = false;
        view.status_bar.enabled = false;
        view.layout = Some(render_bench_layout(&[1]));
        view.recompute_geometry();
        let inner = PaneGeom { pane_id: 1, rect: view.rects[0].1 }.inner();
        view.on_pane_frame(PaneFrame {
            pane_id: 1,
            cols: inner.width,
            rows: inner.height,
            full: true,
            rows_dirty: vec![kumo_protocol::RowPatch {
                row: 1,
                cells: vec![cell("\u{1f600}", 2), cell(" ", 0), cell("x", 1), cell("y", 1), cell("z", 1)],
                links: vec![LinkRange { start: 3, end: 5, url: "https://example.com".into() }],
            }],
            cursor: None,
            scroll: None,
        });
        view.sel = Some(Sel { pane_id: 1, start: (2, 1), end: (2, 1) });
        view.link_mods = true;
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
        term.draw(|f| view.draw(f)).unwrap();

        let buf = term.backend().buffer();
        let at = |col: u16| &buf[(inner.x + col, inner.y + 1)];
        assert_eq!(at(0).symbol(), "\u{1f600}");
        assert_eq!((at(2).symbol(), at(3).symbol(), at(4).symbol()), ("x", "y", "z"));
        assert!(at(2).modifier.contains(Modifier::REVERSED), "selected cell is reversed");
        assert!(!at(2).modifier.contains(Modifier::UNDERLINED));
        assert!(at(3).modifier.contains(Modifier::UNDERLINED) && at(4).modifier.contains(Modifier::UNDERLINED));
        assert!(!at(3).modifier.contains(Modifier::REVERSED));
    }

    #[test]
    #[ignore = "local rendering benchmark; run with --ignored --nocapture"]
    fn benchmark_echo_render_cost() {
        fn run_case(cols: u16, rows: u16, pane_count: usize) -> (u128, u128, u128) {
            let pane_ids: Vec<u64> = (1..=pane_count as u64).collect();
            let mut view = test_view();
            view.cols = cols;
            view.rows = rows;
            view.sidebar_open = false;
            view.status_bar.enabled = false;
            view.layout = Some(render_bench_layout(&pane_ids));
            view.recompute_geometry();

            // Populate every visible pane with a full frame first. Subsequent
            // partial frames model the daemon's row patch after one echoed key.
            let dimensions: Vec<(u64, u16, u16)> = view
                .rects
                .iter()
                .map(|&(pid, rect)| {
                    let inner = PaneGeom { pane_id: pid, rect }.inner();
                    (pid, inner.width.max(1), inner.height.max(1))
                })
                .collect();
            let backend = ratatui::backend::TestBackend::new(cols, rows);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            for &(pid, pane_cols, pane_rows) in &dimensions {
                view.on_pane_frame(render_bench_frame(pid, pane_cols, pane_rows, true, 0, pid as u8));
            }
            terminal.draw(|frame| view.draw(frame)).unwrap();

            let mut samples = Vec::with_capacity(200);
            for i in 0..200u8 {
                let (pid, pane_cols, pane_rows) = dimensions[(i as usize) % dimensions.len()];
                view.on_pane_frame(render_bench_frame(
                    pid,
                    pane_cols,
                    pane_rows,
                    false,
                    i as u16 % pane_rows.max(1),
                    i,
                ));
                let started = Instant::now();
                terminal.draw(|frame| view.draw(frame)).unwrap();
                samples.push(started.elapsed().as_micros());
            }
            samples.sort_unstable();
            let mean = samples.iter().sum::<u128>() / samples.len() as u128;
            let p50 = samples[samples.len() / 2];
            let p95 = samples[samples.len() * 95 / 100];
            (mean, p50, p95)
        }

        let typical = run_case(120, 40, 1);
        let large = run_case(240, 80, 4);
        eprintln!(
            "echo render benchmark (microseconds/frame): 120x40/1-pane mean={} p50={} p95={}; 240x80/4-pane mean={} p50={} p95={}",
            typical.0, typical.1, typical.2, large.0, large.1, large.2,
        );
        std::hint::black_box((typical, large));
    }

    #[test]
    fn render_draws_pane_borders_and_status() {
        // One session with a single focused pane, plus a matching grid.
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus: 1, zoom: false, root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane { id: 1, title: " shell ".into(), cwd: std::path::PathBuf::from("/tmp"), is_ai: false, agent: None, mouse_reporting: false, alt_screen: false }))) }],
                focus: 1,
                zoom: false,
                branch: None,
                root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                    id: 1,
                    title: " shell ".into(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    is_ai: false,
                    agent: None,
                    mouse_reporting: false,
                    alt_screen: false,
                }))),
            }],
        };
        let mut g = grid();
        g.cells = vec![vec![cell("hi", 1), cell(" ", 1)], vec![cell("yo", 1), cell(" ", 1)]];
        let mut view = test_view();
        view.layout = Some(layout);
        view.grids.insert(1, g);
        view.recompute_geometry();
        // 80x24 with sidebar+tab bar: the pane area starts at col 26, row 1 (tab bar at row 0).
        assert_eq!(view.rects, vec![(1, Rect::new(26, 1, 54, 22))]);

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();
        // Tab bar at top — active tab uses the primary accent, followed by one
        // spacer cell and the persistent close action.
        assert_eq!(buf.cell((26, 0)).unwrap().symbol(), "1");
        assert_eq!(buf.cell((26, 0)).unwrap().fg, contrast_fg(&view.current_theme(), view.current_theme().accent));
        assert_eq!(buf.cell((26, 0)).unwrap().bg, view.current_theme().accent);
        assert_eq!(buf.cell((27, 0)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((28, 0)).unwrap().symbol(), "x");
        // Pane border at the top-left of the pane area (below tab bar) — now rounded by default.
        assert_eq!(buf.cell((26, 1)).unwrap().symbol(), "╭");
        // The title chip carries the pane label (pane frame at y=1).
        assert_eq!(buf.cell((27, 1)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((28, 1)).unwrap().symbol(), "s", "plain shell pane should expose its title");
        let title_cell = buf.cell((28, 1)).unwrap();
        assert_eq!(title_cell.fg, view.current_theme().accent);
        assert_eq!(title_cell.bg, RColor::Reset, "focused pane title must not add a competing accent fill");
        // The status bar shows NORMAL + the session name.
        let status_line: String = (0..40).map(|x| buf.cell((x, 23)).unwrap().symbol().to_string()).collect();
        assert!(status_line.contains("NORMAL"), "status chip missing: {status_line:?}");
        assert!(status_line.contains("sess"), "session name missing: {status_line:?}");
    }

    #[test]
    fn pane_titles_preserve_ai_prefix_but_plain_titles_stand_alone() {
        let mut view = test_view();
        view.layout = Some(panes_layout(&[(1, AgentStatus::Working)]));
        assert_eq!(view.pane_label(1), " AI · agent1 ");

        if let Some(LayoutNode::Pane(pane)) = view
            .layout
            .as_mut()
            .and_then(|layout| layout.sessions.first_mut())
            .and_then(|session| session.tabs.first_mut())
            .and_then(|tab| tab.root.as_deref_mut())
        {
            pane.title = "custom title".into();
            pane.agent.as_mut().unwrap().name = "   ".into();
        }
        assert_eq!(view.pane_label(1), " AI · custom title ");
        if let Some(LayoutNode::Pane(pane)) = view
            .layout
            .as_mut()
            .and_then(|layout| layout.sessions.first_mut())
            .and_then(|session| session.tabs.first_mut())
            .and_then(|tab| tab.root.as_deref_mut())
        {
            pane.agent = None;
            pane.is_ai = false;
        }
        assert_eq!(view.pane_label(1), " custom title ");
        if let Some(LayoutNode::Pane(pane)) = view
            .layout
            .as_mut()
            .and_then(|layout| layout.sessions.first_mut())
            .and_then(|session| session.tabs.first_mut())
            .and_then(|tab| tab.root.as_deref_mut())
        {
            pane.title = "   ".into();
        }
        assert_eq!(view.pane_label(1), " ");
    }

    #[test]
    fn pane_borders_prioritize_blocked_then_done_attention() {
        let rect = Rect::new(2, 2, 12, 5);
        for (status, expected) in [
            (AgentStatus::Blocked, test_view().current_theme().orange),
            (AgentStatus::Done, test_view().current_theme().green),
        ] {
            let mut view = test_view();
            view.layout = Some(panes_layout(&[(1, status)]));
            view.rects = vec![(1, rect)];
            let backend = ratatui::backend::TestBackend::new(20, 10);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            term.draw(|frame| view.render_pane_frame(frame, rect, true, " title ")).unwrap();
            let buffer = term.backend().buffer();
            assert_eq!(buffer.cell((rect.x, rect.y)).unwrap().fg, expected, "attention state owns the focused pane border");
            let title_cell = buffer.cell((rect.x + 1, rect.y)).unwrap();
            assert_eq!(title_cell.fg, view.current_theme().accent, "focused title keeps the focus accent");
            assert!(title_cell.modifier.contains(Modifier::BOLD), "focused title remains bold alongside attention state");
        }
    }

    /// Rendering must never panic at any (tiny → large) terminal size, with
    /// every overlay open — the classic TUI's full frame was drawn every
    /// cycle, so the client's draw path has to be total over all geometry.
    #[test]
    fn render_is_total_across_sizes_and_overlays() {
        let layout = Layout {
            active: Some("sess".into()),
            sessions: vec![SessionLayout {
                name: "sess".into(),
                workspace: std::path::PathBuf::from("/tmp"),
                project_root: None,
                is_linked_worktree: false,
                active_tab: 0,
                tabs: vec![kumo_protocol::TabLayout { id: 1, name: "1".into(), focus: 1, zoom: false, root: Some(Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane { id: 1, title: " shell ".into(), cwd: std::path::PathBuf::from("/tmp"), is_ai: false, agent: None, mouse_reporting: false, alt_screen: false }))) }],
                focus: 1,
                zoom: false,
                branch: Some(WireBranch { name: "main".into(), ahead: 1, behind: 0 }),
                root: Some(Box::new(LayoutNode::Split {
                    id: 1,
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    a: Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                        id: 1,
                        title: " shell 1 ".into(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        is_ai: false,
                        agent: None,
                        mouse_reporting: false,
                        alt_screen: false,
                    })),
                    b: Box::new(LayoutNode::Pane(kumo_protocol::LayoutPane {
                        id: 2,
                        title: " AI CLI ".into(),
                        cwd: std::path::PathBuf::from("/tmp"),
                        is_ai: true,
                        agent: Some(kumo_protocol::AgentInfo {
                            name: "opencode".into(),
                            status: AgentStatus::Blocked,
                            cpu: 0.0,
                            mem_kb: 0,
                            pane_id: 2,
                            pane_index: 2,
                            tab_index: 1,
                        }),
                        mouse_reporting: true,
                        alt_screen: false,
                    })),
                })),
            }],
        };
        let mut g = grid();
        g.cells = vec![vec![cell("hi", 1), cell(" ", 1), cell("x", 1), cell("y", 1)]];
        for (w, h) in [(2u16, 2u16), (5, 4), (20, 8), (40, 12), (80, 24), (120, 40)] {
            let mut view = test_view();
            view.cols = w;
            view.rows = h;
            view.layout = Some(layout.clone());
            view.grids.insert(1, g.clone());
            view.recompute_geometry();
            // Open every overlay: it must still render without panicking.
            view.menu.open = true;
            view.ctx_menu.open = true;
            view.popup.open = true;
            view.popup.name = "worktree/name".into();
            view.keybind_overlay.open = true;
            view.settings.open = true;
            view.worktree_picker.open = true;
            view.worktree_picker.items = vec![
                WireWorktree { path: std::path::PathBuf::from("/tmp"), branch: Some("main".into()), is_main: true, open: false, comment: None, status: None, is_ephemeral: false },
            ];
            view.pane_numbers = Some(Instant::now());
            view.update_notice = Some(("key".into(), "nightly".into()));
            view.agent_toasts = vec![
                AgentToast { source: ToastSource::Agent { pane_id: 2, kind: ToastKind::Blocked }, title: "opencode is blocked".into(), body: "/tmp".into(), born: Instant::now() },
                AgentToast { source: ToastSource::Agent { pane_id: 1, kind: ToastKind::Finished }, title: "claude finished".into(), body: String::new(), born: Instant::now() },
            ];
            view.sel = Some(Sel { pane_id: 1, start: (0, 0), end: (1, 0) });
            view.link_mods = true;
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            term.draw(|f| view.draw(f)).unwrap();
        }
    }

    /// The status-bar dropdown opens flush above the bar; its drop shadow must
    /// stop at the chrome floor instead of repainting the status row (which
    /// used to wipe out the MENU button).
    #[test]
    fn menu_shadow_never_paints_over_status_bar() {
        let mut view = test_view();
        view.menu.open = true;
        let theme = view.current_theme();
        let shadow_bg = dim_toward(theme.panel_sep, ColorRgb::new(0, 0, 0), &theme.palette, 2, 5);
        let dd = view.menu_dropdown_rect().expect("dropdown fits at 80x24");
        assert_eq!(dd.bottom(), 23, "dropdown sits flush above the status row");

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer();

        // The MENU button on the status row is intact under the dropdown
        // (button starts after the mode chip + " · " separator).
        let btn: String = (11..17).map(|x| buf.cell((x, 23)).unwrap().symbol().to_string()).collect();
        assert_eq!(btn, " MENU ", "shadow must not repaint the status row");

        // Vertical strip hugs the modal side but stops above the floor.
        assert_eq!(buf.cell((dd.right(), dd.bottom() - 1)).unwrap().bg, shadow_bg, "shadow beside the modal");
        assert_ne!(buf.cell((dd.right(), dd.bottom())).unwrap().bg, shadow_bg, "no shadow on the status row");
    }
}
