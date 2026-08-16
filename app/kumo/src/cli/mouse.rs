//! Mouse helpers for the TUI client: SGR-mouse byte encoding for forwarding
//! gestures to panes that own the mouse (e.g. opencode).

/// Encode an SGR mouse event for one pane: `CSI < b ; x ; y M|m`. `button` is
/// the 0-based button (add 32 for motion/drag), `col`/`row` are 1-based, and
/// `release` emits the `m` release form.
pub fn sgr_mouse(button: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let b = if release { button | 3 } else { button };
    format!("\x1b[<{b};{col};{row}{}", if release { "m" } else { "M" }).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_and_release_forms() {
        assert_eq!(sgr_mouse(0, 1, 1, false), b"\x1b[<0;1;1M");
        assert_eq!(sgr_mouse(35, 5, 3, false), b"\x1b[<35;5;3M");
        assert_eq!(sgr_mouse(0, 5, 3, true), b"\x1b[<3;5;3m", "release sets the low bits + m");
    }
}
