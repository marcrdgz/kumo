//! The "Spider Web" sidebar: a collapsible, floating macOS-style pill.

use std::time::Duration;

use gpui::{
    div, pulsating_between, px, prelude::*, Animation, AnimationExt, AnyElement, Context,
    ElementId, IntoElement, MouseDownEvent, MouseButton, Render, SharedString, WeakEntity, Window,
};

use kumo_protocol::{AgentInfo, AgentStatus, Layout, LayoutNode, SessionLayout};

use crate::theme::{self, Chrome};
use crate::KumoWindow;

pub struct Sidebar {
    parent: WeakEntity<KumoWindow>,
}

impl Sidebar {
    pub fn new(parent: WeakEntity<KumoWindow>) -> Self {
        Self { parent }
    }
}

impl Render for Sidebar {
    /// Comet-style sidebar: a flat, fully transparent column sitting directly
    /// on the frosted window background — the glass reads through it, and the
    /// rows use low-alpha washes so they never bury the frost. No header: the
    /// wordmark and the collapse toggle live in the titlebar.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let parent = self.parent.upgrade().expect("sidebar outlives its window");
        let data = parent.read(cx);
        let chrome = data.chrome();
        let collapsed = data.sidebar_collapsed;
        let layout = data.layout.clone();
        let width = data.sidebar_w;

        if collapsed {
            return self.collapsed_rail(cx, &layout, &chrome, width);
        }

        let bstyle = kumo_core::config::sidebar_borders().style;
        let hidden = bstyle == kumo_core::config::BorderStyle::Hidden;
        let mut root = div()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .pt(px(8.0));
        if !hidden {
            root = root.border_r_1().border_color(theme::hairline());
        }
        root.child(self.body(cx, layout.as_ref(), &chrome))
    }
}

impl Sidebar {
    // ------------------------------------------------------------------
    // Body: Sessions & Agents
    // ------------------------------------------------------------------

    fn ordered_sections(&self) -> Vec<kumo_core::config::SidebarSection> {
        let cfg = kumo_core::config::sidebar();
        let mut out = Vec::new();
        for sec in cfg.order.iter().copied() {
            let visible = match sec {
                kumo_core::config::SidebarSection::Sessions => cfg.sections.sessions,
                kumo_core::config::SidebarSection::Agents => cfg.sections.agents,
            };
            if visible && !out.contains(&sec) {
                out.push(sec);
            }
        }
        if out.is_empty() {
            out.push(kumo_core::config::SidebarSection::Sessions);
        }
        out
    }

    fn body(&self, cx: &mut Context<Self>, layout: Option<&Layout>, chrome: &Chrome) -> impl IntoElement {
        let mut scroll = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p(px(6.0))
            .gap(px(2.0));

        let Some(layout) = layout else {
            return scroll.child(
                div()
                    .p(px(10.0))
                    .child("connecting to kumo daemon…")
                    .text_size(px(11.5))
                    .text_color(chrome.muted()),
            );
        };

        if layout.sessions.is_empty() {
            return scroll.child(
                div()
                    .rounded(px(theme::RADIUS_MD))
                    .border_1()
                    .border_color(theme::hairline())
                    .bg(chrome.accent_soft())
                    .px(px(10.0))
                    .py(px(8.0))
                    .child("starting a session…")
                    .text_size(px(11.5))
                    .text_color(chrome.muted()),
            );
        }

        for sec in self.ordered_sections() {
            match sec {
                kumo_core::config::SidebarSection::Sessions => {
                    scroll = scroll.child(self.section_label("SESSIONS", chrome));
                    for s in &layout.sessions {
                        let is_active = layout.active.as_deref() == Some(&s.name);
                        scroll = scroll.child(self.session_row(cx, s, is_active, chrome));
                    }
                    scroll = scroll.child(self.new_session_row(cx, chrome));
                }
                kumo_core::config::SidebarSection::Agents => {
                    scroll = scroll.child(self.section_label("AGENTS", chrome));
                    let mut has_agents = false;
                    for session in &layout.sessions {
                        for tab in &session.tabs {
                            collect_agents(&tab.root, &mut |pid, agent| {
                                has_agents = true;
                                scroll.extend([self.agent_row(cx, &session.name, pid, agent, chrome).into_any_element()]);
                            });
                        }
                    }
                    if !has_agents {
                        scroll = scroll.child(
                            div()
                                .p(px(10.0))
                                .child("no agents running")
                                .text_size(px(11.5))
                                .text_color(chrome.muted()),
                        );
                    }
                }
            }
        }

        scroll
    }

