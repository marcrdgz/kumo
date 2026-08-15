//! Kumo Desktop: a native GPUI client for the kumo daemon.
//!
//! Unlike the TUI client, this app does **not** render the daemon's composed
//! interface. It consumes the structured snapshot (sessions/panes/agents with
//! geometry) and per-pane `PaneFrame`s, then paints panes itself: each pane is
//! a card in its session's layout, with its own title chip and agent status,
//! inside a native sidebar and status strip. Input is forwarded to the daemon,
//! which remains the single source of truth for PTYs, layout, and focus.

mod daemon;
mod grid;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

use gpui::{
    div, hsla, point, px, rgb, size, App, Application, Bounds, Context, Element, ElementId, Font,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, Keystroke, LayoutId, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, SharedString, Style, StyledText, TextStyle, Window, WindowBounds,
    WindowOptions, fill, prelude::*,
};
use kumo_protocol::{
    AgentInfo, AgentStatus, ClientMsg, PaneRect, ServerMsg, SessionInfo, WireKeyCode,
    WireKeyEvent, WireModifiers, WireMouseButton, WireMouseEvent, WireMouseKind,
};

use crate::grid::Grid;

const SIDEBAR_W: f32 = 244.0;
const STATUS_H: f32 = 28.0;
const CANVAS_BG: u32 = 0x0d0d10;
const CARD_BG: u32 = 0x111116;
const FOCUS_ACCENT: u32 = 0x4c6ef5;

struct KumoDesktop {
    to_view: mpsc::Receiver<ServerMsg>,
    from_view: mpsc::Sender<ClientMsg>,
    connected: bool,
    status: SharedString,
    sessions: Vec<SessionInfo>,
    panes: HashMap<u64, Grid>,
    subscribed: HashSet<u64>,
    selected: Option<String>,
    last_sent: Option<(u16, u16)>,
    // scaling (recomputed every frame from the window size)
    cell_w: f32,
    cell_h: f32,
    font_size: f32,
    line_height_ratio: f32,
    advance_ratio: f32,
    font: Font,
    default_fg: Hsla,
    canvas_origin: Point<Pixels>,
    canvas_size: (f32, f32),
    base: TextStyle,
    dim: TextStyle,
}

