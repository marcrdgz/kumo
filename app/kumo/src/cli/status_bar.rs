//! Status bar widgets: data sources + formatting for the customizable bar.
//!
//! The daemon never renders chrome — all widget content is derived client-side
//! from the `Layout` snapshot (`SessionLayout.branch`, `LayoutPane.agent`), the
//! local hostname, and the wall clock. This module owns the formatting helpers
//! and the shared `hostname`/`clock` utilities so `client_view::View` stays
//! focused on geometry and input.

use std::sync::OnceLock;

use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::text::Span;

use kumo_core::config::{
    AgentWidgetConfig, AgentWidgetStyle, BranchWidgetConfig, HostnameStyle, HostnameWidgetConfig,
    SessionWidgetConfig, StatusWidget,
};
use kumo_core::theme::OwnedTheme;
use kumo_protocol::{SessionLayout, WireBranch};

// ---------------------------------------------------------------------------
// hostname + clock (client-local)
// ---------------------------------------------------------------------------

static HOSTNAME_CACHE: OnceLock<String> = OnceLock::new();

fn raw_hostname() -> String {
    // Prefer the `hostname` crate (cross-platform `gethostname(2)`).
    // Fall back to $HOSTNAME / $HOST / short fallback so the widget never
    // panics on an unusual platform.
    if let Ok(name) = hostname::get() {
        if let Some(s) = name.to_str() {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Cached short hostname (stable for the process lifetime).
pub fn cached_hostname() -> String {
    HOSTNAME_CACHE.get_or_init(raw_hostname).clone()
}

pub fn is_ssh_session() -> bool {
    std::env::var("SSH_CONNECTION").is_ok()
        || std::env::var("SSH_CLIENT").is_ok()
        || std::env::var("SSH_TTY").is_ok()
}

pub fn short_host(name: &str) -> String {
    name.split('.').next().unwrap_or(name).to_string()
}

pub fn hostname_display(raw: &str, cfg: &HostnameWidgetConfig) -> String {
    let base = match cfg.style {
        HostnameStyle::Short => short_host(raw),
        HostnameStyle::Fqdn => raw.to_string(),
    };
    if cfg.show_user {
        if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
            let u = user.trim();
            if !u.is_empty() {
                return format!("{u}@{base}");
            }
        }
    }
    base
}

/// Format the clock using `chrono`'s `strftime` language. Falls back to
/// `"%H:%M"` on an empty format so a broken config never blanks the widget.
pub fn format_clock(format: &str) -> String {
    let fmt = if format.trim().is_empty() { "%H:%M" } else { format };
    let now = chrono::Local::now();
    // `format` is trusted from the user's config; if chrono produced an error
    // (should not happen for valid strftime), fall back to HH:MM.
    let s = now.format(fmt).to_string();
    if s.is_empty() { now.format("%H:%M").to_string() } else { s }
}

// ---------------------------------------------------------------------------
// per-widget formatters -> Vec<Span>
// ---------------------------------------------------------------------------

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

pub fn branch_spans(
    branch: Option<&WireBranch>,
    cfg: &BranchWidgetConfig,
    theme: &OwnedTheme,
) -> Option<Vec<Span<'static>>> {
    let b = branch?;
    let mut spans = Vec::new();
    let avail = cfg.max_len.max(1);
    let name = fit_branch_name(&b.name, avail);
    spans.push(Span::styled(name, Style::default().fg(theme.fg)));
    if cfg.show_ahead_behind {
        if b.ahead > 0 {
            spans.push(Span::styled(" ", Style::default().fg(theme.fg)));
            spans.push(Span::styled(
                format!("↑{}", b.ahead),
                Style::default().fg(theme.green),
            ));
        }
        if b.behind > 0 {
            spans.push(Span::styled(" ", Style::default().fg(theme.fg)));
            spans.push(Span::styled(
                format!("~{}", b.behind),
                Style::default().fg(theme.orange),
            ));
        }
    }
    Some(spans)
}

fn pane_count(session: &SessionLayout) -> usize {
    session
        .tabs
        .iter()
        .map(|t| {
            let mut n = 0;
            let mut stack: Vec<&kumo_protocol::LayoutNode> = Vec::new();
            if let Some(root) = &t.root {
                stack.push(root);
            }
            while let Some(node) = stack.pop() {
                match node {
                    kumo_protocol::LayoutNode::Pane(_) => n += 1,
                    kumo_protocol::LayoutNode::Split { a, b, .. } => {
                        stack.push(a);
                        stack.push(b);
                    }
                }
            }
            n
        })
        .sum()
}

/// Agent-pane counts by lifecycle state, for the status bar widget. `done`
/// (finished-but-unseen) and `unknown` (classification failed) roll up here
/// alongside the traditional states.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct AgentCounts {
    blocked: usize,
    done: usize,
    working: usize,
    idle: usize,
    unknown: usize,
}

