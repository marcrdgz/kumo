use super::*;

impl View {
    pub(super) fn render_status(&self, f: &mut Frame) {
        self.config_chip.set(None);
        if self.status_h() == 0 {
            return;
        }
        let theme = self.current_theme();
        let area = Rect::new(0, self.rows.saturating_sub(1), self.cols, 1);
        fill(f, area, RColor::Reset);

        // Build widget contexts: all status widgets are client-side, derived from Layout.
        let session = self.active_session();
        let ctx = SlotContext {
            cfg: &self.status_bar,
            session,
            theme: &theme,
            hostname: &self.hostname,
            clock_str: &self.clock_str,
            is_ssh: self.is_ssh,
            is_leader: self.mode == Mode::Leader,
            menu_open: self.menu.open,
            sidebar_open: self.sidebar_open,
            github: self.github_status.as_ref(),
            spinner: self.spinner_char(),
        };

        // Snapshot slots so we can iteratively drop low-priority widgets on overflow.
        let mut left_cfg = self.status_bar.left.clone();
        let mut center_cfg = self.status_bar.center.clone();
        let mut right_cfg = self.status_bar.right.clone();

        // Helper to measure a slot.
        let measure = |slot: &[StatusWidget]| -> u16 {
            let spans = status_bar::slot_spans(slot, &ctx);
            status_bar::spans_width(&spans)
        };

        // Priority of widgets to drop first when the bar overflows (least important first).
        // Mode/Menu are never dropped (they host the leader chip and the menu button).
        let drop_priority: &[StatusWidget] = &[
            StatusWidget::Hostname,
            StatusWidget::Clock,
            StatusWidget::Github,
            StatusWidget::Branch,
            StatusWidget::AgentStatus,
            StatusWidget::Session,
        ];

        // Reserve width for the right-aligned transient (leader hint / copied toast / copy-mode hint)
        // so it stays readable on a narrow terminal; it overlays the bar.
        let copy_hint: Option<String> = if self.mode == Mode::Copy {
            if let Some(cs) = self.copy.as_ref() {
                if cs.search_active { None } else {
                    Some(if cs.hits.is_empty() {
                        if let Some(q) = cs.search_query.as_ref() {
                            format!(" COPY {} (no matches) · q:quit ", q)
                        } else {
                            " COPY h/j/k/l · 0/$ · g/G · v/V · y · /? · n/N · q ".to_string()
                        }
                    } else {
                        let idx = cs.hit_idx.map(|i| i+1).unwrap_or(0);
                        format!(" COPY {} [{}/{}] · n/N · q ", cs.search_query.as_deref().unwrap_or(""), idx, cs.hits.len())
                    })
                }
            } else { Some(" COPY ".to_string()) }
        } else { None };
        let transient_w: u16 = if self.mode == Mode::Leader {
            let hint = bindings::leader_hint(&self.keymap);
            hint.chars().count() as u16
        } else if let Some((msg, t)) = &self.status_msg {
            if t.elapsed() < TOAST_TIMEOUT {
                msg.chars().count() as u16
            } else if let Some(h) = &copy_hint {
                h.chars().count() as u16
            } else { 0 }
        } else if let Some(h) = &copy_hint {
            h.chars().count() as u16
        } else { 0 };

        // Overflow loop: drop lowest-priority widget present until everything fits
        // or only Mode/Menu remain.
        loop {
            let left_w = measure(&left_cfg);
            let center_w = measure(&center_cfg);
            let right_w = measure(&right_cfg);
            let total = left_w + center_w + right_w + transient_w;
            // Gaps: 1 col between left|center and center|right when both sides non-empty
            let gaps = (if center_w > 0 && left_w > 0 { 1 } else { 0 }) + (if right_w > 0 && (center_w > 0 || left_w > 0) { 1 } else { 0 });
            if total + gaps <= area.width {
                break;
            }
            // Find the lowest-priority widget that is still present and removable.
            let mut removed = false;
            for &w in drop_priority {
                if right_cfg.contains(&w) {
                    right_cfg.retain(|&x| x != w);
                    removed = true;
                    break;
                }
                if center_cfg.contains(&w) {
                    center_cfg.retain(|&x| x != w);
                    removed = true;
                    break;
                }
                if left_cfg.contains(&w) {
                    // Never drop the last Session if it's the only left content beyond Mode/Menu,
                    // but allow dropping when overflow is severe.
                    if left_cfg.len() == 1 && left_cfg[0] == w && matches!(w, StatusWidget::Session) && left_cfg.len() == 1 {
                        // keep at least one indicator; fall through to truncation below
                        continue;
                    }
                    left_cfg.retain(|&x| x != w);
                    removed = true;
                    break;
                }
            }
            if !removed {
                break;
            }
        }

        let mut left_spans = status_bar::slot_spans(&left_cfg, &ctx);
        let mut center_spans = status_bar::slot_spans(&center_cfg, &ctx);
        let mut right_spans = status_bar::slot_spans(&right_cfg, &ctx);

        // The config chip and the notice (⚠) are left-side additions after the
        // session widget, like the pre-widget bar. Keep them visible even when
        // widgets were dropped. The chip stays until the config is fixed, so
        // it is kept short; the command palette lists the problems in full.
        let separator = |spans: &mut Vec<ratatui::text::Span<'static>>| {
            if !spans.is_empty() {
                spans.push(ratatui::text::Span::styled(" · ", Style::default().fg(theme.panel_muted)));
            }
        };
        let mut chip = None;
        if !self.config_warnings.is_empty() {
            separator(&mut left_spans);
            let label = format!("⚠ config ({})", self.config_warnings.len());
            chip = Some((status_bar::spans_width(&left_spans), label.chars().count() as u16));
            let style = Style::default().fg(theme.orange).add_modifier(Modifier::BOLD);
            left_spans.push(ratatui::text::Span::styled(label, style));
        }
        if let Some((msg, t)) = &self.notice {
            if t.elapsed() < TOAST_TIMEOUT {
                separator(&mut left_spans);
                left_spans.push(ratatui::text::Span::styled(format!("⚠ {msg}"), Style::default().fg(theme.secondary)));
            }
        }