impl KumoDesktop {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let conn = daemon::spawn();
        let base = crate::grid::base_text_style(window);
        let dim = crate::grid::dim_text_style(window);
        let (line_height_ratio, advance_ratio) = crate::grid::font_ratios(window, &base);
        let font = base.font();
        let default_fg = base.color;
        let mut this = KumoDesktop {
            to_view: conn.to_view,
            from_view: conn.from_view,
            connected: false,
            status: SharedString::from("connecting to kumo daemon…"),
            sessions: Vec::new(),
            panes: HashMap::new(),
            subscribed: HashSet::new(),
            selected: None,
            last_sent: None,
            cell_w: 7.8,
            cell_h: 17.0,
            font_size: 13.0,
            line_height_ratio,
            advance_ratio,
            font,
            default_fg,
            canvas_origin: point(px(SIDEBAR_W), px(0.0)),
            canvas_size: (800.0, 600.0),
            base,
            dim,
        };
        // Declare ourselves as a snapshot + pane consumer and close the daemon's
        // chrome so panes get the full width (this app paints its own).
        let _ = this.send(ClientMsg::SubscribeSnapshot);
        let _ = this.send(ClientMsg::SetSidebar { open: false });
        cx.spawn(|this: gpui::WeakEntity<KumoDesktop>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    cx.background_executor().timer(Duration::from_millis(30)).await;
                    let _ = this.update(&mut cx, |this, cx| this.pump(cx));
                }
            }
        })
        .detach();
        this
    }

    fn send(&mut self, msg: ClientMsg) -> Result<(), ()> {
        self.from_view.send(msg).map_err(|_| ())
    }

    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------

    fn pump(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(msg) = self.to_view.try_recv() {
            match msg {
                ServerMsg::Welcome { .. } => {
                    self.connected = true;
                    self.status = SharedString::from("connected");
                    changed = true;
                }
                ServerMsg::Snapshot { sessions } => {
                    self.sessions = sessions;
                    self.on_snapshot();
                    changed = true;
                }
                ServerMsg::PaneFrame { frame } => {
                    self.panes.entry(frame.pane_id).or_default().apply(&frame);
                    changed = true;
                }
                ServerMsg::Restarting => {
                    self.status = SharedString::from("daemon restarting…");
                    changed = true;
                }
                ServerMsg::Shutdown => {
                    self.connected = false;
                    self.status = SharedString::from("daemon stopped");
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Follow the daemon's active session and keep pane subscriptions in sync
    /// with it.
    fn on_snapshot(&mut self) {
        let active = self
            .sessions
            .iter()
            .find(|s| s.active)
            .map(|s| s.name.clone())
            .or_else(|| self.sessions.first().map(|s| s.name.clone()));
        if self.selected != active {
            self.selected = active;
            if let Some(name) = &self.selected {
                let _ = self.send(ClientMsg::FocusSession { name: name.clone() });
            }
        }
        let session = self.selected_session().cloned();
        if let Some(s) = session {
            self.resubscribe(&s);
        }
    }

    /// Subscribe to / unsubscribe from the selected session's pane streams.
    fn resubscribe(&mut self, session: &SessionInfo) {
        let want: HashSet<u64> = session.panes.iter().map(|p| p.id).collect();
        let to_add: Vec<u64> = want.difference(&self.subscribed).copied().collect();
        let to_remove: Vec<u64> = self.subscribed.difference(&want).copied().collect();
        for id in to_add {
            let _ = self.send(ClientMsg::SubscribePane { pane_id: id });
        }
        for id in to_remove {
            let _ = self.send(ClientMsg::UnsubscribePane { pane_id: id });
            self.panes.remove(&id);
        }
        self.subscribed = want;
    }

    fn selected_session(&self) -> Option<&SessionInfo> {
        let name = self.selected.as_ref()?;
        self.sessions.iter().find(|s| &s.name == name)
    }

    fn select_session(&mut self, name: String) {
        self.selected = Some(name.clone());
        let _ = self.send(ClientMsg::FocusSession { name });
        let session = self.selected_session().cloned();
        if let Some(s) = session {
            self.resubscribe(&s);
        }
    }

    /// The pane under a cell coordinate, as `(session, pane_id)`.
    fn pane_at(&self, col: u16, row: u16) -> Option<(String, u64)> {
        let s = self.selected_session()?;
        for p in &s.panes {
            let r = p.rect;
            if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                return Some((s.name.clone(), p.id));
            }
        }
        None
    }

    fn focused_pane_label(&self) -> String {
        let s = self.selected_session();
        let focus = s.and_then(|s| s.focus);
        let Some(focus) = focus else { return "-".into() };
        s.and_then(|s| s.panes.iter().find(|p| p.id == focus))
            .map(|p| p.title.trim().to_string())
            .unwrap_or_else(|| format!("pane {focus}"))
    }

    // ------------------------------------------------------------------
    // Geometry
    // ------------------------------------------------------------------

    /// Extent (in cells) of the selected session's rendered grid: the max pane
    /// bottom/right edge, plus one row for the status strip.
    fn selected_bounds(&self) -> (u16, u16) {
        if let Some(s) = self.selected_session() {
            let mut w = 0u16;
            let mut h = 0u16;
            for p in &s.panes {
                w = w.max(p.rect.x + p.rect.width);
                h = h.max(p.rect.y + p.rect.height);
            }
            if w > 0 && h > 0 {
                return (w, h + 1);
            }
        }
        let (c, r) = self.last_sent.unwrap_or((80, 24));
        (c, r.saturating_sub(1))
    }

    /// Choose a daemon grid size (~13px cells) and compute the scaled cell
    /// metrics so the session's panes fill the available area.
    fn update_geometry(&mut self, window: &mut Window) {
        let vp = window.viewport_size();
        let avail_w = (f32::from(vp.width) - SIDEBAR_W).max(1.0);
        let avail_h = (f32::from(vp.height) - STATUS_H).max(1.0);
        let target_w = 13.0 * self.advance_ratio;
        let target_h = 13.0 * self.line_height_ratio;
        let cols = (avail_w / target_w).floor().max(20.0) as u16;
        let rows = (avail_h / target_h).floor().max(10.0) as u16 + 1; // + status row
        if self.last_sent != Some((cols, rows)) {
            self.last_sent = Some((cols, rows));
            let _ = self.send(ClientMsg::Resize { cols, rows });
        }
        let (gw, gh) = self.selected_bounds();
        self.cell_h = avail_h / (gh.max(1) as f32);
        self.font_size = (self.cell_h / self.line_height_ratio).clamp(6.0, 34.0);
        self.cell_w = self.font_size * self.advance_ratio;
        let canvas_w = gw as f32 * self.cell_w;
        self.canvas_origin = point(px(SIDEBAR_W + (avail_w - canvas_w).max(0.0) * 0.5), px(0.0));
        self.canvas_size = (canvas_w, avail_h);
    }

    fn cell_from_position(&self, pos: Point<Pixels>) -> Option<(u16, u16)> {
        let gx = f32::from(pos.x) - f32::from(self.canvas_origin.x);
        let gy = f32::from(pos.y) - f32::from(self.canvas_origin.y);
        if gx < 0.0 || gy < 0.0 {
            return None;
        }
        Some(((gx / self.cell_w).min(u16::MAX as f32) as u16, (gy / self.cell_h).min(u16::MAX as f32) as u16))
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let button = wire_button(ev.button);
        let Some((col, row)) = self.cell_from_position(ev.position) else { return };
        if let Some((session, pid)) = self.pane_at(col, row) {
            let _ = self.send(ClientMsg::FocusPane { session, pane_id: pid });
        }
        self.send_mouse(WireMouseKind::Down(button), col, row, ev.modifiers);
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        let button = wire_button(ev.button);
        let Some((col, row)) = self.cell_from_position(ev.position) else { return };
        self.send_mouse(WireMouseKind::Up(button), col, row, ev.modifiers);
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        let kind = match ev.pressed_button {
            Some(b) => WireMouseKind::Drag(wire_button(b)),
            None => WireMouseKind::Moved,
        };
        let Some((col, row)) = self.cell_from_position(ev.position) else { return };
        self.send_mouse(kind, col, row, ev.modifiers);
    }

    fn on_scroll_wheel(&mut self, ev: &ScrollWheelEvent, _: &mut Window, _: &mut Context<Self>) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(p) => p.y * 8.0,
        };
        let kind = if dy > 0.0 { WireMouseKind::ScrollUp } else { WireMouseKind::ScrollDown };
        let Some((col, row)) = self.cell_from_position(ev.position) else { return };
        self.send_mouse(kind, col, row, ev.modifiers);
    }

    fn send_mouse(&mut self, kind: WireMouseKind, col: u16, row: u16, mods: Modifiers) {
        let _ = self.send(ClientMsg::Mouse {
            event: WireMouseEvent { kind, col, row, modifiers: wire_modifiers(mods) },
        });
    }

    // ------------------------------------------------------------------
    // Sidebar / status
    // ------------------------------------------------------------------

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_W))
            .h_full()
            .bg(rgb(0x131318))
            .border_r_1()
            .border_color(rgb(0x1e1e24))
            .p(px(10.))
            .gap(px(3.))
            .child(self.header());
        if self.sessions.is_empty() {
            col = col.child(
                div().pt(px(6.)).child(
                    StyledText::new(SharedString::from(
                        if self.connected { "no sessions".to_string() } else { self.status.to_string() },
                    ))
                    .with_default_highlights(&self.dim, []),
                ),
            );
        } else {
            col = col.child(self.section_label("SESSIONS"));
            for s in &self.sessions {
                col = col.child(self.session_card(s, cx));
            }
            col = col.child(self.section_label("AGENTS"));
            let mut has_agents = false;
            for s in &self.sessions {
                for pane in &s.panes {
                    if let Some(agent) = &pane.agent {
                        has_agents = true;
                        col = col.child(self.agent_row(&s.name, agent, cx));
                    }
                }
            }
            if !has_agents {
                col = col.child(
                    div().child(StyledText::new(SharedString::from("no agents running")).with_default_highlights(&self.dim, [])),
                );
            }
        }
        col
    }

    fn header(&self) -> impl IntoElement {
        let dot: Hsla = if self.connected { rgb(0x2ee06b).into() } else { rgb(0x7d7d82).into() };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .pb(px(6.))
            .child(div().size_2().rounded_full().bg(dot))
            .child(StyledText::new(SharedString::from("KUMO")).with_default_highlights(&self.base, []))
    }

    fn section_label(&self, label: &str) -> impl IntoElement {
        div()
            .pt(px(6.))
            .pb(px(2.))
            .child(StyledText::new(SharedString::from(label.to_string())).with_default_highlights(&self.dim, []))
    }

    fn session_card(&self, s: &SessionInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let name = s.name.clone();
        let bg: Hsla = if s.active { rgb(0x23232c).into() } else { hsla(0.0, 0.0, 0.0, 0.0) };
        let border: Hsla = if s.active { rgb(FOCUS_ACCENT).into() } else { rgb(0x222228).into() };
        let mut card = div()
            .w_full()
            .rounded_md()
            .px(px(8.))
            .py(px(5.))
            .bg(bg)
            .border_1()
            .border_color(border)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, _cx| {
                this.select_session(name.clone());
            }));
        let title = format!(
            "{}  · {} panes{}",
            s.name,
            s.panes.len(),
            if s.zoomed { " (zoom)" } else { "" }
        );
        card = card
            .child(div().child(StyledText::new(SharedString::from(title)).with_default_highlights(&self.base, [])))
            .child(
                div().child(
                    StyledText::new(SharedString::from(s.workspace.display().to_string()))
                        .with_default_highlights(&self.dim, []),
                ),
            );
        for pane in &s.panes {
            if let Some(agent) = &pane.agent {
                card = card.child(self.agent_badge(agent));
            }
        }
        card
    }

    fn agent_badge(&self, agent: &AgentInfo) -> impl IntoElement {
        let color = agent_status_color(agent.status);
        let text = format!("{}  {} · {}", agent.name, agent.status.label(), agent_status_label(agent.status));
        let mut style = self.dim.clone();
        style.color = color;
        div().pt(px(2.)).child(StyledText::new(SharedString::from(text)).with_default_highlights(&style, []))
    }

    fn agent_row(&self, session: &str, agent: &AgentInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let session = session.to_string();
        let text = format!("{} · {} · {}", session, agent.name, agent.status.label());
        let color = agent_status_color(agent.status);
        let mut style = self.dim.clone();
        style.color = color;
        div()
            .w_full()
            .px(px(8.))
            .py(px(2.))
            .rounded_md()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, _cx| {
                this.select_session(session.clone());
            }))
            .child(StyledText::new(SharedString::from(text)).with_default_highlights(&style, []))
    }

    fn status_strip(&self) -> impl IntoElement {
        let text = format!(
            "{}   session {}   pane {}",
            if self.connected { "connected" } else { "disconnected" },
            self.selected.as_deref().unwrap_or("-"),
            self.focused_pane_label()
        );
        div()
            .w_full()
            .h(px(STATUS_H))
            .flex()
            .items_center()
            .px(px(10.))
            .border_t_1()
            .border_color(rgb(0x1e1e24))
            .bg(rgb(0x101014))
            .child(StyledText::new(SharedString::from(text)).with_default_highlights(&self.dim, []))
    }
}