impl AgentCounts {
    fn total(self) -> usize {
        self.blocked + self.done + self.working + self.idle + self.unknown
    }
}

fn agent_counts(session: Option<&SessionLayout>) -> AgentCounts {
    let mut counts = AgentCounts::default();
    let Some(s) = session else {
        return counts;
    };
    for tab in &s.tabs {
        let mut stack: Vec<&kumo_protocol::LayoutNode> = Vec::new();
        if let Some(root) = &tab.root {
            stack.push(root);
        }
        while let Some(node) = stack.pop() {
            match node {
                kumo_protocol::LayoutNode::Pane(p) => {
                    if let Some(agent) = &p.agent {
                        match agent.status {
                            kumo_protocol::AgentStatus::Blocked => counts.blocked += 1,
                            kumo_protocol::AgentStatus::Done => counts.done += 1,
                            kumo_protocol::AgentStatus::Working => counts.working += 1,
                            kumo_protocol::AgentStatus::Idle => counts.idle += 1,
                            kumo_protocol::AgentStatus::Unknown => counts.unknown += 1,
                        }
                    }
                }
                kumo_protocol::LayoutNode::Split { a, b, .. } => {
                    stack.push(a);
                    stack.push(b);
                }
            }
        }
    }
    counts
}

pub fn agent_spans(
    session: Option<&SessionLayout>,
    cfg: &AgentWidgetConfig,
    theme: &OwnedTheme,
    spinner: &str,
) -> Option<Vec<Span<'static>>> {
    let counts = agent_counts(session);
    if counts.total() == 0 {
        return None;
    }
    if cfg.only_blocked && counts.blocked == 0 && counts.done == 0 {
        return None;
    }
    let fg_of = |status| {
        let (r, g, b) = kumo_core::theme::agent_status_color(status);
        RColor::Rgb(r, g, b)
    };
    match cfg.style {
        AgentWidgetStyle::Counts => {
            let mut segs: Vec<Vec<Span<'static>>> = Vec::new();
            if counts.blocked > 0 {
                segs.push(vec![
                    Span::styled("◉", Style::default().fg(fg_of(kumo_protocol::AgentStatus::Blocked)).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{}", counts.blocked), Style::default().fg(fg_of(kumo_protocol::AgentStatus::Blocked))),
                ]);
            }
            if counts.done > 0 {
                segs.push(vec![
                    Span::styled("✓", Style::default().fg(fg_of(kumo_protocol::AgentStatus::Done)).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{}", counts.done), Style::default().fg(fg_of(kumo_protocol::AgentStatus::Done))),
                ]);
            }
            if !cfg.only_blocked && counts.working > 0 {
                segs.push(vec![
                    Span::styled(spinner.to_string(), Style::default().fg(fg_of(kumo_protocol::AgentStatus::Working)).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{}", counts.working), Style::default().fg(fg_of(kumo_protocol::AgentStatus::Working))),
                ]);
            }
            if !cfg.only_blocked && counts.unknown > 0 {
                segs.push(vec![
                    Span::styled("?", Style::default().fg(fg_of(kumo_protocol::AgentStatus::Unknown)).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{}", counts.unknown), Style::default().fg(fg_of(kumo_protocol::AgentStatus::Unknown))),
                ]);
            }
            if !cfg.only_blocked && counts.idle > 0 {
                let idle_fg = fg_of(kumo_protocol::AgentStatus::Idle);
                // Show idle only when it adds signal: hide idle when
                // blocked/done/working dominate unless the bar is wide. For
                // now show it when it is the only state.
                if counts.blocked == 0 && counts.working == 0 && counts.done == 0 {
                    segs.push(vec![
                        Span::styled("○", Style::default().fg(idle_fg)),
                        Span::styled(format!("{}", counts.idle), Style::default().fg(idle_fg)),
                    ]);
                } else if counts.idle > 0 && (counts.blocked > 0 || counts.working > 0 || counts.done > 0) {
                    // Compact idle count (muted) alongside active counts.
                    segs.push(vec![
                        Span::styled("○", Style::default().fg(idle_fg)),
                        Span::styled(format!("{}", counts.idle), Style::default().fg(idle_fg)),
                    ]);
                }
            }
            if segs.is_empty() {
                return None;
            }
            // Join with " · " muted
            let mut out = Vec::new();
            for (i, seg) in segs.into_iter().enumerate() {
                if i > 0 {
                    out.push(Span::styled(" · ", Style::default().fg(theme.panel_muted)));
                }
                out.extend(seg);
            }
            Some(out)
        }
        AgentWidgetStyle::Dots => {
            let mut s = String::new();
            s.extend(std::iter::repeat_n('◉', counts.blocked));
            s.extend(std::iter::repeat_n('✓', counts.done));
            s.extend(std::iter::repeat_n(spinner.chars().next().unwrap_or('●'), if cfg.only_blocked { 0 } else { counts.working }));
            s.extend(std::iter::repeat_n('○', if cfg.only_blocked { 0 } else { counts.idle }));
            s.extend(std::iter::repeat_n('?', if cfg.only_blocked { 0 } else { counts.unknown }));
            if s.is_empty() {
                return None;
            }
            // Color the whole run muted; per-dot coloring would need multiple spans.
            // For dots we keep it simple: use the fixed palette of the dominant state.
            let fg = if counts.blocked > 0 {
                fg_of(kumo_protocol::AgentStatus::Blocked)
            } else if counts.done > 0 {
                fg_of(kumo_protocol::AgentStatus::Done)
            } else if counts.working > 0 {
                fg_of(kumo_protocol::AgentStatus::Working)
            } else if counts.unknown > 0 {
                fg_of(kumo_protocol::AgentStatus::Unknown)
            } else {
                fg_of(kumo_protocol::AgentStatus::Idle)
            };
            Some(vec![Span::styled(s, Style::default().fg(fg))])
        }
        AgentWidgetStyle::List => {
            // List agent names with status, truncated later by the bar's width.
            let s = session?;
            let mut parts: Vec<String> = Vec::new();
            for tab in &s.tabs {
                let mut stack: Vec<&kumo_protocol::LayoutNode> = Vec::new();
                if let Some(root) = &tab.root { stack.push(root); }
                while let Some(node) = stack.pop() {
                    match node {
                        kumo_protocol::LayoutNode::Pane(p) => {
                            if let Some(agent) = &p.agent {
                                let label = match agent.status {
                                    kumo_protocol::AgentStatus::Blocked => "blocked",
                                    kumo_protocol::AgentStatus::Done => "done",
                                    kumo_protocol::AgentStatus::Working => "working",
                                    kumo_protocol::AgentStatus::Idle => "idle",
                                    kumo_protocol::AgentStatus::Unknown => "unknown",
                                };
                                parts.push(format!("{}:{label}", agent.name));
                            }
                        }
                        kumo_protocol::LayoutNode::Split { a, b, .. } => { stack.push(a); stack.push(b); }
                    }
                }
            }
            if parts.is_empty() { return None; }
            let text = parts.join(", ");
            let fg = if counts.blocked > 0 {
                fg_of(kumo_protocol::AgentStatus::Blocked)
            } else if counts.done > 0 {
                fg_of(kumo_protocol::AgentStatus::Done)
            } else if counts.working > 0 {
                fg_of(kumo_protocol::AgentStatus::Working)
            } else if counts.unknown > 0 {
                fg_of(kumo_protocol::AgentStatus::Unknown)
            } else {
                fg_of(kumo_protocol::AgentStatus::Idle)
            };
            Some(vec![Span::styled(text, Style::default().fg(fg))])
        }
    }
}

pub fn session_spans(
    session: Option<&SessionLayout>,
    cfg: &SessionWidgetConfig,
    sidebar_open: bool,
    theme: &OwnedTheme,
) -> Option<Vec<Span<'static>>> {
    let s = session?;
    let mut spans = Vec::new();
    spans.push(Span::styled(s.name.clone(), Style::default().fg(theme.fg)));
    if cfg.show_tabs || cfg.show_panes {
        let t = s.tabs.len();
        let n = pane_count(s);
        let mut suffix = String::new();
        if cfg.show_tabs {
            suffix.push_str(&format!(" · {t} tabs"));
        }
        if cfg.show_panes {
            suffix.push_str(&format!(" · {n} panes"));
        }
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, Style::default().fg(theme.panel_muted)));
        }
    }
    let zoomed = s.tabs.get(s.active_tab).map(|tab| tab.zoom).unwrap_or(false);
    if cfg.show_zoom && zoomed {
        spans.push(Span::styled(" · zoomed", Style::default().fg(theme.secondary)));
    }
    if !sidebar_open {
        spans.push(Span::styled(" · sidebar hidden", Style::default().fg(theme.panel_muted)));
    }
    Some(spans)
}

