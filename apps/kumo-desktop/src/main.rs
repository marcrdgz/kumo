//! Kumo Desktop: a native GPUI client for the kumo daemon.
//!
//! The daemon never renders chrome. This app subscribes to the semantic layout
//! (sessions → splits in ratios → panes), computes its own geometry, requests
//! per-pane sizes (`PaneResize`), and paints each pane as a card (border,
//! title chip, agent status) with its cells — all chrome drawn natively.

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
    AgentInfo, AgentStatus, Command, DaemonEvent, Layout, LayoutNode, SplitDir,
    WireKeyCode, WireKeyEvent, WireModifiers, WireMouseButton, WireMouseEvent, WireMouseKind,
};

use crate::grid::Grid;

const SIDEBAR_W: f32 = 244.0;
const STATUS_H: f32 = 28.0;
const CANVAS_BG: u32 = 0x0d0d10;
const CARD_BG: u32 = 0x111116;
const FOCUS_ACCENT: u32 = 0x4c6ef5;

/// A rectangle in cell coordinates (client-computed from the semantic tree).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CellRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

struct KumoDesktop {
    to_view: mpsc::Receiver<DaemonEvent>,
    from_view: mpsc::Sender<Command>,
    connected: bool,
    status: SharedString,
    layout: Option<Layout>,
    panes: HashMap<u64, Grid>,
    subscribed: HashSet<u64>,
    rects: Vec<(u64, CellRect)>,
    grid_size: (u16, u16),
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
            layout: None,
            panes: HashMap::new(),
            subscribed: HashSet::new(),
            rects: Vec::new(),
            grid_size: (80, 24),
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
        let _ = this.send(Command::SubscribeLayout);
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

    fn send(&mut self, msg: Command) -> Result<(), ()> {
        self.from_view.send(msg).map_err(|_| ())
    }

    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------