    fn section_label(&self, label: &'static str, chrome: &Chrome) -> impl IntoElement {
        div()
            .pt(px(8.0))
            .pb(px(3.0))
            .px(px(6.0))
            .child(label)
            .text_size(px(9.5))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(chrome.muted())
    }

    fn session_row(&self, cx: &mut Context<Self>, s: &SessionLayout, is_active: bool, chrome: &Chrome) -> impl IntoElement {
        let name = s.name.clone();
        let toggle = self.parent.clone();
        let (name_ctx, toggle_ctx) = (name.clone(), toggle.clone());
        let is_zoomed = s.tabs.get(s.active_tab).map(|t| t.zoom).unwrap_or(false);
        let title = if is_zoomed { format!("{} (zoom)", s.name) } else { s.name.clone() };
        let branch = s
            .branch
            .as_ref()
            .map(|b| {
                let mut text = b.name.clone();
                if b.ahead > 0 {
                    text.push_str(&format!(" ↑{}", b.ahead));
                }
                if b.behind > 0 {
                    text.push_str(&format!(" ~{}", b.behind));
                }
                text
            });

        let bg_color = if is_active { chrome.accent_soft() } else { gpui::hsla(0.0, 0.0, 0.0, 0.0) };

        let mut row = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(theme::RADIUS_MD))
            .bg(bg_color)
            .cursor_pointer()
            .hover(|style| style.bg(theme::wash(0x08)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _ev, _window, cx| {
                    let _ = toggle.update(cx, |parent, _cx| parent.select_session(name.clone()));
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_this: &mut Sidebar, ev: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Sidebar>| {
                    let _ = toggle_ctx.update(cx, |parent, cx| {
                        parent.open_session_ctx_menu(name_ctx.clone(), ev.position, cx)
                    });
                }),
            )
            .child(
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(if is_active { chrome.accent() } else { gpui::hsla(0.0, 0.0, 0.0, 0.0) }),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .child(title)
                    .text_size(px(12.0))
                    .font_weight(if is_active { gpui::FontWeight::MEDIUM } else { gpui::FontWeight::NORMAL })
                    .text_color(if is_active { chrome.accent() } else { chrome.text() }),
            );
        if let Some(branch) = branch {
            row = row.child(
                div()
                    .truncate()
                    .max_w(px(110.0))
                    .child(branch)
                    .text_size(px(9.5))
                    .text_color(chrome.muted()),
            );
        }
        row
    }

    /// The "+ new session" affordance under the session rows (opens the name
    /// popup, like `leader+c`).
    fn new_session_row(&self, cx: &mut Context<Self>, chrome: &Chrome) -> impl IntoElement {
        let toggle = self.parent.clone();
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(theme::RADIUS_MD))
            .cursor_pointer()
            .hover(|style| style.bg(theme::wash(0x08)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _ev, _window, cx| {
                    let _ = toggle.update(cx, |parent, cx| parent.open_session_popup(cx));
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(14.0))
                    .rounded(px(4.0))
                    .bg(chrome.accent_soft())
                    .child("+")
                    .text_size(px(10.0))
                    .text_color(chrome.accent()),
            )
            .child(
                div()
                    .child("new session")
                    .text_size(px(11.5))
                    .text_color(chrome.muted()),
            )
    }

