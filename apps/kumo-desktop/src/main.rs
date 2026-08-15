//! Kumo Desktop: a native GPUI client for the kumo daemon.
//!
//! The daemon is the single source of truth (sessions, panes, PTYs, agents)
//! and streams everything over a unix socket. This app attaches like the TUI
//! client — rendering the daemon's composed `WireCell` frames in a native grid
//! view with full keyboard/mouse input — and additionally subscribes to the
//! structured snapshot to drive a native sessions/agents sidebar. Because it is
//! just another client, you can use the terminal, this app, or both at once.

mod daemon;
mod grid;

use std::sync::mpsc;
use std::time::Duration;

use gpui::{
    div, hsla, px, rgb, App, Application, Bounds, Context, ElementId, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, SharedString, StyledText, TextStyle, Window, WindowBounds, WindowOptions,
    prelude::*,
};
use kumo_protocol::{
    ClientMsg, ServerMsg, SessionInfo, WireKeyCode, WireKeyEvent, WireModifiers, WireMouseButton,
    WireMouseEvent, WireMouseKind,
};

use crate::grid::Grid;

const SIDEBAR_W: f32 = 230.0;

struct KumoDesktop {
    to_view: mpsc::Receiver<ServerMsg>,
    from_view: mpsc::Sender<ClientMsg>,
    connected: bool,
    status: SharedString,
    grid: Grid,
    snapshot: Vec<SessionInfo>,
    cell_w: f32,
    cell_h: f32,
    base: TextStyle,
    dim: TextStyle,
    last_sent: Option<(u16, u16)>,
}

impl KumoDesktop {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let conn = daemon::spawn();
        let base = grid::base_text_style(window);
        let dim = grid::dim_text_style(window);
        let (cell_w, cell_h) = grid::cell_size(window, &base);
        let this = KumoDesktop {
            to_view: conn.to_view,
            from_view: conn.from_view,
            connected: false,
            status: SharedString::from("connecting to kumo daemon…"),
            grid: Grid::default(),
            snapshot: Vec::new(),
            cell_w,
            cell_h,
            base,
            dim,
            last_sent: None,
        };
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

    fn send(&mut self, msg: ClientMsg) {
        let _ = self.from_view.send(msg);
    }

