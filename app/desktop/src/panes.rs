//! Terminal viewports: `TerminalPane` + the raw-GPU `PaneCanvas`.
//!
//! Each pane card renders the `PaneFrame` grid Ghostty produces (painted
//! directly with `window.paint_*`, not ANSI chrome), framed by native GPUI
//! chrome: rounded cards with soft shadows, a hairline border that lights up in
//! the neon accent for the focused pane, and hover-glow drag separators.

use gpui::{
    div, point, px, size, App, Bounds, BoxShadow, Corners, Edges, Element, ElementId, Font,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, Render,
    SharedString, Style, TextRun, WeakEntity, Window, fill, quad, BorderStyle, prelude::*,
};

use kumo_protocol::{AgentStatus, SplitDir};

use crate::theme::{self, Chrome};
use crate::{KumoWindow, find_pane, pane_metrics};

/// Pixel gap kept around every pane card (and between adjacent panes).
pub(crate) const PANE_GAP: f32 = 12.0;
/// Height reserved at the top of each card for the title pill.
pub(crate) const TITLE_H: f32 = 20.0;
/// Corner radius of pane cards.
pub(crate) const CORNER_RADIUS: f32 = 10.0;

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
            .items_center()
            .justify_center()
            .flex_grow()
            .size_full()
            .child(PaneCanvas { view: parent })
    }
}

// ---------------------------------------------------------------------------
// Canvas Data Structures
// ---------------------------------------------------------------------------

struct CanvasPane {
    focused: bool,
    title: String,
    agent: Option<(String, AgentStatus)>,
    grid: Option<crate::grid::Grid>,
    m: PaneMetrics,
}

pub(crate) struct CanvasData {
    font: Font,
    default_fg: Hsla,
    chrome: &'static Chrome,
    panes: Vec<CanvasPane>,
    splitters: Vec<SplitGeom>,
    hover_splitter: Option<u64>,
    drag_splitter: Option<u64>,
    canvas_origin: Point<Pixels>,
    cell_w: f32,
    cell_h: f32,
}

pub(crate) struct PaneCanvas {
    view: gpui::Entity<KumoWindow>,
}

impl PaneCanvas {
    fn extract(&self, cx: &App) -> CanvasData {
        let model = self.view.read(cx);
        let mut panes = Vec::with_capacity(model.rects.len());

        if let Some(session) = model.active_session() {
            let focus = session.focus;
            for (pid, r) in &model.rects {
                let info = session.root.as_deref().and_then(|root| find_pane(root, *pid));
                let grid = model.panes.get(pid).cloned();
                
                let title = match info {
                    Some(p) if !p.title.trim().is_empty() => p.title.trim().to_string(),
                    _ => format!("pane {pid}"),
                };

                let agent = info.and_then(|p| p.agent.as_ref()).map(|a| (a.name.clone(), a.status));

                panes.push(CanvasPane {
                    focused: focus == *pid,
                    title,
                    agent,
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
    // Frost: the whole canvas sits on the translucent glass scrim.
    window.paint_quad(fill(bounds, chrome.glass()));

    if data.panes.is_empty() {
        paint_empty_state(data, bounds, window, cx);
        return;
    }

    for pane in &data.panes {
        let m = pane.m;
        let card = Bounds::new(point(m.x, m.y), size(m.w, m.h));
        let corners = theme::corners(CORNER_RADIUS);

        // Soft drop shadow so the cards float over the translucent backdrop;
        // the focused card gets an accent-tinted glow on top.
        let mut shadows = theme::card_shadow();
        if pane.focused {
            shadows.push(BoxShadow {
                color: chrome.accent().with_a(0.30),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(22.0),
                spread_radius: px(0.0),
            });
        }
        window.paint_shadows(card, corners, &shadows);

        let border_color = if pane.focused { chrome.accent() } else { theme::hairline() };
        let border_width = if pane.focused { 1.5 } else { 1.0 };

        window.paint_quad(quad(
            card,
            corners,
            chrome.card(),
            Edges::all(px(border_width)),
            border_color,
            BorderStyle::Solid,
        ));

        let font_size = px(m.font_size);
        let line_h = px(m.cell_h);

        if let Some(grid) = &pane.grid {
            for row in 0..grid.rows() {
                let cells = grid.row(row).unwrap_or_default();
                let (text, runs) = crate::grid::row_runs(cells, &data.font, data.default_fg, chrome.card());
                let line = window
                    .text_system()
                    .shape_line(SharedString::from(text), font_size, &runs, None);
                let origin = point(m.content_x, m.content_y + px(row as f32 * m.cell_h));
                let _ = line.paint_background(origin, line_h, window, cx);
                let _ = line.paint(origin, line_h, window, cx);
            }

            if let Some((ccx, ccy)) = grid.cursor() {
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
        }

        paint_title_chip(data, pane, m.content_x, m.y + px(PANE_GAP), window, cx);
    }

    paint_splitter_highlights(data, window);
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
        let glow_color = chrome.accent().with_a(0.18);

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
        window.paint_quad(fill(line_bounds, chrome.accent()));
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
    let chrome = data.chrome;
    let mut text = String::with_capacity(32 + pane.title.len());
    text.push(' ');
    text.push_str(&pane.title);
    
    let title_len = text.len();
    let mut runs = Vec::with_capacity(2);
    runs.push(TextRun {
        len: title_len,
        font: data.font.clone(),
        color: chrome.text(),
        background_color: None,
        underline: None,
        strikethrough: None,
    });

    if let Some((name, status)) = &pane.agent {
        let start_len = text.len();
        text.push_str("  ");
        text.push_str(name);
        text.push_str(" · ");
        text.push_str(status.label());

        runs.push(TextRun {
            len: text.len() - start_len,
            font: data.font.clone(),
            color: chrome.status(*status),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    let font_size = px(11.0);
    let line = window.text_system().shape_line(SharedString::from(text), font_size, &runs, None);

    let pill_w = px(f32::from(line.width) + 20.0);
    let pill_h = px(TITLE_H);
    let pill = Bounds::new(point(x, y), size(pill_w, pill_h));

    let pill_bg: Hsla = if pane.focused {
        chrome.accent().with_a(0.16)
    } else {
        chrome.surface_raised().with_a(0.9)
    };

    let border_color = if pane.focused { chrome.accent() } else { theme::hairline() };

    window.paint_quad(quad(
        pill,
        Corners::all(px(TITLE_H / 2.0)),
        pill_bg,
        Edges::all(px(1.0)),
        border_color,
        BorderStyle::Solid,
    ));

    let origin = point(x + px(10.0), y + px(2.0));
    let _ = line.paint(origin, pill_h, window, cx);
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
