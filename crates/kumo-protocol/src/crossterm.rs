//! Conversions between the wire types and `crossterm` event types, gated behind
//! the `crossterm` feature (enabled by the TUI client and daemon, off for the
//! desktop app / mobile so they never pull `crossterm` in).
//!
//! The daemon and TUI client interpret keys, mouse events, and modifiers through
//! `crossterm`; keeping the conversions here (behind the feature) means the
//! protocol stays usable by clients that speak it directly.

use crate::{WireKeyCode, WireKeyEvent, WireModifiers, WireMouseButton, WireMouseEvent, WireMouseKind};

impl WireKeyCode {
    pub fn from_crossterm(code: crossterm::event::KeyCode) -> Self {
        use crossterm::event::KeyCode as K;
        match code {
            K::Backspace => Self::Backspace,
            K::Enter => Self::Enter,
            K::Left => Self::Left,
            K::Right => Self::Right,
            K::Up => Self::Up,
            K::Down => Self::Down,
            K::Home => Self::Home,
            K::End => Self::End,
            K::PageUp => Self::PageUp,
            K::PageDown => Self::PageDown,
            K::Tab => Self::Tab,
            K::BackTab => Self::BackTab,
            K::Delete => Self::Delete,
            K::Insert => Self::Insert,
            K::Esc => Self::Esc,
            K::Null => Self::Null,
            K::F(n) => Self::F(n),
            K::Char(c) => Self::Char(c),
            K::CapsLock
            | K::ScrollLock
            | K::NumLock
            | K::PrintScreen
            | K::Pause
            | K::Menu
            | K::KeypadBegin => Self::Modifier,
            _ => Self::Media,
        }
    }

    pub fn to_crossterm(self) -> crossterm::event::KeyCode {
        use crossterm::event::KeyCode as K;
        match self {
            Self::Backspace => K::Backspace,
            Self::Enter => K::Enter,
            Self::Left => K::Left,
            Self::Right => K::Right,
            Self::Up => K::Up,
            Self::Down => K::Down,
            Self::Home => K::Home,
            Self::End => K::End,
            Self::PageUp => K::PageUp,
            Self::PageDown => K::PageDown,
            Self::Tab => K::Tab,
            Self::BackTab => K::BackTab,
            Self::Delete => K::Delete,
            Self::Insert => K::Insert,
            Self::Esc => K::Esc,
            Self::Null => K::Null,
            Self::F(n) => K::F(n),
            Self::Char(c) => K::Char(c),
            _ => K::Null,
        }
    }
}

impl WireModifiers {
    pub fn from_crossterm(m: crossterm::event::KeyModifiers) -> Self {
        use crossterm::event::KeyModifiers as M;
        let mut bits = 0u8;
        if m.contains(M::SHIFT) {
            bits |= 1 << 0;
        }
        if m.contains(M::CONTROL) {
            bits |= 1 << 1;
        }
        if m.contains(M::ALT) {
            bits |= 1 << 2;
        }
        if m.contains(M::SUPER) {
            bits |= 1 << 3;
        }
        if m.contains(M::HYPER) {
            bits |= 1 << 4;
        }
        if m.contains(M::META) {
            bits |= 1 << 5;
        }
        Self::from_raw(bits)
    }

    pub fn to_crossterm(self) -> crossterm::event::KeyModifiers {
        use crossterm::event::KeyModifiers as M;
        let mut m = M::empty();
        if self.shift() {
            m |= M::SHIFT;
        }
        if self.control() {
            m |= M::CONTROL;
        }
        if self.alt() {
            m |= M::ALT;
        }
        if self.super_key() {
            m |= M::SUPER;
        }
        if self.hyper() {
            m |= M::HYPER;
        }
        if self.meta() {
            m |= M::META;
        }
        m
    }
}

impl From<crossterm::event::KeyEvent> for WireKeyEvent {
    fn from(k: crossterm::event::KeyEvent) -> Self {
        Self::new(WireKeyCode::from_crossterm(k.code), WireModifiers::from_crossterm(k.modifiers))
    }
}

impl WireKeyEvent {
    pub fn to_crossterm(self) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(self.code.to_crossterm(), self.modifiers.to_crossterm())
    }
}

fn wire_button_to_crossterm(b: WireMouseButton) -> crossterm::event::MouseButton {
    match b {
        WireMouseButton::Left => crossterm::event::MouseButton::Left,
        WireMouseButton::Right => crossterm::event::MouseButton::Right,
        WireMouseButton::Middle => crossterm::event::MouseButton::Middle,
    }
}

fn crossterm_button_to_wire(b: crossterm::event::MouseButton) -> WireMouseButton {
    match b {
        crossterm::event::MouseButton::Left => WireMouseButton::Left,
        crossterm::event::MouseButton::Right => WireMouseButton::Right,
        _ => WireMouseButton::Middle,
    }
}

impl From<crossterm::event::MouseEvent> for WireMouseEvent {
    fn from(m: crossterm::event::MouseEvent) -> Self {
        use crossterm::event::MouseEventKind as K;
        let kind = match m.kind {
            K::Down(b) => WireMouseKind::Down(crossterm_button_to_wire(b)),
            K::Up(b) => WireMouseKind::Up(crossterm_button_to_wire(b)),
            K::Drag(b) => WireMouseKind::Drag(crossterm_button_to_wire(b)),
            K::Moved => WireMouseKind::Moved,
            K::ScrollUp => WireMouseKind::ScrollUp,
            K::ScrollDown => WireMouseKind::ScrollDown,
            K::ScrollLeft => WireMouseKind::ScrollLeft,
            K::ScrollRight => WireMouseKind::ScrollRight,
        };
        WireMouseEvent {
            kind,
            col: m.column,
            row: m.row,
            modifiers: WireModifiers::from_crossterm(m.modifiers),
        }
    }
}

impl WireMouseEvent {
    pub fn to_crossterm(self) -> crossterm::event::MouseEvent {
        use crossterm::event::MouseEventKind as K;
        let kind = match self.kind {
            WireMouseKind::Down(b) => K::Down(wire_button_to_crossterm(b)),
            WireMouseKind::Up(b) => K::Up(wire_button_to_crossterm(b)),
            WireMouseKind::Drag(b) => K::Drag(wire_button_to_crossterm(b)),
            WireMouseKind::Moved => K::Moved,
            WireMouseKind::ScrollUp => K::ScrollUp,
            WireMouseKind::ScrollDown => K::ScrollDown,
            WireMouseKind::ScrollLeft => K::ScrollLeft,
            WireMouseKind::ScrollRight => K::ScrollRight,
        };
        crossterm::event::MouseEvent {
            kind,
            column: self.col,
            row: self.row,
            modifiers: self.modifiers.to_crossterm(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_roundtrip() {
        for code in [
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyCode::F(12),
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyCode::Backspace,
        ] {
            let k = crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::CONTROL);
            let wire: WireKeyEvent = k.into();
            let back = wire.to_crossterm();
            assert_eq!(back.code, code);
            assert!(back.modifiers.contains(crossterm::event::KeyModifiers::CONTROL));
        }
    }

    #[test]
    fn modifiers_roundtrip_all_bits() {
        use crossterm::event::KeyModifiers as M;
        let mods = M::SHIFT | M::CONTROL | M::ALT | M::SUPER | M::HYPER | M::META;
        let wire = WireModifiers::from_crossterm(mods);
        assert!(wire.shift() && wire.control() && wire.alt() && wire.super_key() && wire.hyper() && wire.meta());
        assert_eq!(wire.to_crossterm(), mods);
    }
}
