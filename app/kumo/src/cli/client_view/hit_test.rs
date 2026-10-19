//! Overlay geometry and hit-testing (menus, popups, settings, toasts, notices).

use super::*;

impl View {
    // ------------------------------------------------------------------
    // Overlay hit-testing (menus / popup / settings)
    // ------------------------------------------------------------------

    pub(super) fn agent_inbox_rect(&self) -> Option<Rect> {
        if self.mode != Mode::Inbox || self.cols < 28 || self.rows < 11 {
            return None;
        }
        let width = self.cols.saturating_sub(4).min(72);
        let height = self.rows.saturating_sub(2).min(18);
        Some(Rect::new((self.cols - width) / 2, (self.rows - height) / 2, width, height))
    }

    pub(super) fn agent_inbox_visible_rows(&self) -> usize {
        self.agent_inbox_rect().map(|rect| rect.height.saturating_sub(8) as usize).unwrap_or(1).max(1)
    }

    pub(super) fn agent_inbox_view_start(&self) -> usize {
        let len = self.inbox_entries().len();
        let visible = self.agent_inbox_visible_rows();
        let selected = self.inbox.as_ref().map(|inbox| inbox.sel).unwrap_or(0).min(len.saturating_sub(1));
        selected.saturating_sub(visible.saturating_sub(1)).min(len.saturating_sub(visible))
    }

