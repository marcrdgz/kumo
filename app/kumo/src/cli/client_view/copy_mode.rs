//! Copy-mode: keyboard/mouse selection over a pane's grid, search and yank.

use super::*;

impl View {
    // ------------------------------------------------------------------
    // Copy-mode
    // ------------------------------------------------------------------

    pub(super) fn enter_copy_mode(&mut self) {
        // Pick the focused pane of the active tab; refuse on alt-screen.
        let Some(tab) = self.active_tab() else {
            self.notice = Some(("no active tab".to_string(), Instant::now()));
            self.mark_dirty();
            return;
        };
        let pid = tab.focus;
        if let Some(layout) = self.layout.as_ref() {
            if let Some(sess) = layout.sessions.iter().find(|s| s.name == layout.active.as_deref().unwrap_or("")) {
                for t2 in &sess.tabs {
                    if let Some(p) = Self::find_layout_pane_in_tab(t2, pid) {
                        if p.alt_screen {
                            self.notice = Some(("copy-mode unavailable in alternate screen".to_string(), Instant::now()));
                            self.mark_dirty();
                            return;
                        }
                    }
                }
            }
        }
        // Also check grids for alt_screen? Use layout Pane alt_screen above.
        let (_cols, rows) = self.copy_pane_dims(pid).unwrap_or((80, 24));
        let start_cursor = (0u16, rows.saturating_sub(1));
        self.copy = Some(CopyState {
            pane_id: pid,
            cursor: start_cursor,
            anchor: None,
            linewise: false,
            search_active: false,
            search_input: String::new(),
            search_cursor: 0,
            search_forward: true,
            search_query: None,
            hits: Vec::new(),
            hit_idx: None,
        });
        self.mode = Mode::Copy;
        self.mark_dirty();
    }

    pub(super) fn enter_copy_mode_with_search(&mut self, forward: bool) {
        self.enter_copy_mode();
        if self.mode == Mode::Copy {
            if let Some(cs) = self.copy.as_mut() {
                cs.search_active = true;
                cs.search_input.clear();
                cs.search_cursor = 0;
                cs.search_forward = forward;
            }
            self.mark_dirty();
        }
    }

    fn find_layout_pane(node: &LayoutNode, pid: u64) -> Option<kumo_protocol::LayoutPane> {
        match node {
            LayoutNode::Pane(p) if p.id == pid => Some(p.clone()),
            LayoutNode::Pane(_) => None,
            LayoutNode::Split { a, b, .. } => Self::find_layout_pane(a, pid).or_else(|| Self::find_layout_pane(b, pid)),
        }
    }
    pub(super) fn find_layout_pane_in_tab(tab: &kumo_protocol::TabLayout, pid: u64) -> Option<kumo_protocol::LayoutPane> {
        if let Some(root) = &tab.root {
            return Self::find_layout_pane(root, pid);
        }
        None
    }

    fn leave_copy_mode(&mut self, scroll_to_bottom: bool) {
        if let Some(cs) = self.copy.take() {
            // Clear daemon selection if any
            let _ = self.send(&Command::CopyClearSelection { pane_id: cs.pane_id });
            if scroll_to_bottom {
                let _ = self.send(&Command::CopyScrollTo { pane_id: cs.pane_id, row: u32::MAX });
                // u32::MAX is clamped by daemon to bottom; alternatively we could
                // send a dedicated bottom command. Use CopyScrollTo with MAX to trigger bottom.
                // Fallback: also send delta large to ensure bottom
                // Actually daemon clamps ROW to total-len, so MAX goes to bottom.
            }
        }
        self.mode = Mode::Normal;
        self.mark_dirty();
    }

    pub(super) fn copy_pane_dims(&self, pane_id: u64) -> Option<(u16, u16)> {
        for (pid, rect) in &self.rects {
            if *pid == pane_id {
                let inner = PaneGeom { pane_id, rect: *rect }.inner();
                return Some((inner.width.max(1), inner.height.max(1)));
            }
        }
        // fallback to grid dims
        self.grids.get(&pane_id).map(|g| (g.cols as u16, g.rows as u16))
    }

    fn copy_scroll(&mut self, delta: i32) {
        if let Some(cs) = self.copy.as_ref() {
            let _ = self.send(&Command::CopyScroll { pane_id: cs.pane_id, delta });
        }
    }

    fn copy_scroll_to(&mut self, row: u32) {
        if let Some(cs) = self.copy.as_ref() {
            let _ = self.send(&Command::CopyScrollTo { pane_id: cs.pane_id, row });
        }
    }

