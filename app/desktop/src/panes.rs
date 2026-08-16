//! Terminal viewports: `TerminalPane` + the raw-GPU `PaneCanvas`.
//!
//! Each pane renders the `PaneFrame` grid Ghostty produces (painted directly
//! with `window.paint_*`, cell backgrounds included), sitting on the frosted
//! window glass. A neon accent ring marks the focused pane, and the drag
//! separators glow on hover with a short fade-in.

use gpui::{
    div, point, px, quad, size, App, Bounds, Element, ElementId, Font,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, Render,
    SharedString, Style, TextRun, WeakEntity, Window, fill, prelude::*,
};

use kumo_protocol::SplitDir;

use crate::theme::{self, corners, Chrome};
use crate::{KumoWindow, pane_metrics};

/// Pixel gap kept around every pane card (and between adjacent panes).
pub(crate) const PANE_GAP: f32 = 8.0;

/// A rectangle in cell coordinates (client-computed from the semantic tree).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CellRect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

/// A split's divider strip (in cell coords), where mouse drags resize it.
#[derive(Clone, Copy)]
pub(crate) struct SplitGeom {
    pub(crate) split_id: u64,
    pub(crate) dir: SplitDir,
    pub(crate) area: CellRect,
    pub(crate) strip: CellRect,
}

/// An active text selection in one pane (cell coords, both corners inclusive).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sel {
    pub(crate) pane_id: u64,
    pub(crate) start: (u16, u16),
    pub(crate) end: (u16, u16),
}

/// A selection normalized so `start <= end` (row-major), ready for painting.
pub(crate) fn normalize_sel(sel: Sel) -> ((u16, u16), (u16, u16)) {
    let (mut r0, mut c0, mut r1, mut c1) = (sel.start.1, sel.start.0, sel.end.1, sel.end.0);
    if r1 < r0 || (r1 == r0 && c1 < c0) {
        std::mem::swap(&mut r0, &mut r1);
        std::mem::swap(&mut c0, &mut c1);
    }
    ((r0, c0), (r1, c1))
}

/// An in-flight divider drag.
#[derive(Clone, Copy)]
pub(crate) struct SplitDrag {
    pub(crate) split_id: u64,
    pub(crate) dir: SplitDir,
    pub(crate) area: CellRect,
}

/// Pixel card bounds + per-pane cell metrics for one pane rect.
#[derive(Clone, Copy)]
pub(crate) struct PaneMetrics {
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) w: Pixels,
    pub(crate) h: Pixels,
    pub(crate) cell_w: f32,
    pub(crate) cell_h: f32,
    pub(crate) font_size: f32,
    pub(crate) content_x: Pixels,
    pub(crate) content_y: Pixels,
}

pub struct TerminalPane {
    parent: WeakEntity<KumoWindow>,
}

impl TerminalPane {
    pub fn new(parent: WeakEntity<KumoWindow>) -> Self {
        Self { parent }
    }
}

impl Render for TerminalPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let parent = self.parent.upgrade().expect("terminal pane outlives its window");
        div()
            .flex()
            .items_start()
            .justify_start()
            .flex_grow()
            .size_full()
            .child(PaneCanvas { view: parent })
    }
}

// ---------------------------------------------------------------------------
// Canvas Data Structures
// ---------------------------------------------------------------------------

struct CanvasPane {
    pid: u64,
    /// 1-based pane number, shown while the pane-number overlay is up.
    num: Option<u8>,
    grid: Option<std::rc::Rc<std::cell::RefCell<crate::grid::Grid>>>,
    m: PaneMetrics,
}

pub(crate) struct CanvasData {
    font: Font,
    default_fg: Hsla,
    chrome: Chrome,
    panes: Vec<CanvasPane>,
    splitters: Vec<SplitGeom>,
    hover_splitter: Option<u64>,
    drag_splitter: Option<u64>,
    canvas_origin: Point<Pixels>,
    cell_w: f32,
    cell_h: f32,
    sel: Option<Sel>,
    cursor_on: bool,
    focused_pid: Option<u64>,
    /// Splitter hover glow intensity (0..1, faded in over ~140 ms).
    splitter_glow: f32,
}

pub(crate) struct PaneCanvas {
    view: gpui::Entity<KumoWindow>,
}

