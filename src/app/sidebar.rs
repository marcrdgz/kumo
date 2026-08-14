use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style};

use super::ui::{fill, put, text};
use super::App;
use super::tasks::BranchInfo;
use crate::agents::AgentStatus;
use crate::theme::Theme;

/// Sidebar tabs: SESSIONS or AGENTS. Each is a full tab in the panel (0.5.0),
/// replacing the two stacked, independently-scrolling sections.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SidebarTab {
    Sessions,
    Agents,
}

/// Stable rows of the left sidebar, shared by rendering and mouse hit-testing.
#[derive(Clone)]
enum SidebarRow {
    Header(String),
    Spacer,
    Session(usize),
    Branch(usize, BranchInfo),
    AgentDir(usize, u64),
    AgentName(usize, u64),
    NewSession,
}

/// Scroll offsets for the sidebar's sessions and AGENTS tabs.
pub(super) struct SidebarScroll {
    pub(super) sessions: u16,
    pub(super) agents: u16,
}

/// Y row of the tab bar (below the header + spacer).
const TAB_BAR_Y: u16 = 2;
/// First content row of the active tab (right below the tab bar).
const CONTENT_Y: u16 = 3;

impl App {
    /// Last row of the sidebar content: the row above the status bar.
    fn sidebar_footer_y(&self) -> u16 {
        self.term_size.1.saturating_sub(2)
    }

    /// Height (in rows) of the active tab's content region.
    fn content_region_h(&self) -> u16 {
        self.sidebar_footer_y().saturating_sub(CONTENT_Y - 1)
    }

    /// Rows of the active tab's content list.
    fn active_tab_items(&self) -> Vec<SidebarRow> {
        match self.sidebar_tab {
            SidebarTab::Sessions => self.sessions_content(),
            SidebarTab::Agents => self.agents_content(),
        }
    }

    /// Scroll offset of the active tab.
    fn active_scroll(&self) -> u16 {
        match self.sidebar_tab {
            SidebarTab::Sessions => self.sidebar_scroll.sessions,
            SidebarTab::Agents => self.sidebar_scroll.agents,
        }
    }

    /// Max scroll offset for the active tab.
    fn active_scroll_max(&self) -> u16 {
        let region_h = self.content_region_h() as usize;
        self.active_tab_items().len().saturating_sub(region_h) as u16
    }

    /// Which tab a click at `(x, y)` lands on, if the tab bar.
    fn tab_at(&self, x: u16, y: u16) -> Option<SidebarTab> {
        if y != TAB_BAR_Y || x >= self.sidebar_width {
            return None;
        }
        let half = (self.sidebar_width / 2).max(4);
        Some(if x < half { SidebarTab::Sessions } else { SidebarTab::Agents })
    }

    /// Sessions content: session rows (+ branch) followed by "+ new session".
    fn sessions_content(&self) -> Vec<SidebarRow> {
        let mut out = Vec::new();
        for (i, _s) in self.sessions.iter().enumerate() {
            out.push(SidebarRow::Session(i));
            if let Some(branch) = self.session_branch(i) {
                out.push(SidebarRow::Branch(i, branch));
            }
        }
        out.push(SidebarRow::NewSession);
        out
    }

    /// Sort rank for the AGENTS section: blocked agents float to the top so a
    /// permission wait is visible without scrolling, then working, then idle.
    fn agent_rank(status: AgentStatus) -> u8 {
        match status {
            AgentStatus::Blocked => 0,
            AgentStatus::Working => 1,
            AgentStatus::Idle => 2,
        }
    }

