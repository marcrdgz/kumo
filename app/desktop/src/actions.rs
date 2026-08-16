//! Desktop leader keymap: the same stock actions as the TUI client, matched
//! against GPUI keystrokes and overridable via `[keymap.bindings]` in the
//! shared kumo-core config.
//!
//! [`Chord`] is the GPUI-native counterpart of the CLI's crossterm chords:
//! a key name (`"v"`, `"tab"`, `"f1"`) plus modifiers, so both clients honor
//! the same config strings while keeping their own event types.

use std::collections::HashMap;

use gpui::Keystroke;

/// Directional focus target (geometry-based, like the TUI's `pane_toward`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// A leader-mode action, the second half of a binding's `chord + action` pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    SplitVertical,
    SplitHorizontal,
    SplitAi,
    NewSession,
    NewWorktree,
    ClosePane,
    Zoom,
    Focus(Dir),
    Resize(kumo_protocol::ResizeDir),
    CyclePane,
    SwapPanes,
    RotateLayout,
    ShowPaneNumbers,
    NextSession,
    PrevSession,
    JumpSession(u8),
    ToggleSidebar,
    Detach,
    ShowKeybinds,
}

/// A key + modifiers, in GPUI key-name form (`"v"`, `"tab"`, `"f1"`, `"space"`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Chord {
    pub(crate) key: String,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
    pub(crate) super_key: bool,
}

impl Chord {
    /// True when a GPUI keystroke is exactly this chord.
    pub(crate) fn matches(&self, ks: &Keystroke) -> bool {
        self.key == ks.key
            && self.ctrl == ks.modifiers.control
            && self.shift == ks.modifiers.shift
            && self.alt == ks.modifiers.alt
            && self.super_key == ks.modifiers.platform
    }
}

/// One leader binding: the dispatch chord, showcase strings, and its action.
#[derive(Clone)]
pub(crate) struct Binding {
    pub(crate) chord: Chord,
    pub(crate) keys: &'static str,
    pub(crate) desc: &'static str,
    pub(crate) action: Action,
}

/// Stock leader bindings, in showcase order (same table as the TUI client):
/// `(gpui key name, shift, showcase keys, description, action)`.
const BINDINGS: &[(&str, bool, &str, &str, Action)] = &[
    ("v", false, "v", "split the focused pane vertically", Action::SplitVertical),
    ("-", false, "-", "split the focused pane horizontally", Action::SplitHorizontal),
    ("z", false, "z", "zoom the focused pane", Action::Zoom),
    ("h", false, "h/j/k/l", "move focus left / down / up / right", Action::Focus(Dir::Left)),
    ("j", false, "h/j/k/l", "move focus left / down / up / right", Action::Focus(Dir::Down)),
    ("k", false, "h/j/k/l", "move focus left / down / up / right", Action::Focus(Dir::Up)),
    ("l", false, "h/j/k/l", "move focus left / down / up / right", Action::Focus(Dir::Right)),
    ("h", true, "H/J/K/L", "resize the focused pane", Action::Resize(kumo_protocol::ResizeDir::Left)),
    ("j", true, "H/J/K/L", "resize the focused pane", Action::Resize(kumo_protocol::ResizeDir::Down)),
    ("k", true, "H/J/K/L", "resize the focused pane", Action::Resize(kumo_protocol::ResizeDir::Up)),
    ("l", true, "H/J/K/L", "resize the focused pane", Action::Resize(kumo_protocol::ResizeDir::Right)),
    ("a", false, "a", "spawn an AI CLI pane in a vertical split", Action::SplitAi),
    ("x", false, "x", "close the focused pane", Action::ClosePane),
    ("tab", false, "Tab", "cycle focus between panes", Action::CyclePane),
    ("s", false, "s", "swap the focused pane with its sibling", Action::SwapPanes),
    ("o", false, "o", "rotate the pane layout", Action::RotateLayout),
    ("q", false, "q", "show pane numbers (press a number to jump)", Action::ShowPaneNumbers),
    ("c", false, "c", "create a new session (name it in the popup)", Action::NewSession),
    ("w", false, "w", "create a git worktree in a new session", Action::NewWorktree),
    ("n", false, "n/p", "cycle to the next / previous session", Action::NextSession),
    ("p", false, "n/p", "cycle to the next / previous session", Action::PrevSession),
    ("1", false, "1-9", "jump to the session at that list position", Action::JumpSession(1)),
    ("2", false, "1-9", "jump to the session at that list position", Action::JumpSession(2)),
    ("3", false, "1-9", "jump to the session at that list position", Action::JumpSession(3)),
    ("4", false, "1-9", "jump to the session at that list position", Action::JumpSession(4)),
    ("5", false, "1-9", "jump to the session at that list position", Action::JumpSession(5)),
    ("6", false, "1-9", "jump to the session at that list position", Action::JumpSession(6)),
    ("7", false, "1-9", "jump to the session at that list position", Action::JumpSession(7)),
    ("8", false, "1-9", "jump to the session at that list position", Action::JumpSession(8)),
    ("9", false, "1-9", "jump to the session at that list position", Action::JumpSession(9)),
    ("b", false, "b", "toggle the sidebar", Action::ToggleSidebar),
    ("d", false, "d", "detach (daemon keeps running)", Action::Detach),
    ("?", false, "?", "show all keybindings", Action::ShowKeybinds),
];

