//! Display table of kumo's leader bindings: the single source of truth for the
//! leader-mode status-bar hint and the `leader+?` keybind showcase. The dispatch
//! itself lives in `App::leader_command` (`src/app.rs`); this table is the
//! reference that keeps the hint and the showcase from drifting.
//!
//! Keep `KEYBINDINGS` and `leader_command` in sync when adding a binding.

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

/// One leader binding, as shown in the showcase.
pub(super) struct Binding {
    /// Keys pressed after the leader, e.g. "h/j/k/l".
    pub(super) keys: &'static str,
    /// Longer description for the showcase.
    pub(super) desc: &'static str,
    pub(super) group: Group,
}

/// All leader bindings, grouped in showcase order. The `?` binding is included
/// so it lists in the showcase, and the hint builder points at it.
pub(super) const KEYBINDINGS: &[Binding] = &[
    Binding { keys: "v", desc: "split the focused pane vertically", group: Group::Layout },
    Binding { keys: "-", desc: "split the focused pane horizontally", group: Group::Layout },
    Binding { keys: "z", desc: "zoom the focused pane", group: Group::Layout },
    Binding { keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout },
    Binding { keys: "a", desc: "spawn an AI CLI pane in a vertical split", group: Group::Panes },
    Binding { keys: "x", desc: "close the focused pane", group: Group::Panes },
    Binding { keys: "Tab", desc: "cycle focus between panes", group: Group::Panes },
    Binding { keys: "c", desc: "create a new session (name it in the popup)", group: Group::Sessions },
    Binding { keys: "n/p", desc: "cycle to the next / previous session", group: Group::Sessions },
    Binding { keys: "1-9", desc: "jump to the session at that list position", group: Group::Sessions },
    Binding { keys: "b", desc: "toggle the sidebar", group: Group::Chrome },
    Binding { keys: "d", desc: "detach (daemon keeps running)", group: Group::General },
    Binding { keys: "?", desc: "show all keybindings", group: Group::General },
];

/// The leader-mode status-bar hint: just the `?` pointer to the keybind
/// showcase. Everything else lives in the showcase, so the strip stays tiny and
/// never drifts from the table.
pub(super) fn leader_hint() -> String {
    let help = KEYBINDINGS.iter().find(|b| b.keys == "?").expect("help binding present");
    format!(" {}: {} ", help.keys, help.desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_non_empty_and_has_all_groups() {
        assert!(!KEYBINDINGS.is_empty());
        for group in Group::ALL {
            assert!(
                KEYBINDINGS.iter().any(|b| b.group == group),
                "no binding in group {:?}",
                group
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
}