    fn copy_jump_to_hit(&mut self, idx: usize) {
        let (pane_id, row) = {
            let Some(cs) = self.copy.as_ref() else { return; };
            if idx >= cs.hits.len() { return; }
            let hit = &cs.hits[idx];
            (cs.pane_id, hit.row)
        };
        // Scroll so hit row is visible: place it roughly centered
        // Compute: need offset so that hit row in middle of viewport.
        // Simpler: scroll to hit.row (top) then adjust. Use copy_scroll_to directly.
        // To center, scroll to hit.row saturating_sub(rows/2)
        let dims = self.copy_pane_dims(pane_id);
        let half = dims.map(|(_, h)| h / 2).unwrap_or(0) as u32;
        let target = row.saturating_sub(half);
        let _ = self.send(&Command::CopyScrollTo { pane_id, row: target });
        // Move cursor to hit start within viewport
        if let Some(cs) = self.copy.as_mut() {
            // capture hit start before any other borrow issues
            let hit_start = cs.hits.get(idx).map(|h| h.start_col).unwrap_or(0);
            if let Some(g) = self.grids.get(&pane_id) {
                if let Some(scroll) = g.scroll {
                    // viewport top = scroll.offset (screen offset)
                    let top = scroll.offset as u32;
                    let rows = dims.map(|(_, h)| h).unwrap_or(24);
                    let viewport_y = row.saturating_sub(top);
                    if viewport_y < rows as u32 {
                        cs.cursor = (hit_start.min(dims.map(|(w,_)| w.saturating_sub(1)).unwrap_or(0)), viewport_y as u16);
                    }
                } else {
                    cs.cursor.0 = hit_start;
                }
            } else {
                cs.cursor.0 = hit_start;
            }
            cs.hit_idx = Some(idx);
        }
        self.mark_dirty();
    }

