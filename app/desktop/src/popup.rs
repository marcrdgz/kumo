//! The name popup: a small modal the desktop uses for new session, new
//! worktree, and rename flows (the same targets as the TUI's `Popup`).
//!
//! GPUI 0.2 has no built-in text input, so the popup renders its value from
//! [`NamePopup`] state and the window's global keystroke observer feeds it
//! (printable chars, backspace, ctrl+w word-delete, ctrl+u line-delete).

use gpui::{div, px, MouseButton, SharedString, StyledText, prelude::*};
use kumo_protocol::Command;

use crate::theme;
use crate::KumoWindow;

/// What `enter` commits to (mirrors the TUI's `PopupTarget`).
#[derive(Clone, Debug)]
pub(crate) enum PopupTarget {
    NewSession,
    NewWorktree(String),
    RenamePane(u64),
    RenameSession(String),
}

/// The popup's editable state.
pub(crate) struct NamePopup {
    pub(crate) target: PopupTarget,
    pub(crate) title: SharedString,
    pub(crate) input: String,
    pub(crate) error: Option<String>,
}

impl NamePopup {
    pub(crate) fn open(target: PopupTarget, title: &str, initial: String) -> Self {
        NamePopup { target, title: SharedString::from(title.to_string()), input: initial, error: None }
    }

    /// A printable character typed into the input.
    pub(crate) fn insert(&mut self, ch: char) {
        self.input.push(ch);
    }

    /// Backspace: a character, a word (ctrl+w), or the whole line (ctrl+u).
    pub(crate) fn backspace(&mut self, word: bool, line: bool) {
        if line {
            self.input.clear();
        } else if word {
            let trimmed = self.input.trim_end();
            let cut = trimmed.char_indices().rfind(|(_, c)| c.is_whitespace()).map_or(0, |(i, _)| i);
            self.input.truncate(cut);
        } else {
            self.input.pop();
        }
    }
}

impl KumoWindow {
    /// The full-window modal layer, or `None` when no popup is open. Swallows
    /// mouse events so clicks don't reach the panes behind it.
    pub(crate) fn popup_layer(&self) -> Option<impl IntoElement> {
        let popup = self.popup.as_ref()?;
        let chrome = self.chrome();
        let input_with_caret = if self.cursor_on {
            format!("{}▌", popup.input)
        } else {
            format!("{} ", popup.input)
        };
        let mut title_style = self.base.clone();
        title_style.color = chrome.accent();
        title_style.font_size = px(12.0).into();
        title_style.font_weight = gpui::FontWeight::BOLD;

        let mut input_style = self.base.clone();
        input_style.color = gpui::rgba(0xeeeeefff).into();
        input_style.font_size = px(15.0).into();

        let mut card = div()
            .w(px(440.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme::hairline())
            .bg(gpui::rgba(0x121218f2))
            .px(px(18.0))
            .py(px(16.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(StyledText::new(popup.title.clone()).with_default_highlights(&title_style, []))
            .child(StyledText::new(input_with_caret).with_default_highlights(&input_style, []));
        if let Some(error) = popup.error.as_deref() {
            let mut err_style = self.base.clone();
            err_style.color = gpui::rgba(0xff7b72ff).into();
            err_style.font_size = px(11.5).into();
            card = card.child(StyledText::new(SharedString::from(error.to_string())).with_default_highlights(&err_style, []));
        }
        let mut hint_style = self.dim.clone();
        hint_style.font_size = px(11.0).into();
        card = card.child(
            StyledText::new(SharedString::from("enter to confirm · esc to cancel"))
                .with_default_highlights(&hint_style, []),
        );
        Some(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000066))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(card),
        )
    }

    // ------------------------------------------------------------------
    // Popup openers (ports of the TUI's open_* helpers)
    // ------------------------------------------------------------------

    /// `leader+c`: name a new session (prefilled with the next free name).
    pub(crate) fn open_session_popup(&mut self, cx: &mut gpui::Context<Self>) {
        let initial = self.next_session_name();
        self.popup = Some(NamePopup::open(PopupTarget::NewSession, "new session", initial));
        cx.notify();
    }

    /// `leader+w`: name a git worktree branch for a new session. Requires the
    /// active session's workspace to be a git repository.
    pub(crate) fn open_worktree_popup(&mut self, cx: &mut gpui::Context<Self>) {
        let ws = self.active_session().map(|s| s.workspace.clone());
        if ws.as_deref().and_then(kumo_core::worktrees::repo_root).is_none() {
            let shown = ws.map(|w| w.display().to_string()).unwrap_or_default();
            self.status = SharedString::from(format!("{shown}: not a git repository"));
            cx.notify();
            return;
        }
        let session = self.active_session().map(|s| s.name.clone()).unwrap_or_default();
        self.popup = Some(NamePopup::open(
            PopupTarget::NewWorktree(session),
            "new worktree (branch name)",
            String::new(),
        ));
        cx.notify();
    }

    /// Context menu → rename pane: prefilled with the pane's current title.
    pub(crate) fn open_rename_pane_popup(&mut self, pid: u64, cx: &mut gpui::Context<Self>) {
        let initial = self
            .active_session()
            .and_then(|s| s.root.as_deref())
            .and_then(|r| crate::find_pane(r, pid))
            .map(|p| p.title.trim().to_string())
            .unwrap_or_default();
        self.popup = Some(NamePopup::open(PopupTarget::RenamePane(pid), "rename pane", initial));
        cx.notify();
    }

    /// Context menu → rename session: prefilled with the current name.
    pub(crate) fn open_rename_session_popup(&mut self, old: String, cx: &mut gpui::Context<Self>) {
        let title = format!("rename session {old}");
        self.popup = Some(NamePopup::open(PopupTarget::RenameSession(old.clone()), &title, old));
        cx.notify();
    }

    fn next_session_name(&self) -> String {
        let names: Vec<String> = self
            .layout
            .as_ref()
            .map(|l| l.sessions.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        let mut n = 1;
        loop {
            let cand = format!("session-{n}");
            if !names.contains(&cand) {
                return cand;
            }
            n += 1;
        }
    }

    /// `enter`: validate and send the command for the popup's target.
    pub(crate) fn commit_popup(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(popup) = self.popup.as_ref() else { return };
        let name = popup.input.trim().to_string();
        if name.is_empty() {
            if let Some(p) = self.popup.as_mut() {
                p.error = Some("name cannot be empty".into());
            }
            cx.notify();
            return;
        }
        match popup.target.clone() {
            PopupTarget::NewSession => {
                if self.layout.as_ref().map(|l| l.sessions.iter().any(|s| s.name == name)).unwrap_or(false) {
                    if let Some(p) = self.popup.as_mut() {
                        p.error = Some(format!("a session named '{name}' already exists"));
                    }
                    cx.notify();
                    return;
                }
                self.popup = None;
                let _ = self.send(Command::SessionNew { name: Some(name), workspace: None });
            }
            PopupTarget::NewWorktree(session) => {
                self.popup = None;
                let _ = self.send(Command::WorktreeCreate { session, branch: name });
            }
            PopupTarget::RenamePane(pid) => {
                let session = self.active_session().map(|s| s.name.clone());
                if let Some(session) = session {
                    self.popup = None;
                    let _ = self.send(Command::PaneRename { session, pane_id: pid, name });
                }
            }
            PopupTarget::RenameSession(old) => {
                self.popup = None;
                let _ = self.send(Command::SessionRename { session: old, new_name: name });
            }
        }
        cx.notify();
    }
}
