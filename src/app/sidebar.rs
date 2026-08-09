use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style};

use super::ui::{fill, put, text};
use super::{App, GREEN, ORANGE, PANEL_MUTED, PANEL_SEP};
use crate::pane::{AgentStatus, ACCENT, FG};

/// Stable rows of the left sidebar, shared by rendering and mouse hit-testing.
#[derive(Clone)]
enum SidebarRow {
    Header(String),
    Spacer,
    Section(String),
    Session(usize),
    Branch(String),
    AgentDir(usize, u64),
    AgentName(usize, u64),
    NewSession,
}

/// Scroll offsets for the sidebar's sessions and AGENTS sections.
pub(super) struct SidebarScroll {
    pub(super) sessions: u16,
    pub(super) agents: u16,
}

impl App {
    /// Row of the AGENTS section label: the sidebar midpoint, so the sessions
    /// list (above it) never pushes the agents section past halfway.
    fn sidebar_agents_y(&self) -> u16 {
        let footer_y = self.term_size.1.saturating_sub(2);
        (self.term_size.1 / 2).max(4).min(footer_y)
    }

    /// Sessions content: session rows (+ branch) followed by "+ new session".
    fn sessions_content(&self) -> Vec<SidebarRow> {
        let mut out = Vec::new();
        for (i, _s) in self.sessions.iter().enumerate() {
            out.push(SidebarRow::Session(i));
            if let Some(branch) = self.session_branch(i) {
                out.push(SidebarRow::Branch(branch));
            }
        }
        out.push(SidebarRow::NewSession);
        out
    }