impl PaneCanvas {
    fn extract(&self, cx: &App) -> CanvasData {
        let model = self.view.read(cx);
        let mut panes = Vec::with_capacity(model.rects.len());

        if let Some(_session) = model.active_session() {
            for (i, (pid, r)) in model.rects.iter().enumerate() {
                let grid = model.panes.get(pid).cloned();

                panes.push(CanvasPane {
                    pid: *pid,
                    num: model.pane_numbers.map(|_| (i + 1) as u8),
                    grid,
                    m: pane_metrics(model, r),
                });
            }
        }

        CanvasData {
            font: model.font.clone(),
            default_fg: model.default_fg,
            chrome: model.chrome(),
            panes,
            splitters: model.splitters.clone(),
            hover_splitter: model.hover_splitter,
            drag_splitter: model.drag.map(|d| d.split_id),
            canvas_origin: model.canvas_origin,
            cell_w: model.cell_w,
            cell_h: model.cell_h,
            sel: model.sel,
            cursor_on: model.cursor_on,
            focused_pid: model.active_session().map(|s| s.focus),
            splitter_glow: model
                .hover_since
                .map(|t| {
                    let ms = t.elapsed().as_millis() as f32;
                    (ms / 140.0).clamp(0.0, 1.0)
                })
                .unwrap_or(0.0),
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

// ---------------------------------------------------------------------------
// Rendering pipeline
// ---------------------------------------------------------------------------

fn paint_canvas(data: &CanvasData, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let chrome = data.chrome;

    if data.panes.is_empty() {
        paint_empty_state(data, bounds, window, cx);
        return;
    }

    for pane in &data.panes {
        let m = pane.m;
        let font_size = px(m.font_size);
        let line_h = px(m.cell_h);

        if let Some(grid) = &pane.grid {
            let mut grid = grid.borrow_mut();
            let sel = data.sel.filter(|s| s.pane_id == pane.pid);
            let sel_range = sel.map(normalize_sel);
            let sel_wash = chrome.accent().with_a(0.25);
            let grid_cols = grid.cols();
            for row in 0..grid.rows() {
                if let Some(art) = grid.row_art_cached(row, &data.font, data.default_fg, chrome.card()) {
                    let row_y = m.content_y + px(row as f32 * m.cell_h);
                    // Cell backgrounds first (merged spans), then the glyphs on top.
                    for span in &art.bg {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(m.content_x + px(span.x * m.cell_w), row_y),
                                size(px(span.w * m.cell_w), px(m.cell_h)),
                            ),
                            span.color,
                        ));
                    }
                    // Selection tint sits under the glyphs, over the backgrounds.
                    if let Some(((r0, c0), (r1, c1))) = sel_range {
                        if row >= r0 && row <= r1 {
                            let cols = grid_cols;
                            let sx = if row == r0 { c0 } else { 0 };
                            let ex = if row == r1 { c1.saturating_add(1) } else { cols };
                            if ex > sx {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(m.content_x + px(sx as f32 * m.cell_w), row_y),
                                        size(px((ex - sx) as f32 * m.cell_w), px(m.cell_h)),
                                    ),
                                    sel_wash,
                                ));
                            }
                        }
                    }
                    let line = window
                        .text_system()
                        .shape_line(SharedString::from(art.text.clone()), font_size, &art.runs, None);
                    let origin = point(m.content_x, row_y);
                    let _ = line.paint(origin, line_h, window, cx);
                }
            }

            // The focused pane's cursor blinks; unfocused panes stay steady.
            let cursor_visible = data.focused_pid != Some(pane.pid) || data.cursor_on;
            if let Some((ccx, ccy)) = grid.cursor().filter(|_| cursor_visible) {
                let cw = px(m.cell_w);
                let cursor_y = m.content_y + px(ccy as f32 * m.cell_h) + px(m.cell_h) - px(1.5);
                window.paint_quad(fill(
                    Bounds::new(
                        point(m.content_x + px(ccx as f32 * m.cell_w), cursor_y),
                        size(cw, px(1.5)),
                    ),
                    chrome.accent(),
                ));
            }

            // Scrollback scrollbar: a hairline track with an accent thumb,
            // only when there is scrollback beyond the viewport.
            if let Some(scroll) = grid.scroll() {
                if scroll.total > scroll.screen && scroll.screen > 0 {
                    let track_h = f32::from(m.h) - 2.0 * PANE_GAP;
                    let thumb_h = ((scroll.screen as f32 / scroll.total as f32) * track_h).clamp(12.0, track_h);
                    let max_offset = (scroll.total - scroll.screen).max(1);
                    let travel = (track_h - thumb_h).max(0.0);
                    let thumb_y = m.content_y + px((scroll.offset as f32 / max_offset as f32) * travel);
                    let x = m.x + m.w - px(4.0);
                    window.paint_quad(fill(
                        Bounds::new(point(x, m.content_y), size(px(2.0), px(track_h))),
                        theme::hairline(),
                    ));
                    window.paint_quad(fill(
                        Bounds::new(point(x, thumb_y), size(px(2.0), px(thumb_h))),
                        chrome.accent().with_a(0.55),
                    ));
                }
            }
        }
    }

    paint_focus_ring(data, window);
    paint_splitter_highlights(data, window);
    paint_pane_numbers(data, window, cx);
}