        let left_w = status_bar::spans_width(&left_spans);
        let center_w = status_bar::spans_width(&center_spans);
        let right_w = status_bar::spans_width(&right_spans);

        // If still too wide after dropping, truncate each slot proportionally.
        // Right is truncated first (it shares the edge with the transient).
        let gaps = (if center_w > 0 && left_w > 0 { 1 } else { 0 }) + (if right_w > 0 && (center_w > 0 || left_w > 0) { 1 } else { 0 });
        let total = left_w + center_w + right_w + transient_w + gaps;
        if total > area.width {
            let mut avail = area.width.saturating_sub(transient_w);
            // Reserve left first, then center, then right gets the remainder.
            let left_max = left_w.min(avail.saturating_sub(gaps));
            if status_bar::spans_width(&left_spans) > left_max {
                left_spans = status_bar::truncate_spans(left_spans, left_max);
            }
            avail = avail.saturating_sub(status_bar::spans_width(&left_spans)).saturating_sub(if left_spans.is_empty() {0} else {1});
            let center_max = center_w.min(avail);
            if status_bar::spans_width(&center_spans) > center_max {
                center_spans = status_bar::truncate_spans(center_spans, center_max);
            }
            avail = avail.saturating_sub(status_bar::spans_width(&center_spans)).saturating_sub(if center_spans.is_empty() {0} else {1});
            let right_max = right_w.min(avail);
            if status_bar::spans_width(&right_spans) > right_max {
                right_spans = status_bar::truncate_spans(right_spans, right_max);
            }
        }

        let left_w = status_bar::spans_width(&left_spans);
        let center_w = status_bar::spans_width(&center_spans);
        let right_w = status_bar::spans_width(&right_spans);