    fn pump(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(msg) = self.to_view.try_recv() {
            match msg {
                DaemonEvent::Welcome { .. } => {
                    self.connected = true;
                    self.status = SharedString::from("connected");
                    changed = true;
                }
                DaemonEvent::Layout { layout } => {
                    self.on_layout(&layout);
                    changed = true;
                }
                DaemonEvent::PaneFrame { frame } => {
                    self.panes.entry(frame.pane_id).or_default().apply(&frame);
                    changed = true;
                }
                DaemonEvent::Reply { message } => {
                    self.status = SharedString::from(message);
                    changed = true;
                }
                DaemonEvent::Restarting => {
                    self.status = SharedString::from("daemon restarting…");
                    changed = true;
                }
                DaemonEvent::Shutdown => {
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

    /// Follow the active session: compute geometry from the semantic tree,
    /// request pane sizes, and keep pane subscriptions in sync.
    fn on_layout(&mut self, layout: &Layout) {
        self.layout = Some(layout.clone());
        let mut want: HashSet<u64> = HashSet::new();
        if let Some(session) = layout.sessions.iter().find(|s| Some(&s.name) == layout.active.as_ref()) {
            let (gw, gh) = self.grid_size;
            let area = CellRect { x: 0, y: 0, width: gw, height: gh.saturating_sub(1) };
            let rects = if session.zoom {
                vec![(session.focus, area)]
            } else if let Some(root) = &session.root {
                compute_rects(root, area)
            } else {
                Vec::new()
            };
            self.rects = rects;
            for (pid, _r) in self.rects.clone() {
                want.insert(pid);
                if self.subscribed.insert(pid) {
                    let _ = self.send(Command::SubscribePane { pane_id: pid });
                }
            }
        }
        for pid in self.subscribed.difference(&want).copied().collect::<Vec<_>>() {
            self.subscribed.remove(&pid);
            let _ = self.send(Command::UnsubscribePane { pane_id: pid });
            self.panes.remove(&pid);
        }
    }

    /// The active session (following the daemon's active session).
    fn active_session(&self) -> Option<&kumo_protocol::SessionLayout> {
        let layout = self.layout.as_ref()?;
        let name = layout.active.as_deref()?;
        layout.sessions.iter().find(|s| s.name == name)
    }

    fn select_session(&mut self, name: String) {
        let _ = self.send(Command::SessionFocus { name });
    }

    /// The pane under a cell coordinate (client-computed geometry).
    fn pane_at(&self, col: u16, row: u16) -> Option<(String, u64)> {
        let session = self.active_session()?;
        for (pid, r) in &self.rects {
            if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                return Some((session.name.clone(), *pid));
            }
        }
        None
    }

    fn focused_pane_label(&self) -> String {
        let Some(session) = self.active_session() else { return "-".into() };
        let focus = session.focus;
        session
            .root
            .as_deref()
            .and_then(|r| find_pane(r, focus))
            .map(|p| p.title.trim().to_string())
            .unwrap_or_else(|| format!("pane {focus}"))
    }

    // ------------------------------------------------------------------
    // Geometry
    // ------------------------------------------------------------------

    /// Choose the cell grid size (from the window at a ~13px target) and
    /// re-derive the scaled cell metrics so the session's panes fill the area.
    fn update_geometry(&mut self, window: &mut Window) {
        let vp = window.viewport_size();
        let avail_w = (f32::from(vp.width) - SIDEBAR_W).max(1.0);
        let avail_h = (f32::from(vp.height) - STATUS_H).max(1.0);
        let target_w = 13.0 * self.advance_ratio;
        let target_h = 13.0 * self.line_height_ratio;
        let gw = (avail_w / target_w).floor().max(20.0) as u16;
        let gh = (avail_h / target_h).floor().max(10.0) as u16 + 1; // + status row
        if self.grid_size != (gw, gh) {
            self.grid_size = (gw, gh);
            // The daemon sizes panes within this composed grid.
            let _ = self.send(Command::Resize { cols: gw, rows: gh });
            if let Some(layout) = self.layout.clone() {
                self.on_layout(&layout);
            }
        }
        let (_, gh) = self.grid_size;
        let pane_rows = (gh.saturating_sub(1)).max(1) as f32;
        self.cell_h = avail_h / pane_rows;
        self.font_size = (self.cell_h / self.line_height_ratio).clamp(6.0, 34.0);
        self.cell_w = self.font_size * self.advance_ratio;
        let canvas_w = self.grid_size.0 as f32 * self.cell_w;
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
            let _ = self.send(Command::PaneFocus { session, pane_id: pid });
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
        let _ = self.send(Command::Mouse {
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
        if let Some(layout) = &self.layout {
            if layout.sessions.is_empty() {
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
                for s in &layout.sessions {
                    col = col.child(self.session_card(s, cx));
                }
                col = col.child(self.section_label("AGENTS"));
                let mut has_agents = false;
                for s in &layout.sessions {
                    for (_, agent) in session_agents(&s.root) {
                        has_agents = true;
                        col = col.child(self.agent_row(&s.name, &agent, cx));
                    }
                }
                if !has_agents {
                    col = col.child(
                        div().child(StyledText::new(SharedString::from("no agents running")).with_default_highlights(&self.dim, [])),
                    );
                }
            }
        } else {
            col = col.child(
                div().pt(px(6.)).child(
                    StyledText::new(SharedString::from(&self.status)).with_default_highlights(&self.dim, []),
                ),
            );
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

    fn session_card(&self, s: &kumo_protocol::SessionLayout, cx: &mut Context<Self>) -> impl IntoElement {
        let name = s.name.clone();
        let is_active = self.layout.as_ref().map(|l| l.active.as_deref()) == Some(Some(&s.name));
        let bg: Hsla = if is_active { rgb(0x23232c).into() } else { hsla(0.0, 0.0, 0.0, 0.0) };
        let border: Hsla = if is_active { rgb(FOCUS_ACCENT).into() } else { rgb(0x222228).into() };
        let pane_count = count_panes(&s.root);
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
        let title = format!("{}  · {} panes{}", s.name, pane_count, if s.zoom { " (zoom)" } else { "" });
        card = card
            .child(div().child(StyledText::new(SharedString::from(title)).with_default_highlights(&self.base, [])))
            .child(
                div().child(
                    StyledText::new(SharedString::from(s.workspace.display().to_string()))
                        .with_default_highlights(&self.dim, []),
                ),
            );
        for (_, agent) in session_agents(&s.root) {
            card = card.child(self.agent_badge(&agent));
        }
        card
    }

    fn agent_badge(&self, agent: &AgentInfo) -> impl IntoElement {
        let color = agent_status_color(agent.status);
        let text = format!("{}  {} · {}", agent.name, agent.status.label(), agent.status.label());
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
            self.layout.as_ref().and_then(|l| l.active.clone()).unwrap_or_else(|| "-".into()),
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
// Pane canvas
// ---------------------------------------------------------------------------

struct CanvasPane {
    rect: CellRect,
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
        if let Some(session) = model.active_session() {
            let focus = session.focus;
            for (pid, r) in &model.rects {
                let info = session.root.as_deref().and_then(|root| find_pane(root, *pid));
                let grid = model.panes.get(pid).cloned();
                panes.push(CanvasPane {
                    rect: *r,
                    focused: focus == *pid,
                    title: info.map(|p| p.title.trim().to_string()).unwrap_or_else(|| format!("pane {pid}")),
                    agent: info.and_then(|p| p.agent.as_ref()).map(|a| (a.name.clone(), a.status)),
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
        let w = px((r.width as f32 * cell_w).max(1.0));
        let h = px((r.height as f32 * cell_h).max(1.0));
        let card = Bounds::new(point(x, y), size(w, h));
        if pane.focused {
            window.paint_quad(fill(card, rgb(FOCUS_ACCENT)));
        }
        let inner = Bounds::new(
            point(x + px(1.5), y + px(1.5)),
            size((w - px(3.0)).max(px(1.0)), (h - px(3.0)).max(px(1.0))),
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
// Semantic layout helpers
// ---------------------------------------------------------------------------

/// Recursively lay a semantic tree out over a cell area.
fn compute_rects(node: &LayoutNode, area: CellRect) -> Vec<(u64, CellRect)> {
    match node {
        LayoutNode::Pane(p) => vec![(p.id, area)],
        LayoutNode::Split { dir, ratio, a, b } => {
            let mut out = Vec::new();
            let ratio = ratio.clamp(0.01, 0.99);
            match dir {
                SplitDir::Vertical => {
                    if area.width <= 1 {
                        return vec![];
                    }
                    let aw = ((area.width as f32) * ratio).round().clamp(1.0, (area.width - 1) as f32) as u16;
                    out.extend(compute_rects(a, CellRect { x: area.x, y: area.y, width: aw, height: area.height }));
                    out.extend(compute_rects(b, CellRect { x: area.x + aw, y: area.y, width: area.width - aw, height: area.height }));
                }
                SplitDir::Horizontal => {
                    if area.height <= 1 {
                        return vec![];
                    }
                    let ah = ((area.height as f32) * ratio).round().clamp(1.0, (area.height - 1) as f32) as u16;
                    out.extend(compute_rects(a, CellRect { x: area.x, y: area.y, width: area.width, height: ah }));
                    out.extend(compute_rects(b, CellRect { x: area.x, y: area.y + ah, width: area.width, height: area.height - ah }));
                }
            }
            out
        }
    }
}

fn count_panes(node: &Option<Box<LayoutNode>>) -> usize {
    match node {
        Some(n) => match n.as_ref() {
            LayoutNode::Pane(_) => 1,
            LayoutNode::Split { a, b, .. } => count_panes(&Some(a.clone())) + count_panes(&Some(b.clone())),
        },
        None => 0,
    }
}

fn find_pane(node: &LayoutNode, pid: u64) -> Option<&kumo_protocol::LayoutPane> {
    match node {
        LayoutNode::Pane(p) if p.id == pid => Some(p),
        LayoutNode::Split { a, b, .. } => {
            find_pane(a, pid).or_else(|| find_pane(b, pid))
        }
        _ => None,
    }
}

/// All (pane_id, agent) pairs in a session tree.
fn session_agents(node: &Option<Box<LayoutNode>>) -> Vec<(u64, AgentInfo)> {
    let mut out = Vec::new();
    let mut stack: Vec<&LayoutNode> = Vec::new();
    if let Some(n) = node {
        stack.push(n);
    }
    while let Some(n) = stack.pop() {
        match n {
            LayoutNode::Pane(p) => {
                if let Some(a) = &p.agent {
                    out.push((p.id, a.clone()));
                }
            }
            LayoutNode::Split { a, b, .. } => {
                stack.push(a);
                stack.push(b);
            }
        }
    }
    out
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

/// Map a GPUI keystroke to a wire key event.
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
                        let _ = this.send(Command::Paste { text });
                    }
                });
                return;
            }
            view.update(cx, move |this, _cx| {
                if let Some(key) = wire_key(&ks) {
                    let _ = this.send(Command::Input { key });
                }
            });
        })
        .detach();
        cx.activate(true);
    });
}
