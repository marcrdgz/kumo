use super::*;

impl View {
    pub(super) fn render_copy_overlay(&self, f: &mut Frame, pid: u64, rect: Rect) {
        let Some(cs) = self.copy.as_ref() else { return; };
        if cs.pane_id != pid { return; }
        let Some(grid) = self.grids.get(&pid) else { return; };
        let inner = PaneGeom { pane_id: pid, rect }.inner();
        if inner.width == 0 || inner.height == 0 { return; }
        let theme = self.current_theme();
        // Search hits: underline + tint, active hit bold/reversed
        if !cs.hits.is_empty() {
            let scroll = grid.scroll;
            for (idx, hit) in cs.hits.iter().enumerate() {
                let top = scroll.map(|s| s.offset as u32).unwrap_or(0);
                let rows = inner.height as u32;
                if hit.row < top || hit.row >= top + rows { continue; }
                let y = inner.y + (hit.row - top) as u16;
                let is_active = cs.hit_idx == Some(idx);
                for c in hit.start_col..hit.end_col {
                    if c >= inner.width { break; }
                    let x = inner.x + c;
                    if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                        if is_active {
                            cell.set_style(cell.style().add_modifier(Modifier::REVERSED).add_modifier(Modifier::BOLD));
                            cell.set_bg(theme.accent);
                            cell.set_fg(RColor::Black);
                        } else {
                            cell.set_style(cell.style().add_modifier(Modifier::UNDERLINED));
                            // tint background slightly
                            cell.set_bg(theme.secondary);
                            cell.set_fg(RColor::Black);
                        }
                    }
                }
            }
        }
        // Selection fallback (client charwise highlight) when daemon selection is charwise?
        // Daemon selection already appears as REVERSED via PaneFrame; we add an extra
        // overlay for the anchor→cursor range so a client-only selection (no daemon) still shows.
        if let Some(anchor) = cs.anchor {
            let cur = cs.cursor;
            let (top, bottom, left, right) = if cs.linewise {
                let top = anchor.1.min(cur.1);
                let bottom = anchor.1.max(cur.1);
                (top, bottom, 0u16, inner.width.saturating_sub(1))
            } else {
                let (s_col, s_row) = anchor;
                let (c_col, c_row) = cur;
                let (start, end) = if (s_row, s_col) <= (c_row, c_col) { ((s_col, s_row), (c_col, c_row)) } else { ((c_col, c_row), (s_col, s_row)) };
                // We'll highlight per-row range below; store for per-cell check
                // For simplicity, use sel_corners-style check inline per cell
                // We'll just flag and handle in loop
                // Return as bounding box plus per-cell filter
                (start.1, end.1, start.0, end.0) // use as markers, filter logic uses original
            };
            // Iterate viewport rows/cols inside inner and highlight if in selection
            for r in 0..inner.height {
                for c in 0..inner.width {
                    let in_sel = if cs.linewise {
                        r >= top && r <= bottom
                    } else {
                        let s = anchor;
                        let e = cur;
                        let (sr, sc) = (s.1, s.0);
                        let (er, ec) = (e.1, e.0);
                        let (tr, tc, br, bc) = if (sr, sc) <= (er, ec) { (sr, sc, er, ec) } else { (er, ec, sr, sc) };
                        let pos = (r, c);
                        let top_pos = (tr, tc);
                        let bottom_pos = (br, bc);
                        pos >= top_pos && pos <= bottom_pos
                    };
                    if !in_sel { continue; }
                    let x = inner.x + c;
                    let y = inner.y + r;
                    if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                        // avoid double-highlighting search active hit (already bold)
                        let is_search_active = if let Some(hidx) = cs.hit_idx {
                            if let Some(hit) = cs.hits.get(hidx) {
                                let scroll = grid.scroll.map(|s| s.offset as u32).unwrap_or(0);
                                hit.row == scroll + r as u32 && c >= hit.start_col && c < hit.end_col
                            } else { false }
                        } else { false };
                        if !is_search_active {
                            cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
                        }
                    }
                    let _ = (top, bottom, left, right); // keep bindings
                }
            }
        }
        // Cursor: block cursor at copy position (always visible)
        let (cx, cy) = cs.cursor;
        if cx < inner.width && cy < inner.height {
            let x = inner.x + cx;
            let y = inner.y + cy;
            if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                // If on an active search hit, keep hit styling plus add bold
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED).add_modifier(Modifier::BOLD));
                // Use accent bg to distinguish copy cursor from terminal cursor
                // Keep fg inversion for readability
                let fg = cell.style().fg.unwrap_or(theme.fg);
                let bg = cell.style().bg.unwrap_or(RColor::Reset);
                // invert
                cell.set_fg(bg);
                cell.set_bg(theme.accent);
                let _ = fg;
            }
        }
        // Copy-mode label at pane title: add "[COPY]" chip?
        // Instead status bar shows mode; we also tint border accent when in copy
    }

    pub(super) fn render_copy_search_bar(&self, f: &mut Frame) {
        let Some(cs) = self.copy.as_ref() else { return; };
        if !cs.search_active {
            // Hint moved to status bar right side (like "copied" toast) — nothing to draw here.
            return;
        }
        // Active search — rectangle popup (black bg, red border) like rename, below tabs with gap
        let theme = self.current_theme();
        if let Some(dd) = self.copy_search_rect() {
            // black bg, red border
            let border = Style::default().fg(theme.red).bg(RColor::Black);
            fill(f, dd, RColor::Black);
            let (bx0, by0, bx1, by1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
            put(f, bx0, by0, "┌", border);
            put(f, bx1, by0, "┐", border);
            put(f, bx0, by1, "└", border);
            put(f, bx1, by1, "┘", border);
            for x in (bx0 + 1)..bx1 {
                put(f, x, by0, "─", border);
                put(f, x, by1, "─", border);
            }
            for y in (by0 + 1)..by1 {
                put(f, bx0, y, "│", border);
                put(f, bx1, y, "│", border);
            }
            let inner_w = dd.width.saturating_sub(4);
            // title
            let title = Style::default().fg(RColor::White).bg(RColor::Black).add_modifier(Modifier::BOLD);
            let title_text = if cs.search_forward { "search /" } else { "search ?" };
            text(f, dd.x + 2, dd.y + 1, title_text, title, inner_w);
            let field = input_style(&theme);
            let field_w = inner_w;
            for cx in (dd.x + 2)..(dd.x + 2 + field_w) {
                put(f, cx, dd.y + 2, " ", field);
            }
            let mut x = dd.x + 2;
            let prefix = if cs.search_forward { "/" } else { "?" };
            put(f, x, dd.y + 2, prefix, field.add_modifier(Modifier::BOLD));
            x += 1;
            let input = &cs.search_input;
            let cursor = cs.search_cursor.min(input.chars().count());
            let text_w = field_w.saturating_sub(1) as usize;
            let start = (cursor + 1).saturating_sub(text_w);
            let mut col = x;
            for (i, ch) in input.chars().enumerate().skip(start) {
                if (col - x) as usize >= text_w { break; }
                let ch_style = if i == cursor { field.add_modifier(Modifier::REVERSED) } else { field };
                put(f, col, dd.y + 2, &ch.to_string(), ch_style);
                col += 1;
            }
            if cursor == input.chars().count() && col < dd.x + 2 + field_w {
                put(f, col, dd.y + 2, " ", field.add_modifier(Modifier::REVERSED));
            }
            // footer hint (white on black, like context menu)
            let hint = "enter: search  esc: cancel  n/N: next";
            let hint_style = Style::default().fg(RColor::White).bg(RColor::Black);
            text(f, dd.x + 2, dd.y + 3, hint, hint_style, inner_w);
            return;
        }
        // Fallback line bar if terminal too small for rectangle
        let pa = self.panes_area();
        if pa.width == 0 || pa.height == 0 { return; }
        let bar_y = pa.y.saturating_add(1);
        if bar_y >= pa.y + pa.height { return; }
        let rect = Rect::new(pa.x, bar_y, pa.width, 1);
        fill(f, rect, theme.input_bg);
        let prefix = if cs.search_forward { "/" } else { "?" };
        let style = input_style(&theme);
        let mut x = rect.x + 1;
        put(f, x, rect.y, prefix, style.add_modifier(Modifier::BOLD));
        x += 1;
        let input = &cs.search_input;
        let cursor = cs.search_cursor.min(input.chars().count());
        for (i, ch) in input.chars().enumerate() {
            let ch_style = if i == cursor { style.add_modifier(Modifier::REVERSED) } else { style };
            put(f, x, rect.y, &ch.to_string(), ch_style);
            x += 1;
            if x >= rect.right() - 1 { break; }
        }
        if cursor == input.chars().count() && x < rect.right() {
            put(f, x, rect.y, " ", style.add_modifier(Modifier::REVERSED));
        }
        let hint = " enter: search  esc: cancel ";
        let hint_x = rect.right().saturating_sub(hint.len() as u16 + 1);
        if hint_x > x + 1 {
            text(f, hint_x, rect.y, hint, input_hint_style(&theme), hint.len() as u16);
        }
    }
}
