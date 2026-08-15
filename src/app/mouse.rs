use anyhow::Result;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use std::time::Instant;

use super::overlays::{CtxTarget, PopupBtn};
use super::util::{copy_to_clipboard, open_url};
use super::App;
use crate::layout::SplitDir;
use crate::pane::sgr_mouse;

pub(super) enum Drag {
    Splitter { split_id: u64 },
}

/// Mouse text selection inside a pane (viewport-relative coordinates).
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Sel {
    pane_id: u64,
    start: (u16, u16),
    end: (u16, u16),
}

/// A left press in a mouse-reporting pane. The pane owns the mouse: kumo
/// forwards the whole gesture (press on down, drags on move, release on up) to
/// it so the app can do its own text selection.
#[derive(Clone, Copy)]
pub(super) struct PendingClick {
    pane_id: u64,
    col: u16,
    row: u16,
}

impl App {
    pub(super) fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        // Track whether a link modifier (Cmd/Ctrl/Option) is held, so link
        // underlines appear/disappear as the user presses and releases it.
        self.set_link_mods(m.modifiers.intersects(super::link_modifiers()));
        let x = m.column;
        let y = m.row;
        if m.kind == MouseEventKind::Down(MouseButton::Left) && self.update_notice_close_at(x, y) {
            if let Some(notice) = &self.update_notice {
                crate::update::dismiss(&notice.key);
            }
            self.update_notice = None;
            return Ok(());
        }
        if self.keybind_overlay.open {
            // Modal showcase: a click or scroll dismisses it; hover is ignored
            // so moving the mouse over it doesn't close it. `Up` is deliberately
            // excluded: the release of the click that opened it (e.g. the MENU
            // dropdown) arrives after the overlay is already open and would
            // close it instantly.
            if matches!(
                m.kind,
                MouseEventKind::Down(_) | MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
            ) {
                self.keybind_overlay.open = false;
            }
            return Ok(());
        }
        if self.settings.open {
            // Modal settings panel: a click on a tab switches to it, a click on
            // a theme applies it live (the panel stays open), a click outside
            // cancels; hovering moves the selection.
        if self.worktree_picker.open {
            // Modal picker: a click on a row opens it, a click outside cancels,
            // hovering and scrolling move the selection.
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(i) = self.worktree_picker_item_at(x, y) {
                        self.worktree_picker.selected = i;
                        self.pick_worktree(i);
                    } else if self
                        .worktree_picker_rect()
                        .map(|r| r.contains(Position::new(x, y)))
                        .unwrap_or(false)
                    {
                        // Inside the picker but off a row: modal no-op.
                    } else {
                        self.worktree_picker.open = false;
                    }
                    return Ok(());
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.worktree_picker_item_at(x, y) {
                        self.worktree_picker.selected = i;
                    }
                    return Ok(());
                }
                MouseEventKind::ScrollDown => self.worktree_picker_move(1),
                MouseEventKind::ScrollUp => self.worktree_picker_move(-1),
                _ => return Ok(()),
            }
        }
        match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(tab) = self.settings_tab_at(x, y) {
                        self.settings_set_tab(tab);
                        return Ok(());
                    }
                    if let Some(i) = self.settings_item_at(x, y) {
                        self.settings.selected = i;
                        self.select_theme(i);
                        return Ok(());
                    }
                    if self
                        .settings_rect()
                        .map(|r| r.contains(Position::new(x, y)))
                        .unwrap_or(false)
                    {
                        return Ok(());
                    }
                    self.settings.open = false;
                    return Ok(());
                }
                MouseEventKind::Moved => {
                    if let Some(i) = self.settings_item_at(x, y) {
                        self.settings.selected = i;
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.popup.open {
                    // Buttons confirm/cancel; clicks on the popup itself are
                    // modal (no-op); outside cancels.
                    if let Some(btn) = self.name_popup_button_at(x, y) {
                        match btn {
                            PopupBtn::Enter => self.commit_name(),
                            PopupBtn::Cancel => self.popup.open = false,
                        }
                        return Ok(());
                    }
                    if self
                        .name_popup_rect()
                        .map(|r| r.contains(Position::new(x, y)))
                        .unwrap_or(false)
                    {
                        return Ok(());
                    }
                    self.popup.open = false;
                }
                if self.menu.open {
                    if let Some(i) = self.menu_item_at(x, y) {
                        self.menu_select(i)?;
                        return Ok(());
                    }
                    if self.menu_btn_at(x, y) {
                        self.menu.open = false;
                        return Ok(());
                    }
                    self.menu.open = false;
                }
                if self.ctx_menu.open {
                    if let Some(i) = self.ctx_menu_item_at(x, y) {
                        self.ctx_menu_select(i)?;
                        return Ok(());
                    }
                    if self.ctx_menu_at(x, y) {
                        return Ok(());
                    }
                    self.ctx_menu.open = false;
                }
                if self.menu_btn_at(x, y) {
                    self.menu.open = !self.menu.open;
                    self.menu.selected = 0;
                    return Ok(());
                }
                if self.sidebar_open && x < self.sidebar_width && self.sidebar_hit(x, y) {
                    return Ok(());
                }
                if let Some(sg) = self.splitter_at(x, y) {
                    self.drag = Some(Drag::Splitter { split_id: sg.split_id });
                    return Ok(());
                }
                // Cmd+click (or Ctrl/Option+click) on a link opens it: an OSC 8
                // hyperlink, or a plain-text `scheme://` URL detected on the
                // row (like a normal terminal, e.g. `next dev` output).
                // Mirroring Ghostty's `ctrl_or_super+click`, this runs before
                // the selection/mouse-forwarding logic so it works even in
                // panes that own the mouse (e.g. opencode).
                if m.modifiers.intersects(super::link_modifiers()) {
                    if let Some(pg) = self.pane_at(x, y) {
                        let inner = pg.inner();
                        let col = x.saturating_sub(inner.x);
                        let row = y.saturating_sub(inner.y);
                        if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                            if let Some(url) = pane.link_at(col, row) {
                                self.set_focus(pg.pane_id);
                                open_url(&url);
                                return Ok(());
                            }
                        }
                    }
                }
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    let inner = pg.inner();
                    let col = x.saturating_sub(inner.x);
                    let row = y.saturating_sub(inner.y);
                    let reporting = self
                        .panes
                        .get(&pg.pane_id)
                        .map(|p| p.has_mouse_reporting())
                        .unwrap_or(false);
                    if reporting {
                        // The pane owns the mouse: forward the full gesture to
                        // it (press now, drags while held, release on up) so the
                        // app can do its own text selection instead of kumo
                        // drawing a grid selection over its cells. Mirrors herdr.
                        self.pending_click = Some(PendingClick { pane_id: pg.pane_id, col, row });
                        let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                        if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                            pane.write(&sgr_mouse(b, col + 1, row + 1, false));
                        }
                    } else {
                        // A new drag replaces the previous (still-highlighted)
                        // selection: clear it on its pane before starting fresh.
                        if let Some(old) = self.sel {
                            if let Some(pane) = self.panes.get_mut(&old.pane_id) {
                                pane.clear_selection();
                            }
                        }
                        self.sel = Some(Sel {
                            pane_id: pg.pane_id,
                            start: (col, row),
                            end: (col, row),
                        });
                        if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                            pane.set_selection((col, row), (col, row));
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click toggles the context menu: click it again (or
                // anywhere outside a pane/session row) to close; click a pane
                // or a sidebar session row to open it for that target.
                if self.popup.open || self.menu.open {
                    return Ok(());
                }
                if self.ctx_menu_at(x, y) {
                    self.ctx_menu.open = false;
                    return Ok(());
                }
                if let Some(i) = self.sidebar_session_at(x, y) {
                    self.open_ctx_menu(x, y, CtxTarget::Session(i));
                    return Ok(());
                }
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    self.open_ctx_menu(x, y, CtxTarget::Pane(pg.pane_id));
                } else {
                    self.ctx_menu.open = false;
                }
                return Ok(());
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(Drag::Splitter { split_id }) = self.drag {
                    let geom = self.active_geom();
                    if let Some(sg) = geom.splitters.iter().find(|s| s.split_id == split_id) {
                        let ratio = match sg.dir {
                            SplitDir::V => {
                                (x.saturating_sub(sg.area.x)) as f32 / (sg.area.width - 1) as f32
                            }
                            SplitDir::H => {
                                (y.saturating_sub(sg.area.y)) as f32 / (sg.area.height - 1) as f32
                            }
                        };
                        self.sessions[self.active].tree.set_ratio(split_id, ratio);
                    }
                    return Ok(());
                }
                let sel = self.sel;
                if let Some(sel) = sel {
                    if let Some(pg) = self.pane_at(x, y) {
                        if pg.pane_id == sel.pane_id {
                            let inner = pg.inner();
                            let c = x
                                .saturating_sub(inner.x)
                                .min(inner.width.saturating_sub(1));
                            let r = y
                                .saturating_sub(inner.y)
                                .min(inner.height.saturating_sub(1));
                            self.sel.as_mut().unwrap().end = (c, r);
                            if let Some(pane) = self.panes.get_mut(&sel.pane_id) {
                                pane.set_selection(sel.start, (c, r));
                            }
                        }
                    }
                    return Ok(());
                }
                // A press in a mouse-reporting pane forwards its drags to the
                // pane so the app (e.g. opencode) does its own text selection.
                if let Some(pc) = self.pending_click {
                    let pos = self
                        .pane_at(x, y)
                        .filter(|pg| pg.pane_id == pc.pane_id)
                        .map(|pg| {
                            let i = pg.inner();
                            let c = x.saturating_sub(i.x).min(i.width.saturating_sub(1));
                            let r = y.saturating_sub(i.y).min(i.height.saturating_sub(1));
                            (c + 1, r + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    if let Some(pane) = self.panes.get_mut(&pc.pane_id) {
                        pane.write(&sgr_mouse(b + 32, pos.0, pos.1, false));
                    }
                    return Ok(());
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                if let Some(pc) = self.pending_click.take() {
                    // Release the forwarded gesture back to the app; the press
                    // was already delivered on mouse-down.
                    let b = if m.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 0 };
                    let up = self
                        .pane_at(x, y)
                        .filter(|pg| pg.pane_id == pc.pane_id)
                        .map(|pg| {
                            let i = pg.inner();
                            (x.saturating_sub(i.x) + 1, y.saturating_sub(i.y) + 1)
                        })
                        .unwrap_or((pc.col + 1, pc.row + 1));
                    if let Some(pane) = self.panes.get_mut(&pc.pane_id) {
                        pane.write(&sgr_mouse(b, up.0, up.1, true));
                    }
                } else if let Some(sel) = self.sel {
                    // A plain click without drag copies nothing, like a normal
                    // terminal; only an actual drag copies. The copied selection
                    // stays highlighted (kept in `self.sel`) until the next drag
                    // replaces it or a plain click clears it.
                    if sel.start != sel.end {
                        self.copy_selection(&sel);
                    } else {
                        self.sel = None;
                        if let Some(pane) = self.panes.get_mut(&sel.pane_id) {
                            pane.clear_selection();
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = m.kind == MouseEventKind::ScrollUp;
                // Wheel over the sidebar scrolls its sessions/AGENTS sections.
                if self.sidebar_wheel(x, y, up) {
                    return Ok(());
                }
                if let Some(pg) = self.pane_at(x, y) {
                    self.set_focus(pg.pane_id);
                    let inner = pg.inner();
                    let col = x - inner.x + 1;
                    let row = y - inner.y + 1;
                    if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                        if pane.has_mouse_reporting() {
                            let b = if up { 64 } else { 65 };
                            pane.write(&sgr_mouse(b, col, row, false));
                        } else if pane.in_alt_screen() {
                            pane.write(if up { b"\x1b[A" } else { b"\x1b[B" });
                        } else {
                            pane.scroll(if up { -3 } else { 3 });
                        }
                    }
                }
            }
            MouseEventKind::Moved => {
                if self.popup.open {
                    // Hover highlights a popup button.
                    self.popup.hover = self.name_popup_button_at(x, y);
                    return Ok(());
                }
                if self.menu.open {
                    // Modal menu: hovering moves the selection like j/k; don't
                    // forward motion to the panes underneath.
                    if let Some(i) = self.menu_item_at(x, y) {
                        self.menu.selected = i;
                    }
                    return Ok(());
                }
                if self.ctx_menu.open {
                    if let Some(i) = self.ctx_menu_item_at(x, y) {
                        self.ctx_menu.selected = i;
                    }
                    return Ok(());
                }
                // Forward mouse motion to panes that requested any-motion
                // reporting (mode 1003), so apps like opencode can highlight
                // the message under the cursor on hover.
                if let Some(pg) = self.pane_at(x, y) {
                    if let Some(pane) = self.panes.get_mut(&pg.pane_id) {
                        if pane.has_mouse_reporting() {
                            let inner = pg.inner();
                            let col = x.saturating_sub(inner.x) + 1;
                            let row = y.saturating_sub(inner.y) + 1;
                            pane.write(&sgr_mouse(35, col, row, false));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn copy_selection(&mut self, sel: &Sel) {
        if let Some(pane) = self.panes.get_mut(&sel.pane_id) {
            if let Some(text) = pane.selection_text(sel.start, sel.end) {
                if !text.is_empty() {
                    copy_to_clipboard(&text);
                    self.toast = Some(("copied to clipboard".to_string(), Instant::now()));
                }
            }
            // Keep the pane's active selection so it stays highlighted after
            // the copy; a new drag replaces it, a plain click clears it.
        }
    }
}