/// Build the effective keymap: stock bindings with `[keymap.bindings]`
/// overrides applied (rebind or add; invalid entries are ignored, mirroring
/// the CLI's tolerant behavior).
pub(crate) fn build_keymap(overrides: &HashMap<String, String>) -> Vec<Binding> {
    let mut out: Vec<Binding> = BINDINGS
        .iter()
        .map(|(key_name, shift, keys, desc, action)| Binding {
            chord: Chord {
                key: (*key_name).into(),
                ctrl: false,
                shift: *shift,
                alt: false,
                super_key: false,
            },
            keys,
            desc,
            action: *action,
        })
        .collect();
    for (chord_str, action_str) in overrides {
        let Some(action) = action_from_id(action_str) else { continue };
        let Some(chord) = parse_chord(chord_str) else { continue };
        out.retain(|b| b.chord != chord);
        out.push(Binding { keys: "", desc: action_desc(action), chord, action });
    }
    out
}

/// The default leader chord: Ctrl+B, or the config's `leader` when set.
pub(crate) fn leader_chord() -> Chord {
    kumo_core::config::leader()
        .and_then(|s| parse_chord(&s))
        .unwrap_or(Chord { key: "b".into(), ctrl: true, shift: false, alt: false, super_key: false })
}

/// Parse a config chord string (`v`, `tab`, `ctrl+f1`, `ctrl+shift+tab`).
/// Uppercase letters imply Shift. Returns `None` for unknown keys/modifiers.
pub(crate) fn parse_chord(raw: &str) -> Option<Chord> {
    let mut chord = Chord { key: String::new(), ctrl: false, shift: false, alt: false, super_key: false };
    let mut parts: Vec<&str> = raw.split('+').map(|s| s.trim()).collect();
    let key_raw = parts.pop()?;
    if key_raw.is_empty() {
        return None;
    }
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" | "opt" | "option" => chord.alt = true,
            "super" | "cmd" | "command" | "meta" => chord.super_key = true,
            _ => return None,
        }
    }
    let key = key_raw.to_ascii_lowercase();
    if key.len() == 1 {
        if key_raw.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            chord.shift = true;
        }
        chord.key = key;
        return Some(chord);
    }
    let named = match key.as_str() {
        "space" | "spc" => "space",
        "enter" | "return" => "enter",
        "esc" | "escape" => "escape",
        "tab" => "tab",
        "backspace" => "backspace",
        "delete" => "delete",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        f if f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = f[1..].parse().ok()?;
            if !(1..=12).contains(&n) {
                return None;
            }
            // GPUI names function keys f1..f12.
            return Some(Chord { key: f.to_string(), ..chord });
        }
        _ => return None,
    };
    chord.key = named.into();
    Some(chord)
}

