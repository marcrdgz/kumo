//! Kumo Desktop: a native GPUI client for the kumo daemon.
//!
//! The daemon never renders chrome. This app subscribes to the semantic layout
//! (sessions → splits in ratios → panes), computes its own geometry, requests
//! per-pane sizes (`PaneResize`), and paints each pane as a rounded card with
//! native GPUI chrome — a floating "Spider Web" sidebar for sessions + AI
//! agents, drag-to-resize separators with a neon hover glow, and a neon focus
//! ring around the active pane.
//!
//! Component structure:
//! - [`KumoWindow`] — root view: daemon connection, geometry, input routing.
//! - [`Sidebar`](crate::sidebar::Sidebar) — collapsible floating pill.
//! - [`TerminalPane`](crate::panes::TerminalPane) — the GPU pane canvas.

mod daemon;
mod grid;
mod panes;
mod sidebar;
mod theme;
mod window;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

use gpui::{
    div, point, px, size, App, Application, Bounds, Context, Entity, Font, Hsla, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, SharedString, StyledText, TextStyle, WeakEntity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, prelude::*,
};
use kumo_protocol::{
    Command, DaemonEvent, Layout, LayoutNode, SplitDir, WireKeyCode, WireKeyEvent, WireModifiers,
    WireMouseButton, WireMouseEvent, WireMouseKind,
};

/// Messages from the background update manager (startup bootstrap + in-app
/// update checks) to the window.
enum UpdateMsg {
    /// The check finished: here is the current status of the CLI and the app.
    Status(kumo_core::updater::UpdateStatus),
    /// A transient banner line (installing/updating/results).
    Banner(String),
    /// A `kumo` CLI update attempt finished.
    CliDone(Result<(), String>),
    /// A desktop self-update attempt finished (`Ok` = app is relaunching).
    DesktopDone(Result<(), String>),
}

use crate::panes::{
    CellRect, PaneMetrics, Sel, SplitDrag, SplitGeom, TerminalPane,
};
use crate::sidebar::Sidebar;

/// Expanded sidebar width (the floating pill inset by 8px each side).
pub(crate) const SIDEBAR_W: f32 = 268.0;
/// Collapsed sidebar width (a slim rail).
pub(crate) const SIDEBAR_W_COLLAPSED: f32 = 48.0;
/// Height of the custom drag-to-move titlebar (replaces the hidden native bar).
pub(crate) const TITLEBAR_H: f32 = 36.0;
const STATUS_H: f32 = 30.0;