    pub(super) fn agent_inbox_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let rect = self.agent_inbox_rect()?;
        let first_y = rect.y + 3;
        let visible = self.agent_inbox_visible_rows() as u16;
        if x <= rect.x || x >= rect.right().saturating_sub(1) || y < first_y || y >= first_y + visible {
            return None;
        }
        let idx = self.agent_inbox_view_start() + (y - first_y) as usize;
        (idx < self.inbox_entries().len()).then_some(idx)
    }

    fn menu_btn_x(&self) -> u16 {
        // Mirrors status_bar::slot_spans for [Mode, Menu, ..]: the mode chip
        // (" NORMAL " / " LEADER ") followed by the " · " group separator.
        let mode = if self.mode == Mode::Leader { "LEADER" } else { "NORMAL" };
        format!(" {} ", mode).chars().count() as u16 + " · ".chars().count() as u16
    }

    fn menu_btn_rect(&self) -> Option<Rect> {
        if self.status_h() == 0 || !self.status_bar_contains(StatusWidget::Menu) {
            return None;
        }
        let bw = MENU_BTN.chars().count() as u16;
        let x = self.menu_btn_x();
        (self.cols >= x + bw).then(|| Rect::new(x, self.rows.saturating_sub(1), bw, 1))
    }

    pub(super) fn menu_btn_at(&self, x: u16, y: u16) -> bool {
        self.menu_btn_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false)
    }

    pub(super) fn menu_dropdown_rect(&self) -> Option<Rect> {
        if self.status_h() == 0 {
            return None;
        }
        let width = MENU_ITEMS.iter().map(|i| i.chars().count()).max().unwrap_or(0) as u16 + 4;
        let height = MENU_ITEMS.len() as u16 + 2;
        let floor = self.shadow_floor();
        if self.cols < width || floor < height {
            return None;
        }
        let btn_w = MENU_BTN.chars().count() as u16;
        let x = (self.menu_btn_x() + btn_w).saturating_sub(width).min(self.cols.saturating_sub(width));
        // Flush above the status bar, never covering it.
        let y = floor.saturating_sub(height);
        Some(Rect::new(x, y, width, height))
    }

    pub(super) fn menu_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.menu_dropdown_rect()?;
        MENU_ITEMS.iter().enumerate().position(|(i, _)| {
            let item = Rect::new(dd.x + 1, dd.y + 1 + i as u16, dd.width.saturating_sub(2), 1);
            item.contains(Position::new(x, y))
        })
    }

    pub(super) fn ctx_menu_rect(&self) -> Option<Rect> {
        if !self.ctx_menu.open {
            return None;
        }
        let items = self.ctx_items();
        let width = items.iter().map(|i| i.chars().count()).max().unwrap_or(0) as u16 + 4;
        let height = items.len() as u16 + 2;
        // The menu body must stay above the bottom chrome (status bar).
        let floor = self.shadow_floor();
        if self.cols < width || floor < height {
            return None;
        }
        let px = self.ctx_menu.x;
        let py = self.ctx_menu.y;
        let x = if px.saturating_add(1) + width <= self.cols { px + 1 } else { px.saturating_sub(width) };
        let y = if py.saturating_add(1) + height <= floor { py + 1 } else { py.saturating_sub(height) };
        Some(Rect::new(x, y, width, height))
    }

    pub(super) fn ctx_menu_at(&self, x: u16, y: u16) -> bool {
        self.ctx_menu_rect().map(|r| r.contains(Position::new(x, y))).unwrap_or(false)
    }

    pub(super) fn ctx_menu_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.ctx_menu_rect()?;
        let items = self.ctx_items();
        items.iter().enumerate().position(|(i, _)| {
            let item = Rect::new(dd.x + 1, dd.y + 1 + i as u16, dd.width.saturating_sub(2), 1);
            item.contains(Position::new(x, y))
        })
    }

    pub(super) fn name_popup_rect(&self) -> Option<Rect> {
        if self.cols < SESSION_POPUP_W || self.rows < SESSION_POPUP_H {
            return None;
        }
        Some(Rect::new(
            (self.cols - SESSION_POPUP_W) / 2,
            (self.rows - SESSION_POPUP_H) / 2,
            SESSION_POPUP_W,
            SESSION_POPUP_H,
        ))
    }

    pub(super) fn copy_search_rect(&self) -> Option<Rect> {
        // Popup just below tabs with a line gap: tabs at y=0, gap at y=1, popup at y=2
        // Centered in the panes area (not full screen) so sidebar stays visible.
        let pa = self.panes_area();
        if pa.width < COPY_SEARCH_W || pa.height < COPY_SEARCH_H + 1 {
            return None;
        }
        let w = COPY_SEARCH_W.min(pa.width.saturating_sub(2));
        let h = COPY_SEARCH_H.min(pa.height.saturating_sub(1));
        let x = pa.x + (pa.width.saturating_sub(w)) / 2;
        let y = pa.y + 2; // two below tabs line (tabs y=0, gap y=1, popup y=2)
        // Clamp inside panes area
        let y = y.min(pa.y + pa.height.saturating_sub(h));
        Some(Rect::new(x, y, w, h))
    }

    /// Screen rects of the visible agent toasts as `(toast index, rect)`
    /// pairs, anchored per `[notifications] position` (`self.toast_pos`):
    /// corners hug the tab bar or status bar with a one-row gap, `Center`
    /// floats the whole stack mid-screen. The newest toast hugs the anchor
    /// edge, older ones stack inward until the opposite chrome cuts the
    /// stack off; the pairs stay aligned with `agent_toasts` indexes.
    pub(super) fn agent_toast_rects(&self) -> Vec<(usize, Rect)> {
        const GAP: u16 = 1;
        let mut rects = Vec::new();
        let n = self.agent_toasts.len();
        if n == 0 || self.toast_pos == ToastPosition::Off || self.cols < AGENT_TOAST_W + 2 {
            return rects;
        }
        // Vertical band between the chrome: first row below the tab bar and
        // last row above the status bar.
        let floor = self.shadow_floor();
        let top_edge = TAB_H + 1;
        let last_free = floor.saturating_sub(1);
        if self.toast_pos == ToastPosition::Center {
            let x = (self.cols - AGENT_TOAST_W) / 2;
            let total = n as u16 * AGENT_TOAST_H + (n as u16 - 1) * GAP;
            let band = last_free.saturating_sub(top_edge) + 1;
            let y0 = if total >= band { top_edge } else { top_edge + (band - total) / 2 };
            for i in 0..n {
                let y = y0 + i as u16 * (AGENT_TOAST_H + GAP);
                if y + AGENT_TOAST_H > last_free + 1 {
                    break;
                }
                rects.push((i, Rect::new(x, y, AGENT_TOAST_W, AGENT_TOAST_H)));
            }
            return rects;
        }
        let right = matches!(self.toast_pos, ToastPosition::TopRight | ToastPosition::BottomRight);
        let from_top = matches!(self.toast_pos, ToastPosition::TopLeft | ToastPosition::TopRight);
        let x = if right { self.cols - AGENT_TOAST_W - 1 } else { 1 };
        for i in (0..n).rev() {
            let consumed = (n - 1 - i) as u16 * (AGENT_TOAST_H + GAP);
            let y = if from_top {
                let y = top_edge + consumed;
                if y + AGENT_TOAST_H > last_free + 1 {
                    break;
                }
                y
            } else {
                let bottom = floor.saturating_sub(1 + consumed);
                if bottom < AGENT_TOAST_H {
                    break;
                }
                bottom - AGENT_TOAST_H
            };
            rects.push((i, Rect::new(x, y, AGENT_TOAST_W, AGENT_TOAST_H)));
        }
        rects.reverse();
        rects
    }

    /// Which toast (index into `agent_toasts`) covers `(x, y)`, if any.
    pub(super) fn agent_toast_at(&self, x: u16, y: u16) -> Option<usize> {
        if self.agent_toasts.is_empty() {
            return None;
        }
        self.agent_toast_rects()
            .into_iter()
            .find(|(_, r)| r.contains(Position::new(x, y)))
            .map(|(i, _)| i)
    }

    pub(super) fn name_popup_input_cursor(&self) -> Option<(u16, u16)> {
        let dd = self.name_popup_rect()?;
        let text_w = (dd.width - 4) as usize - 1;
        let name = &self.popup.name;
        let cursor = self.popup.cursor.min(name.chars().count());
        let end = cursor + 1;
        let start = end.saturating_sub(text_w);
        let col = dd.x + 2 + cursor.saturating_sub(start) as u16;
        Some((col, dd.y + 3))
    }

    pub(super) fn name_popup_button_rect(&self, btn: PopupBtn) -> Option<Rect> {
        let dd = self.name_popup_rect()?;
        let label = match btn {
            PopupBtn::Enter => "⏎ enter ",
            PopupBtn::Cancel => " esc cancel ",
        };
        let w = label.chars().count() as u16;
        let x = match btn {
            PopupBtn::Enter => dd.x + 2,
            PopupBtn::Cancel => dd.x + 2 + 10,
        };
        Some(Rect::new(x, dd.y + 4, w, 1))
    }

    pub(super) fn name_popup_button_at(&self, x: u16, y: u16) -> Option<PopupBtn> {
        [PopupBtn::Enter, PopupBtn::Cancel].into_iter().find(|btn| {
            self.name_popup_button_rect(*btn).map(|r| r.contains(Position::new(x, y))).unwrap_or(false)
        })
    }

    pub(super) fn settings_rect(&self) -> Option<Rect> {
        if self.cols < 30 || self.rows < 12 {
            return None;
        }
        let width = (self.cols * 3 / 5).max(50).min(self.cols.saturating_sub(4));
        let height = (self.rows * 3 / 5).max(18).min(self.rows.saturating_sub(4));
        Some(Rect::new((self.cols - width) / 2, (self.rows - height) / 2, width, height))
    }

    pub(super) fn settings_tabs_rect(&self) -> Option<Rect> {
        let dd = self.settings_rect()?;
        Some(Rect::new(dd.x + 2, dd.y + 2, 16, dd.height.saturating_sub(4)))
    }

    pub(super) fn settings_content_rect(&self) -> Option<Rect> {
        let dd = self.settings_rect()?;
        let w = dd.width.saturating_sub(4).saturating_sub(17);
        Some(Rect::new(dd.x + 2 + 16 + 1, dd.y + 2, w, dd.height.saturating_sub(4)))
    }

    pub(super) fn settings_tab_at(&self, x: u16, y: u16) -> Option<usize> {
        let tabs = self.settings_tabs_rect()?;
        SETTINGS_TABS.iter().enumerate().position(|(i, _)| {
            let item = Rect::new(tabs.x, tabs.y + i as u16, tabs.width, 1);
            item.contains(Position::new(x, y))
        })
    }

    pub(super) fn settings_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let content = self.settings_content_rect()?;
        match SETTINGS_TABS.get(self.settings.tab).copied()? {
            SettingsTab::Appearance => (0..self.themes_len()).position(|i| {
                let item = Rect::new(content.x, content.y + 1 + i as u16, content.width, 1);
                item.contains(Position::new(x, y))
            }),
            SettingsTab::Agents => (0..AGENT_SKILL_TARGETS.len()).position(|i| {
                let item = Rect::new(content.x, content.y + 3 + (i as u16 * 2), content.width, 2);
                item.contains(Position::new(x, y))
            }),
            SettingsTab::About => None,
        }
    }

    pub(super) fn update_notice_lines(&self) -> Option<(String, String)> {
        let (_, display) = self.update_notice.as_ref()?;
        Some((
            format!("New version {} available", display),
            "run 'kumo update'".to_string(),
        ))
    }

    pub(super) fn update_notice_rect(&self) -> Option<Rect> {
        let (line1, line2) = self.update_notice_lines()?;
        let inner_w = line1.chars().count().max(line2.chars().count()) as u16 + 6;
        let width = inner_w + 2;
        if self.cols < width + 1 || self.rows < 4 {
            return None;
        }
        Some(Rect::new(self.cols - width - 1, 0, width, 4))
    }

    pub(super) fn update_notice_close_at(&self, x: u16, y: u16) -> bool {
        let Some(r) = self.update_notice_rect() else { return false };
        x == r.x + 2 && y == r.y + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::client_view::tests::*;

    /// Toasts stack downward from the tab bar in the default top-right anchor
    /// (newest at the top) and the stack caps at `MAX_AGENT_TOASTS`, dropping
    /// the oldest.
    #[test]
    fn agent_toasts_stack_downward_and_cap() {
        let mut view = test_view();
        for i in 0..(MAX_AGENT_TOASTS + 2) {
            view.on_event(DaemonEvent::Toast {
                pane_id: i as u64,
                kind: ToastKind::Finished,
                title: format!("agent {i}"),
                body: String::new(),
            });
        }
        assert_eq!(view.agent_toasts.len(), MAX_AGENT_TOASTS, "oldest toasts dropped");
        assert_eq!(view.agent_toasts[0].pane_id, 2, "the two oldest were dropped");
        let rects = view.agent_toast_rects();
        assert_eq!(rects.len(), MAX_AGENT_TOASTS);
        // Each rect pair carries its toast index; the newest hugs the anchor
        // edge, the older ones sit below it.
        let expected: Vec<String> = view.agent_toasts.iter().map(|t| format!("agent {}", t.pane_id)).collect();
        for (i, _) in &rects {
            assert_eq!(&view.agent_toasts[*i].title, &expected[*i], "rect aligns with its toast");
        }
        let right_x = view.cols - AGENT_TOAST_W - 1;
        for pair in rects.windows(2) {
            assert!(pair[1].1.y + AGENT_TOAST_H <= pair[0].1.y, "newer toast sits above the older one");
            assert_eq!(pair[0].1.x, right_x, "toasts stay right-aligned");
            assert_eq!(pair[1].1.x, right_x, "toasts stay right-aligned");
        }
        // The newest top row sits directly below the tab bar.
        let last = rects.last().unwrap();
        assert_eq!(last.1.y, TAB_H + 1);
        // The whole stack stays clear of the status bar.
        for (_, r) in &rects {
            assert!(r.y + r.height < view.shadow_floor(), "toasts must not cover the status bar");
        }
    }

    /// The `[notifications] position` setting moves the stack: corners mirror,
    /// `center` floats mid-screen, and `off` drops events before they queue.
    #[test]
    fn agent_toast_positions_follow_config() {
        let push = |view: &mut View, pane_id: u64| {
            view.on_event(DaemonEvent::Toast {
                pane_id,
                kind: ToastKind::Finished,
                title: format!("agent {pane_id}"),
                body: String::new(),
            });
        };
        // Bottom-left mirrors the default: left-aligned, newest just above
        // the status bar.
        let mut view = test_view();
        view.toast_pos = ToastPosition::BottomLeft;
        push(&mut view, 0);
        push(&mut view, 1);
        let rects = view.agent_toast_rects();
        assert_eq!(rects.len(), 2);
        for (_, r) in &rects {
            assert_eq!(r.x, 1, "bottom-left toasts are left-aligned");
        }
        assert!(rects[0].1.y + AGENT_TOAST_H <= rects[1].1.y, "older toast stacks above the newer one");
        let last = rects.last().unwrap();
        assert_eq!(last.1.y + AGENT_TOAST_H, view.shadow_floor() - 1);

        // Top-left anchors below the tab bar like the default corner.
        let mut view = test_view();
        view.toast_pos = ToastPosition::TopLeft;
        push(&mut view, 3);
        let rect = view.agent_toast_rects()[0].1;
        assert_eq!(rect.x, 1);
        assert_eq!(rect.y, TAB_H + 1);

        // Center floats the stack mid-screen.
        let mut view = test_view();
        view.toast_pos = ToastPosition::Center;
        push(&mut view, 4);
        push(&mut view, 5);
        let rects = view.agent_toast_rects();
        assert_eq!(rects.len(), 2);
        let total_h = 2 * AGENT_TOAST_H + 1;
        let band = view.shadow_floor() - 1 - (TAB_H + 1) + 1;
        let y0 = TAB_H + 1 + (band - total_h) / 2;
        assert_eq!(rects[0].1.y, y0, "the stack is vertically centered");
        assert_eq!(rects[1].1.y, y0 + AGENT_TOAST_H + 1);
        assert_eq!(rects[0].1.x, (view.cols - AGENT_TOAST_W) / 2, "the stack is horizontally centered");

        // `off` never queues a toast event.
        let mut view = test_view();
        view.toast_pos = ToastPosition::Off;
        push(&mut view, 9);
        assert!(view.agent_toasts.is_empty(), "position off must drop toast events");
        assert!(view.agent_toast_rects().is_empty());
        assert!(!view.has_transient());
    }
}