    /// Drain messages from the daemon and notify GPUI when the view changed.
    fn pump(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(msg) = self.to_view.try_recv() {
            match msg {
                ServerMsg::Welcome { .. } => {
                    self.connected = true;
                    self.status = SharedString::from("connected");
                    changed = true;
                }
                ServerMsg::Frame { frame } => {
                    self.grid.apply(&frame);
                    changed = true;
                }
                ServerMsg::Snapshot { sessions } => {
                    self.snapshot = sessions;
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

    /// Recompute the terminal size from the window and send `Resize` when it
    /// changes (the daemon renders the whole UI at the client's size).
    fn update_geometry(&mut self, window: &mut Window) {
        let size = window.viewport_size();
        let avail_w = (f32::from(size.width) - SIDEBAR_W).max(1.0);
        let cols = (avail_w / self.cell_w).floor().max(1.0) as u16;
        let rows = (f32::from(size.height) / self.cell_h).floor().max(1.0) as u16;
        if self.last_sent != Some((cols, rows)) {
            self.last_sent = Some((cols, rows));
            self.send(ClientMsg::Resize { cols, rows });
        }
    }

    // ------------------------------------------------------------------
    // Mouse
    // ------------------------------------------------------------------

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let button = wire_button(ev.button);
        self.mouse_at(ev.position, WireMouseKind::Down(button), ev.modifiers);
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.mouse_at(ev.position, WireMouseKind::Up(wire_button(ev.button)), ev.modifiers);
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        let kind = match ev.pressed_button {
            Some(b) => WireMouseKind::Drag(wire_button(b)),
            None => WireMouseKind::Moved,
        };
        self.mouse_at(ev.position, kind, ev.modifiers);
    }

    fn on_scroll_wheel(&mut self, ev: &ScrollWheelEvent, _: &mut Window, _: &mut Context<Self>) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(p) => p.y * 8.0,
        };
        let kind = if dy > 0.0 { WireMouseKind::ScrollUp } else { WireMouseKind::ScrollDown };
        self.mouse_at(ev.position, kind, ev.modifiers);
    }

    fn mouse_at(&mut self, pos: Point<Pixels>, kind: WireMouseKind, mods: Modifiers) {
        let gx = f32::from(pos.x) - SIDEBAR_W;
        let gy = f32::from(pos.y);
        if gx < 0.0 || gy < 0.0 {
            return;
        }
        let col = (gx / self.cell_w).min(u16::MAX as f32) as u16;
        let row = (gy / self.cell_h).min(u16::MAX as f32) as u16;
        self.send(ClientMsg::Mouse { event: WireMouseEvent { kind, col, row, modifiers: wire_modifiers(mods) } });
    }

    // ------------------------------------------------------------------
    // Sidebar
    // ------------------------------------------------------------------

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_W))
            .h_full()
            .bg(rgb(0x1c1c1e))
            .border_r_1()
            .border_color(rgb(0x2a2a2c))
            .p(px(8.))
            .gap(px(2.))
            .child(
                div().pb(px(4.)).child(
                    StyledText::new(SharedString::from("KUMO")).with_default_highlights(&self.base, []),
                ),
            );
        if self.snapshot.is_empty() {
            col = col.child(
                div().child(
                    StyledText::new(SharedString::from(
                        if self.connected { "no sessions".to_string() } else { self.status.to_string() },
                    ))
                    .with_default_highlights(&self.dim, []),
                ),
            );
        } else {
            for s in &self.snapshot {
                col = col.child(self.session_row(s, cx));
            }
        }
        col
    }

    fn session_row(&self, s: &SessionInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let name = s.name.clone();
        let marker = if s.active { "● " } else { "○ " };
        let mut row = div()
            .flex()
            .flex_col()
            .w_full()
            .px(px(6.))
            .py(px(3.))
            .rounded_md()
            .bg(if s.active { rgb(0x2c2c34).into() } else { hsla(0.0, 0.0, 0.0, 0.0) })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, _cx| {
                this.send(ClientMsg::FocusSession { name: name.clone() });
            }))
            .child(
                StyledText::new(SharedString::from(format!("{marker}{}", s.name)))
                    .with_default_highlights(&self.base, []),
            )
            .child(
                StyledText::new(SharedString::from(s.workspace.display().to_string()))
                    .with_default_highlights(&self.dim, []),
            );
        for agent in s.panes.iter().filter_map(|p| p.agent.as_ref()) {
            let dot = match agent.status {
                kumo_protocol::AgentStatus::Blocked => "● ",
                kumo_protocol::AgentStatus::Working => "● ",
                kumo_protocol::AgentStatus::Idle => "○ ",
            };
            let dot_color = match agent.status {
                kumo_protocol::AgentStatus::Blocked => rgb(0xffb84d).into(),
                kumo_protocol::AgentStatus::Working => rgb(0x2ee06b).into(),
                kumo_protocol::AgentStatus::Idle => rgb(0x66666a).into(),
            };
            let mut style = self.dim.clone();
            style.color = dot_color;
            row = row.child(
                div().child(
                    StyledText::new(SharedString::from(format!("{dot}{} · {}", agent.name, agent.status.label())))
                        .with_default_highlights(&style, []),
                ),
            );
        }
        row
    }

    fn grid_view(&self) -> impl IntoElement {
        let rows: usize = self.grid.rows() as usize;
        let mut container = div().flex().flex_col().flex_grow().overflow_hidden();
        if rows == 0 {
            container = container.child(
                div().p(px(12.)).child(
                    StyledText::new(SharedString::from(&self.status))
                        .with_default_highlights(&self.dim, []),
                ),
            );
        } else {
            for y in 0..rows {
                let styled = match self.grid.row(y as u16) {
                    Some(cells) => grid::row_styled_text(cells, &self.base),
                    None => StyledText::new(SharedString::from(" ")).with_default_highlights(&self.base, []),
                };
                container = container.child(
                    div()
                        .id(ElementId::named_usize("grid-row", y))
                        .h(px(self.cell_h))
                        .w_full()
                        .child(styled),
                );
            }
        }
        container
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

fn on_keystroke(ks: &Keystroke, this: &mut KumoDesktop) {
    if let Some(key) = wire_key(ks) {
        this.send(ClientMsg::Input { key });
    }
}

impl Render for KumoDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_geometry(window);
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(0x121214))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(self.sidebar(cx))
            .child(self.grid_view())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, gpui::size(px(1100.), px(720.)), cx);
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
                        this.send(ClientMsg::Paste { text });
                    }
                });
                return;
            }
            view.update(cx, move |this, _cx| on_keystroke(&ks, this));
        })
        .detach();
        cx.activate(true);
    });
}
