//! Leader keymap: the single source of truth for dispatch, the leader-mode
//! status-bar hint, and the `leader+?` keybind showcase.
//!
//! [`BINDING_SPECS`] holds the stock bindings (chord + action + showcase row).
//! [`build_keymap`] turns them into the runtime table the app actually uses,
//! applying any `[keymap.bindings]` overrides from the config. Dispatch, hint,
//! and showcase all read the same runtime table, so they can never drift.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kumo_core::layout::ResizeDir;

/// Directional focus / resize target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Modifiers that reveal/activate links (`Cmd+click` like a normal terminal).
/// SUPER is kept for hosts that forward it; CONTROL (Ctrl) and ALT (Option on
/// macOS) are the ones the SGR mouse protocol can actually deliver.
pub(crate) fn link_modifiers() -> KeyModifiers {
    KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT
}

/// The default leader chord that enters leader mode: Ctrl+B.
///
/// Overridable via the `leader` config key; the client parses it with
/// [`parse_chord`] and falls back to this default.
pub(crate) const LEADER: Chord = Chord::new(KeyCode::Char('b'), KeyModifiers::CONTROL);

/// The keys a user presses: a [`KeyCode`] plus its modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Chord {
    pub(crate) code: KeyCode,
    pub(crate) modifiers: KeyModifiers,
}

impl Chord {
    pub(crate) const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Chord { code, modifiers }
    }

    /// True when `event` is exactly this chord.
    pub(crate) fn matches(self, event: KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
    }

    /// True when `event` is the leader chord: Ctrl+Space in any of its three
    /// crossterm spellings (space-with-ctrl, NUL, literal null).
    pub(crate) fn is_leader(self, event: KeyEvent) -> bool {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        self.matches(event) || (ctrl && matches!(event.code, KeyCode::Char('\0') | KeyCode::Null))
    }
}

/// A leader-mode command, the second half of a binding's `chord + action` pair.
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
    Resize(ResizeDir),
    CyclePane,
    SwapPanes,
    RotateLayout,
    ShowPaneNumbers,
    NextSession,
    PrevSession,
    JumpSession(u8),
    NewTab,
    CloseTab,
    RenameTab,
    NextTab,
    PrevTab,
    JumpTab(u8),
    ToggleSidebar,
    Detach,
    ShowKeybinds,
    EnterCopyMode,
    EnterCopyModeSearch,
}

/// Logical group a binding belongs to, used to organize the showcase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Group {
    Layout,
    Panes,
    Tabs,
    Sessions,
    Chrome,
    General,
}

impl Group {
    pub(crate) const ALL: [Group; 6] =
        [Group::Layout, Group::Panes, Group::Tabs, Group::Sessions, Group::Chrome, Group::General];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Group::Layout => "layout",
            Group::Panes => "panes",
            Group::Tabs => "tabs",
            Group::Sessions => "sessions",
            Group::Chrome => "chrome",
            Group::General => "general",
        }
    }
}

/// One leader binding in the runtime keymap: the dispatch chord, its action,
/// and the showcase row. `keys` is the compact display string — several
/// bindings may share one (e.g. `h/j/k/l`, `1-9`) so the showcase shows a
/// single grouped row for them.
#[derive(Clone)]
pub(crate) struct Binding {
    /// Key pressed after the leader to run `action`.
    pub(crate) key: Chord,
    /// Compact showcase keys, e.g. "h/j/k/l" or "1-9".
    pub(crate) keys: String,
    /// Longer description for the showcase.
    pub(crate) desc: String,
    pub(crate) group: Group,
    pub(crate) action: Action,
}

/// Static stock-binding spec: same fields as [`Binding`] but `&'static str`,
/// so the whole table stays a `const`.
#[derive(Clone, Copy)]
struct BindingSpec {
    key: Chord,
    keys: &'static str,
    desc: &'static str,
    group: Group,
    action: Action,
}

impl Binding {
    fn from_spec(s: &BindingSpec) -> Binding {
        Binding {
            key: s.key,
            keys: s.keys.to_string(),
            desc: s.desc.to_string(),
            group: s.group,
            action: s.action,
        }
    }
}

const fn chord(code: KeyCode) -> Chord {
    Chord::new(code, KeyModifiers::NONE)
}

const fn chord_shift(code: KeyCode) -> Chord {
    Chord::new(code, KeyModifiers::SHIFT)
}