    fn agent_row(
        &self,
        cx: &mut Context<Self>,
        session: &str,
        pid: u64,
        agent: &AgentInfo,
        chrome: &Chrome,
    ) -> impl IntoElement {
        let session = session.to_string();
        let toggle = self.parent.clone();
        let metrics = format!("{:.1}% CPU · {} MB", agent.cpu, agent.mem_kb / 1024);

        div()
            .w_full()
            .rounded(px(theme::RADIUS_MD))
            .px(px(8.0))
            .py(px(5.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme::wash(0x08)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _ev, _window, cx| {
                    // Focus the session AND the pane hosting the agent.
                    let _ = toggle.update(cx, |parent, _cx| {
                        let _ = parent.send(crate::Command::PaneFocus {
                            session: session.clone(),
                            pane_id: pid,
                        });
                    });
                }),
            )
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(self.avatar(&agent.name, chrome))
            .child(
                div()
                    .flex_1()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .child(agent.name.clone())
                                    .text_size(px(12.0))
                                    .text_color(chrome.text()),
                            )
                            .child(self.status_dot(agent.status, &agent.name, chrome)),
                    )
                    .child(self.metrics_pill(metrics, chrome)),
            )
    }

    fn avatar(&self, name: &str, chrome: &Chrome) -> impl IntoElement {
        let glyph = name.chars().next().unwrap_or('?').to_ascii_uppercase().to_string();
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(20.0))
            .rounded_full()
            .bg(chrome.accent_soft())
            .child(glyph)
            .text_size(px(10.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(chrome.accent())
    }

    fn status_dot(&self, status: AgentStatus, key: &str, chrome: &Chrome) -> AnyElement {
        let dot = div().size(px(8.0)).rounded_full().bg(chrome.status(status));
        match status {
            AgentStatus::Working => {
                // Static key ID to prevent dynamic String allocations per frame
                let anim_id = ElementId::NamedInteger(SharedString::from("sidebar-dot"), key.len() as u64);
                dot.with_animation(
                    anim_id,
                    Animation::new(Duration::from_millis(1500))
                        .repeat()
                        .with_easing(pulsating_between(0.45, 1.0)),
                    |dot, delta| dot.opacity(delta),
                )
                .into_any_element()
            }
            _ => dot.into_any_element(),
        }
    }

    fn metrics_pill(&self, text: String, chrome: &Chrome) -> impl IntoElement {
        div()
            .rounded_full()
            .px(px(6.0))
            .py(px(1.0))
            .bg(theme::wash(0x07))
            .child(text)
            .text_size(px(9.5))
            .text_color(chrome.muted())
    }

    // ------------------------------------------------------------------
    // Collapsed Rail
    // ------------------------------------------------------------------

    fn collapsed_rail(
        &self,
        _cx: &mut Context<Self>,
        layout: &Option<Layout>,
        chrome: &Chrome,
        width: f32,
    ) -> gpui::Div {
        let bstyle = kumo_core::config::sidebar_borders().style;
        let hidden = bstyle == kumo_core::config::BorderStyle::Hidden;
        let mut inner = div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(12.0))
            .gap(px(10.0));
        if !hidden {
            inner = inner.border_r_1().border_color(theme::hairline());
        }
        let mut rail = div().w(px(width)).h_full().child(inner);

        if let Some(layout) = layout {
            let mut dots: Vec<AnyElement> = Vec::new();
            if layout.sessions.iter().any(|s| Some(s.name.as_str()) == layout.active.as_deref()) {
                dots.push(div().size(px(8.0)).rounded_full().bg(chrome.accent()).into_any_element());
            }

            for session in &layout.sessions {
                for tab in &session.tabs {
                    collect_agents(&tab.root, &mut |_pid, agent| {
                        dots.push(self.status_dot(agent.status, &agent.name, chrome));
                    });
                }
            }

            if !dots.is_empty() {
                rail = rail.child(
                    div()
                        .w_full()
                        .flex_1()
                        .flex_col()
                        .items_center()
                        .pt(px(6.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(8.0))
                                .children(dots),
                        ),
                );
            }
        }
        rail
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Traverses the layout tree without heap allocations or cloning `AgentInfo`,
/// yielding each agent together with the id of the pane hosting it.
fn collect_agents<'a>(node: &'a Option<Box<LayoutNode>>, callback: &mut impl FnMut(u64, &'a AgentInfo)) {
    let mut stack = Vec::new();
    if let Some(n) = node {
        stack.push(n.as_ref());
    }
    while let Some(n) = stack.pop() {
        match n {
            LayoutNode::Pane(p) => {
                if let Some(a) = &p.agent {
                    callback(p.id, a);
                }
            }
            LayoutNode::Split { a, b, .. } => {
                // `a`/`b` are boxed children; walk both subtrees.
                stack.push(b.as_ref());
                stack.push(a.as_ref());
            }
        }
    }
}
