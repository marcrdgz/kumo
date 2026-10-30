use super::*;

impl View {
    pub(in crate::cli::client_view) fn render_tab_bar(&self, f: &mut Frame) {
        let Some(sess) = self.active_session() else { return };
        if sess.tabs.is_empty() { return; }
        let theme = self.current_theme();
        let area = self.tabs_area();
        // Bar background distinct from terminal
        let bar_bg = theme.panel_sep;
        fill(f, area, bar_bg);
        // Arrows for overflow
        if let Some(r) = self.tab_left_arrow_rect() {
            put(f, r.x, r.y, "‹", Style::default().fg(theme.panel_muted).bg(bar_bg));
        }
        if let Some(r) = self.tab_right_arrow_rect() {
            put(f, r.x, r.y, "›", Style::default().fg(theme.panel_muted).bg(bar_bg));
        }
        for (idx, pill, close) in &self.tab_rects {
            let Some(tab) = sess.tabs.get(*idx) else { continue };
            let active = sess.active_tab == *idx;
            let is_hover = self.tab_hover == Some(*idx);
            let pill_bg = if active { theme.accent } else { lighten(bar_bg, 14) };
            let fg = if active { contrast_fg(&theme, pill_bg) } else { theme.fg };
            fill(f, *pill, pill_bg);
            let base_style = if active {
                Style::default().fg(fg).bg(pill_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(fg).bg(pill_bg)
            };
            // Draw the name, optional aggregate agent state, then the
            // persistent close action. The geometry reserves these cells too.
            let name = &tab_title(*idx + 1, &tab.name);
            let max_w = pill.width.saturating_sub(1);
            let name_w = (name.chars().count() as u16).min(max_w);
            text(f, pill.x, pill.y, name, base_style, name_w);
            if let Some(status) = self.tab_agent_indicator(tab) {
                let marker = self.tab_agent_marker(status);
                let marker_fg = if active {
                    fg
                } else {
                    let (r, g, b) = kumo_core::theme::agent_status_color(status);
                    RColor::Rgb(r, g, b)
                };
                put(
                    f,
                    pill.x + name_w + 1,
                    pill.y,
                    marker,
                    Style::default().fg(marker_fg).bg(pill_bg).add_modifier(Modifier::BOLD),
                );
            }
            if is_hover {
                let x_fg = if active { fg } else { theme.red };
                put(f, close.x, close.y, "x", Style::default().fg(x_fg).bg(pill_bg).add_modifier(Modifier::BOLD));
            } else {
                // Persistent ghost close hint: always visible (clickable without
                // a prior hover), brightening to red on hover.
                let ghost = lighten(pill_bg, if active { 26 } else { 16 });
                put(f, close.x, close.y, "x", Style::default().fg(ghost).bg(pill_bg));
            }
        }
        // Plus button
        if let Some(pr) = self.plus_rect {
            let plus_bg = lighten(bar_bg, 10);
            fill(f, pr, plus_bg);
            let style = Style::default().fg(theme.panel_muted).bg(plus_bg).add_modifier(Modifier::BOLD);
            // Center "+" in the 3-wide pill
            put(f, pr.x + 1, pr.y, "+", style);
        }
        if let Some(x) = self.tab_reorder_indicator_x() {
            put(f, x, area.y, "│", Style::default().fg(theme.accent).bg(bar_bg).add_modifier(Modifier::BOLD));
        }
    }

    pub(in crate::cli::client_view) fn session_name(&self, idx: usize) -> String {
        self.layout.as_ref().and_then(|l| l.sessions.get(idx)).map(|s| s.name.clone()).unwrap_or_default()
    }

    pub(super) fn render_tabs(&self, f: &mut Frame, area: Rect, w: u16) {
        let theme = self.current_theme();
        let y = area.y + 2;
        let visible = self.visible_sidebar_tabs();
        if visible.is_empty() {
            return;
        }
        if visible.len() == 1 {
            let tab = visible[0];
            let bg = theme.panel_sep;
            fill(f, Rect::new(area.x, y, w, 1), bg);
            let style = Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            let label = tab.label().to_uppercase();
            let pad = w.saturating_sub(label.chars().count() as u16) / 2;
            text(f, area.x + pad, y, &label, style, w);
            return;
        }
        let half = (w / 2).max(1);
        let bstyle = kumo_core::config::sidebar_borders().style;
        let sep = if bstyle == kumo_core::config::BorderStyle::Hidden { " " }
                  else if bstyle == kumo_core::config::BorderStyle::Double { "║" }
                  else if bstyle == kumo_core::config::BorderStyle::Heavy { "┃" }
                  else { "│" };
        for (i, tab) in visible.iter().enumerate().take(2) {
            let x0 = area.x + i as u16 * half;
            let x1 = if i == 0 { x0 + half } else { area.x + w };
            let width = x1.saturating_sub(x0);
            if width == 0 {
                continue;
            }
            let active = *tab == self.sidebar_tab;
            let bg = if active { theme.panel_sep } else { RColor::Reset };
            fill(f, Rect::new(x0, y, width, 1), bg);
            let style = if active {
                Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.panel_muted).bg(bg)
            };
            let label = tab.label().to_uppercase();
            let pad = width.saturating_sub(label.chars().count() as u16) / 2;
            text(f, x0 + pad, y, &label, style, width);
        }
        put(f, area.x + half, y, sep, Style::default().fg(theme.panel_sep).bg(RColor::Reset));
    }
}