        // Truncation may have cut the chip short or off entirely.
        self.config_chip.set(chip.and_then(|(x, w)| {
            (x < left_w).then(|| Rect::new(area.x + x, area.y, w.min(left_w - x), 1))
        }));
        if left_w > 0 {
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(left_spans)), Rect::new(area.x, area.y, left_w.min(area.width), 1));
        }
        if center_w > 0 {
            let x = area.x + (area.width.saturating_sub(center_w)) / 2;
            // Avoid overlapping left: push right if needed
            let x = x.max(area.x + left_w + if left_w > 0 { 1 } else { 0 });
            // Avoid overlapping right
            let max_w = area.width.saturating_sub(x).saturating_sub(right_w).saturating_sub(if right_w > 0 {1} else {0});
            let w = center_w.min(max_w);
            if w > 0 {
                let spans = status_bar::truncate_spans(center_spans, w);
                f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans)), Rect::new(x, area.y, w, 1));
            }
        }
        if right_w > 0 {
            let x = area.x + area.width.saturating_sub(right_w);
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(right_spans)), Rect::new(x, area.y, right_w.min(area.width), 1));
        }

        // Transient right-aligned overlay: leader hint / copied toast / copy-mode hint
        // rendered last so they overwrite right-slot widgets. Priority: leader > toast > copy.
        if self.mode == Mode::Leader {
            let hint = bindings::leader_hint(&self.keymap);
            let hint_w = hint.chars().count() as u16;
            let w = hint_w.min(area.width);
            let x = area.width.saturating_sub(w);
            let hint_style = Style::default().fg(contrast_fg(&theme, theme.secondary)).bg(theme.secondary).add_modifier(Modifier::BOLD);
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(vec![ratatui::text::Span::styled(hint.chars().take(w as usize).collect::<String>(), hint_style)])), Rect::new(x, area.y, w, 1));
        } else if let Some((msg, t)) = &self.status_msg {
            if t.elapsed() < TOAST_TIMEOUT {
                let msg_w = msg.chars().count() as u16;
                let w = msg_w.min(area.width);
                let x = area.width.saturating_sub(w);
                let msg_style = Style::default().fg(contrast_fg(&theme, theme.accent)).bg(theme.accent).add_modifier(Modifier::BOLD);
                f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(vec![ratatui::text::Span::styled(msg.chars().take(w as usize).collect::<String>(), msg_style)])), Rect::new(x, area.y, w, 1));
            } else if let Some(h) = &copy_hint {
                let w = (h.chars().count() as u16).min(area.width);
                let x = area.width.saturating_sub(w);
                let s = Style::default().fg(contrast_fg(&theme, theme.secondary)).bg(theme.secondary).add_modifier(Modifier::BOLD);
                f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(vec![ratatui::text::Span::styled(h.chars().take(w as usize).collect::<String>(), s)])), Rect::new(x, area.y, w, 1));
            }
        } else if let Some(h) = &copy_hint {
            let w = (h.chars().count() as u16).min(area.width);
            let x = area.width.saturating_sub(w);
            let s = Style::default().fg(contrast_fg(&theme, theme.secondary)).bg(theme.secondary).add_modifier(Modifier::BOLD);
            f.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(vec![ratatui::text::Span::styled(h.chars().take(w as usize).collect::<String>(), s)])), Rect::new(x, area.y, w, 1));
        }
    }

    pub(super) fn render_update_notice(&self, f: &mut Frame) {
        let Some(rect) = self.update_notice_rect() else { return };
        let Some((line1, line2)) = self.update_notice_lines() else { return };
        let theme = self.current_theme();
        draw_modal(f, rect, &theme, self.shadow_floor());
        let (x0, y0) = (rect.x, rect.y);
        put(f, x0 + 2, y0 + 1, "✕", Style::default().fg(theme.red).bg(theme.panel_sep).add_modifier(Modifier::BOLD));
        let inner_w = rect.width.saturating_sub(2);
        text(f, x0 + 5, y0 + 1, &line1, Style::default().fg(theme.fg).bg(theme.panel_sep), inner_w.saturating_sub(6));
        text(f, x0 + 5, y0 + 2, &line2, Style::default().fg(theme.fg).bg(theme.panel_sep), inner_w.saturating_sub(5));
    }

    /// Agent-lifecycle toasts (anchored per `[notifications] position`,
    /// top-right by default):
    /// transient `blocked` / `finished` announcements pushed by the daemon.
    /// Click one to focus the agent's pane; they expire on their own.
    pub(super) fn render_agent_toasts(&self, f: &mut Frame) {
        if self.agent_toasts.is_empty() {
            return;
        }
        let theme = self.current_theme();
        for (i, rect) in self.agent_toast_rects() {
            let t = &self.agent_toasts[i];
            let accent = match t.source {
                ToastSource::Agent { kind: ToastKind::Blocked, .. } | ToastSource::Config => theme.orange,
                ToastSource::Agent { kind: ToastKind::Finished, .. } => theme.green,
            };
            draw_modal(f, rect, &theme, self.shadow_floor());
            // Re-color the modal's border with the kind accent so a blocked
            // toast reads as attention and a finished one as success.
            paint_border(f, rect, accent, theme.panel_sep);
            let glyph_style = Style::default().fg(accent).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
            let glyph = match t.source {
                ToastSource::Agent { kind: ToastKind::Blocked, .. } => "●",
                ToastSource::Agent { kind: ToastKind::Finished, .. } => "✓",
                ToastSource::Config => "⚠",
            };
            put(f, rect.x + 2, rect.y + 1, glyph, glyph_style);
            let title_w = rect.width.saturating_sub(5);
            text(f, rect.x + 4, rect.y + 1, &t.title, glyph_style, title_w);
            let body_style = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
            text(f, rect.x + 2, rect.y + 2, &t.body, body_style, rect.width.saturating_sub(4));
        }
    }
}