fn action_from_id(id: &str) -> Option<Action> {
    Some(match id {
        "split-vertical" => Action::SplitVertical,
        "split-horizontal" => Action::SplitHorizontal,
        "split-ai" => Action::SplitAi,
        "new-session" => Action::NewSession,
        "new-worktree" => Action::NewWorktree,
        "close-pane" => Action::ClosePane,
        "zoom" => Action::Zoom,
        "focus-left" => Action::Focus(Dir::Left),
        "focus-down" => Action::Focus(Dir::Down),
        "focus-up" => Action::Focus(Dir::Up),
        "focus-right" => Action::Focus(Dir::Right),
        "resize-left" => Action::Resize(kumo_protocol::ResizeDir::Left),
        "resize-down" => Action::Resize(kumo_protocol::ResizeDir::Down),
        "resize-up" => Action::Resize(kumo_protocol::ResizeDir::Up),
        "resize-right" => Action::Resize(kumo_protocol::ResizeDir::Right),
        "cycle-pane" => Action::CyclePane,
        "swap-panes" => Action::SwapPanes,
        "rotate-layout" => Action::RotateLayout,
        "show-pane-numbers" => Action::ShowPaneNumbers,
        "next-session" => Action::NextSession,
        "prev-session" => Action::PrevSession,
        "jump-session-1" => Action::JumpSession(1),
        "jump-session-2" => Action::JumpSession(2),
        "jump-session-3" => Action::JumpSession(3),
        "jump-session-4" => Action::JumpSession(4),
        "jump-session-5" => Action::JumpSession(5),
        "jump-session-6" => Action::JumpSession(6),
        "jump-session-7" => Action::JumpSession(7),
        "jump-session-8" => Action::JumpSession(8),
        "jump-session-9" => Action::JumpSession(9),
        "toggle-sidebar" => Action::ToggleSidebar,
        "detach" => Action::Detach,
        "show-keybinds" => Action::ShowKeybinds,
        _ => return None,
    })
}

fn action_desc(action: Action) -> &'static str {
    match action {
        Action::SplitVertical => "split the focused pane vertically",
        Action::SplitHorizontal => "split the focused pane horizontally",
        Action::SplitAi => "spawn an AI CLI pane in a vertical split",
        Action::NewSession => "create a new session (name it in the popup)",
        Action::NewWorktree => "create a git worktree in a new session",
        Action::ClosePane => "close the focused pane",
        Action::Zoom => "zoom the focused pane",
        Action::Focus(_) => "move focus left / down / up / right",
        Action::Resize(_) => "resize the focused pane",
        Action::CyclePane => "cycle focus between panes",
        Action::SwapPanes => "swap the focused pane with its sibling",
        Action::RotateLayout => "rotate the pane layout",
        Action::ShowPaneNumbers => "show pane numbers (press a number to jump)",
        Action::NextSession | Action::PrevSession => "cycle to the next / previous session",
        Action::JumpSession(_) => "jump to the session at that list position",
        Action::ToggleSidebar => "toggle the sidebar",
        Action::Detach => "detach (daemon keeps running)",
        Action::ShowKeybinds => "show all keybindings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chord_accepts_and_rejects() {
        assert_eq!(parse_chord("ctrl+b").map(|c| (c.key, c.ctrl)), Some(("b".into(), true)));
        assert_eq!(parse_chord("tab").map(|c| (c.key, c.shift)), Some(("tab".into(), false)));
        assert_eq!(parse_chord("H").map(|c| (c.key, c.shift)), Some(("h".into(), true)));
        assert_eq!(parse_chord("f12").map(|c| c.key), Some("f12".into()));
        assert!(parse_chord("ctrl+banana").is_none());
        assert!(parse_chord("").is_none());
    }

    #[test]
    fn dispatch_chords_are_unique() {
        let keymap = build_keymap(&HashMap::new());
        for (i, a) in keymap.iter().enumerate() {
            for b in keymap.iter().skip(i + 1) {
                assert_ne!(a.chord, b.chord, "duplicate chord {:?}", a.chord);
            }
        }
    }

    #[test]
    fn build_keymap_applies_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("s".to_string(), "split-vertical".to_string());
        let keymap = build_keymap(&overrides);
        assert!(keymap.iter().any(|b| b.action == Action::SplitVertical && b.chord.key == "s"));
        assert!(!keymap.iter().any(|b| b.action == Action::SwapPanes));
    }
}
