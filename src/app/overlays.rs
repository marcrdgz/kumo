use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color as RColor, Modifier, Style};

use super::ui::{fill, put, text};
use super::{App, ORANGE, PANEL_MUTED, PANEL_SEP, YELLOW};
use crate::pane::{ACCENT, FG};

/// Label of the MENU button in the status bar.
pub(super) const MENU_BTN: &str = " MENU ";
/// Items shown in the status-bar menu dropdown.
const MENU_ITEMS: [&str; 2] = ["config", "detach"];
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

/// Buttons of the session-name popup.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PopupBtn {
    Enter,
    Cancel,
}

/// Modal popup for naming a new session.
pub(super) struct NamePopup {
    pub(super) open: bool,
    pub(super) name: String,
    /// Cursor position as a char index into `name`.
    pub(super) cursor: usize,
    pub(super) error: Option<String>,
    /// Button under the mouse (highlighted while hovering).
    pub(super) hover: Option<PopupBtn>,
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
        self.popup.open = true;
        self.menu.open = false;
    }

    /// Confirm the popup: create the session if the name is valid.
    pub(super) fn commit_session_name(&mut self) {
        let name = self.popup.name.trim().to_string();
        if name.is_empty() {
            self.popup.error = Some("name cannot be empty".to_string());
            return;
        }
        if self.sessions.iter().any(|s| s.name == name) {
            self.popup.error = Some(format!("a session named '{name}' already exists"));
            return;
        }
        self.popup.open = false;
        let _ = self.new_session_with_name(name);
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
            KeyCode::Enter => self.commit_session_name(),
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
        let action = MENU_ITEMS.get(idx).copied().unwrap_or("detach");
        self.menu.open = false;
        match action {
            "config" => {
                // Placeholder until the config editor lands.
                self.notice = Some(("config: coming soon".to_string(), Instant::now()));
            }
            _ => self.quit = true, // detach (same as leader+d)
        }
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
            let y = y0 + 1 + i as u16;
            let sel = i == self.menu.selected;
            let bg = if sel { PANEL_SEP } else { RColor::Reset };
            let item_style = Style::default().fg(FG).bg(bg);
            let marker = if sel { "▸" } else { " " };
            put(f, x0 + 1, y, marker, Style::default().fg(ACCENT).bg(bg));
            text(f, x0 + 3, y, item, item_style, dd.width.saturating_sub(4));
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
        text(f, x0 + 2, y0 + 1, "new session", title, dd.width.saturating_sub(4));

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

/// Byte offset of the `ci`-th char in `s` (or `s.len()` past the end).
fn char_idx_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}