pub(crate) struct KumoWindow {
    to_view: mpsc::Receiver<DaemonEvent>,
    from_view: mpsc::Sender<Command>,
    connected: bool,
    status: SharedString,
    layout: Option<Layout>,
    panes: HashMap<u64, std::rc::Rc<std::cell::RefCell<crate::grid::Grid>>>,
    subscribed: HashSet<u64>,
    sent_sizes: HashMap<u64, (u16, u16)>,
    rects: Vec<(u64, CellRect)>,
    splitters: Vec<SplitGeom>,
    drag: Option<SplitDrag>,
    hover_splitter: Option<u64>,
    /// Client-side text selection in one pane (kept visible after mouse-up,
    /// cleared by the next click that starts a gesture in a pane).
    sel: Option<Sel>,
    sidebar_collapsed: bool,
    /// Mouse-down on the custom titlebar strip: the next mouse-move hands the
    /// drag to AppKit so the frameless window follows the pointer.
    titlebar_drag_armed: bool,
    /// Whether we already asked the daemon for a first session on attach (so
    /// the app opens a terminal immediately instead of an empty canvas).
    bootstrap_requested: bool,
    sidebar: Entity<Sidebar>,
    terminal: Entity<TerminalPane>,
    grid_size: (u16, u16),
    // update manager (background thread → window)
    update_tx: mpsc::Sender<UpdateMsg>,
    update_rx: mpsc::Receiver<UpdateMsg>,
    updates: kumo_core::updater::UpdateStatus,
    update_banner: SharedString,
    update_banner_dismissed: bool,
    updating_cli: bool,
    updating_desktop: bool,
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

impl KumoWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let conn = daemon::spawn();
        let base = crate::grid::base_text_style(window);
        let dim = crate::grid::dim_text_style(window);
        let (line_height_ratio, advance_ratio) = crate::grid::font_ratios(window, &base);
        let font = base.font();
        let default_fg = base.color;
        let weak_self: WeakEntity<KumoWindow> = cx.weak_entity();
        // Update manager: install the kumo CLI on first run if missing, then
        // report update status for the CLI and this app. Runs on its own
        // thread so a fresh install never blocks the window.
        let (update_tx, update_rx) = mpsc::channel::<UpdateMsg>();
        let boot_tx = update_tx.clone();
        std::thread::spawn(move || {
            if kumo_core::updater::find_kumo().is_none() {
                let _ = boot_tx.send(UpdateMsg::Banner("installing kumo CLI…".into()));
                let _ = kumo_core::updater::install_cli_if_missing();
            }
            let _ = boot_tx.send(UpdateMsg::Status(kumo_core::updater::check_all()));
        });
        let mut this = KumoWindow {
            to_view: conn.to_view,
            from_view: conn.from_view,
            connected: false,
            status: SharedString::from("connecting to kumo daemon…"),
            layout: None,
            panes: HashMap::new(),
            subscribed: HashSet::new(),
            sent_sizes: HashMap::new(),
            rects: Vec::new(),
            splitters: Vec::new(),
            drag: None,
            hover_splitter: None,
            sel: None,
            sidebar_collapsed: false,
            titlebar_drag_armed: false,
            bootstrap_requested: false,
            sidebar: cx.new(|_cx| Sidebar::new(weak_self.clone())),
            terminal: cx.new(|_cx| TerminalPane::new(weak_self.clone())),
            grid_size: (80, 24),
            update_tx,
            update_rx,
            updates: kumo_core::updater::UpdateStatus::default(),
            update_banner: SharedString::from(""),
            update_banner_dismissed: false,
            updating_cli: false,
            updating_desktop: false,
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
        cx.spawn(|this: WeakEntity<KumoWindow>, cx: &mut gpui::AsyncApp| {
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

    pub(crate) fn send(&mut self, msg: Command) -> Result<(), ()> {
        self.from_view.send(msg).map_err(|_| ())
    }

    /// The active chrome palette (comet frost; the daemon's `Theme` events
    /// re-color the terminal panes themselves, not the desktop chrome).
    pub(crate) fn chrome(&self) -> &'static theme::Chrome {
        theme::chrome(0)
    }

    /// The width the sidebar currently occupies (its collapsed rail or the
    /// expanded pill), which the geometry calc subtracts from the viewport.
    fn sidebar_width(&self) -> f32 {
        if self.sidebar_collapsed {
            SIDEBAR_W_COLLAPSED
        } else {
            SIDEBAR_W
        }
    }

    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------

    fn pump(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Ok(msg) = self.update_rx.try_recv() {
            match msg {
                UpdateMsg::Status(status) => {
                    self.updates = status;
                    self.update_banner = SharedString::from("");
                    changed = true;
                }
                UpdateMsg::Banner(text) => {
                    self.update_banner = SharedString::from(text);
                    self.update_banner_dismissed = false;
                    changed = true;
                }
                UpdateMsg::CliDone(result) => {
                    self.updating_cli = false;
                    self.update_banner = SharedString::from(match result {
                        Ok(()) => "kumo CLI updated".to_string(),
                        Err(e) => format!("kumo CLI update failed: {e}"),
                    });
                    changed = true;
                }
                UpdateMsg::DesktopDone(result) => {
                    self.updating_desktop = false;
                    match result {
                        Ok(()) => {
                            // The fresh bundle was installed and a relaunch was
                            // scheduled; quit so it can take over.
                            cx.quit();
                            return;
                        }
                        Err(e) => {
                            self.update_banner = SharedString::from(format!("desktop update failed: {e}"));
                            changed = true;
                        }
                    }
                }
            }
        }
        while let Ok(msg) = self.to_view.try_recv() {
            match msg {
                DaemonEvent::Welcome { .. } => {
                    self.connected = true;
                    self.status = SharedString::from("connected");
                    changed = true;
                }
                DaemonEvent::Layout { layout } => {
                    self.on_layout(&layout);
                    // Open a first terminal on attach (tmux-style): a fresh
                    // desktop window should show a shell right away, not an
                    // empty canvas. One-shot so it never fights the user.
                    if !self.bootstrap_requested && layout.sessions.is_empty() {
                        self.bootstrap_requested = true;
                        let _ = self.send(Command::SessionNew { name: None, workspace: None });
                    }
                    changed = true;
                }
                DaemonEvent::PaneFrame { frame } => {
                    self.panes
                        .entry(frame.pane_id)
                        .or_default()
                        .borrow_mut()
                        .apply(&frame);
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
            let (rects, splitters) = if session.zoom {
                (vec![(session.focus, area)], Vec::new())
            } else if let Some(root) = &session.root {
                let mut splitters = Vec::new();
                let rects = compute_rects(root, area);
                compute_splitters(root, area, &mut splitters);
                (rects, splitters)
            } else {
                (Vec::new(), Vec::new())
            };
            self.rects = rects;
            self.splitters = splitters;
            for (pid, r) in self.rects.clone() {
                want.insert(pid);
                // The pane's terminal matches the daemon's inner() grid: the
                // rect minus its 1-cell border and the left gutter.
                let dims = ((r.width.saturating_sub(3)).max(1), (r.height.saturating_sub(2)).max(1));
                if self.sent_sizes.get(&pid) != Some(&dims) {
                    self.sent_sizes.insert(pid, dims);
                    let _ = self.send(Command::PaneResize { pane_id: pid, cols: dims.0, rows: dims.1 });
                }
                if self.subscribed.insert(pid) {
                    let _ = self.send(Command::SubscribePane { pane_id: pid });
                }
            }
        }
        for pid in self.subscribed.difference(&want).copied().collect::<Vec<_>>() {
            self.subscribed.remove(&pid);
            let _ = self.send(Command::UnsubscribePane { pane_id: pid });
            self.panes.remove(&pid);
            self.sent_sizes.remove(&pid);
        }
    }

    /// The active session (following the daemon's active session).
    pub(crate) fn active_session(&self) -> Option<&kumo_protocol::SessionLayout> {
        let layout = self.layout.as_ref()?;
        let name = layout.active.as_deref()?;
        layout.sessions.iter().find(|s| s.name == name)
    }

    pub(crate) fn select_session(&mut self, name: String) {
        let _ = self.send(Command::SessionFocus { name });
    }

    /// Pixel card bounds + per-pane cell metrics for a pane rect. Cells are
    /// scaled to fit inside the card after subtracting the gap (and the title
    /// strip), so terminal content never collides with the chrome.
    pub(crate) fn pane_metrics(&self, r: &CellRect) -> PaneMetrics {
        pane_metrics(self, r)
    }

    /// The pane under a pixel position, with the cell coordinate inside it
    /// (using the same per-pane metrics the cells are painted at).
    fn pane_at_pixel(&self, pos: Point<Pixels>) -> Option<(String, u64, u16, u16)> {
        let session = self.active_session()?;
        for (pid, r) in &self.rects {
            let m = self.pane_metrics(r);
            if pos.x < m.x || pos.x >= m.x + m.w || pos.y < m.y || pos.y >= m.y + m.h {
                continue;
            }
            let cx = ((f32::from(pos.x - m.content_x) / m.cell_w).floor().max(0.0) as u16)
                .min(r.width.saturating_sub(1));
            let cy = ((f32::from(pos.y - m.content_y) / m.cell_h).floor().max(0.0) as u16)
                .min(r.height.saturating_sub(1));
            return Some((session.name.clone(), *pid, cx, cy));
        }
        None
    }

    /// The splitter strip under a pixel position, if any (start a resize drag).
    fn splitter_at_pixel(&self, pos: Point<Pixels>) -> Option<SplitGeom> {
        let (col, row) = cell_from_position(self, pos)?;
        self.splitters.iter().copied().find(|s| {
            col >= s.strip.x
                && col < s.strip.x + s.strip.width
                && row >= s.strip.y
                && row < s.strip.y + s.strip.height
        })
    }

    /// Absolute ratio (0.05..0.95) for a drag position within a split's area.
    fn drag_ratio(&self, drag: &SplitDrag, pos: Point<Pixels>) -> f32 {
        let origin = self.canvas_origin;
        match drag.dir {
            SplitDir::Vertical => {
                let total = (drag.area.width as f32 * self.cell_w).max(1.0);
                let rel = f32::from(pos.x) - f32::from(origin.x) - drag.area.x as f32 * self.cell_w;
                (rel / total).clamp(0.05, 0.95)
            }
            SplitDir::Horizontal => {
                let total = (drag.area.height as f32 * self.cell_h).max(1.0);
                let rel = f32::from(pos.y) - f32::from(origin.y) - drag.area.y as f32 * self.cell_h;
                (rel / total).clamp(0.05, 0.95)
            }
        }
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
        let avail_w = (f32::from(vp.width) - self.sidebar_width()).max(1.0);
        let avail_h = (f32::from(vp.height) - STATUS_H - TITLEBAR_H).max(1.0);
        let target_w = 13.0 * self.advance_ratio;
        let target_h = 13.0 * self.line_height_ratio;
        let gw = (avail_w / target_w).floor().max(20.0) as u16;
        let gh = (avail_h / target_h).floor().max(10.0) as u16 + 1; // + status row
        if self.grid_size != (gw, gh) {
            self.grid_size = (gw, gh);
            // The pane sizes follow the grid; re-derive geometry and request
            // new per-pane sizes.
            if let Some(layout) = self.layout.clone() {
                self.on_layout(&layout);
            }
        }
        let (_, gh) = self.grid_size;
        let pane_rows = (gh.saturating_sub(1)).max(1) as f32;
        let pad_y = 12.0;
        let cells_h = (avail_h - 2.0 * pad_y).max(1.0);
        self.cell_h = cells_h / pane_rows;
        self.font_size = (self.cell_h / self.line_height_ratio).clamp(6.0, 34.0);
        let gw = self.grid_size.0 as f32;
        let max_cell_w = ((avail_w - 2.0 * panes::PANE_GAP).max(1.0)) / gw;
        let nominal_cell_w = self.font_size * self.advance_ratio;
        let cell_w = nominal_cell_w.min(max_cell_w);
        if cell_w < nominal_cell_w {
            self.font_size = (cell_w / self.advance_ratio).clamp(6.0, 34.0);
        }
        self.cell_w = cell_w;
        let canvas_w = gw * self.cell_w;
        let sidebar_gap = if self.sidebar_collapsed {
            ((avail_w - canvas_w) * 0.5).clamp(panes::PANE_GAP, 24.0)
        } else {
            0.0 // sin gap cuando el sidebar está abierto, los panes pegados al sidebar
        };
        let total_pane_h = pane_rows * self.cell_h;
        let y_offset = ((avail_h - total_pane_h) * 0.5).max(pad_y);
        // canvas_origin es la posición absoluta del canvas en la ventana
        self.canvas_origin = point(px(self.sidebar_width() + sidebar_gap), px(TITLEBAR_H + y_offset));
        self.canvas_size = (canvas_w, avail_h);
    }

    fn cell_from_position(&self, pos: Point<Pixels>) -> Option<(u16, u16)> {
        cell_from_position(self, pos)
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    /// Whether a pane's program enabled mouse reporting (its own selection,
    /// etc.) — in that case gestures are forwarded unless Shift is held.
    fn pane_reports_mouse(&self, pid: u64) -> bool {
        self.active_session()
            .and_then(|s| s.root.as_deref())
            .and_then(|r| find_pane(r, pid))
            .map(|p| p.mouse_reporting)
            .unwrap_or(false)
    }

    /// Clamp a cell position to the pane's actual streamed grid (the layout
    /// rect is a few cells wider/taller than the terminal inside it).
    fn clamp_to_grid(&self, pid: u64, col: u16, row: u16) -> (u16, u16) {
        match self.panes.get(&pid) {
            Some(g) => {
                let g = g.borrow();
                (col.min(g.cols().saturating_sub(1)), row.min(g.rows().saturating_sub(1)))
            }
            None => (col, row),
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let button = wire_button(ev.button);
        // Left press on a divider starts a resize drag (the daemon owns the
        // ratios; we stream `PaneResizeTo` as the pointer moves).
        if ev.button == MouseButton::Left {
            if let Some(sg) = self.splitter_at_pixel(ev.position) {
                self.drag = Some(SplitDrag { split_id: sg.split_id, dir: sg.dir, area: sg.area });
                return;
            }
        }
        if let Some((session, pid, col, row)) = self.pane_at_pixel(ev.position) {
            let _ = self.send(Command::PaneFocus { session, pane_id: pid });
            let reporting = self.pane_reports_mouse(pid);
            if ev.button == MouseButton::Left && (!reporting || ev.modifiers.shift) {
                // Plain selection: ours, not the pane program's.
                let (col, row) = self.clamp_to_grid(pid, col, row);
                self.sel = Some(Sel { pane_id: pid, start: (col, row), end: (col, row) });
            } else {
                self.sel = None;
                self.send_mouse(WireMouseKind::Down(button), col, row, ev.modifiers);
            }
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        // Finishing a selection copies it (the highlight stays until the next
        // click); the Up is not forwarded, matching the TUI.
        if ev.button == MouseButton::Left {
            if let Some(sel) = self.sel {
                let text = self
                    .panes
                    .get(&sel.pane_id)
                    .map(|g| g.borrow().selection_text(sel.start, sel.end))
                    .unwrap_or_default();
                if !text.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                cx.notify();
                return;
            }
        }
        let button = wire_button(ev.button);
        if let Some((_session, _pid, col, row)) = self.pane_at_pixel(ev.position) {
            self.send_mouse(WireMouseKind::Up(button), col, row, ev.modifiers);
        } else if let Some((col, row)) = self.cell_from_position(ev.position) {
            self.send_mouse(WireMouseKind::Up(button), col, row, ev.modifiers);
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Titlebar drag: the first move after a titlebar press hands the drag
        // to AppKit, so the frameless window follows the pointer.
        if self.titlebar_drag_armed {
            self.titlebar_drag_armed = false;
            crate::window::start_system_move();
            return;
        }
        if let Some(drag) = self.drag {
            if ev.pressed_button == Some(MouseButton::Left) {
                let ratio = self.drag_ratio(&drag, ev.position);
                if let Some(session) = self.active_session().map(|s| s.name.clone()) {
                    let _ = self.send(Command::PaneResizeTo { session, split_id: drag.split_id, ratio });
                }
                return;
            }
        }
        // Extend an active selection with the pointer (not forwarded to the
        // pane program while a client-side selection is in progress).
        if ev.pressed_button == Some(MouseButton::Left) && self.sel.is_some() {
            if let Some((_session, pid, col, row)) = self.pane_at_pixel(ev.position) {
                if self.sel.map(|s| s.pane_id) == Some(pid) {
                    let (col, row) = self.clamp_to_grid(pid, col, row);
                    let changed = self.sel.map(|s| s.end) != Some((col, row));
                    if let Some(sel) = &mut self.sel {
                        sel.end = (col, row);
                    }
                    if changed {
                        cx.notify();
                    }
                    return;
                }
            }
        }
        // Hover highlight for the drag separator (native GPUI indicator).
        let hovered = self.splitter_at_pixel(ev.position).map(|s| s.split_id);
        if hovered != self.hover_splitter {
            self.hover_splitter = hovered;
            cx.notify();
        }
        let kind = match ev.pressed_button {
            Some(b) => WireMouseKind::Drag(wire_button(b)),
            None => WireMouseKind::Moved,
        };
        if let Some((_session, _pid, col, row)) = self.pane_at_pixel(ev.position) {
            self.send_mouse(kind, col, row, ev.modifiers);
        } else if let Some((col, row)) = self.cell_from_position(ev.position) {
            self.send_mouse(kind, col, row, ev.modifiers);
        }
    }

    /// Whether a pane's program switched to the alternate screen (vim, htop).
    fn pane_alt_screen(&self, pid: u64) -> bool {
        self.active_session()
            .and_then(|s| s.root.as_deref())
            .and_then(|r| find_pane(r, pid))
            .map(|p| p.alt_screen)
            .unwrap_or(false)
    }

    fn on_scroll_wheel(&mut self, ev: &ScrollWheelEvent, _: &mut Window, _: &mut Context<Self>) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(p) => p.y * 8.0,
        };
        let up = dy > 0.0;
        if let Some((session, pid, col, row)) = self.pane_at_pixel(ev.position) {
            let _ = self.send(Command::PaneFocus { session, pane_id: pid });
            // Same routing as the TUI: mouse-reporting apps get SGR wheel
            // bytes, alt-screen apps get arrow keys, and plain panes scroll
            // their scrollback viewport.
            if self.pane_reports_mouse(pid) {
                let b = if up { 64 } else { 65 };
                let bytes = sgr_mouse(b, col + 1, row + 1, false);
                let _ = self.send(Command::PaneWrite { pane_id: pid, bytes });
            } else if self.pane_alt_screen(pid) {
                let bytes: Vec<u8> = if up { b"\x1b[A".to_vec() } else { b"\x1b[B".to_vec() };
                let _ = self.send(Command::PaneWrite { pane_id: pid, bytes });
            } else {
                let _ = self.send(Command::PaneScroll { pane_id: pid, up });
            }
        }
    }

    /// Keyboard pane resize (`ctrl+alt+arrow`), nudging the focused pane's
    /// split in `dir` like the TUI's `leader+H/J/K/L`.
    fn resize_focused(&mut self, dir: kumo_protocol::ResizeDir) {
        let Some(session) = self.active_session().map(|s| s.name.clone()) else { return };
        let _ = self.send(Command::PaneResizeRatio { session, dir });
    }

    /// Copy the active selection (if any) to the clipboard.
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(sel) = self.sel {
            let text = self
                .panes
                .get(&sel.pane_id)
                .map(|g| g.borrow().selection_text(sel.start, sel.end))
                .unwrap_or_default();
            if !text.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
        }
    }

    fn send_mouse(&mut self, kind: WireMouseKind, col: u16, row: u16, mods: gpui::Modifiers) {
        let _ = self.send(Command::Mouse {
            event: WireMouseEvent { kind, col, row, modifiers: wire_modifiers(mods) },
        });
    }

    // ------------------------------------------------------------------
    // Status strip
    // ------------------------------------------------------------------

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
            .border_color(theme::hairline())
            .child(StyledText::new(SharedString::from(text)).with_default_highlights(&self.dim, []))
    }

    // ------------------------------------------------------------------
    // Updates (kumo CLI + desktop app)
    // ------------------------------------------------------------------

    /// The status line of what has an update available (empty = nothing).
    fn updates_line(&self) -> Option<String> {
        let mut parts = Vec::new();
        match &self.updates.cli {
            kumo_core::updater::ComponentStatus::OutOfDate { latest, .. } => {
                parts.push(format!("kumo CLI {latest}"));
            }
            kumo_core::updater::ComponentStatus::Missing => {
                parts.push("kumo CLI not installed".to_string());
            }
            _ => {}
        }
        if let kumo_core::updater::ComponentStatus::OutOfDate { latest, .. } = &self.updates.desktop {
            parts.push(format!("Kumo Desktop {latest}"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    /// A slim banner under the titlebar: update availability + one-click
    /// buttons, or a transient line while an install/update runs.
    fn update_banner(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.update_banner.is_empty() {
            return div()
                .w_full()
                .h(px(24.0))
                .flex()
                .items_center()
                .px(px(10.0))
                .gap(px(8.0))
                .border_b_1()
                .border_color(theme::hairline())
                .child(StyledText::new(self.update_banner.clone()).with_default_highlights(&self.dim, []))
                .child(self.banner_dismiss(cx));
        }
        if self.update_banner_dismissed {
            return div();
        }
        let Some(line) = self.updates_line() else {
            return div();
        };
        let mut row = div()
            .w_full()
            .h(px(24.0))
            .flex()
            .items_center()
            .px(px(10.0))
            .gap(px(12.0))
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                StyledText::new(SharedString::from(format!("↑ {line}")))
                    .with_default_highlights(&self.dim, []),
            );
        if self.updating_cli || self.updating_desktop {
            row = row.child(
                div().child("updating…").text_size(px(11.5)).text_color(self.chrome().muted()),
            );
        } else {
            let cli_available = matches!(
                self.updates.cli,
                kumo_core::updater::ComponentStatus::OutOfDate { .. }
                    | kumo_core::updater::ComponentStatus::Missing
            );
            if cli_available {
                row = row.child(self.banner_button(cx, "Update CLI", Self::on_update_cli));
            }
            if matches!(self.updates.desktop, kumo_core::updater::ComponentStatus::OutOfDate { .. }) {
                row = row.child(self.banner_button(cx, "Update Desktop", Self::on_update_desktop));
            }
        }
        row = row.child(self.banner_dismiss(cx));
        row
    }

    fn banner_button(
        &self,
        cx: &mut Context<Self>,
        label: &'static str,
        action: fn(&mut Self, &mut Context<Self>),
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .px(px(8.0))
            .h(px(18.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme::wash(0x0c)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| action(this, cx)))
            .child(label)
            .text_size(px(11.0))
            .text_color(self.chrome().accent())
    }

    fn banner_dismiss(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme::wash(0x0c)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, _w, _cx| {
                this.update_banner_dismissed = true;
                this.update_banner = SharedString::from("");
            }))
            .child("×")
            .text_size(px(12.0))
            .text_color(self.chrome().muted())
    }

    /// Run `kumo update` (via the cargo-dist installer) in the background.
    fn on_update_cli(&mut self, cx: &mut Context<Self>) {
        if self.updating_cli {
            return;
        }
        self.updating_cli = true;
        self.update_banner = SharedString::from("");
        cx.notify();
        let tx = self.update_tx.clone();
        std::thread::spawn(move || {
            let result = kumo_core::updater::update_cli().map_err(|e| format!("{e:#}"));
            let _ = tx.send(UpdateMsg::CliDone(result));
        });
    }

    /// Download the new `.dmg`, replace this app in /Applications and relaunch.
    fn on_update_desktop(&mut self, cx: &mut Context<Self>) {
        if self.updating_desktop {
            return;
        }
        self.updating_desktop = true;
        self.update_banner = SharedString::from("");
        cx.notify();
        let tx = self.update_tx.clone();
        std::thread::spawn(move || {
            let result = kumo_core::updater::update_desktop().map_err(|e| format!("{e:#}"));
            let _ = tx.send(UpdateMsg::DesktopDone(result));
        });
    }
}

impl Render for KumoWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_geometry(window);
        div()
            .flex()
            .flex_col()
            .size_full()
            // No glass fill here — the window's `Blurred` background already
            // composites the frosted desktop. A translucent fill on top would
            // bury the blur under every element.
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(self.titlebar(cx))
            .child(self.update_banner(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .size_full()
                    .min_h(px(0.0))
                    .child(self.sidebar.clone())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_grow()
                            .size_full()
                            .child(self.terminal.clone())
                            .child(self.status_strip()),
                    ),
            )
    }
}

impl KumoWindow {
    fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.chrome();
        let mut wordmark = self.base.clone();
        wordmark.color = chrome.accent();
        wordmark.font_size = px(11.5).into();
        wordmark.font_weight = gpui::FontWeight::BOLD;
        div()
            .w_full()
            .h(px(TITLEBAR_H))
            .flex()
            .items_center()
            .pl(px(78.0))
            .pr(px(12.0))
            .border_b_1()
            .border_color(theme::hairline())
            .window_control_area(gpui::WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _window, _cx| {
                    this.titlebar_drag_armed = true;
                }),
            )
            .child(StyledText::new(SharedString::from("KUMO")).with_default_highlights(&wordmark, []))
    }
}

// ---------------------------------------------------------------------------
// Semantic layout helpers
// ---------------------------------------------------------------------------

/// Recursively lay a semantic tree out over a cell area, mirroring the
/// daemon's own `compute_geometry` (which reserves a 1-cell separator between
/// the two halves of every split), so the client's pane rects exactly match
/// the grids the daemon streams.
fn compute_rects(node: &LayoutNode, area: CellRect) -> Vec<(u64, CellRect)> {
    match node {
        LayoutNode::Pane(p) => vec![(p.id, area)],
        LayoutNode::Split { dir, ratio, a, b, .. } => {
            let mut out = Vec::new();
            let ratio = ratio.clamp(0.01, 0.99);
            match dir {
                SplitDir::Vertical => {
                    if area.width <= 2 {
                        return vec![];
                    }
                    let aw = ((area.width as f32) * ratio).round().clamp(1.0, (area.width - 1) as f32) as u16;
                    out.extend(compute_rects(a, CellRect { x: area.x, y: area.y, width: aw, height: area.height }));
                    out.extend(compute_rects(b, CellRect { x: area.x + aw + 1, y: area.y, width: area.width - aw - 1, height: area.height }));
                }
                SplitDir::Horizontal => {
                    if area.height <= 2 {
                        return vec![];
                    }
                    let ah = ((area.height as f32) * ratio).round().clamp(1.0, (area.height - 1) as f32) as u16;
                    out.extend(compute_rects(a, CellRect { x: area.x, y: area.y, width: area.width, height: ah }));
                    out.extend(compute_rects(b, CellRect { x: area.x, y: area.y + ah + 1, width: area.width, height: area.height - ah - 1 }));
                }
            }
            out
        }
    }
}

/// Collect each split's divider strip (in cell coords), so mouse drags can
/// target a split for an absolute ratio resize.
fn compute_splitters(node: &LayoutNode, area: CellRect, out: &mut Vec<SplitGeom>) {
    match node {
        LayoutNode::Pane(_) => {}
        LayoutNode::Split { id, dir, ratio, a, b } => {
            let ratio = ratio.clamp(0.01, 0.99);
            let (strip, ra, rb) = match dir {
                SplitDir::Vertical => {
                    let aw = ((area.width as f32) * ratio).round().clamp(1.0, (area.width - 1) as f32) as u16;
                    // The visual divider spans the separator cell plus the
                    // pixel gap on either side (~3 cells wide).
                    let strip = CellRect {
                        x: area.x + aw.saturating_sub(1),
                        y: area.y,
                        width: 3,
                        height: area.height,
                    };
                    let ra = CellRect { x: area.x, y: area.y, width: aw, height: area.height };
                    let rb = CellRect { x: area.x + aw + 1, y: area.y, width: area.width - aw - 1, height: area.height };
                    (strip, ra, rb)
                }
                SplitDir::Horizontal => {
                    let ah = ((area.height as f32) * ratio).round().clamp(1.0, (area.height - 1) as f32) as u16;
                    let strip = CellRect {
                        x: area.x,
                        y: area.y + ah.saturating_sub(1),
                        width: area.width,
                        height: 3,
                    };
                    let ra = CellRect { x: area.x, y: area.y, width: area.width, height: ah };
                    let rb = CellRect { x: area.x, y: area.y + ah + 1, width: area.width, height: area.height - ah - 1 };
                    (strip, ra, rb)
                }
            };
            out.push(SplitGeom { split_id: *id, dir: *dir, area, strip });
            compute_splitters(a, ra, out);
            compute_splitters(b, rb, out);
        }
    }
}

/// Pixel card bounds + per-pane cell metrics for a pane rect.
pub(crate) fn pane_metrics(model: &KumoWindow, r: &CellRect) -> PaneMetrics {
    let x = model.canvas_origin.x + px(r.x as f32 * model.cell_w);
    let y = model.canvas_origin.y + px(r.y as f32 * model.cell_h);
    let w = px((r.width as f32 * model.cell_w).max(1.0));
    let h = px((r.height as f32 * model.cell_h).max(1.0));
    let content_w = (f32::from(w) - 2.0 * panes::PANE_GAP).max(1.0);
    let content_h = (f32::from(h) - 2.0 * panes::PANE_GAP).max(1.0);
    let cw = content_w / r.width.max(1) as f32;
    let ch = content_h / r.height.max(1) as f32;
    PaneMetrics {
        x,
        y,
        w,
        h,
        cell_w: cw,
        cell_h: ch,
        font_size: (ch / model.line_height_ratio).clamp(6.0, 34.0),
        content_x: x + px(panes::PANE_GAP),
        content_y: y + px(panes::PANE_GAP),
    }
}

pub(crate) fn cell_from_position(model: &KumoWindow, pos: Point<Pixels>) -> Option<(u16, u16)> {
    let gx = f32::from(pos.x) - f32::from(model.canvas_origin.x);
    let gy = f32::from(pos.y) - f32::from(model.canvas_origin.y);
    if gx < 0.0 || gy < 0.0 {
        return None;
    }
    Some(((gx / model.cell_w).min(u16::MAX as f32) as u16, (gy / model.cell_h).min(u16::MAX as f32) as u16))
}

pub(crate) fn find_pane(node: &LayoutNode, pid: u64) -> Option<&kumo_protocol::LayoutPane> {
    match node {
        LayoutNode::Pane(p) if p.id == pid => Some(p),
        LayoutNode::Split { a, b, .. } => {
            find_pane(a, pid).or_else(|| find_pane(b, pid))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wire_button(b: MouseButton) -> WireMouseButton {
    match b {
        MouseButton::Left => WireMouseButton::Left,
        MouseButton::Right => WireMouseButton::Right,
        _ => WireMouseButton::Middle,
    }
}

/// SGR (xterm 1006) mouse sequence — a local copy of the CLI client's helper,
/// used to forward wheel events to mouse-reporting programs.
fn sgr_mouse(button: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let b = if release { button | 3 } else { button };
    format!("\x1b[<{b};{col};{row}{}", if release { "m" } else { "M" }).into_bytes()
}

fn wire_modifiers(m: gpui::Modifiers) -> WireModifiers {
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

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Frameless vibrancy: the native title bar is hidden and the
                    // content extends to the top of the window, with the blurred
                    // desktop showing through behind the translucent chrome.
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("kumo".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    // Blurred desktop behind the translucent backdrop: the
                    // gaps between panes let the workspace show through.
                    window_background: WindowBackgroundAppearance::Blurred,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| KumoWindow::new(window, cx)),
            )
            .unwrap();
        let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.observe_keystrokes(move |ev, _, cx| {
            let ks = ev.keystroke.clone();
            if ks.key == "q" && ks.modifiers.platform {
                cx.quit();
                return;
            }
            if ks.key == "c" && ks.modifiers.platform {
                view.update(cx, |this, cx| this.copy_selection(cx));
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
            // `ctrl+alt+arrow` resizes the focused pane's split (mouse drag on
            // the divider works too).
            if ks.modifiers.control && ks.modifiers.alt {
                let dir = match ks.key.as_str() {
                    "left" => Some(kumo_protocol::ResizeDir::Left),
                    "right" => Some(kumo_protocol::ResizeDir::Right),
                    "up" => Some(kumo_protocol::ResizeDir::Up),
                    "down" => Some(kumo_protocol::ResizeDir::Down),
                    _ => None,
                };
                if let Some(dir) = dir {
                    view.update(cx, move |this, _cx| this.resize_focused(dir));
                    return;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: u64) -> LayoutNode {
        LayoutNode::Pane(kumo_protocol::LayoutPane {
            id,
            title: String::new(),
            cwd: std::path::PathBuf::from("/tmp"),
            is_ai: false,
            agent: None,
            mouse_reporting: false,
            alt_screen: false,
        })
    }

    #[test]
    fn rects_match_daemon_separator_geometry() {
        let root = LayoutNode::Split {
            id: 1,
            dir: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(pane(11)),
            b: Box::new(pane(12)),
        };
        let area = CellRect { x: 0, y: 0, width: 40, height: 20 };
        let rects = compute_rects(&root, area);
        // a gets 20 cols; b gets the rest minus the 1 separator column.
        assert_eq!(
            rects,
            vec![
                (11, CellRect { x: 0, y: 0, width: 20, height: 20 }),
                (12, CellRect { x: 21, y: 0, width: 19, height: 20 }),
            ]
        );
    }

    #[test]
    fn splitters_strip_covers_the_divider() {
        let root = LayoutNode::Split {
            id: 7,
            dir: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(pane(11)),
            b: Box::new(pane(12)),
        };
        let mut out = Vec::new();
        compute_splitters(&root, CellRect { x: 0, y: 0, width: 40, height: 20 }, &mut out);
        assert_eq!(out.len(), 1);
        let sg = out[0];
        assert_eq!(sg.split_id, 7);
        assert_eq!(sg.area, CellRect { x: 0, y: 0, width: 40, height: 20 });
        // The strip spans the separator column (20) plus the gap around it.
        assert_eq!(sg.strip.x, 19);
        assert_eq!(sg.strip.width, 3);
        assert_eq!(sg.strip.height, 20);
    }
}