    pub(super) fn on_copy_key(&mut self, key: KeyEvent) -> Result<()> {
        // If search input is active, handle editing
        if self.copy.as_ref().map(|c| c.search_active).unwrap_or(false) {
            return self.on_copy_search_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let dims = self.copy.as_ref().and_then(|c| self.copy_pane_dims(c.pane_id)).unwrap_or((80, 24));
        let (cols, rows) = dims;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if !ctrl => {
                self.leave_copy_mode(true);
                return Ok(());
            }
            KeyCode::Char('h') | KeyCode::Left => self.copy_move(-1, 0, cols, rows),
            KeyCode::Char('j') | KeyCode::Down => self.copy_move(0, 1, cols, rows),
            KeyCode::Char('k') | KeyCode::Up => self.copy_move(0, -1, cols, rows),
            KeyCode::Char('l') | KeyCode::Right => self.copy_move(1, 0, cols, rows),
            KeyCode::Char('0') | KeyCode::Home => {
                if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = 0; }
                self.mark_dirty();
            }
            KeyCode::Char('$') | KeyCode::End => {
                if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = cols.saturating_sub(1); }
                // For linewise, also handle yank later
                self.mark_dirty();
            }
            KeyCode::Char('g') => {
                // g -> top of history
                self.copy_scroll_to(0);
                if let Some(cs) = self.copy.as_mut() { cs.cursor = (0, 0); }
                self.mark_dirty();
            }
            KeyCode::Char('G') => {
                // G -> bottom
                self.copy_scroll_to(u32::MAX);
                if let Some(cs) = self.copy.as_mut() { cs.cursor = (0, rows.saturating_sub(1)); }
                self.mark_dirty();
            }
            KeyCode::Char('v') if !ctrl => {
                if let Some(cs) = self.copy.as_mut() {
                    if cs.anchor.is_none() {
                        cs.anchor = Some(cs.cursor);
                        cs.linewise = false;
                        // install daemon selection for highlight
                        let pane_id = cs.pane_id;
                        let start = cs.cursor;
                        let _ = self.send(&Command::CopySetSelection { pane_id, start, end: start });
                    } else {
                        // toggle off if already selecting charwise at same point? keep
                        cs.anchor = Some(cs.cursor);
                        cs.linewise = false;
                    }
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::Char('V') => {
                if let Some(cs) = self.copy.as_mut() {
                    cs.anchor = Some(cs.cursor);
                    cs.linewise = true;
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::Char('y') | KeyCode::Enter if !ctrl => {
                self.copy_yank();
                self.leave_copy_mode(true);
            }
            KeyCode::Char('/') => {
                if let Some(cs) = self.copy.as_mut() {
                    cs.search_active = true;
                    cs.search_input.clear();
                    cs.search_cursor = 0;
                    cs.search_forward = true;
                }
                self.mark_dirty();
            }
            KeyCode::Char('?') => {
                if let Some(cs) = self.copy.as_mut() {
                    cs.search_active = true;
                    cs.search_input.clear();
                    cs.search_cursor = 0;
                    cs.search_forward = false;
                }
                self.mark_dirty();
            }
            KeyCode::Char('n') if !ctrl => self.copy_next_hit(true),
            KeyCode::Char('N') => self.copy_next_hit(false),
            _ if ctrl && matches!(key.code, KeyCode::Char('u')) => {
                let half = (rows as i32 / 2).max(1);
                self.copy_scroll(-half);
                if let Some(cs) = self.copy.as_mut() {
                    cs.cursor.1 = cs.cursor.1.saturating_sub(half as u16);
                    if cs.cursor.1 == 0 { cs.cursor.1 = 0; }
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            _ if ctrl && matches!(key.code, KeyCode::Char('d')) => {
                let half = (rows as i32 / 2).max(1);
                self.copy_scroll(half);
                if let Some(cs) = self.copy.as_mut() {
                    cs.cursor.1 = (cs.cursor.1 + half as u16).min(rows.saturating_sub(1));
                }
                self.sync_copy_selection();
                self.mark_dirty();
            }
            _ if ctrl && matches!(key.code, KeyCode::Char('b')) => {
                self.copy_scroll(-(rows as i32));
                self.sync_copy_selection();
                self.mark_dirty();
            }
            _ if ctrl && matches!(key.code, KeyCode::Char('f')) => {
                self.copy_scroll(rows as i32);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::PageUp => {
                self.copy_scroll(-(rows as i32));
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::PageDown => {
                self.copy_scroll(rows as i32);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            KeyCode::Char('w') => self.copy_word(1, cols, rows),
            KeyCode::Char('b') => self.copy_word(-1, cols, rows),
            _ => {}
        }
        Ok(())
    }

    fn on_copy_search_key(&mut self, key: KeyEvent) -> Result<()> {
        let mut commit: Option<String> = None;
        let mut cancel = false;
        if let Some(cs) = self.copy.as_mut() {
            match key.code {
                KeyCode::Esc => { cancel = true; }
                KeyCode::Enter => {
                    commit = Some(cs.search_input.clone());
                }
                KeyCode::Backspace => {
                    if cs.search_cursor > 0 && !cs.search_input.is_empty() {
                        let mut chars: Vec<char> = cs.search_input.chars().collect();
                        let idx = cs.search_cursor.min(chars.len()).saturating_sub(1);
                        chars.remove(idx);
                        cs.search_input = chars.into_iter().collect();
                        cs.search_cursor = cs.search_cursor.saturating_sub(1);
                    }
                }
                KeyCode::Delete => {
                    let mut chars: Vec<char> = cs.search_input.chars().collect();
                    if cs.search_cursor < chars.len() {
                        chars.remove(cs.search_cursor);
                        cs.search_input = chars.into_iter().collect();
                    }
                }
                KeyCode::Left => { cs.search_cursor = cs.search_cursor.saturating_sub(1); }
                KeyCode::Right => {
                    let len = cs.search_input.chars().count();
                    if cs.search_cursor < len { cs.search_cursor += 1; }
                }
                KeyCode::Home => { cs.search_cursor = 0; }
                KeyCode::End => { cs.search_cursor = cs.search_input.chars().count(); }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    let mut chars: Vec<char> = cs.search_input.chars().collect();
                    let idx = cs.search_cursor.min(chars.len());
                    chars.insert(idx, c);
                    cs.search_input = chars.into_iter().collect();
                    cs.search_cursor += 1;
                }
                _ => {}
            }
        }
        if cancel {
            if let Some(cs) = self.copy.as_mut() {
                cs.search_active = false;
                cs.search_input.clear();
                cs.search_cursor = 0;
            }
            self.mark_dirty();
            return Ok(());
        }
        if let Some(q) = commit {
            let pane_id = self.copy.as_ref().map(|c| c.pane_id).unwrap_or(0);
            let forward = self.copy.as_ref().map(|c| c.search_forward).unwrap_or(true);
            if let Some(cs) = self.copy.as_mut() {
                cs.search_active = false;
                cs.search_query = if q.is_empty() { None } else { Some(q.clone()) };
                cs.search_forward = forward;
                // keep input for display? clear after commit but keep query
                cs.search_input.clear();
                cs.search_cursor = 0;
            }
            if !q.is_empty() {
                let _ = self.send(&Command::CopySearch { pane_id, query: q });
            } else {
                if let Some(cs) = self.copy.as_mut() {
                    cs.hits.clear();
                    cs.hit_idx = None;
                }
            }
            self.mark_dirty();
            return Ok(());
        }
        self.mark_dirty();
        Ok(())
    }

    fn copy_move(&mut self, dx: i32, dy: i32, cols: u16, rows: u16) {
        let need_scroll = {
            let Some(cs) = self.copy.as_mut() else { return; };
            let mut nx = cs.cursor.0 as i32 + dx;
            let mut ny = cs.cursor.1 as i32 + dy;
            let mut scroll_delta: Option<i32> = None;
            if nx < 0 { nx = 0; }
            if nx >= cols as i32 { nx = cols as i32 - 1; }
            if ny < 0 {
                scroll_delta = Some(-1);
                ny = 0;
            } else if ny >= rows as i32 {
                scroll_delta = Some(1);
                ny = rows as i32 - 1;
            }
            cs.cursor = (nx as u16, ny as u16);
            scroll_delta
        };
        if let Some(delta) = need_scroll {
            self.copy_scroll(delta);
        }
        self.sync_copy_selection();
        self.mark_dirty();
    }

    fn copy_word(&mut self, dir: i32, cols: u16, rows: u16) {
        // Simple word motion within viewport (no scroll across history for now)
        let Some(cs) = self.copy.as_ref() else { return; };
        let pane_id = cs.pane_id;
        let cur = cs.cursor;
        let Some(grid) = self.grids.get(&pane_id) else {
            self.copy_move(dir, 0, cols, rows);
            return;
        };
        // Build line string for current row
        let row = cur.1 as usize;
        let line = Self::grid_row_text(grid, row);
        let col = cur.0 as usize;
        let chars: Vec<char> = line.chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if dir > 0 {
            // forward: skip current word chars, then skip non-word, land at start of next word
            let mut i = col.min(chars.len());
            // if at word, skip rest of word
            if i < chars.len() && is_word(chars[i]) {
                while i < chars.len() && is_word(chars[i]) { i += 1; }
            }
            // skip non-word (spaces/punct) to next word
            while i < chars.len() && !is_word(chars[i]) { i += 1; }
            let new_col = (i as u16).min(cols.saturating_sub(1));
            if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = new_col; }
        } else {
            // backward: move to start of previous word
            let mut i = col.min(chars.len());
            if i == 0 {
                // try previous row
                if row > 0 {
                    let prev_line = Self::grid_row_text(grid, row - 1);
                    let _prev_chars: Vec<char> = prev_line.chars().collect();
                    let trimmed = prev_line.trim_end();
                    let end = trimmed.chars().count().min(cols as usize);
                    if let Some(cs) = self.copy.as_mut() {
                        cs.cursor = (end.saturating_sub(1) as u16, (row - 1) as u16);
                    }
                }
            } else {
                // step back one, skip non-word, then skip word to its start
                i = i.saturating_sub(1);
                while i > 0 && !is_word(chars[i]) { i -= 1; }
                while i > 0 && is_word(chars[i - 1]) { i -= 1; }
                if let Some(cs) = self.copy.as_mut() { cs.cursor.0 = i as u16; }
            }
        }
        self.sync_copy_selection();
        self.mark_dirty();
    }

    fn grid_row_text(grid: &Grid, row: usize) -> String {
        if row >= grid.rows { return String::new(); }
        let cells = &grid.cells[row];
        let mut s = String::new();
        for c in cells {
            if c.cell_width == 0 { continue; }
            s.push_str(&c.text);
        }
        s.trim_end().to_string()
    }

    fn sync_copy_selection(&mut self) {
        let (pane_id, start, end, linewise) = match self.copy.as_ref() {
            Some(cs) if cs.anchor.is_some() => {
                let a = cs.anchor.unwrap();
                let b = cs.cursor;
                // Normalize order for daemon selection (viewport coords)
                let (s, e) = if cs.linewise {
                    // whole lines between rows
                    let top = a.1.min(b.1);
                    let bottom = a.1.max(b.1);
                    // need cols
                    let dims = self.copy_pane_dims(cs.pane_id).unwrap_or((80, 24));
                    ((0, top), (dims.0.saturating_sub(1), bottom))
                } else {
                    // charwise: ensure start <= end in row-major order
                    if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) }
                };
                (cs.pane_id, s, e, cs.linewise)
            }
            _ => return,
        };
        let _ = self.send(&Command::CopySetSelection { pane_id, start, end });
        let _ = linewise; // keep for future block selection parity
    }

    fn copy_yank(&mut self) {
        let Some(cs) = self.copy.as_ref() else { return; };
        let pane_id = cs.pane_id;
        let Some(grid) = self.grids.get(&pane_id) else { return; };
        let Some(anchor) = cs.anchor else {
            // No selection: yank current line? Like tmux if no selection, yank? For now do nothing
            return;
        };
        let cursor = cs.cursor;
        let linewise = cs.linewise;
        let text = if linewise {
            let top = anchor.1.min(cursor.1) as usize;
            let bottom = anchor.1.max(cursor.1) as usize;
            let mut out = String::new();
            for r in top..=bottom {
                let line = Self::grid_row_text(grid, r);
                out.push_str(&line);
                if r != bottom { out.push('\n'); }
            }
            out
        } else {
            // charwise: row-major extraction similar to Sel logic but using grid cells
            let (start, end) = if (anchor.1, anchor.0) <= (cursor.1, cursor.0) { (anchor, cursor) } else { (cursor, anchor) };
            let mut out = String::new();
            for r in start.1..=end.1 {
                let cells = grid.cells.get(r as usize);
                if cells.is_none() { continue; }
                let cells = cells.unwrap();
                let c0 = if r == start.1 { start.0 as usize } else { 0 };
                let c1 = if r == end.1 { end.0 as usize } else { cells.len().saturating_sub(1) };
                for cell in cells.iter().take(c1.min(cells.len().saturating_sub(1)) + 1).skip(c0) {
                    if cell.cell_width == 0 { continue; }
                    out.push_str(&cell.text);
                }
                if r != end.1 { out.push('\n'); }
            }
            // Trim trailing whitespace per row already handled? Keep as is.
            out
        };
        if !text.trim().is_empty() {
            crate::cli::util::copy_to_clipboard(&text);
            self.status_msg = Some((format!("copied {} chars", text.chars().count()), Instant::now()));
            self.notice = None;
        }
        let _ = self.send(&Command::CopyClearSelection { pane_id });
    }

    fn copy_next_hit(&mut self, forward: bool) {
        let (len, cur) = match self.copy.as_ref() {
            Some(cs) if !cs.hits.is_empty() => (cs.hits.len(), cs.hit_idx),
            _ => return,
        };
        let next = match cur {
            Some(idx) if forward => (idx + 1) % len,
            Some(idx) if !forward => (idx + len - 1) % len,
            None if forward => 0,
            None => len - 1,
            _ => 0,
        };
        self.copy_jump_to_hit(next);
    }

    pub(super) fn on_copy_mouse(&mut self, m: MouseEvent) -> Result<()> {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.copy_scroll(-3);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            MouseEventKind::ScrollDown => {
                self.copy_scroll(3);
                self.sync_copy_selection();
                self.mark_dirty();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let x = m.column;
                let y = m.row;
                if let Some((pid, rect)) = self.pane_at(x, y) {
                    if self.copy.as_ref().map(|c| c.pane_id == pid).unwrap_or(false) {
                        let inner = PaneGeom { pane_id: pid, rect }.inner();
                        if inner.contains(Position::new(x, y)) {
                            let cx = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let cy = y.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                            if let Some(cs) = self.copy.as_mut() {
                                cs.cursor = (cx, cy);
                            }
                            self.sync_copy_selection();
                            self.mark_dirty();
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let x = m.column;
                let y = m.row;
                if let Some(cs) = self.copy.as_ref() {
                    let pid = cs.pane_id;
                    if let Some((_, rect)) = self.rects.iter().find(|(id, _)| *id == pid) {
                        let inner = PaneGeom { pane_id: pid, rect: *rect }.inner();
                        if inner.contains(Position::new(x, y)) {
                            let cx = x.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
                            let cy = y.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
                            if let Some(cs) = self.copy.as_mut() {
                                // auto-start selection on drag if not yet anchoring
                                if cs.anchor.is_none() {
                                    cs.anchor = Some(cs.cursor);
                                    cs.linewise = false;
                                }
                                cs.cursor = (cx, cy);
                            }
                            self.sync_copy_selection();
                            self.mark_dirty();
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // nothing extra; selection already synced on drag
            }
            _ => {}
        }
        Ok(())
    }
}
