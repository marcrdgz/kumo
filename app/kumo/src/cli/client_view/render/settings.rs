use super::*;

impl View {
    pub(super) fn render_settings(&self, f: &mut Frame) {
        if !self.settings.open {
            return;
        }
        let theme = self.current_theme();
        let Some(dd) = self.settings_rect() else { return };
        draw_modal(f, dd, &theme, self.shadow_floor());
        let sep_style = Style::default().fg(theme.accent).bg(theme.panel_sep);
        let title = Style::default().fg(theme.fg).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, dd.x + 2, dd.y + 1, "settings", title, dd.width.saturating_sub(4));
        let Some(tabs) = self.settings_tabs_rect() else { return };
        for (i, tab) in SETTINGS_TABS.iter().enumerate() {
            let sel = i == self.settings.tab;
            let y = tabs.y + i as u16;
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in tabs.x..(tabs.x + tabs.width) {
                put(f, cx, y, " ", Style::default().bg(bg));
            }
            let row_fg = if sel { contrast_fg(&theme, bg) } else { theme.fg };
            put(f, tabs.x, y, "▸", Style::default().fg(row_fg).bg(bg).add_modifier(Modifier::BOLD));
            text(f, tabs.x + 2, y, tab.label(), Style::default().fg(row_fg).bg(bg), tabs.width.saturating_sub(2));
        }
        let sep_x = tabs.x + tabs.width;
        for y in tabs.y..dd.bottom() {
            put(f, sep_x, y, "│", sep_style);
        }
        match SETTINGS_TABS.get(self.settings.tab).copied().unwrap_or(SettingsTab::Appearance) {
            SettingsTab::Appearance => self.render_settings_appearance(f, dd),
            SettingsTab::Agents => self.render_settings_agents(f, dd),
            SettingsTab::About => self.render_settings_about(f, dd),
        }
        let footer = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
        let action = if SETTINGS_TABS.get(self.settings.tab) == Some(&SettingsTab::Agents) {
            "install/update/remove"
        } else {
            "apply"
        };
        text(
            f,
            dd.x + 2,
            dd.bottom() - 2,
            &format!("j/k: move · h/l: tab · enter: {action} · esc: close"),
            footer,
            dd.width.saturating_sub(4),
        );
    }

    fn render_settings_appearance(&self, f: &mut Frame, dd: Rect) {
        let theme = self.current_theme();
        let Some(content) = self.settings_content_rect() else { return };
        let label = Style::default().fg(theme.secondary).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, content.x, content.y, "theme", label, content.width);
        for (i, t) in self.all_themes().iter().enumerate() {
            let sel = i == self.settings.selected;
            let active = i == self.theme_idx;
            let y = content.y + 1 + i as u16;
            if y >= dd.bottom().saturating_sub(1) {
                break;
            }
            let bg = if sel { theme.accent } else { theme.panel_sep };
            for cx in content.x..(content.x + content.width) {
                put(f, cx, y, " ", Style::default().bg(bg));
            }
            let row_fg = if sel { contrast_fg(&theme, bg) } else { theme.fg };
            let marker = if active { "●" } else { "○" };
            let marker_style = if active && !sel {
                Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(row_fg).bg(bg).add_modifier(Modifier::BOLD)
            };
            put(f, content.x, y, marker, marker_style);
            text(f, content.x + 2, y, &t.name, Style::default().fg(row_fg).bg(bg), content.width.saturating_sub(2));
            // Right-aligned swatch strip: six ANSI colors from the theme's own
            // palette, so rows can be told apart at a glance.
            const SWATCHES: [usize; 6] = [1, 2, 3, 4, 5, 6];
            if content.width as usize >= SWATCHES.len() + 18 {
                let sx = content.x + content.width - SWATCHES.len() as u16;
                for (j, ci) in SWATCHES.iter().enumerate() {
                    let c = t.palette[*ci];
                    put(f, sx + j as u16, y, " ", Style::default().bg(RColor::Rgb(c.r, c.g, c.b)));
                }
            }
            // "in use" tag sits left of the swatches when both fit.
            if active && content.width as usize >= SWATCHES.len() + 18 + 8 {
                let ix = content.x + content.width - SWATCHES.len() as u16 - 7;
                text(f, ix, y, "in use", Style::default().fg(theme.panel_muted).bg(bg), 6);
            }
        }
    }

    fn render_settings_agents(&self, f: &mut Frame, dd: Rect) {
        let theme = self.current_theme();
        let Some(content) = self.settings_content_rect() else { return };
        let label = Style::default().fg(theme.secondary).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, content.x, content.y, "agent skill", label, content.width);
        text(
            f,
            content.x,
            content.y + 1,
            "click to install/update · installed removes",
            Style::default().fg(theme.panel_muted).bg(theme.panel_sep),
            content.width,
        );

        for (i, target) in AGENT_SKILL_TARGETS.iter().copied().enumerate() {
            let y = content.y + 3 + (i as u16 * 2);
            if y + 1 >= dd.bottom().saturating_sub(2) {
                break;
            }
            let selected = i == self.settings.selected;
            let bg = if selected { lighten(theme.panel_sep, 28) } else { theme.panel_sep };
            fill(f, Rect::new(content.x, y, content.width, 2), bg);
            let active_action = self
                .settings
                .skill_installing
                .and_then(|(active, action, _)| (active == target).then_some(action));
            let (marker, status, status_color) = if let Some(action) = active_action {
                let status = match action {
                    AgentSkillAction::Install => "installing…",
                    AgentSkillAction::Update => "updating…",
                    AgentSkillAction::Uninstall => "uninstalling…",
                };
                ("◌", status, theme.orange)
            } else {
                match target.status() {
                    AgentSkillStatus::Current => ("●", "installed", theme.green),
                    AgentSkillStatus::Outdated => ("●", "update available", theme.orange),
                    AgentSkillStatus::Missing => ("○", "not installed", theme.panel_muted),
                }
            };
            let mut name_style = Style::default().fg(theme.fg).bg(bg);
            if selected {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }
            text(f, content.x, y, marker, Style::default().fg(status_color).bg(bg), 1);
            text(f, content.x + 2, y, target.label(), name_style, content.width.saturating_sub(2));
            let status_width = status.chars().count() as u16;
            if content.width > status_width + 4 {
                text(
                    f,
                    content.right().saturating_sub(status_width),
                    y,
                    status,
                    Style::default().fg(status_color).bg(bg).add_modifier(Modifier::BOLD),
                    status_width,
                );
            }
            let display_path = target
                .display_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|error| format!("<unavailable: {error}>"));
            text(
                f,
                content.x + 2,
                y + 1,
                &display_path,
                Style::default().fg(theme.panel_muted).bg(bg),
                content.width.saturating_sub(2),
            );
        }

        if let Some((_, success, message)) = &self.settings.skill_feedback {
            let y = content.y + 3 + (AGENT_SKILL_TARGETS.len() as u16 * 2) + 1;
            if y < dd.bottom().saturating_sub(2) {
                text(
                    f,
                    content.x,
                    y,
                    message,
                    Style::default()
                        .fg(if *success { theme.green } else { theme.red })
                        .bg(theme.panel_sep),
                    content.width,
                );
            }
        }
    }

    fn render_settings_about(&self, f: &mut Frame, dd: Rect) {
        let theme = self.current_theme();
        let Some(content) = self.settings_content_rect() else { return };
        let label = Style::default().fg(theme.secondary).bg(theme.panel_sep).add_modifier(Modifier::BOLD);
        text(f, content.x, content.y, "kumo", label, content.width);
        let session = self.active_session().map(|s| s.name.clone()).unwrap_or_default();
        let rows: [(&str, String); 5] = [
            ("version", format!("{} ({})", env!("CARGO_PKG_VERSION"), kumo_core::update::current_channel_label())),
            ("session", session),
            ("sessions", format!("{}", self.layout.as_ref().map(|l| l.sessions.len()).unwrap_or(0))),
            ("panes", format!("{}", self.rects.len())),
            ("shell", kumo_core::config::default_shell()),
        ];
        for (i, (k, v)) in rows.iter().enumerate() {
            let y = content.y + 1 + i as u16;
            if y >= dd.bottom().saturating_sub(1) {
                break;
            }
            let kst = Style::default().fg(theme.panel_muted).bg(theme.panel_sep);
            text(f, content.x, y, k, kst, 12);
            let vst = Style::default().fg(theme.fg).bg(theme.panel_sep);
            text(f, content.x + 12, y, v, vst, content.width.saturating_sub(14));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::client_view::tests::*;

    #[test]
    fn settings_agent_skill_tab_renders_targets_and_installing_state() {
        let mut view = test_view();
        view.settings.open = true;
        view.settings.tab = SETTINGS_TABS
            .iter()
            .position(|tab| *tab == SettingsTab::Agents)
            .unwrap();
        view.settings.skill_installing = Some((
            AgentSkillTarget::Codex,
            AgentSkillAction::Install,
            Instant::now(),
        ));
        let content = view.settings_content_rect().unwrap();
        let backend = ratatui::backend::TestBackend::new(view.cols, view.rows);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| view.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let line = |y| {
            (content.x..content.right())
                .map(|x| buffer.cell((x, y)).unwrap().symbol().to_string())
                .collect::<String>()
        };
        assert!(line(content.y + 3).contains("Codex"));
        assert!(line(content.y + 3).contains("installing"));
        assert!(line(content.y + 5).contains("Claude Code"));
        assert!(line(content.y + 7).contains("Gemini CLI"));
        assert!(line(content.y + 9).contains("OpenCode"));
    }
}