    /// AGENTS content: a workspace + name row per AI pane, in session order.
    fn agents_content(&self) -> Vec<SidebarRow> {
        let mut out = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            for pid in s.tree.pane_ids() {
                if !self.panes.get(&pid).map(|p| p.is_ai_cli()).unwrap_or(false) {
                    continue;
                }
                out.push(SidebarRow::AgentDir(i, pid));
                out.push(SidebarRow::AgentName(i, pid));
            }
        }
        out
    }

    /// Max scroll offset for the sessions section.
    fn sessions_scroll_max(&self) -> u16 {
        let agents_y = self.sidebar_agents_y();
        let region_h = agents_y.saturating_sub(3) as usize;
        self.sessions_content().len().saturating_sub(region_h) as u16
    }

    /// Max scroll offset for the AGENTS section.
    fn agents_scroll_max(&self) -> u16 {
        let agents_y = self.sidebar_agents_y();
        let footer_y = self.term_size.1.saturating_sub(2);
        let region_h = footer_y.saturating_sub(agents_y) as usize;
        self.agents_content().len().saturating_sub(region_h) as u16
    }

    /// Static rows of the sidebar (shared by render + mouse hit-testing).
    ///
    /// Sessions live above the midpoint and scroll once they would push the
    /// AGENTS section past it; AGENTS scrolls once it reaches the bottom edge.
    fn sidebar_rows(&self) -> Vec<(u16, SidebarRow)> {
        let mut out = Vec::new();
        let mut y: u16 = 0;
        out.push((y, SidebarRow::Header("kumo".into())));
        y += 1;
        out.push((y, SidebarRow::Spacer));
        y += 1;
        out.push((y, SidebarRow::Section("sessions".into())));
        y += 1;

        let agents_y = self.sidebar_agents_y();
        let footer_y = self.term_size.1.saturating_sub(2);

        // Sessions region: rows 3 .. agents_y-1.
        let region_h = agents_y.saturating_sub(3) as usize;
        let items = self.sessions_content();
        let offset = (self.sidebar_scroll.sessions as usize).min(items.len().saturating_sub(region_h));
        for item in items.iter().skip(offset).take(region_h) {
            out.push((y, item.clone()));
            y += 1;
        }

        out.push((agents_y, SidebarRow::Section("agents".into())));

        // AGENTS region: rows agents_y+1 .. footer_y.
        let region_h = footer_y.saturating_sub(agents_y) as usize;
        let items = self.agents_content();
        let offset = (self.sidebar_scroll.agents as usize).min(items.len().saturating_sub(region_h));
        for (ay, item) in (agents_y + 1..).zip(items.iter().skip(offset).take(region_h)) {
            out.push((ay, item.clone()));
        }
        out
    }

    /// Mouse-wheel scroll for the sidebar: scrolls the sessions section above
    /// the midpoint and the AGENTS section below it. Returns whether the
    /// event was consumed.
    pub(super) fn sidebar_wheel(&mut self, x: u16, y: u16, up: bool) -> bool {
        if !self.sidebar_open || x >= self.sidebar_width {
            return false;
        }
        const STEP: u16 = 3;
        let agents_y = self.sidebar_agents_y();
        let footer_y = self.term_size.1.saturating_sub(2);
        if y >= 3 && y < agents_y {
            let max = self.sessions_scroll_max();
            self.sidebar_scroll.sessions = if up {
                self.sidebar_scroll.sessions.saturating_sub(STEP)
            } else {
                self.sidebar_scroll.sessions.saturating_add(STEP).min(max)
            };
            true
        } else if y > agents_y && y <= footer_y {
            let max = self.agents_scroll_max();
            self.sidebar_scroll.agents = if up {
                self.sidebar_scroll.agents.saturating_sub(STEP)
            } else {
                self.sidebar_scroll.agents.saturating_add(STEP).min(max)
            };
            true
        } else {
            false
        }
    }

    pub(super) fn sidebar_hit(&mut self, _x: u16, y: u16) -> bool {
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
            put(f, area.x + area.width, y, "│", Style::default().fg(PANEL_SEP));
        }
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
                        .fg(ACCENT)
                        .bg(RColor::Reset)
                        .add_modifier(Modifier::BOLD);
                    let pad = max.saturating_sub(t.chars().count() as u16) / 2;
                    text(f, x + pad, y, &t, style, max);
                }
                SidebarRow::Spacer => {
                    put(f, x, y, " ", Style::default().bg(RColor::Reset));
                }
                SidebarRow::Section(t) => {
                    let style = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
                    text(f, x, y, &format!("  {}", t.to_uppercase()), style, max);
                }
                SidebarRow::Session(i) => {
                    let active = i == self.active;
                    let name = &self.sessions[i].name;
                    let (marker, fg) = if active {
                        ("▸", ACCENT)
                    } else {
                        (" ", PANEL_MUTED)
                    };
                    let line = format!(" {marker} {}", name);
                    text(f, x, y, &line, Style::default().fg(fg).bg(RColor::Reset), max);
                }
                SidebarRow::Branch(b) => {
                    let style = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
                    text(f, x, y, &format!("    {}", b), style, max);
                }
                SidebarRow::AgentDir(i, pid) | SidebarRow::AgentName(i, pid) => {
                    let focused =
                        i == self.active && self.sessions[self.active].tree.focus == pid;
                    let bg = if focused { PANEL_SEP } else { RColor::Reset };
                    // Light up the whole sidebar row when this agent pane is focused.
                    if focused {
                        fill(f, Rect::new(x, y, max + 1, 1), bg);
                    }
                    let status = self.agent_status_cache.get(&pid).copied().unwrap_or(AgentStatus::Idle);
                    let status_color = match status {
                        AgentStatus::Working => GREEN,
                        AgentStatus::Blocked => ORANGE,
                        AgentStatus::Idle => PANEL_MUTED,
                    };
                    match row {
                        SidebarRow::AgentDir(_, _) => {
                            put(f, x + 2, y, "●", Style::default().fg(status_color).bg(bg));
                            let path = short_workspace(&self.sessions[i].workspace);
                            let path_color = if focused { FG } else { PANEL_MUTED };
                            text(f, x + 4, y, &path, Style::default().fg(path_color).bg(bg), max.saturating_sub(4));
                        }
                        SidebarRow::AgentName(_, _) => {
                            let name = self.agent_label(pid);
                            text(f, x + 4, y, &name, Style::default().fg(status_color).bg(bg), max.saturating_sub(4));
                        }
                        _ => {}
                    }
                }
                SidebarRow::NewSession => {
                    let style = Style::default()
                        .fg(FG)
                        .bg(RColor::Reset)
                        .add_modifier(Modifier::BOLD);
                    text(f, x, y, "  + NEW SESSION", style, max);
                }
            }
        }
        // Section scrollbars (rightmost sidebar column) when the content
        // overflows its region.
        let scroll_x = w.saturating_sub(1);
        let agents_y = self.sidebar_agents_y();
        let footer_y = self.term_size.1.saturating_sub(2);

        let sess_region = agents_y.saturating_sub(3);
        let sess_items = self.sessions_content();
        if sess_items.len() > sess_region as usize {
            let offset = (self.sidebar_scroll.sessions as usize)
                .min(sess_items.len() - sess_region as usize);
            draw_scrollbar(f, scroll_x, 3, sess_region, offset, sess_items.len());
        }

        let agent_region = footer_y.saturating_sub(agents_y);
        let agent_items = self.agents_content();
        if agent_items.len() > agent_region as usize {
            let offset = (self.sidebar_scroll.agents as usize)
                .min(agent_items.len() - agent_region as usize);
            draw_scrollbar(f, scroll_x, agents_y + 1, agent_region, offset, agent_items.len());
        }
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
fn draw_scrollbar(f: &mut Frame, x: u16, y_top: u16, region_h: u16, offset: usize, total: usize) {
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
            put(f, x, y, "▐", Style::default().fg(ACCENT));
        } else {
            put(f, x, y, "░", Style::default().fg(PANEL_SEP));
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

    fn build_app(n: usize) -> App {
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
            last_agent_debug: Instant::now(),
            last_status_refresh: Instant::now(),
            agent_status_cache: HashMap::new(),
            last_focused: None,
            pane_cache: HashMap::new(),
            quit: false,
            menu: super::super::Menu { open: false, selected: 0 },
            ctx_menu: super::super::CtxMenu { open: false, x: 0, y: 0, selected: 0, target: super::super::CtxTarget::Pane(0) },
            sidebar_scroll: SidebarScroll { sessions: 0, agents: u16::MAX },
            popup: NamePopup { open: false, target: None, name: String::new(), cursor: 0, error: None, hover: None },
            notice: None,
            update_notice: None,
            update_rx,
        }
    }

    #[test]
    fn clicking_session_row_switches_active() {
        let mut app = build_app(3);
        // h=24 -> agents_y=12 -> session rows 3,4,5.
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
        assert_eq!(app.sidebar_session_at(0, 2), None, "section label is not a session");
        assert_eq!(app.sidebar_session_at(99, y), None, "outside the sidebar");
    }

    #[test]
    fn session_rows_scroll_consistently() {
        let mut app = build_app(10);
        // 10 sessions + "new session" = 11 items, region 9 rows -> max offset 2.
        app.sidebar_scroll.sessions = 5;
        let rows = app.sidebar_rows();
        let sess_rows: Vec<(u16, usize)> = rows
            .iter()
            .filter_map(|(y, r)| match r {
                SidebarRow::Session(i) => Some((*y, *i)),
                _ => None,
            })
            .collect();
        // Offset caps at 2: first visible session is index 2 at y=3.
        assert_eq!(sess_rows.first(), Some(&(3, 2)), "scrolled rows must match render");
        assert_eq!(sess_rows.last(), Some(&(10, 9)));

        let (y, _) = sess_rows.first().unwrap();
        assert!(app.sidebar_hit(0, *y));
        assert_eq!(app.active, 2);

        let (y, _) = sess_rows.last().unwrap();
        assert!(app.sidebar_hit(0, *y));
        assert_eq!(app.active, 9);
    }
}