const fn chord_alt(code: KeyCode) -> Chord {
    Chord::new(code, KeyModifiers::ALT)
}

/// Stock leader bindings, in showcase order. Bindings that share a `keys`
/// display string must stay adjacent so the showcase collapses them into one
/// grouped row.
const BINDING_SPECS: &[BindingSpec] = &[
    BindingSpec { key: chord(KeyCode::Char('v')), keys: "v", desc: "split the focused pane vertically", group: Group::Layout, action: Action::SplitVertical },
    BindingSpec { key: chord(KeyCode::Char('-')), keys: "-", desc: "split the focused pane horizontally", group: Group::Layout, action: Action::SplitHorizontal },
    BindingSpec { key: chord(KeyCode::Char('z')), keys: "z", desc: "zoom the focused pane", group: Group::Layout, action: Action::Zoom },
    BindingSpec { key: chord(KeyCode::Char('h')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Left) },
    BindingSpec { key: chord(KeyCode::Char('j')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Down) },
    BindingSpec { key: chord(KeyCode::Char('k')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Up) },
    BindingSpec { key: chord(KeyCode::Char('l')), keys: "h/j/k/l", desc: "move focus left / down / up / right", group: Group::Layout, action: Action::Focus(Dir::Right) },
    BindingSpec { key: chord_shift(KeyCode::Char('H')), keys: "H/J/K/L", desc: "resize the focused pane", group: Group::Layout, action: Action::Resize(ResizeDir::Left) },
    BindingSpec { key: chord_shift(KeyCode::Char('J')), keys: "H/J/K/L", desc: "resize the focused pane", group: Group::Layout, action: Action::Resize(ResizeDir::Down) },
    BindingSpec { key: chord_shift(KeyCode::Char('K')), keys: "H/J/K/L", desc: "resize the focused pane", group: Group::Layout, action: Action::Resize(ResizeDir::Up) },
    BindingSpec { key: chord_shift(KeyCode::Char('L')), keys: "H/J/K/L", desc: "resize the focused pane", group: Group::Layout, action: Action::Resize(ResizeDir::Right) },
    BindingSpec { key: chord(KeyCode::Char('a')), keys: "a", desc: "spawn an AI CLI pane in a vertical split", group: Group::Panes, action: Action::SplitAi },
    BindingSpec { key: chord(KeyCode::Char('x')), keys: "x", desc: "close the focused pane", group: Group::Panes, action: Action::ClosePane },
    BindingSpec { key: chord(KeyCode::Tab), keys: "Tab", desc: "cycle focus between panes", group: Group::Panes, action: Action::CyclePane },
    BindingSpec { key: chord(KeyCode::Char('s')), keys: "s", desc: "swap the focused pane with its sibling", group: Group::Panes, action: Action::SwapPanes },
    BindingSpec { key: chord(KeyCode::Char('o')), keys: "o", desc: "rotate the pane layout", group: Group::Panes, action: Action::RotateLayout },
    BindingSpec { key: chord(KeyCode::Char('q')), keys: "q", desc: "show pane numbers (press a number to jump)", group: Group::Panes, action: Action::ShowPaneNumbers },
    BindingSpec { key: chord(KeyCode::Char('t')), keys: "t", desc: "create a new tab", group: Group::Tabs, action: Action::NewTab },
    BindingSpec { key: chord(KeyCode::Char('&')), keys: "&", desc: "close the active tab", group: Group::Tabs, action: Action::CloseTab },
    BindingSpec { key: chord(KeyCode::Char(',')), keys: ",", desc: "rename the active tab", group: Group::Tabs, action: Action::RenameTab },
    BindingSpec { key: chord(KeyCode::Char('n')), keys: "n/p", desc: "cycle to the next / previous tab", group: Group::Tabs, action: Action::NextTab },
    BindingSpec { key: chord(KeyCode::Char('p')), keys: "n/p", desc: "cycle to the next / previous tab", group: Group::Tabs, action: Action::PrevTab },
    BindingSpec { key: chord(KeyCode::Char('1')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(1) },
    BindingSpec { key: chord(KeyCode::Char('2')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(2) },
    BindingSpec { key: chord(KeyCode::Char('3')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(3) },
    BindingSpec { key: chord(KeyCode::Char('4')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(4) },
    BindingSpec { key: chord(KeyCode::Char('5')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(5) },
    BindingSpec { key: chord(KeyCode::Char('6')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(6) },
    BindingSpec { key: chord(KeyCode::Char('7')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(7) },
    BindingSpec { key: chord(KeyCode::Char('8')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(8) },
    BindingSpec { key: chord(KeyCode::Char('9')), keys: "1-9", desc: "jump to the tab at that position", group: Group::Tabs, action: Action::JumpTab(9) },
    BindingSpec { key: chord(KeyCode::Char('c')), keys: "c", desc: "create a new session (name it in the popup)", group: Group::Sessions, action: Action::NewSession },
    BindingSpec { key: chord(KeyCode::Char('w')), keys: "w", desc: "create a git worktree in a new session", group: Group::Sessions, action: Action::NewWorktree },
    BindingSpec { key: chord(KeyCode::Char(']')), keys: "]/[", desc: "cycle to the next / previous session", group: Group::Sessions, action: Action::NextSession },
    BindingSpec { key: chord(KeyCode::Char('[')), keys: "]/[", desc: "cycle to the next / previous session", group: Group::Sessions, action: Action::PrevSession },
    BindingSpec { key: chord_alt(KeyCode::Char('1')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(1) },
    BindingSpec { key: chord_alt(KeyCode::Char('2')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(2) },
    BindingSpec { key: chord_alt(KeyCode::Char('3')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(3) },
    BindingSpec { key: chord_alt(KeyCode::Char('4')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(4) },
    BindingSpec { key: chord_alt(KeyCode::Char('5')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(5) },
    BindingSpec { key: chord_alt(KeyCode::Char('6')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(6) },
    BindingSpec { key: chord_alt(KeyCode::Char('7')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(7) },
    BindingSpec { key: chord_alt(KeyCode::Char('8')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(8) },
    BindingSpec { key: chord_alt(KeyCode::Char('9')), keys: "alt+1-9", desc: "jump to the session at that list position", group: Group::Sessions, action: Action::JumpSession(9) },
    BindingSpec { key: chord(KeyCode::Char('y')), keys: "y", desc: "enter copy-mode (vi scroll / search / yank)", group: Group::Panes, action: Action::EnterCopyMode },
    BindingSpec { key: chord(KeyCode::Char('/')), keys: "/", desc: "search forward in scrollback (copy-mode)", group: Group::Panes, action: Action::EnterCopyModeSearch },
    BindingSpec { key: chord(KeyCode::Char('b')), keys: "b", desc: "toggle the sidebar", group: Group::Chrome, action: Action::ToggleSidebar },
    BindingSpec { key: chord(KeyCode::Char('d')), keys: "d", desc: "detach (daemon keeps running)", group: Group::General, action: Action::Detach },
    BindingSpec { key: chord(KeyCode::Char('?')), keys: "?", desc: "show all keybindings", group: Group::General, action: Action::ShowKeybinds },
];

/// The stock bindings as a runtime table.
fn stock_bindings() -> Vec<Binding> {
    BINDING_SPECS.iter().map(Binding::from_spec).collect()
}

/// Build the effective keymap: the stock bindings with `[keymap.bindings]`
/// overrides applied. An override rebinds an existing chord or adds a new one;
/// chords that fail to parse and unknown action ids are ignored with a warning.
pub(crate) fn build_keymap(overrides: &HashMap<String, String>) -> Vec<Binding> {
    let mut out = stock_bindings();
    for (key_str, action_str) in overrides {
        let Some(chord) = parse_chord(key_str) else {
            log::warn!("kumo: ignoring keymap binding with invalid key {:?}", key_str);
            continue;
        };
        let Some(action) = action_from_id(action_str) else {
            log::warn!("kumo: ignoring keymap binding with unknown action {:?}", action_str);
            continue;
        };
        out.retain(|b| b.key != chord);
        out.push(Binding {
            key: chord,
            keys: chord_display(chord),
            desc: action_desc(action).to_string(),
            group: action_group(action),
            action,
        });
    }
    out
}

/// Parse a key or chord string from the config, e.g. `v`, `tab`, `f12`,
/// `ctrl+b`, `ctrl+space`, `ctrl+shift+tab`. An uppercase letter implies
/// Shift (so `H` parses to the same chord a terminal reports for Shift+h).
pub(crate) fn parse_chord(raw: &str) -> Option<Chord> {
    let mut modifiers = KeyModifiers::NONE;
    let mut parts: Vec<&str> = raw.split('+').map(|s| s.trim()).collect();
    let key_raw = parts.pop()?;
    let key = key_raw.to_ascii_lowercase();
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "alt" | "opt" | "option" => modifiers |= KeyModifiers::ALT,
            "super" | "cmd" | "command" | "meta" => modifiers |= KeyModifiers::SUPER,
            _ => return None,
        }
    }
    let code = match key.as_str() {
        "space" | "spc" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        _ if key.len() == 1 => {
            let ch = key_raw.chars().next().unwrap();
            if ch.is_ascii_uppercase() {
                modifiers |= KeyModifiers::SHIFT;
            }
            KeyCode::Char(ch)
        }
        f if f.starts_with('f') && f.len() > 1 && f[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = f[1..].parse().unwrap_or(0);
            if (1..=12).contains(&n) {
                KeyCode::F(n)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(Chord::new(code, modifiers))
}

/// Canonical `[keymap.bindings]` id for an action, e.g. `split-vertical`.
#[allow(dead_code)]
pub(crate) fn action_id(action: Action) -> &'static str {
    match action {
        Action::SplitVertical => "split-vertical",
        Action::SplitHorizontal => "split-horizontal",
        Action::SplitAi => "split-ai",
        Action::NewSession => "new-session",
        Action::NewWorktree => "new-worktree",
        Action::ClosePane => "close-pane",
        Action::Zoom => "zoom",
        Action::Focus(Dir::Left) => "focus-left",
        Action::Focus(Dir::Down) => "focus-down",
        Action::Focus(Dir::Up) => "focus-up",
        Action::Focus(Dir::Right) => "focus-right",
        Action::Resize(ResizeDir::Left) => "resize-left",
        Action::Resize(ResizeDir::Down) => "resize-down",
        Action::Resize(ResizeDir::Up) => "resize-up",
        Action::Resize(ResizeDir::Right) => "resize-right",
        Action::CyclePane => "cycle-pane",
        Action::SwapPanes => "swap-panes",
        Action::RotateLayout => "rotate-layout",
        Action::ShowPaneNumbers => "show-pane-numbers",
        Action::NextSession => "next-session",
        Action::PrevSession => "prev-session",
        Action::JumpSession(n) => match n {
            1 => "jump-session-1",
            2 => "jump-session-2",
            3 => "jump-session-3",
            4 => "jump-session-4",
            5 => "jump-session-5",
            6 => "jump-session-6",
            7 => "jump-session-7",
            8 => "jump-session-8",
            9 => "jump-session-9",
            _ => unreachable!("jump-session only supports 1-9"),
        },
        Action::NewTab => "new-tab",
        Action::CloseTab => "close-tab",
        Action::RenameTab => "rename-tab",
        Action::NextTab => "next-tab",
        Action::PrevTab => "prev-tab",
        Action::JumpTab(n) => match n {
            1 => "jump-tab-1",
            2 => "jump-tab-2",
            3 => "jump-tab-3",
            4 => "jump-tab-4",
            5 => "jump-tab-5",
            6 => "jump-tab-6",
            7 => "jump-tab-7",
            8 => "jump-tab-8",
            9 => "jump-tab-9",
            _ => unreachable!("jump-tab only supports 1-9"),
        },
        Action::ToggleSidebar => "toggle-sidebar",
        Action::Detach => "detach",
        Action::ShowKeybinds => "show-keybinds",
        Action::EnterCopyMode => "copy-mode",
        Action::EnterCopyModeSearch => "copy-mode-search",
    }
}

/// Reverse of [`action_id`]. `None` for unknown ids.
pub(crate) fn action_from_id(id: &str) -> Option<Action> {
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
        "resize-left" => Action::Resize(ResizeDir::Left),
        "resize-down" => Action::Resize(ResizeDir::Down),
        "resize-up" => Action::Resize(ResizeDir::Up),
        "resize-right" => Action::Resize(ResizeDir::Right),
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
        "new-tab" => Action::NewTab,
        "close-tab" => Action::CloseTab,
        "rename-tab" => Action::RenameTab,
        "next-tab" => Action::NextTab,
        "prev-tab" => Action::PrevTab,
        "jump-tab-1" => Action::JumpTab(1),
        "jump-tab-2" => Action::JumpTab(2),
        "jump-tab-3" => Action::JumpTab(3),
        "jump-tab-4" => Action::JumpTab(4),
        "jump-tab-5" => Action::JumpTab(5),
        "jump-tab-6" => Action::JumpTab(6),
        "jump-tab-7" => Action::JumpTab(7),
        "jump-tab-8" => Action::JumpTab(8),
        "jump-tab-9" => Action::JumpTab(9),
        "toggle-sidebar" => Action::ToggleSidebar,
        "detach" => Action::Detach,
        "show-keybinds" => Action::ShowKeybinds,
        "copy-mode" | "enter-copy-mode" => Action::EnterCopyMode,
        "copy-mode-search" | "copy-search" => Action::EnterCopyModeSearch,
        _ => return None,
    })
}

/// Showcase description for an action (used for config-added bindings).
pub(crate) fn action_desc(action: Action) -> &'static str {
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
        Action::NewTab => "create a new tab",
        Action::CloseTab => "close the active tab",
        Action::RenameTab => "rename the active tab",
        Action::NextTab | Action::PrevTab => "cycle to the next / previous tab",
        Action::JumpTab(_) => "jump to the tab at that position",
        Action::ToggleSidebar => "toggle the sidebar",
        Action::Detach => "detach (daemon keeps running)",
        Action::ShowKeybinds => "show all keybindings",
        Action::EnterCopyMode => "enter copy-mode (vi scroll / search / yank)",
        Action::EnterCopyModeSearch => "search forward (enter copy-mode)",
    }
}

/// Showcase group for an action (used for config-added bindings).
pub(crate) fn action_group(action: Action) -> Group {
    match action {
        Action::SplitVertical
        | Action::SplitHorizontal
        | Action::Zoom
        | Action::Focus(_)
        | Action::Resize(_) => Group::Layout,
        Action::SplitAi | Action::ClosePane | Action::CyclePane | Action::SwapPanes
        | Action::RotateLayout | Action::ShowPaneNumbers | Action::EnterCopyMode | Action::EnterCopyModeSearch => Group::Panes,
        Action::NewTab | Action::CloseTab | Action::RenameTab | Action::NextTab | Action::PrevTab | Action::JumpTab(_) => Group::Tabs,
        Action::NewSession | Action::NewWorktree | Action::NextSession | Action::PrevSession
        | Action::JumpSession(_) => Group::Sessions,
        Action::ToggleSidebar => Group::Chrome,
        Action::Detach | Action::ShowKeybinds => Group::General,
    }
}

/// Human-readable form of a chord, e.g. `v`, `ctrl+b`, `H`. Used for the
/// showcase row of config-added bindings.
pub(crate) fn chord_display(c: Chord) -> String {
    let implicit_shift = matches!(c.code, KeyCode::Char(ch) if ch.is_ascii_uppercase());
    let mut s = String::new();
    if c.modifiers.contains(KeyModifiers::CONTROL) {
        s.push_str("ctrl+");
    }
    if !implicit_shift && c.modifiers.contains(KeyModifiers::SHIFT) {
        s.push_str("shift+");
    }
    if c.modifiers.contains(KeyModifiers::ALT) {
        s.push_str("alt+");
    }
    if c.modifiers.contains(KeyModifiers::SUPER) {
        s.push_str("super+");
    }
    s.push_str(&key_name(c.code));
    s
}

fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}"),
    }
}

/// The dispatch chord bound to `action` in the stock keymap, if any. Useful to
/// validate config remaps ("is this action remappable?") and to show an
/// action's default key.
#[allow(dead_code)]
pub(crate) fn key_for(action: Action) -> Option<Chord> {
    stock_bindings().iter().find(|b| b.action == action).map(|b| b.key)
}

/// The leader-mode status-bar hint: just the `?` pointer to the keybind
/// showcase. Everything else lives in the showcase, so the strip stays tiny and
/// never drifts from the table.
#[allow(dead_code)]
pub(crate) fn leader_hint(keymap: &[Binding]) -> String {
    let help = keymap.iter().find(|b| b.keys == "?").expect("help binding present");
    format!(" {}: {} ", help.keys, help.desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_non_empty_and_has_all_groups() {
        let bindings = stock_bindings();
        assert!(!bindings.is_empty());
        for group in Group::ALL {
            assert!(
                bindings.iter().any(|b| b.group == group),
                "no binding in group {:?}",
                group
            );
        }
    }

    #[test]
    fn every_binding_has_a_dispatch_chord() {
        for b in stock_bindings() {
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
        let mut chords: Vec<Chord> = stock_bindings().iter().map(|b| b.key).collect();
        chords.sort_by_key(|c| format!("{:?}{:?}", c.code, c.modifiers));
        for pair in chords.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate dispatch chord {:?}", pair[0]);
        }
    }

    #[test]
    fn key_for_reverses_dispatch() {
        for b in stock_bindings() {
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
        let hint = leader_hint(&stock_bindings());
        assert!(hint.contains("?: show all keybindings"));
        // The hint no longer lists every binding; the showcase is the reference.
        assert!(!hint.contains("v-split"));
        assert!(!hint.contains("esc: exit"));
    }

    #[test]
    fn default_leader_is_ctrl_b() {
        assert_eq!(LEADER, Chord::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    }

    #[test]
    fn ctrl_space_leader_matches_all_spellings() {
        let chord = parse_chord("ctrl+space").expect("ctrl+space parses");
        assert_eq!(chord, Chord::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Char('\0'), KeyModifiers::CONTROL)));
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Null, KeyModifiers::CONTROL)));
        assert!(!chord.is_leader(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(!chord.is_leader(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn ctrl_b_leader_matches_ctrl_b_only() {
        let chord = parse_chord("ctrl+b").expect("ctrl+b parses");
        assert_eq!(chord, LEADER);
        assert!(chord.is_leader(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)));
        assert!(!chord.is_leader(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)));
        assert!(!chord.is_leader(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)));
    }

    #[test]
    fn parse_chord_accepts_and_rejects() {
        assert_eq!(parse_chord("F12"), Some(Chord::new(KeyCode::F(12), KeyModifiers::NONE)));
        assert_eq!(parse_chord("ctrl+f1"), Some(Chord::new(KeyCode::F(1), KeyModifiers::CONTROL)));
        assert_eq!(parse_chord("ctrl+shift+tab"), Some(Chord::new(KeyCode::Tab, KeyModifiers::CONTROL | KeyModifiers::SHIFT)));
        assert_eq!(parse_chord("ctrl+b"), Some(Chord::new(KeyCode::Char('b'), KeyModifiers::CONTROL)));
        assert_eq!(parse_chord("x"), Some(Chord::new(KeyCode::Char('x'), KeyModifiers::NONE)));
        // Uppercase implies Shift, matching the chord a terminal reports.
        assert_eq!(parse_chord("H"), Some(Chord::new(KeyCode::Char('H'), KeyModifiers::SHIFT)));
        assert_eq!(parse_chord("ctrl+banana"), None, "unknown key rejected");
        assert_eq!(parse_chord("nonsense+ctrl"), None, "unknown modifier rejected");
        assert_eq!(parse_chord(""), None, "empty string rejected");
        assert_eq!(parse_chord("f13"), None, "f13 is out of range");
    }

    #[test]
    fn action_ids_round_trip() {
        for b in stock_bindings() {
            assert_eq!(
                action_from_id(action_id(b.action)),
                Some(b.action),
                "id round-trip failed for {:?}",
                b.action
            );
        }
    }

    #[test]
    fn build_keymap_overrides_and_adds_bindings() {
        let mut overrides = HashMap::new();
        overrides.insert("s".to_string(), "split-vertical".to_string());
        overrides.insert("v".to_string(), "close-pane".to_string()); // rebind v
        let keymap = build_keymap(&overrides);
        assert!(keymap.iter().any(|b| b.action == Action::ClosePane && b.key.code == KeyCode::Char('v')));
        assert!(keymap.iter().any(|b| b.action == Action::SplitVertical && b.key.code == KeyCode::Char('s')));
        // A config-added binding gets a showcase row with the parsed chord.
        let added = keymap.iter().find(|b| b.key.code == KeyCode::Char('s')).unwrap();
        assert_eq!(added.keys, "s");
        assert_eq!(added.desc, action_desc(Action::SplitVertical));
    }

    #[test]
    fn build_keymap_ignores_invalid_entries() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl+banana".to_string(), "zoom".to_string());
        overrides.insert("s".to_string(), "no-such-action".to_string());
        let keymap = build_keymap(&overrides);
        let stock = stock_bindings();
        assert_eq!(keymap.len(), stock.len(), "invalid entries must be ignored");
    }
}
