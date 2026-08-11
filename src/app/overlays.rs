use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color as RColor, Modifier, Style};

use super::bindings::{Binding, Group, KEYBINDINGS};
use super::ui::{fill, put, text};
use super::{App, ORANGE, PANEL_MUTED, PANEL_SEP, YELLOW};
use crate::layout::SplitDir;
use crate::pane::{ACCENT, FG};

/// Label of the MENU button in the status bar.
pub(super) const MENU_BTN: &str = " MENU ";
/// Items shown in the status-bar menu dropdown.
const MENU_ITEMS: [&str; 3] = ["config", "keybinds", "exit"];
/// Size of the session-name popup.
const SESSION_POPUP_W: u16 = 44;
const SESSION_POPUP_H: u16 = 7;
/// Light background of the popup's text input, so it reads as an editable field.
const INPUT_BG: RColor = RColor::Rgb(0xcd, 0xd6, 0xf4); // Catppuccin lavender

/// Status-bar menu: a small dropdown anchored to the MENU button.
pub(super) struct Menu {
    pub(super) open: bool,
    pub(super) selected: usize,
}

/// What the right-click context menu targets.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum CtxTarget {
    /// A pane (`pane id`).
    Pane(u64),
    /// A session (`session index`).
    Session(usize),
}

/// Right-click context menu items for a target: a pane gets rename, both split
/// directions and close; a session gets rename and close.
fn ctx_items(target: CtxTarget) -> &'static [&'static str] {
    match target {
        CtxTarget::Pane(_) => &["rename", "split vertical", "split horizontal", "close"],
        CtxTarget::Session(_) => &["rename", "close"],
    }
}

/// Right-click context menu, anchored at the cursor.
pub(super) struct CtxMenu {
    pub(super) open: bool,
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) selected: usize,
    pub(super) target: CtxTarget,
}

/// Buttons of the session-name popup.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PopupBtn {
    Enter,
    Cancel,
}

/// What the name popup is editing.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PopupTarget {
    /// Naming a brand-new session.
    NewSession,
    /// Renaming the pane `pid`.
    RenamePane(u64),
    /// Renaming the session at index `idx`.
    RenameSession(usize),
}

/// Modal popup for naming a new session or renaming a pane.
pub(super) struct NamePopup {
    pub(super) open: bool,
    pub(super) target: Option<PopupTarget>,
    pub(super) name: String,
    /// Cursor position as a char index into `name`.
    pub(super) cursor: usize,
    pub(super) error: Option<String>,
    /// Button under the mouse (highlighted while hovering).
    pub(super) hover: Option<PopupBtn>,
}

/// The `leader+?` keybind showcase: a modal overlay listing every binding,
/// generated from the `bindings` table.
pub(super) struct KeybindOverlay {
    pub(super) open: bool,
    /// Scroll offset of the body rows.
    pub(super) scroll: u16,
}