    /// AGENTS content: a workspace + name row per AI pane, blocked first, then
    /// working, then idle, stable within each group (session order).
    fn agents_content(&self) -> Vec<SidebarRow> {
        let mut out: Vec<(u8, usize, u64, SidebarRow)> = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            for pid in s.tree.pane_ids() {
                if !self.panes.get(&pid).map(|p| p.is_ai_cli()).unwrap_or(false) {
                    continue;
                }
                let rank = self
                    .agent_status_cache
                    .get(&pid)
                    .copied()
                    .map(Self::agent_rank)
                    .unwrap_or(2);
                out.push((rank, i, pid, SidebarRow::AgentDir(i, pid)));
                out.push((rank, i, pid, SidebarRow::AgentName(i, pid)));
            }
        }
        out.sort_by_key(|(rank, i, pid, _)| (*rank, *i, *pid));
        out.into_iter().map(|(_, _, _, row)| row).collect()
    }

    /// Static rows of the sidebar (shared by render + mouse hit-testing):
    /// the header, spacer, then the active tab's content rows. The tab bar row
    /// itself is hit-tested separately (`tab_at`).
    fn sidebar_rows(&self) -> Vec<(u16, SidebarRow)> {
        let mut out = vec![
            (0, SidebarRow::Header("kumo".into())),
            (1, SidebarRow::Spacer),
        ];
        let region_h = self.content_region_h() as usize;
        let items = self.active_tab_items();
        let offset = (self.active_scroll() as usize).min(items.len().saturating_sub(region_h));
        for (i, item) in items.iter().skip(offset).take(region_h).enumerate() {
            out.push((CONTENT_Y + i as u16, item.clone()));
        }
        out
    }

    /// Mouse-wheel scroll for the sidebar: scrolls the active tab's content.
    /// Returns whether the event was consumed.
    pub(super) fn sidebar_wheel(&mut self, x: u16, y: u16, up: bool) -> bool {
        if !self.sidebar_open || x >= self.sidebar_width {
            return false;
        }
        if y < CONTENT_Y || y > self.sidebar_footer_y() {
            return false;
        }
        const STEP: u16 = 3;
        let max = self.active_scroll_max();
        let scroll = match self.sidebar_tab {
            SidebarTab::Sessions => &mut self.sidebar_scroll.sessions,
            SidebarTab::Agents => &mut self.sidebar_scroll.agents,
        };
        *scroll = if up {
            scroll.saturating_sub(STEP)
        } else {
            scroll.saturating_add(STEP).min(max)
        };
        true
    }

    pub(super) fn sidebar_hit(&mut self, x: u16, y: u16) -> bool {
        // A click on the tab bar switches tabs (switching to the already-active
        // tab is a no-op but still consumes the click).
        if let Some(tab) = self.tab_at(x, y) {
            if tab != self.sidebar_tab {
                self.sidebar_tab = tab;
            }
            return true;
        }
        for (ry, row) in self.sidebar_rows() {
            if ry != y {
                continue;
            }
            match row {
                SidebarRow::Session(i) => {
                    self.active = i;
                    return true;
                }
                SidebarRow::AgentDir(i, pid) | SidebarRow::AgentName(i, pid) => {
                    self.active = i;
                    self.sessions[i].tree.focus = pid;
                    return true;
                }
                SidebarRow::NewSession => {
                    self.open_session_popup();
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    /// Session index under a sidebar row, if any (for right-click rename).
    pub(super) fn sidebar_session_at(&self, x: u16, y: u16) -> Option<usize> {
        if !self.sidebar_open || x >= self.sidebar_width {
            return None;
        }
        self.sidebar_rows()
            .into_iter()
            .find(|(ry, _)| *ry == y)
            .and_then(|(_, row)| match row {
                SidebarRow::Session(i) => Some(i),
                _ => None,
            })
    }

    pub(super) fn render_sidebar(&self, f: &mut Frame, size: Rect) {
        let w = self.sidebar_width.min(size.width);
        let area = Rect::new(0, 0, w, size.height.saturating_sub(1));
        fill(f, area, RColor::Reset);
        // Separator between sidebar and panes.
        for y in area.y..(area.y + area.height) {
            put(f, area.x + area.width, y, "│", Style::default().fg(self.theme.panel_sep));
        }
        self.render_tabs(f, area, w);
        for (y, row) in self.sidebar_rows() {
            if y > area.y + area.height {
                break;
            }
            let x = area.x;
            // Reserve the last column for section scrollbars.
            let max = w.saturating_sub(2);
            match row {
                SidebarRow::Header(t) => {
                    let style = Style::default()
                        .fg(self.theme.accent)
                        .bg(RColor::Reset)
                        .add_modifier(Modifier::BOLD);
                    let pad = max.saturating_sub(t.chars().count() as u16) / 2;
                    text(f, x + pad, y, &t, style, max);
                }
                SidebarRow::Spacer => {
                    put(f, x, y, " ", Style::default().bg(RColor::Reset));
                }
                SidebarRow::Session(i) => {
                    let active = i == self.active;
                    let bg = if active { self.theme.panel_sep } else { RColor::Reset };
                    let name = &self.sessions[i].name;
                    if active {
                        fill(f, Rect::new(x, y, max + 1, 1), bg);
                        put(f, x + 1, y, "▸", Style::default().fg(self.theme.accent).bg(bg));
                        text(f, x + 3, y, name, Style::default().fg(self.theme.fg).bg(bg), max.saturating_sub(3));
                    } else {
                        put(f, x + 1, y, " ", Style::default().bg(bg));
                        text(
                            f,
                            x + 3,
                            y,
                            name,
                            Style::default().fg(self.theme.panel_muted).bg(bg),
                            max.saturating_sub(3),
                        );
                    }
                }
                SidebarRow::Branch(i, b) => {
                    let active = i == self.active;
                    let bg = if active { self.theme.panel_sep } else { RColor::Reset };
                    let name_color = if active { self.theme.fg } else { self.theme.panel_muted };
                    if active {
                        fill(f, Rect::new(x, y, max + 1, 1), bg);
                    }
                    let avail = max.saturating_sub(4) as usize;
                    // Full ahead/behind suffix, e.g. ` ↑2 ~3`.
                    let suffix = match (b.ahead, b.behind) {
                        (0, 0) => String::new(),
                        (a, 0) => format!(" \u{2191}{}", a),
                        (0, be) => format!(" ~{}", be),
                        (a, be) => format!(" \u{2191}{}~{}", a, be),
                    };
                    // The suffix always keeps its space; the name gets an
                    // ellipsis if it would otherwise cover it.
                    let suffix_w = suffix.chars().count().min(avail);
                    let name_avail = avail.saturating_sub(suffix_w);
                    let shown = fit_branch_name(&b.name, name_avail);
                    text(
                        f,
                        x + 4,
                        y,
                        &shown,
                        Style::default().fg(name_color).bg(bg),
                        avail as u16,
                    );
                    let mut cx = x + 4 + shown.chars().count() as u16;
                    let mut remaining = (avail as u16).saturating_sub(shown.chars().count() as u16);
                    if b.ahead > 0 && remaining > 1 {
                        put(f, cx, y, " ", Style::default().bg(bg));
                        cx += 1;
                        remaining -= 1;
                        let s = format!("\u{2191}{}", b.ahead);
                        let w = (s.chars().count() as u16).min(remaining);
                        text(f, cx, y, &s, Style::default().fg(self.theme.green).bg(bg), remaining);
                        cx += w;
                        remaining = remaining.saturating_sub(w);
                    }
                    if b.behind > 0 && remaining > 1 {
                        put(f, cx, y, " ", Style::default().bg(bg));
                        cx += 1;
                        remaining -= 1;
                        let s = format!("~{}", b.behind);
                        text(f, cx, y, &s, Style::default().fg(self.theme.orange).bg(bg), remaining);
                    }
                }
                SidebarRow::AgentDir(i, pid) | SidebarRow::AgentName(i, pid) => {
                    let focused =
                        i == self.active && self.sessions[self.active].tree.focus == pid;
                    let bg = if focused { self.theme.panel_sep } else { RColor::Reset };
                    // Light up the whole sidebar row when this agent pane is focused.
                    if focused {
                        fill(f, Rect::new(x, y, max + 1, 1), bg);
                    }
                    let status = self.agent_status_cache.get(&pid).copied().unwrap_or(AgentStatus::Idle);
                    let status_color = match status {
                        AgentStatus::Working => self.theme.green,
                        AgentStatus::Blocked => self.theme.orange,
                        AgentStatus::Idle => self.theme.panel_muted,
                    };
                    // Blocked agents get a filled dot and a bold label so the
                    // waiting state stands out even at a glance.
                    let dot = if status == AgentStatus::Blocked { "◉" } else { "●" };
                    let name_style = if status == AgentStatus::Blocked {
                        Style::default().fg(status_color).bg(bg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(status_color).bg(bg)
                    };
                    match row {
                        SidebarRow::AgentDir(_, _) => {
                            put(f, x + 2, y, dot, Style::default().fg(status_color).bg(bg));
                            let path = short_workspace(&self.sessions[i].workspace);
                            let path_color = if focused { self.theme.fg } else { self.theme.panel_muted };
                            text(f, x + 4, y, &path, Style::default().fg(path_color).bg(bg), max.saturating_sub(4));
                        }
                        SidebarRow::AgentName(_, _) => {
                            let name = self.agent_label(pid);
                            let avail = max.saturating_sub(4) as usize;
                            const HINT: &str = " ·blocked";
                            let label = if status == AgentStatus::Blocked
                                && name.chars().count() + HINT.len() <= avail
                            {
                                format!("{name}{HINT}")
                            } else {
                                name
                            };
                            text(f, x + 4, y, &label, name_style, max.saturating_sub(4));
                        }
                        _ => {}
                    }
                }
                SidebarRow::NewSession => {
                    let style = Style::default()
                        .fg(self.theme.fg)
                        .bg(RColor::Reset)
                        .add_modifier(Modifier::BOLD);
                    text(f, x, y, "  + NEW SESSION", style, max);
                }
            }
        }
        // Scrollbar (rightmost sidebar column) when the active tab overflows
        // its content region.
        let scroll_x = w.saturating_sub(1);
        let region_h = self.content_region_h();
        let items = self.active_tab_items();
        if items.len() > region_h as usize {
            let offset = (self.active_scroll() as usize).min(items.len() - region_h as usize);
            draw_scrollbar(f, scroll_x, CONTENT_Y, region_h, offset, items.len(), &self.theme);
        }
    }

    /// Draw the tab bar: two half-width tabs, the active one highlighted. The
    /// label of the active tab is also underlined so the selection is legible
    /// without a highlighted background.
    fn render_tabs(&self, f: &mut Frame, area: Rect, w: u16) {
        let y = area.y + TAB_BAR_Y;
        let half = (w / 2).max(4);
        let tabs = [("sessions", SidebarTab::Sessions), ("agents", SidebarTab::Agents)];
        for (i, (label, tab)) in tabs.iter().enumerate() {
            let x0 = area.x + i as u16 * half;
            let x1 = if i == 0 { x0 + half } else { area.x + w };
            let active = *tab == self.sidebar_tab;
            let bg = if active { self.theme.panel_sep } else { RColor::Reset };
            fill(f, Rect::new(x0, y, x1 - x0, 1), bg);
            let style = if active {
                Style::default()
                    .fg(self.theme.accent)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(self.theme.panel_muted).bg(bg)
            };
            let label = label.to_uppercase();
            let pad = (x1 - x0).saturating_sub(label.chars().count() as u16) / 2;
            text(f, x0 + pad, y, &label, style, x1 - x0);
        }
        // Separator between the two tabs, drawn last so the fills don't cover it.
        put(
            f,
            area.x + half,
            y,
            "│",
            Style::default().fg(self.theme.panel_sep).bg(RColor::Reset),
        );
    }
}

/// Truncate a git branch name to `avail` columns, appending `…` when it is
/// cut, so the reserved ahead/behind suffix is never covered.
fn fit_branch_name(name: &str, avail: usize) -> String {
    if name.chars().count() <= avail {
        name.to_string()
    } else if avail == 0 {
        String::new()
    } else {
        let mut s: String = name.chars().take(avail - 1).collect();
        s.push('…');
        s
    }
}

/// Short display form of a workspace path, e.g. `.../kumo`.
fn short_workspace(ws: &std::path::Path) -> String {
    let text = ws.to_string_lossy();
    if let Some(base) = ws.file_name() {
        let base = base.to_string_lossy();
        if ws.parent().is_some() {
            format!(".../{base}")
        } else {
            base.into_owned()
        }
    } else {
        text.into_owned()
    }
}

/// Draw a vertical scrollbar in a `region_h`-tall strip starting at
/// `(x, y_top)`, with `offset` of `total` items scrolled into view.
fn draw_scrollbar(f: &mut Frame, x: u16, y_top: u16, region_h: u16, offset: usize, total: usize, theme: &Theme) {
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
        if i >= y_start && i < y_start + thumb {
            put(f, x, y, "▐", Style::default().fg(theme.secondary));
        } else {
            put(f, x, y, "░", Style::default().fg(theme.panel_sep));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Instant;

    use crate::app::{Mode, NamePopup, Session};
    use crate::layout::LayoutTree;
    use crate::pane::Pane;

    #[test]
    fn branch_name_fits_untouched_when_short() {
        assert_eq!(fit_branch_name("fix/domain", 20), "fix/domain");
    }

    #[test]
    fn long_branch_row_keeps_ahead_suffix() {
        let mut app = build_app(1);
        app.branch_cache.insert(
            PathBuf::from("/tmp"),
            (
                Some(BranchInfo {
                    name: "fixfixfixfixfixfix".into(),
                    ahead: 1,
                    behind: 0,
                }),
                Instant::now(),
            ),
        );
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| app.render_sidebar(f, f.area())).unwrap();
        let buf = term.backend().buffer();
        // Branch row for session 0 sits at y=4 (header, spacer, tabs, session).
        let line: String = (0..26).map(|x| buf.cell((x, 4)).unwrap().symbol()).collect();
        assert_eq!(line.trim_end(), "    fixfixfixfixfixf… ↑1");
        let up = buf.cell((22, 4)).unwrap();
        assert_eq!(up.style().fg, Some(app.theme.green));
    }

    #[test]
    fn branch_name_truncates_with_ellipsis() {
        assert_eq!(fit_branch_name("very/long/feature-branch-name", 8), "very/lo…");
    }

    #[test]
    fn branch_name_keeps_suffix_room_when_exact() {
        assert_eq!(fit_branch_name("fix/domain", 8), "fix/dom…");
    }

    #[test]
    fn branch_name_empty_when_no_room() {
        assert_eq!(fit_branch_name("anything", 0), "");
    }

    fn build_app(n: usize) -> App {
        // Tests that mutate `$HOME` (config, app, server) serialize behind
        // `TEST_ENV_LOCK`; hold it while spawning so `Pty::spawn` never reads a
        // concurrently-deleted home directory for the child cwd.
        let _lock = crate::config::TEST_ENV_LOCK.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        let (_update_tx, update_rx) = mpsc::channel::<Option<crate::update::UpdateNotice>>();
        let mut panes = HashMap::new();
        let mut sessions = Vec::new();
        for i in 0..n {
            let sid = (i + 1) as u64;
            let pid = (i + 1) as u64;
            let pane = Pane::spawn(
                sid,
                pid,
                "/bin/sh".into(),
                Some(("/usr/bin/true".into(), Vec::new())),
                None,
                80,
                24,
                false,
                tx.clone(),
                &crate::theme::THEMES[crate::theme::DEFAULT_THEME_IDX],
            )
            .unwrap();
            panes.insert(pid, pane);
            sessions.push(Session {
                id: sid,
                name: format!("sess-{}", i + 1),
                tree: LayoutTree::new(pid),
                zoom: false,
                workspace: PathBuf::from("/tmp"),
            });
        }
        App {
            sessions,
            active: 0,
            panes,
            mode: Mode::Normal,
            leader: super::super::bindings::LEADER,
            keymap: super::super::bindings::build_keymap(&HashMap::new()),
            pane_numbers: None,
            drag: None,
            sel: None,
            pending_click: None,
            events_tx: tx,
            events_rx: rx,
            shell: "/bin/sh".into(),
            ai: ("opencode".into(), Vec::new()),
            workspace: PathBuf::from("/tmp"),
            term_size: (80, 24),
            last_sizes: HashMap::new(),
            sidebar_open: true,
            sidebar_width: 26,
            branch_cache: HashMap::new(),
            last_ai_scan: Instant::now(),
            last_follow_scan: Instant::now(),
            last_agent_debug: Instant::now(),
            last_status_refresh: Instant::now(),
            agent_status_cache: HashMap::new(),
            last_agent_status: HashMap::new(),
            last_agent_sound: HashMap::new(),
            last_focused: None,
            pane_cache: HashMap::new(),
            quit: false,
            detach_requested: false,
            menu: super::super::Menu { open: false, selected: 0 },
            ctx_menu: super::super::CtxMenu { open: false, x: 0, y: 0, selected: 0, target: super::super::CtxTarget::Pane(0) },
            sidebar_scroll: SidebarScroll { sessions: 0, agents: u16::MAX },
            sidebar_tab: SidebarTab::Sessions,
            popup: NamePopup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            keybind_overlay: super::super::KeybindOverlay { open: false, scroll: 0 },
            settings: super::super::SettingsPanel {
                open: false,
                tab: 0,
                selected: crate::theme::DEFAULT_THEME_IDX,
            },
            worktree_picker: super::super::WorktreePicker {
                open: false,
                session: 0,
                items: Vec::new(),
                selected: 0,
                scroll: 0,
                error: None,
            },
            theme: crate::theme::THEMES[crate::theme::DEFAULT_THEME_IDX],
            theme_idx: crate::theme::DEFAULT_THEME_IDX,
            notice: None,
            update_notice: None,
            update_rx,
        }
    }

    #[test]
    fn clicking_session_row_switches_active() {
        let mut app = build_app(3);
        // h=24 -> sessions tab content rows 3,4,5.
        let rows = app.sidebar_rows();
        let sess_rows: Vec<(u16, usize)> = rows
            .iter()
            .filter_map(|(y, r)| match r {
                SidebarRow::Session(i) => Some((*y, *i)),
                _ => None,
            })
            .collect();
        assert_eq!(sess_rows, vec![(3, 0), (4, 1), (5, 2)]);

        let (y, _) = sess_rows[1];
        assert!(app.sidebar_hit(0, y), "click on sess-2 row should be handled");
        assert_eq!(app.active, 1, "clicking sess-2 must make it active");

        assert_eq!(app.sidebar_session_at(0, 3), Some(0), "right-click sess-1 row");
        assert_eq!(app.sidebar_session_at(0, y), Some(1), "right-click sess-2 row");
        assert_eq!(app.sidebar_session_at(0, 2), None, "the tab bar is not a session");
        assert_eq!(app.sidebar_session_at(99, y), None, "outside the sidebar");
    }

    #[test]
    fn session_rows_scroll_consistently() {
        let mut app = build_app(10);
        // Shrink the terminal so 11 items overflow the sessions tab's region:
        // h=8 -> content rows 3..6 (4 rows) -> max offset 7.
        app.term_size = (80, 8);
        app.sidebar_scroll.sessions = 5;
        let rows = app.sidebar_rows();
        let sess_rows: Vec<(u16, usize)> = rows
            .iter()
            .filter_map(|(y, r)| match r {
                SidebarRow::Session(i) => Some((*y, *i)),
                _ => None,
            })
            .collect();
        // Offset 5: first visible session is index 5 at y=3, last index 8.
        assert_eq!(sess_rows.first(), Some(&(3, 5)), "scrolled rows must match render");
        assert_eq!(sess_rows.last(), Some(&(6, 8)));

        let (y, _) = sess_rows.first().unwrap();
        assert!(app.sidebar_hit(0, *y));
        assert_eq!(app.active, 5);

        let (y, _) = sess_rows.last().unwrap();
        assert!(app.sidebar_hit(0, *y));
        assert_eq!(app.active, 8);
    }

    #[test]
    fn clicking_tab_bar_switches_section() {
        let mut app = build_app(1);
        // y=2 is the tab bar; with sidebar_width 26 the split is at x=13.
        assert!(app.sidebar_hit(15, 2), "click on the AGENTS tab must be handled");
        assert_eq!(app.sidebar_tab, SidebarTab::Agents, "clicking the tab must switch");
        assert!(
            app.sidebar_rows().iter().all(|(y, r)| *y < 3 || matches!(r, SidebarRow::AgentDir(..) | SidebarRow::AgentName(..))),
            "the AGENTS tab lists agent rows only"
        );

        assert!(app.sidebar_hit(3, 2), "click on the SESSIONS tab must be handled");
        assert_eq!(app.sidebar_tab, SidebarTab::Sessions);

        // A click on an already-active tab is consumed but keeps the tab.
        assert!(app.sidebar_hit(3, 2));
        assert_eq!(app.sidebar_tab, SidebarTab::Sessions);
        // The header row is not a tab and is not handled.
        assert!(!app.sidebar_hit(3, 0));
    }

    #[test]
    fn wheel_scrolls_the_active_tab() {
        let mut app = build_app(10);
        app.term_size = (80, 8);
        // SESSIONS tab is active by default: wheel scrolls sessions only.
        assert!(app.sidebar_wheel(5, 5, false), "wheel over the content region is consumed");
        assert_eq!(app.sidebar_scroll.sessions, 3, "sessions scrolls down by STEP");
        assert_eq!(app.sidebar_scroll.agents, u16::MAX, "agents scroll stays untouched");
        assert!(app.sidebar_wheel(5, 5, true));
        assert_eq!(app.sidebar_scroll.sessions, 0, "sessions scrolls back up");

        // The wheel is ignored over the tab bar and below the footer rows.
        assert!(!app.sidebar_wheel(5, 2, false));
        assert!(!app.sidebar_wheel(5, 23, false));

        // Switching to AGENTS routes the wheel to the agents offset.
        app.sidebar_tab = SidebarTab::Agents;
        assert!(app.sidebar_wheel(5, 5, false));
        assert_eq!(app.sidebar_scroll.agents, 0, "an empty agents list cannot scroll");
    }

    #[test]
    fn agents_content_sorts_blocked_first() {
        let mut app = build_app(3);
        // Mark pane 2 (session 1) blocked and pane 1 (session 0) working.
        app.panes.get_mut(&1).unwrap().is_ai = true;
        app.panes.get_mut(&2).unwrap().is_ai = true;
        app.panes.get_mut(&3).unwrap().is_ai = true;
        app.agent_status_cache.insert(1, AgentStatus::Working);
        app.agent_status_cache.insert(2, AgentStatus::Blocked);
        app.agent_status_cache.insert(3, AgentStatus::Idle);
        let dirs: Vec<(usize, u64)> = app
            .agents_content()
            .iter()
            .filter_map(|r| match r {
                SidebarRow::AgentDir(i, pid) => Some((*i, *pid)),
                _ => None,
            })
            .collect();
        // Blocked (pane 2) first, then working (pane 1), then idle (pane 3).
        assert_eq!(dirs, vec![(1, 2), (0, 1), (2, 3)]);
    }

    #[test]
    fn worktree_picker_scroll_keeps_selection_visible() {
        use super::super::overlays::PickerWorktree;
        use super::super::worktrees::WorktreeInfo;

        let mut app = build_app(1);
        app.term_size = (80, 12);
        app.worktree_picker.open = true;
        // h=12 -> picker height 8 -> 3 visible rows (title, header, 3 rows,
        // footer).
        let items: Vec<PickerWorktree> = (0..10)
            .map(|i| PickerWorktree {
                info: WorktreeInfo {
                    path: PathBuf::from(format!("/work/wt{i}")),
                    branch: Some(format!("b{i}")),
                },
                is_main: i == 0,
                open: false,
            })
            .collect();
        app.worktree_picker.items = items;
        app.worktree_picker.selected = 0;

        // Moving within the visible region does not scroll.
        app.worktree_picker_move(1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (1, 0));
        app.worktree_picker_move(1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (2, 0));

        // Row 3 is past the visible bottom: the scroll follows it.
        app.worktree_picker_move(1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (3, 1));

        // Moving back keeps the selected row visible; only the top is clamped
        // once the selection reaches it.
        app.worktree_picker_move(-1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (2, 1));
        app.worktree_picker_move(-1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (1, 1));
        app.worktree_picker_move(-1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (0, 0));

        // Wrapping past the end lands on the last row (scrolled into view),
        // and past the top wraps back to row 0 with the scroll reset.
        app.worktree_picker_move(-1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (9, 7));
        app.worktree_picker_move(1);
        assert_eq!((app.worktree_picker.selected, app.worktree_picker.scroll), (0, 0));
    }
}