pub fn hostname_spans(
    hostname: &str,
    cfg: &HostnameWidgetConfig,
    is_ssh: bool,
    theme: &OwnedTheme,
) -> Option<Vec<Span<'static>>> {
    if cfg.only_ssh && !is_ssh {
        return None;
    }
    if hostname.trim().is_empty() {
        return None;
    }
    let display = hostname_display(hostname, cfg);
    if display.trim().is_empty() {
        return None;
    }
    Some(vec![Span::styled(display, Style::default().fg(theme.panel_muted))])
}

pub fn clock_spans(clock_str: &str, theme: &OwnedTheme) -> Vec<Span<'static>> {
    vec![Span::styled(clock_str.to_string(), Style::default().fg(theme.fg))]
}

pub fn mode_spans(is_leader: bool, theme: &OwnedTheme) -> Vec<Span<'static>> {
    let (label, style) = if is_leader {
        (" LEADER ", Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD))
    } else {
        (" NORMAL ", Style::default().fg(RColor::Black).bg(theme.accent))
    };
    vec![Span::styled(label.to_string(), style)]
}

pub fn menu_spans(is_open: bool, theme: &OwnedTheme) -> Vec<Span<'static>> {
    let style = if is_open {
        Style::default().fg(RColor::Black).bg(theme.secondary).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg).bg(RColor::Reset).add_modifier(Modifier::BOLD)
    };
    vec![Span::styled(" MENU ".to_string(), style)]
}