// ---------------------------------------------------------------------------
// Pane canvas: paints each pane as a card (border, title chip, agent status)
// with its cells, so the app renders natively instead of showing the daemon's
// composed UI.
// ---------------------------------------------------------------------------

/// Owned snapshot of what the canvas paints this frame, extracted in
/// `prepaint` so `paint` can borrow `App` freely while shaping text.
struct CanvasPane {
    rect: PaneRect,
    focused: bool,
    title: String,
    agent: Option<(String, AgentStatus)>,
    grid: Option<Grid>,
}

struct CanvasData {
    cell_w: f32,
    cell_h: f32,
    font_size: f32,
    font: Font,
    default_fg: Hsla,
    panes: Vec<CanvasPane>,
}

struct PaneCanvas {
    view: gpui::Entity<KumoDesktop>,
}

impl PaneCanvas {
    fn extract(&self, cx: &App) -> CanvasData {
        let model = self.view.read(cx);
        let mut panes = Vec::new();
        if let Some(session) = model.selected_session() {
            for pane in &session.panes {
                let grid = model.panes.get(&pane.id).cloned();
                panes.push(CanvasPane {
                    rect: pane.rect,
                    focused: session.focus == Some(pane.id),
                    title: pane.title.trim().to_string(),
                    agent: pane.agent.as_ref().map(|a| (a.name.clone(), a.status)),
                    grid,
                });
            }
        }
        CanvasData {
            cell_w: model.cell_w,
            cell_h: model.cell_h,
            font_size: model.font_size,
            font: model.font.clone(),
            default_fg: model.default_fg,
            panes,
        }
    }
}