impl App {
    /// Open the modal popup to name a new session, pre-filled with the next
    /// free default name.
    pub(super) fn open_session_popup(&mut self) {
        let name = self.default_session_name();
        self.popup.name = name.clone();
        self.popup.cursor = name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::NewSession);
        self.popup.open = true;
        self.menu.open = false;
    }

    /// Open the modal popup to rename `pid`, pre-filled with its current label.
    pub(super) fn open_rename_popup(&mut self, pid: u64) {
        let name = self.pane_label(pid);
        self.popup.name = name.clone();
        self.popup.cursor = name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::RenamePane(pid));
        self.popup.open = true;
        self.ctx_menu.open = false;
    }

    /// Open the modal popup to rename the session at `idx`, pre-filled with
    /// its current name.
    pub(super) fn open_rename_session_popup(&mut self, idx: usize) {
        let name = self.sessions.get(idx).map(|s| s.name.clone()).unwrap_or_default();
        self.popup.name = name.clone();
        self.popup.cursor = name.chars().count();
        self.popup.error = None;
        self.popup.hover = None;
        self.popup.target = Some(PopupTarget::RenameSession(idx));
        self.popup.open = true;
        self.ctx_menu.open = false;
    }

    /// Confirm the popup: create the session or rename the pane if valid.
    pub(super) fn commit_name(&mut self) {
        let name = self.popup.name.trim().to_string();
        if name.is_empty() {
            self.popup.error = Some("name cannot be empty".to_string());
            return;
        }
        match self.popup.target {
            Some(PopupTarget::NewSession) => {
                if self.sessions.iter().any(|s| s.name == name) {
                    self.popup.error = Some(format!("a session named '{name}' already exists"));
                    return;
                }
                self.popup.open = false;
                let _ = self.new_session_with_name(name);
            }
            Some(PopupTarget::RenamePane(pid)) => {
                let taken = self
                    .sessions[self.active]
                    .tree
                    .pane_ids()
                    .into_iter()
                    .filter(|id| *id != pid)
                    .map(|id| self.pane_label(id))
                    .any(|l| l == name);
                if taken {
                    self.popup.error = Some(format!("a pane named '{name}' already exists"));
                    return;
                }
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.custom_name = Some(name);
                }
                self.popup.open = false;
            }
            Some(PopupTarget::RenameSession(idx)) => {
                if self.sessions.get(idx).is_none() {
                    self.popup.open = false;
                    return;
                }
                let taken = self
                    .sessions
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != idx)
                    .any(|(_, s)| s.name == name);
                if taken {
                    self.popup.error = Some(format!("a session named '{name}' already exists"));
                    return;
                }
                self.sessions[idx].name = name;
                self.popup.open = false;
            }
            None => {}
        }
    }

    /// Insert `ch` at the popup cursor and advance it.
    fn popup_insert(&mut self, ch: char) {
        let b = char_idx_to_byte(&self.popup.name, self.popup.cursor);
        self.popup.name.insert(b, ch);
        self.popup.cursor += 1;
    }

    /// Delete the char before the popup cursor.
    fn popup_backspace(&mut self) {
        if self.popup.cursor == 0 {
            return;
        }
        let b = char_idx_to_byte(&self.popup.name, self.popup.cursor);
        let prev_len = self.popup.name[..b].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0);
        let start = b - prev_len;
        self.popup.name.replace_range(start..b, "");
        self.popup.cursor -= 1;
    }

    /// Handle a key while the session-name popup is open.
    pub(super) fn on_popup_key(&mut self, key: KeyEvent) {
        if is_leader(key) || key.code == KeyCode::Esc {
            self.popup.open = false;
            return;
        }
        match key.code {
            KeyCode::Enter => self.commit_name(),
            KeyCode::Backspace => self.popup_backspace(),
            KeyCode::Left => self.popup.cursor = self.popup.cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = self.popup.name.chars().count();
                self.popup.cursor = self.popup.cursor.min(len).saturating_add(1).min(len);
            }
            KeyCode::Home => self.popup.cursor = 0,
            KeyCode::End => self.popup.cursor = self.popup.name.chars().count(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.popup_insert(c);
            }
            _ => {}
        }
    }

    /// Handle a key while the status-bar menu is open.
    pub(super) fn on_menu_key(&mut self, key: KeyEvent) {
        if is_leader(key) || key.code == KeyCode::Esc {
            self.menu.open = false;
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.menu.selected = (self.menu.selected + 1) % MENU_ITEMS.len();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.menu.selected = self.menu.selected.saturating_sub(1);
            }
            KeyCode::Enter => self.menu_select(self.menu.selected),
            _ => {}
        }
    }

    /// Run the action for menu item `idx` and close the menu.
    pub(super) fn menu_select(&mut self, idx: usize) {
        let action = MENU_ITEMS.get(idx).copied().unwrap_or("exit");
        self.menu.open = false;
        match action {
            "config" => {
                // Placeholder until the config editor lands.
                self.notice = Some(("config: coming soon".to_string(), Instant::now()));
            }
            "keybinds" => self.open_keybind_overlay(),
            _ => self.quit = true, // exit (same as leader+d)
        }
    }

    /// Open (or reposition) the right-click context menu for `target` at
    /// `(x, y)`.
    pub(super) fn open_ctx_menu(&mut self, x: u16, y: u16, target: CtxTarget) {
        self.ctx_menu.open = true;
        self.ctx_menu.x = x;
        self.ctx_menu.y = y;
        self.ctx_menu.selected = 0;
        self.ctx_menu.target = target;
    }

    /// Run the action for context-menu item `idx` and close the menu.
    pub(super) fn ctx_menu_select(&mut self, idx: usize) -> Result<()> {
        let items = ctx_items(self.ctx_menu.target);
        let action = items.get(idx).copied().unwrap_or("close");
        let target = self.ctx_menu.target;
        self.ctx_menu.open = false;
        match action {
            "rename" => match target {
                CtxTarget::Pane(pid) => self.open_rename_popup(pid),
                CtxTarget::Session(idx) => self.open_rename_session_popup(idx),
            },
            "split vertical" => {
                if let CtxTarget::Pane(pid) = target {
                    self.set_focus(pid);
                    self.split_active(SplitDir::V, false)?;
                }
            }
            "split horizontal" => {
                if let CtxTarget::Pane(pid) = target {
                    self.set_focus(pid);
                    self.split_active(SplitDir::H, false)?;
                }
            }
            "close" => match target {
                CtxTarget::Pane(pid) => self.close_pane(pid),
                CtxTarget::Session(idx) => self.close_session(idx),
            },
            _ => {}
        }
        Ok(())
    }

    /// Handle a key while the right-click context menu is open.
    pub(super) fn on_ctx_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        if is_leader(key) || key.code == KeyCode::Esc {
            self.ctx_menu.open = false;
            return Ok(());
        }
        let items = ctx_items(self.ctx_menu.target);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.ctx_menu.selected = (self.ctx_menu.selected + 1) % items.len();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.ctx_menu.selected = self.ctx_menu.selected.saturating_sub(1);
            }
            KeyCode::Enter => self.ctx_menu_select(self.ctx_menu.selected)?,
            _ => {}
        }
        Ok(())
    }

    /// Open the keybind showcase (from `leader+?`).
    pub(super) fn open_keybind_overlay(&mut self) {
        self.keybind_overlay.open = true;
        self.keybind_overlay.scroll = 0;
    }

    /// Handle a key while the keybind showcase is open.
    pub(super) fn on_overlay_key(&mut self, key: KeyEvent) {
        if is_leader(key) || key.code == KeyCode::Esc || key.code == KeyCode::Char('?') {
            self.keybind_overlay.open = false;
            return;
        }
        let max = self.keybind_overlay_scroll_max();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.keybind_overlay.scroll = (self.keybind_overlay.scroll + 1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.keybind_overlay.scroll = self.keybind_overlay.scroll.saturating_sub(1);
            }
            KeyCode::Home => self.keybind_overlay.scroll = 0,
            KeyCode::End => self.keybind_overlay.scroll = max,
            _ => {}
        }
    }

    /// Max scroll offset of the showcase body, so the last row stays reachable.
    fn keybind_overlay_scroll_max(&self) -> u16 {
        let Some(dd) = self.keybind_overlay_rect() else { return 0 };
        let lines = keybind_lines().len();
        let visible = dd.height.saturating_sub(4) as usize;
        lines.saturating_sub(visible) as u16
    }

    /// Centered rect of the keybind showcase, sized to fit the longest row.
    fn keybind_overlay_rect(&self) -> Option<Rect> {
        let (w, h) = self.term_size;
        let max_keys = KEYBINDINGS.iter().map(|b| b.keys.chars().count()).max().unwrap_or(4) as u16;
        let max_desc = KEYBINDINGS.iter().map(|b| b.desc.chars().count()).max().unwrap_or(10) as u16;
        let inner = (max_keys + 2 + max_desc).max(20);
        let width = (inner + 6).min(w.saturating_sub(4));
        let lines = keybind_lines().len();
        let height = ((lines + 4) as u16).min(h.saturating_sub(4)).max(3);
        if w < width || h < height {
            return None;
        }
        Some(Rect::new((w - width) / 2, (h - height) / 2, width, height))
    }

    /// Draw the keybind showcase while it is open.
    pub(super) fn render_keybind_overlay(&self, f: &mut Frame) {
        if !self.keybind_overlay.open {
            return;
        }
        let Some(dd) = self.keybind_overlay_rect() else { return };
        let border = Style::default().fg(ACCENT).bg(PANEL_SEP);
        fill(f, dd, PANEL_SEP);
        let (x0, y0, x1, y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y1, "└", border);
        put(f, x1, y1, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y1, "─", border);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border);
            put(f, x1, y, "│", border);
        }

        let inner_w = dd.width.saturating_sub(4);
        let title = Style::default()
            .fg(FG)
            .bg(PANEL_SEP)
            .add_modifier(Modifier::BOLD);
        text(f, x0 + 2, y0 + 1, "keybindings", title, inner_w);

        let max_keys = KEYBINDINGS.iter().map(|b| b.keys.chars().count()).max().unwrap_or(4) as u16;
        let scroll = self.keybind_overlay.scroll as usize;
        let body_top = y0 + 2;
        let body_bottom = y1 - 1; // footer row
        for (i, line) in keybind_lines().iter().skip(scroll).enumerate() {
            let y = body_top + i as u16;
            if y >= body_bottom {
                break;
            }
            match line {
                KbLine::Header(label) => {
                    let st = Style::default()
                        .fg(ORANGE)
                        .bg(PANEL_SEP)
                        .add_modifier(Modifier::BOLD);
                    text(f, x0 + 2, y, label, st, inner_w);
                }
                KbLine::Bind(b) => {
                    let keys = Style::default()
                        .fg(ACCENT)
                        .bg(PANEL_SEP)
                        .add_modifier(Modifier::BOLD);
                    let desc = Style::default().fg(FG).bg(PANEL_SEP);
                    text(f, x0 + 2, y, b.keys, keys, max_keys);
                    text(f, x0 + 2 + max_keys + 2, y, b.desc, desc, inner_w.saturating_sub(max_keys + 2));
                }
            }
        }

        let footer = Style::default().fg(PANEL_MUTED).bg(PANEL_SEP);
        text(f, x0 + 2, y1 - 1, "j/k: scroll · esc / ?: close", footer, inner_w);
    }

    /// Rect of the context-menu dropdown, anchored above the right-click point.
    /// Only meaningful while the menu is open; once closed the stale anchor is
    /// ignored so a later right-click can't hit-test against it.
    fn ctx_menu_rect(&self) -> Option<Rect> {
        if !self.ctx_menu.open {
            return None;
        }
        let items = ctx_items(self.ctx_menu.target);
        let (w, h) = self.term_size;
        let width = items.iter().map(|i| i.chars().count()).max().unwrap_or(0) as u16 + 4;
        let height = items.len() as u16 + 2;
        if w < width || h < height {
            return None;
        }
        // Prefer opening down-right of the cursor, like a normal context menu;
        // flip up/left when there's no room in that direction.
        let px = self.ctx_menu.x;
        let py = self.ctx_menu.y;
        let x = if px.saturating_add(1) + width <= w { px + 1 } else { px.saturating_sub(width) };
        let y = if py + 1 + height <= h { py + 1 } else { py.saturating_sub(height) };
        Some(Rect::new(x, y, width, height))
    }

    /// Whether `(x, y)` is inside the open context menu.
    pub(super) fn ctx_menu_at(&self, x: u16, y: u16) -> bool {
        self.ctx_menu_rect()
            .map(|r| r.contains(Position::new(x, y)))
            .unwrap_or(false)
    }

    /// Context-menu item index under `(x, y)`, if the menu covers it.
    pub(super) fn ctx_menu_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.ctx_menu_rect()?;
        let items = ctx_items(self.ctx_menu.target);
        items
            .iter()
            .enumerate()
            .position(|(i, _)| {
                let item = Rect::new(dd.x + 1, dd.y + 1 + i as u16, dd.width.saturating_sub(2), 1);
                item.contains(Position::new(x, y))
            })
    }

    /// x of the MENU button: right after the mode chip + separator space.
    pub(super) fn menu_btn_x(&self) -> u16 {
        let mode = if self.mode == super::Mode::Leader { "LEADER" } else { "NORMAL" };
        format!(" {} ", mode).chars().count() as u16 + 1
    }

    /// Rect of the MENU button, right after the mode chip in the status bar.
    fn menu_btn_rect(&self) -> Option<Rect> {
        let (w, h) = self.term_size;
        let bw = MENU_BTN.chars().count() as u16;
        let x = self.menu_btn_x();
        (w >= x + bw).then(|| Rect::new(x, h.saturating_sub(1), bw, 1))
    }

    /// Rect of the dropdown box, anchored above the MENU button.
    fn menu_dropdown_rect(&self) -> Option<Rect> {
        let (w, h) = self.term_size;
        let width = MENU_ITEMS.iter().map(|i| i.chars().count()).max().unwrap_or(0) as u16 + 4;
        let height = MENU_ITEMS.len() as u16 + 2;
        if w < width || h < height + 1 {
            return None;
        }
        let btn_w = MENU_BTN.chars().count() as u16;
        let x = (self.menu_btn_x() + btn_w).saturating_sub(width).min(w.saturating_sub(width));
        let y = h.saturating_sub(1).saturating_sub(height);
        Some(Rect::new(x, y, width, height))
    }

    pub(super) fn menu_btn_at(&self, x: u16, y: u16) -> bool {
        self.menu_btn_rect()
            .map(|r| r.contains(Position::new(x, y)))
            .unwrap_or(false)
    }

    /// Menu item index under `(x, y)`, if the dropdown is open and covers it.
    pub(super) fn menu_item_at(&self, x: u16, y: u16) -> Option<usize> {
        let dd = self.menu_dropdown_rect()?;
        MENU_ITEMS
            .iter()
            .enumerate()
            .position(|(i, _)| {
                let item = Rect::new(dd.x + 1, dd.y + 1 + i as u16, dd.width.saturating_sub(2), 1);
                item.contains(Position::new(x, y))
            })
    }

    /// Centered rect of the session-name popup.
    pub(super) fn name_popup_rect(&self) -> Option<Rect> {
        let (w, h) = self.term_size;
        if w < SESSION_POPUP_W || h < SESSION_POPUP_H {
            return None;
        }
        Some(Rect::new((w - SESSION_POPUP_W) / 2, (h - SESSION_POPUP_H) / 2, SESSION_POPUP_W, SESSION_POPUP_H))
    }

    /// Terminal cursor position inside the popup's name field (row 3).
    pub(super) fn name_popup_input_cursor(&self) -> Option<(u16, u16)> {
        let dd = self.name_popup_rect()?;
        let text_w = (dd.width - 4) as usize - 1;
        let name = &self.popup.name;
        let cursor = self.popup.cursor.min(name.chars().count());
        let end = cursor + 1;
        let start = end.saturating_sub(text_w);
        let col = dd.x + 2 + cursor.saturating_sub(start) as u16;
        Some((col, dd.y + 3))
    }

    /// Rect of a popup button.
    fn name_popup_button_rect(&self, btn: PopupBtn) -> Option<Rect> {
        let dd = self.name_popup_rect()?;
        let label = match btn {
            PopupBtn::Enter => "⏎ enter ",
            PopupBtn::Cancel => " esc cancel ",
        };
        let w = label.chars().count() as u16;
        let x = match btn {
            PopupBtn::Enter => dd.x + 2,
            PopupBtn::Cancel => dd.x + 2 + 10,
        };
        Some(Rect::new(x, dd.y + 4, w, 1))
    }

    /// Button under `(x, y)` in the popup, if any.
    pub(super) fn name_popup_button_at(&self, x: u16, y: u16) -> Option<PopupBtn> {
        [PopupBtn::Enter, PopupBtn::Cancel].into_iter().find(|btn| {
            self.name_popup_button_rect(*btn)
                .map(|r| r.contains(Position::new(x, y)))
                .unwrap_or(false)
        })
    }

    /// Draw the dropdown above the MENU button while it is open.
    pub(super) fn render_menu(&self, f: &mut Frame) {
        if !self.menu.open {
            return;
        }
        let Some(dd) = self.menu_dropdown_rect() else { return };
        let border = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
        fill(f, dd, RColor::Reset);
        let (x0, y0, x1, y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y1, "└", border);
        put(f, x1, y1, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y1, "─", border);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border);
            put(f, x1, y, "│", border);
        }
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            render_item_row(
                f,
                x0,
                y0 + 1 + i as u16,
                dd.width.saturating_sub(2),
                item,
                i == self.menu.selected,
            );
        }
    }

    /// Draw the right-click context menu while it is open.
    pub(super) fn render_ctx_menu(&self, f: &mut Frame) {
        if !self.ctx_menu.open {
            return;
        }
        let Some(dd) = self.ctx_menu_rect() else { return };
        let border = Style::default().fg(PANEL_MUTED).bg(RColor::Reset);
        fill(f, dd, RColor::Reset);
        let (x0, y0, x1, y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y1, "└", border);
        put(f, x1, y1, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y1, "─", border);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border);
            put(f, x1, y, "│", border);
        }
        for (i, item) in ctx_items(self.ctx_menu.target).iter().enumerate() {
            render_item_row(
                f,
                x0,
                y0 + 1 + i as u16,
                dd.width.saturating_sub(2),
                item,
                i == self.ctx_menu.selected,
            );
        }
    }

    /// Draw the centered session-name popup while it is open.
    pub(super) fn render_name_popup(&self, f: &mut Frame) {
        if !self.popup.open {
            return;
        }
        let Some(dd) = self.name_popup_rect() else { return };
        let (x0, y0, x1, y1) = (dd.x, dd.y, dd.right() - 1, dd.bottom() - 1);
        let border = Style::default().fg(ACCENT).bg(PANEL_SEP);
        fill(f, dd, PANEL_SEP);
        put(f, x0, y0, "┌", border);
        put(f, x1, y0, "┐", border);
        put(f, x0, y1, "└", border);
        put(f, x1, y1, "┘", border);
        for x in (x0 + 1)..x1 {
            put(f, x, y0, "─", border);
            put(f, x, y1, "─", border);
        }
        for y in (y0 + 1)..y1 {
            put(f, x0, y, "│", border);
            put(f, x1, y, "│", border);
        }

        // Title.
        let title = Style::default()
            .fg(FG)
            .bg(PANEL_SEP)
            .add_modifier(Modifier::BOLD);
        let title_text = match self.popup.target {
            Some(PopupTarget::RenamePane(_)) => "rename pane",
            Some(PopupTarget::RenameSession(_)) => "rename session",
            _ => "new session",
        };
        text(f, x0 + 2, y0 + 1, title_text, title, dd.width.saturating_sub(4));

        // "name:" label.
        let label = Style::default().fg(FG).bg(PANEL_SEP);
        text(f, x0 + 2, y0 + 2, "name:", label, dd.width.saturating_sub(4));

        // Light input field, right-scrolled to keep the cursor visible.
        let field = Style::default().fg(RColor::Black).bg(INPUT_BG);
        let field_w = dd.width.saturating_sub(4);
        for cx in (x0 + 2)..(x0 + 2 + field_w) {
            put(f, cx, y0 + 3, " ", field);
        }
        let text_w = field_w as usize - 1;
        let name = &self.popup.name;
        let cursor = self.popup.cursor.min(name.chars().count());
        let end = cursor + 1;
        let start = end.saturating_sub(text_w);
        let mut col = x0 + 2;
        for (i, ch) in name.chars().enumerate() {
            if i < start {
                continue;
            }
            if i - start >= text_w {
                break;
            }
            put(f, col, y0 + 3, &ch.to_string(), field);
            col += 1;
        }

        // Buttons, styled like the status-bar menu button.
        for btn in [PopupBtn::Enter, PopupBtn::Cancel] {
            let Some(rect) = self.name_popup_button_rect(btn) else { continue };
            let label = match btn {
                PopupBtn::Enter => "⏎ enter ",
                PopupBtn::Cancel => " esc cancel ",
            };
            let hovered = self.popup.hover == Some(btn);
            let st = if hovered {
                Style::default()
                    .fg(RColor::Black)
                    .bg(YELLOW)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FG).bg(PANEL_SEP).add_modifier(Modifier::BOLD)
            };
            text(f, rect.x, rect.y, label, st, rect.width);
        }

        // Error line.
        if let Some(err) = &self.popup.error {
            text(f, x0 + 2, y0 + 5, err, Style::default().fg(ORANGE).bg(PANEL_SEP), dd.width.saturating_sub(4));
        }
    }
}