// ---------------------------------------------------------------------------
// layout helpers (measure / join / truncate)
// ---------------------------------------------------------------------------

pub fn spans_width(spans: &[Span<'_>]) -> u16 {
    spans.iter().map(|s| s.content.chars().count() as u16).sum()
}

pub fn join_with_sep(groups: Vec<Vec<Span<'static>>>, sep: &str, theme: &OwnedTheme) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let sep_style = Style::default().fg(theme.panel_muted);
    for (i, g) in groups.into_iter().enumerate() {
        if i > 0 {
            out.push(Span::styled(sep.to_string(), sep_style));
        }
        out.extend(g);
    }
    out
}

/// Truncate a span run to `max` columns (chars, not bytes) by trimming the
/// last span's content and appending `…` when trimmed.
pub fn truncate_spans(mut spans: Vec<Span<'static>>, max: u16) -> Vec<Span<'static>> {
    let w = spans_width(&spans);
    if w <= max || max == 0 {
        return spans;
    }
    let mut remaining = max;
    let mut out = Vec::new();
    for span in spans.drain(..) {
        let len = span.content.chars().count() as u16;
        if len <= remaining {
            remaining -= len;
            out.push(span);
        } else {
            if remaining == 0 {
                break;
            }
            // Need to cut this span.
            let take = (remaining as usize).saturating_sub(1);
            let mut s: String = span.content.chars().take(take).collect();
            s.push('…');
            out.push(Span::styled(s, span.style));
            break;
        }
    }
    out
}

/// Build the per-slot span lists for the current config and context.
pub fn slot_spans(
    slot: &[StatusWidget],
    ctx: &SlotContext<'_>,
) -> Vec<Span<'static>> {
    let mut groups: Vec<Vec<Span<'static>>> = Vec::new();
    for w in slot {
        let spans = match w {
            StatusWidget::Mode => Some(mode_spans(ctx.is_leader, ctx.theme)),
            StatusWidget::Menu => Some(menu_spans(ctx.menu_open, ctx.theme)),
            StatusWidget::Session => session_spans(ctx.session, &ctx.cfg.widgets.session, ctx.sidebar_open, ctx.theme),
            StatusWidget::Branch => branch_spans(ctx.session.and_then(|s| s.branch.as_ref()), &ctx.cfg.widgets.branch, ctx.theme),
            StatusWidget::AgentStatus => agent_spans(ctx.session, &ctx.cfg.widgets.agent, ctx.theme, ctx.spinner),
            StatusWidget::Hostname => {
                if ctx.hostname.is_empty() {
                    None
                } else {
                    hostname_spans(ctx.hostname, &ctx.cfg.widgets.hostname, ctx.is_ssh, ctx.theme)
                }
            }
            StatusWidget::Clock => Some(clock_spans(ctx.clock_str, ctx.theme)),
        };
        if let Some(spans) = spans {
            if !spans.is_empty() {
                groups.push(spans);
            }
        }
    }
    if groups.is_empty() {
        return Vec::new();
    }
    // Hardcoded separator as requested.
    join_with_sep(groups, " · ", ctx.theme)
}

pub struct SlotContext<'a> {
    pub cfg: &'a kumo_core::config::StatusBarConfig,
    pub session: Option<&'a SessionLayout>,
    pub theme: &'a OwnedTheme,
    pub hostname: &'a str,
    pub clock_str: &'a str,
    pub is_ssh: bool,
    pub is_leader: bool,
    pub menu_open: bool,
    pub sidebar_open: bool,
    /// Current braille spinner frame, shown while agents are `Working`.
    pub spinner: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_core::config::{AgentWidgetConfig, AgentWidgetStyle, StatusBarConfig, StatusWidget};
    use kumo_protocol::{AgentInfo, AgentStatus, LayoutNode, LayoutPane, TabLayout};
    use kumo_protocol::WireBranch;

    fn theme() -> OwnedTheme {
        OwnedTheme::from(kumo_core::theme::THEMES[kumo_core::theme::DEFAULT_THEME_IDX])
    }

    fn session_with_agent(status: AgentStatus) -> SessionLayout {
        SessionLayout {
            name: "s".into(),
            workspace: std::path::PathBuf::from("/tmp"),
            active_tab: 0,
            tabs: vec![TabLayout {
                id: 1,
                name: "t".into(),
                focus: 7,
                zoom: false,
                root: Some(Box::new(LayoutNode::Pane(LayoutPane {
                    id: 7,
                    title: "AI CLI".into(),
                    cwd: std::path::PathBuf::from("/tmp"),
                    is_ai: true,
                    agent: Some(AgentInfo { name: "opencode".into(), status, cpu: 0.0, mem_kb: 0 }),
                    mouse_reporting: false,
                    alt_screen: false,
                }))),
            }],
            branch: None,
            focus: 7,
            zoom: false,
            root: None,
        }
    }

    #[test]
    fn agent_spans_shows_spinner_while_working() {
        let cfg = AgentWidgetConfig { style: AgentWidgetStyle::Counts, ..Default::default() };
        let s = session_with_agent(AgentStatus::Working);
        let spans = agent_spans(Some(&s), &cfg, &theme(), "⠋").unwrap();
        let text: String = spans.iter().map(|x| x.content.as_ref()).collect();
        assert!(text.contains("⠋"), "spinner frame expected in: {text}");
        assert!(text.contains('1'));
    }

    #[test]
    fn agent_spans_static_dot_when_idle() {
        let cfg = AgentWidgetConfig { style: AgentWidgetStyle::Counts, ..Default::default() };
        let s = session_with_agent(AgentStatus::Idle);
        let spans = agent_spans(Some(&s), &cfg, &theme(), "⠋").unwrap();
        let text: String = spans.iter().map(|x| x.content.as_ref()).collect();
        assert!(text.contains('○'), "idle dot expected in: {text}");
        assert!(!text.contains('⠋'), "no spinner when idle: {text}");
    }

    #[test]
    fn cache_hostname_nonempty() {
        let h = cached_hostname();
        assert!(!h.trim().is_empty());
    }

    #[test]
    fn clock_format_fallback() {
        let s = format_clock("%H:%M");
        assert!(s.contains(':'), "clock should contain colon: {s}");
        let empty = format_clock("");
        assert!(!empty.is_empty());
    }

    #[test]
    fn branch_truncates() {
        let b = WireBranch { name: "very-long-branch-name-that-exceeds".into(), ahead: 2, behind: 1 };
        let cfg = BranchWidgetConfig { show_ahead_behind: true, max_len: 10 };
        let spans = branch_spans(Some(&b), &cfg, &theme()).unwrap();
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('…'));
        assert!(text.contains('↑'));
    }

    #[test]
    fn spans_width_counts_chars() {
        let spans = vec![Span::raw("ab"), Span::raw("c")];
        assert_eq!(spans_width(&spans), 3);
    }

    #[test]
    fn join_with_sep_inserts_separator() {
        let t = theme();
        let groups = vec![vec![Span::raw("a")], vec![Span::raw("b")]];
        let joined = join_with_sep(groups, " · ", &t);
        let text: String = joined.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "a · b");
    }

    #[test]
    fn slot_spans_empty_when_no_widgets_match() {
        let cfg = StatusBarConfig { left: vec![StatusWidget::Branch], center: vec![], right: vec![], ..Default::default() };
        let ctx = SlotContext {
            cfg: &cfg,
            session: None,
            theme: &theme(),
            hostname: "",
            clock_str: "12:00",
            is_ssh: false,
            is_leader: false,
            menu_open: false,
            sidebar_open: true,
            spinner: "●",
        };
        let spans = slot_spans(&cfg.left, &ctx);
        assert!(spans.is_empty(), "branch with no session should hide");
    }
}