impl IntoElement for PaneCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PaneCanvas {
    type RequestLayoutState = ();
    type PrepaintState = CanvasData;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("pane-canvas".into()))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (w, h) = self.view.read(cx).canvas_size;
        let mut style = Style::default();
        style.size.width = px(w).into();
        style.size.height = px(h).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.extract(cx)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint_canvas(prepaint, bounds, window, cx);
    }
}

fn paint_canvas(data: &CanvasData, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    window.paint_quad(fill(bounds, rgb(CANVAS_BG)));
    let cell_w = data.cell_w;
    let cell_h = data.cell_h;
    let font_size = px(data.font_size);
    let line_h = px(cell_h);
    for pane in &data.panes {
        let r = pane.rect;
        let x = bounds.left() + px(r.x as f32 * cell_w);
        let y = bounds.top() + px(r.y as f32 * cell_h);
        let w = px(r.width as f32 * cell_w);
        let h = px(r.height as f32 * cell_h);
        let card = Bounds::new(point(x, y), size(w, h));
        if pane.focused {
            window.paint_quad(fill(card, rgb(FOCUS_ACCENT)));
        }
        let inner = Bounds::new(
            point(x + px(1.5), y + px(1.5)),
            size(w - px(3.0), h - px(3.0)),
        );
        window.paint_quad(fill(inner, rgb(CARD_BG)));

        // Cells.
        if let Some(grid) = &pane.grid {
            for row in 0..grid.rows() {
                let cells = grid.row(row).unwrap_or_default();
                let (text, runs) =
                    crate::grid::row_runs(cells, &data.font, data.default_fg, rgb(CARD_BG).into());
                let line = window
                    .text_system()
                    .shape_line(SharedString::from(text), font_size, &runs, None);
                let origin = point(x, y + px(row as f32 * cell_h));
                let _ = line.paint_background(origin, line_h, window, cx);
                let _ = line.paint(origin, line_h, window, cx);
            }
            // Native cursor: an underline under the terminal cursor cell.
            if let Some((ccx, ccy)) = grid.cursor() {
                let cw = px(cell_w);
                let ch = px(cell_h);
                let cursor_y = y + px(ccy as f32 * cell_h) + ch - px(1.5);
                window.paint_quad(fill(
                    Bounds::new(point(x + px(ccx as f32 * cell_w), cursor_y), size(cw, px(1.5))),
                    rgb(0x8aa7ff),
                ));
            }
        }

        paint_title_chip(data, pane, x, y, window, cx);
    }
}

