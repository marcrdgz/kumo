//! Canonical leader-keymap table: the single source of truth for dispatch,
//! the leader-mode status-bar hint, and the `leader+?` keybind showcase.
//!
//! Each [`Binding`] pairs a dispatch [`Chord`] (the key pressed after the
//! leader) with an [`Action`] and its showcase row. `App::leader_command`
//! looks up the chord and runs the action; the hint and the showcase read the
//! same table, so dispatch, hint, and showcase can never drift. Adding a
//! binding is a one-line table entry, no code elsewhere.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Dir;

/// The leader chord that enters leader mode: Ctrl+Space.
///
/// Terminals report it as NUL, space-with-ctrl, or a literal space in the
/// enhanced keyboard protocol; `Chord::is_leader` normalizes all three.
pub(super) const LEADER: Chord = Chord::new(KeyCode::Char(' '), KeyModifiers::CONTROL);

/// The keys a user presses: a [`KeyCode`] plus its modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Chord {
    pub(super) code: KeyCode,
    pub(super) modifiers: KeyModifiers,
}

impl Chord {
    pub(super) const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Chord { code, modifiers }
    }

    /// True when `event` is exactly this chord.
    pub(super) fn matches(self, event: KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
    }

    /// True when `event` is the leader chord: Ctrl+Space in any of its three
    /// crossterm spellings (space-with-ctrl, NUL, literal null).
    pub(super) fn is_leader(self, event: KeyEvent) -> bool {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        self.matches(event) || (ctrl && matches!(event.code, KeyCode::Char('\0') | KeyCode::Null))
    }
}

/// A leader-mode command, the second half of a binding's `chord + action` pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    SplitVertical,
    SplitHorizontal,
    SplitAi,
    NewSession,
    ClosePane,
    Zoom,
    Focus(Dir),
    CyclePane,
    NextSession,
    PrevSession,
    JumpSession(u8),
    ToggleSidebar,
    Detach,
    ShowKeybinds,
}

/// Logical group a binding belongs to, used to organize the showcase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Group {
    Layout,
    Panes,
    Sessions,
    Chrome,
    General,
}

impl Group {
    pub(super) const ALL: [Group; 5] =
        [Group::Layout, Group::Panes, Group::Sessions, Group::Chrome, Group::General];

    pub(super) fn label(self) -> &'static str {
        match self {
            Group::Layout => "layout",
            Group::Panes => "panes",
            Group::Sessions => "sessions",
            Group::Chrome => "chrome",
            Group::General => "general",
        }
    }
}

/// One leader binding: the dispatch chord, its action, and the showcase row.
/// `keys` is the compact display string — several bindings may share one (e.g.
/// `h/j/k/l`, `1-9`) so the showcase shows a single grouped row for them.
pub(super) struct Binding {
    /// Key pressed after the leader to run `action`.
    pub(super) key: Chord,
    /// Compact showcase keys, e.g. "h/j/k/l" or "1-9".
    pub(super) keys: &'static str,
    /// Longer description for the showcase.
    pub(super) desc: &'static str,
    pub(super) group: Group,
    pub(super) action: Action,
}

const fn chord(code: KeyCode) -> Chord {
    Chord::new(code, KeyModifiers::NONE)
}