/// True when `key` is the leader chord: Ctrl+Space. Terminals report it as
/// NUL, space-with-ctrl, or a literal space in the enhanced keyboard protocol.
pub(super) fn is_leader(key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    ctrl && matches!(key.code, KeyCode::Char(' ') | KeyCode::Char('\0') | KeyCode::Null)
}

/// One display row of the keybind showcase: a group header or a binding.
enum KbLine<'a> {
    Header(&'a str),
    Bind(&'a Binding),
}

/// Flatten `KEYBINDINGS` into showcase rows, one header per group followed by
/// its bindings, in `Group::ALL` order.
fn keybind_lines() -> Vec<KbLine<'static>> {
    let mut lines = Vec::new();
    for group in Group::ALL {
        let mut pushed = false;
        for b in KEYBINDINGS {
            if b.group == group {
                if !pushed {
                    lines.push(KbLine::Header(group.label()));
                    pushed = true;
                }
                lines.push(KbLine::Bind(b));
            }
        }
    }
    lines
}

/// Draw one dropdown/context-menu item as a full-width button: the whole row
/// gets a filled background (yellow when selected, surface0 otherwise), with
/// the `▸` marker and the item label drawn on top.
fn render_item_row(f: &mut Frame, x0: u16, y: u16, width: u16, item: &str, sel: bool) {    let bg = if sel { YELLOW } else { PANEL_SEP };
    for cx in (x0 + 1)..(x0 + 1 + width) {
        put(f, cx, y, " ", Style::default().bg(bg));
    }
    let (marker, marker_style, label_style) = if sel {
        (
            "▸",
            Style::default().fg(RColor::Black).bg(bg).add_modifier(Modifier::BOLD),
            Style::default().fg(RColor::Black).bg(bg).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ",
            Style::default().fg(ACCENT).bg(bg),
            Style::default().fg(FG).bg(bg),
        )
    };
    put(f, x0 + 1, y, marker, marker_style);
    text(f, x0 + 3, y, item, label_style, width.saturating_sub(2));
}

/// Byte offset of the `ci`-th char in `s` (or `s.len()` past the end).
fn char_idx_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}