fn paint_title_chip(
    data: &CanvasData,
    pane: &CanvasPane,
    x: Pixels,
    y: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let chip_bg: Hsla = if pane.focused { rgb(FOCUS_ACCENT).into() } else { rgb(0x26262e).into() };
    let mut text = format!(" {}", pane.title);
    let mut runs = vec![gpui::TextRun {
        len: text.len(),
        font: data.font.clone(),
        color: rgb(0xf2f2f4).into(),
        background_color: Some(chip_bg),
        underline: None,
        strikethrough: None,
    }];
    if let Some((name, status)) = &pane.agent {
        let label = format!("  {name}  {}", status.label());
        text.push_str(&label);
        runs.push(gpui::TextRun {
            len: label.len(),
            font: data.font.clone(),
            color: agent_status_color(*status),
            background_color: Some(chip_bg),
            underline: None,
            strikethrough: None,
        });
    }
    let line = window.text_system().shape_line(SharedString::from(text), px(11.0), &runs, None);
    let origin = point(x + px(3.0), y + px(3.0));
    let _ = line.paint_background(origin, px(15.0), window, cx);
    let _ = line.paint(origin, px(15.0), window, cx);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn agent_status_color(status: AgentStatus) -> Hsla {
    match status {
        AgentStatus::Blocked => rgb(0xffb84d).into(),
        AgentStatus::Working => rgb(0x2ee06b).into(),
        AgentStatus::Idle => rgb(0x7d7d82).into(),
    }
}

fn agent_status_label(status: AgentStatus) -> &'static str {
    status.label()
}