/// All leader bindings, in showcase order. Bindings that share a `keys`
/// display string must stay adjacent so the showcase collapses them into one
/// grouped row.
pub(super) const BINDINGS: &[Binding] = &[
    Binding { key: chord(KeyCode::Char('v')), keys: "v", desc: "split the focused pane vertically", group: Group::Layout, action: Action::SplitVertical },
    Binding { key: chord(KeyCode::Char('-')), keys: "-", desc: "split the focused pane horizontally", group: Group::Layout, action: Action::SplitHorizontal },
    Binding { key: chord(KeyCode::Char('z')), keys: "z", desc: "zoom the focused pane", group: Group::Layout, action: Action::Zoom },
    Binding { key: chord(KeyCode::Char('h')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Left) },
    Binding { key: chord(KeyCode::Char('j')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Down) },
    Binding { key: chord(KeyCode::Char('k')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Up) },
    Binding { key: chord(KeyCode::Char('l')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Right) },
    Binding { key: chord(KeyCode::Char('a')), keys: "a", desc: "spawn an AI CLI pane in a vertical split", group: Group::Panes, action: Action::SplitAi },
    Binding { key: chord(KeyCode::Char('x')), keys: "x", desc: "close the focused pane", group: Group::Panes, action: Action::ClosePane },
    Binding { key: chord(KeyCode::Tab), keys: "Tab", desc: "cycle focus between panes", group: Group::Panes, action: Action::CyclePane },
    Binding { key: chord(KeyCode::Char('c')), keys: "c", desc: "create a new session (name it in the popup)", group: Group::Sessions, action: Action::NewSession },
    Binding { key: chord(KeyCode::Char('n')), keys: "n/p", desc: "cycle to the next / previous session", group: Group::Sessions, action: Action::NextSession },
    Binding { key: chord(KeyCode::Char('p')), keys: "n/p", desc: "cycle to the next / previous session", group: Group::Sessions, action: Action::PrevSession },
    Binding { key: chord(KeyCode::Char('1')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(1) },
    Binding { key: chord(KeyCode::Char('2')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(2) },
    Binding { key: chord(KeyCode::Char('3')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(3) },
    Binding { key: chord(KeyCode::Char('4')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(4) },
    Binding { key: chord(KeyCode::Char('5')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(5) },
    Binding { key: chord(KeyCode::Char('6')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(6) },
    Binding { key: chord(KeyCode::Char('7')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(7) },
    Binding { key: chord(KeyCode::Char('8')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(8) },
    Binding { key: chord(KeyCode::Char('9')), keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(9) },
    Binding { key: chord(KeyCode::Char('b')), keys: "b", desc: "toggle the sidebar", group: Group::Chrome, action: Action::ToggleSidebar },
    Binding { key: chord(KeyCode::Char('d')), keys: "d", desc: "detach (daemon keeps running)", group: Group::General, action: Action::Detach },
    Binding { key: chord(KeyCode::Char('?')), keys: "?", desc: "show all keybindings", group: Group::General, action: Action::ShowKeybinds },
];

/// The dispatch chord bound to `action`, if any. Useful to validate config
/// remaps ("is this action remappable?") and to show an action's current key.
#[allow(dead_code)]
pub(super) fn key_for(action: Action) -> Option<Chord> {
    BINDINGS.iter().find(|b| b.action == action).map(|b| b.key)
}

/// The leader-mode status-bar hint: just the `?` pointer to the keybind
/// showcase. Everything else lives in the showcase, so the strip stays tiny and
/// never drifts from the table.
pub(super) fn leader_hint() -> String {
    let help = BINDINGS.iter().find(|b| b.keys == "?").expect("help binding present");
    format!(" {}: {} ", help.keys, help.desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_non_empty_and_has_all_groups() {
        assert!(!BINDINGS.is_empty());
        for group in Group::ALL {
            assert!(
                BINDINGS.iter().any(|b| b.group == group),
                "no binding in group {:?}",
                group
            );
        }
    }

    #[test]
    fn every_binding_has_a_dispatch_chord() {
        for b in BINDINGS {
            assert!(
                b.key.code != KeyCode::Null && b.key.code != KeyCode::Char('\0'),
                "binding {:?} has no dispatch key",
                b.action
            );
        }
    }

    #[test]
    fn dispatch_chords_are_unique() {
        // A duplicated dispatch key would silently shadow one of its bindings;
        // config remaps would make that ambiguity worse.
        let mut chords: Vec<Chord> = BINDINGS.iter().map(|b| b.key).collect();
        chords.sort_by_key(|c| format!("{:?}{:?}", c.code, c.modifiers));
        for pair in chords.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate dispatch chord {:?}", pair[0]);
        }
    }

    #[test]
    fn key_for_reverses_dispatch() {
        for b in BINDINGS {
            assert_eq!(
                key_for(b.action),
                Some(b.key),
                "key_for({:?}) must return the binding's own chord",
                b.action
            );
        }
    }

    #[test]
    fn hint_points_to_the_showcase_only() {
        let hint = leader_hint();
        assert!(hint.contains("?: show all keybindings"));
        // The hint no longer lists every binding; the showcase is the reference.
        assert!(!hint.contains("v-split"));
        assert!(!hint.contains("esc: exit"));
    }

    #[test]
    fn leader_matches_all_ctrl_space_spellings() {
        let chord = LEADER;
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Char('\0'), KeyModifiers::CONTROL)));
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Null, KeyModifiers::CONTROL)));
        assert!(!chord.is_leader(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(!chord.is_leader(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)));
    }
}