/// A neon hairline around the focused pane's card — the accent tone of the
/// active theme, rounded to match the chrome.
fn paint_focus_ring(data: &CanvasData, window: &mut Window) {
    let Some(focus) = data.focused_pid else { return };
    let Some(pane) = data.panes.iter().find(|p| p.pid == focus) else { return };
    let m = pane.m;
    let bounds = Bounds::new(point(m.x - px(2.0), m.y - px(2.0)), size(m.w + px(4.0), m.h + px(4.0)));
    window.paint_quad(quad(
        bounds,
        corners(10.0),
        gpui::transparent_black(),
        px(1.5),
        data.chrome.accent().with_a(0.85),
        gpui::BorderStyle::Solid,
    ));
}

/// While the pane-number overlay is up, badge every pane with its 1-based
/// number (matching the digit that jumps to it).
fn paint_pane_numbers(data: &CanvasData, window: &mut Window, cx: &mut App) {
    for pane in &data.panes {
        let Some(num) = pane.num else { continue };
        let m = pane.m;
        let text = SharedString::from(num.to_string());
        let font_size = px(m.font_size);
        let line = window.text_system().shape_line(
            text.clone(),
            font_size,
            &[TextRun {
                len: text.len(),
                font: data.font.clone(),
                color: data.chrome.accent(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let chip_w = line.width + px(10.0);
        let origin = point(m.x + px(6.0), m.y + px(6.0));
        window.paint_quad(fill(
            Bounds::new(origin, size(chip_w, px(m.cell_h + 6.0))),
            theme::wash(0x30),
        ));
        let _ = line.paint(point(origin.x + px(5.0), origin.y + px(3.0)), px(m.cell_h + 6.0), window, cx);
    }
}

fn paint_empty_state(data: &CanvasData, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let text = SharedString::from("KUMO  ·  no session — it will open one automatically");
    let len = text.len();
    let font_size = px(13.0);
    let line = window.text_system().shape_line(
        text,
        font_size,
        &[TextRun {
            len,
            font: data.font.clone(),
            color: data.chrome.accent().with_a(0.7),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let origin = point(
        bounds.center().x - px(f32::from(line.width) / 2.0),
        bounds.center().y - px(7.0),
    );
    let _ = line.paint(origin, px(14.0), window, cx);
}

fn paint_splitter_highlights(data: &CanvasData, window: &mut Window) {
    let active = data.drag_splitter.or(data.hover_splitter);
    let Some(active_id) = active else { return };

    for split in &data.splitters {
        if split.split_id != active_id {
            continue;
        }

        let chrome = data.chrome;
        let strip = &split.strip;
        let origin_x = data.canvas_origin.x + px(strip.x as f32 * data.cell_w);
        let origin_y = data.canvas_origin.y + px(strip.y as f32 * data.cell_h);
        // The glow fades in on hover and snaps to full while dragging.
        let intensity = if data.drag_splitter == Some(active_id) {
            1.0
        } else {
            data.splitter_glow
        };
        let glow_color = chrome.accent().with_a(0.18 * intensity);
        let line_color = chrome.accent().with_a(0.55 + 0.45 * intensity);

        let (glow_bounds, line_bounds) = match split.dir {
            SplitDir::Vertical => {
                let h = px(strip.height as f32 * data.cell_h);
                (
                    Bounds::new(point(origin_x - px(10.0), origin_y), size(px(22.0), h)),
                    Bounds::new(point(origin_x + px(11.0), origin_y), size(px(2.0), h)),
                )
            }
            SplitDir::Horizontal => {
                let w = px(strip.width as f32 * data.cell_w);
                (
                    Bounds::new(point(origin_x, origin_y - px(10.0)), size(w, px(22.0))),
                    Bounds::new(point(origin_x, origin_y + px(11.0)), size(w, px(2.0))),
                )
            }
        };

        window.paint_quad(fill(glow_bounds, glow_color));
        window.paint_quad(fill(line_bounds, line_color));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

trait WithAlpha {
    fn with_a(self, a: f32) -> Self;
}

impl WithAlpha for Hsla {
    fn with_a(mut self, a: f32) -> Self {
        self.a = a;
        self
    }
}