fn wire_button(b: MouseButton) -> WireMouseButton {
    match b {
        MouseButton::Left => WireMouseButton::Left,
        MouseButton::Right => WireMouseButton::Right,
        _ => WireMouseButton::Middle,
    }
}

fn wire_modifiers(m: Modifiers) -> WireModifiers {
    let mut wm = WireModifiers::none();
    wm.set_shift(m.shift)
        .set_control(m.control)
        .set_alt(m.alt)
        .set_super(m.platform)
        .set_hyper(m.function);
    wm
}

/// Map a GPUI keystroke to a wire key event, or `None` if it has no terminal
/// equivalent (e.g. a lone modifier press).
fn wire_key(ks: &Keystroke) -> Option<WireKeyEvent> {
    let mods = wire_modifiers(ks.modifiers);
    let key = ks.key.as_str();
    let code = match key {
        "backspace" => WireKeyCode::Backspace,
        "delete" => WireKeyCode::Delete,
        "enter" => WireKeyCode::Enter,
        "tab" => {
            if mods.shift() {
                WireKeyCode::BackTab
            } else {
                WireKeyCode::Tab
            }
        }
        "escape" => WireKeyCode::Esc,
        "left" => WireKeyCode::Left,
        "right" => WireKeyCode::Right,
        "up" => WireKeyCode::Up,
        "down" => WireKeyCode::Down,
        "home" => WireKeyCode::Home,
        "end" => WireKeyCode::End,
        "pageup" => WireKeyCode::PageUp,
        "pagedown" => WireKeyCode::PageDown,
        "insert" => WireKeyCode::Insert,
        "space" => WireKeyCode::Char(' '),
        _ if key.len() > 1 && key.starts_with('f') => match key[1..].parse::<u8>() {
            Ok(n) => WireKeyCode::F(n),
            Err(_) => return None,
        },
        _ => {
            let ch = ks.key_char.as_deref().unwrap_or(key).chars().next()?;
            if ch.is_control() {
                return None;
            }
            WireKeyCode::Char(ch)
        }
    };
    Some(WireKeyEvent::new(code, mods))
}

impl Render for KumoDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_geometry(window);
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(CANVAS_BG))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_grow()
                    .size_full()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_grow()
                            .w_full()
                            .child(PaneCanvas { view: cx.entity() }),
                    )
                    .child(self.status_strip()),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| KumoDesktop::new(window, cx)),
            )
            .unwrap();
        let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        // Forward every keystroke to the daemon (except the app-level chords
        // handled here), like a terminal forwards keys to the attached session.
        cx.observe_keystrokes(move |ev, _, cx| {
            let ks = ev.keystroke.clone();
            if ks.key == "q" && ks.modifiers.platform {
                cx.quit();
                return;
            }
            if ks.key == "v" && ks.modifiers.platform {
                let text = cx.read_from_clipboard().and_then(|item| item.text());
                view.update(cx, move |this, _cx| {
                    if let Some(text) = text {
                        let _ = this.send(ClientMsg::Paste { text });
                    }
                });
                return;
            }
            view.update(cx, move |this, _cx| {
                if let Some(key) = wire_key(&ks) {
                    let _ = this.send(ClientMsg::Input { key });
                }
            });
        })
        .detach();
        cx.activate(true);
    });
}
