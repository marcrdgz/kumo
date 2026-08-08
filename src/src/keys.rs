use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode a crossterm `KeyEvent` into the byte stream to send to the PTY.
/// The terminal emulator in the pane decodes it exactly like a real terminal
/// would (terminfo-style CSI sequences).
pub fn encode(e: KeyEvent) -> Vec<u8> {
    let ctrl = e.modifiers.contains(KeyModifiers::CONTROL);
    let alt = e.modifiers.contains(KeyModifiers::ALT);
    let shift = e.modifiers.contains(KeyModifiers::SHIFT);

    match e.code {
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => {
            // On macOS, Option+Delete (Alt+Backspace) deletes a whole word.
            // Send ESC DEL (Meta+Backspace) so the shell's word-delete
            // binding fires instead of a single-character delete.
            if alt {
                vec![0x1b, 0x7f]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Tab if shift => vec![0x1b, b'[', b'Z'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi_arrow(b'A', ctrl, alt, shift),
        KeyCode::Down => csi_arrow(b'B', ctrl, alt, shift),
        KeyCode::Right => csi_arrow(b'C', ctrl, alt, shift),
        KeyCode::Left => csi_arrow(b'D', ctrl, alt, shift),
        KeyCode::Home => csi_mod(b'H', b'H', ctrl, alt, shift),
        KeyCode::End => csi_mod(b'F', b'F', ctrl, alt, shift),
        KeyCode::PageUp => csi_tilde(b'5', ctrl, alt, shift),
        KeyCode::PageDown => csi_tilde(b'6', ctrl, alt, shift),
        KeyCode::Delete => csi_tilde(b'3', ctrl, alt, shift),
        KeyCode::Insert => csi_tilde(b'2', ctrl, alt, shift),
        KeyCode::F(n) => function_key(n),
        KeyCode::Char(c) => encode_char(c, ctrl, alt),
        KeyCode::Null => vec![0x00],
        _ => vec![],
    }
}

fn csi_arrow(code: u8, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
    if ctrl || alt || shift {
        let m = modifier_param(ctrl, alt, shift);
        format!("\x1b[1;{}{}", m, code as char).into_bytes()
    } else {
        vec![0x1b, b'[', code]
    }
}

fn csi_mod(bare: u8, mod_key: u8, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
    if ctrl || alt || shift {
        let m = modifier_param(ctrl, alt, shift);
        format!("\x1b[1;{}{}", m, mod_key as char).into_bytes()
    } else {
        vec![0x1b, b'[', bare]
    }
}

fn csi_tilde(code: u8, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
    if ctrl || alt || shift {
        let m = modifier_param(ctrl, alt, shift);
        format!("\x1b[{};{}~", code as char, m).into_bytes()
    } else {
        format!("\x1b[{}~", code as char).into_bytes()
    }
}

fn modifier_param(ctrl: bool, alt: bool, shift: bool) -> u8 {
    match (shift, alt, ctrl) {
        (false, false, true) => 5,
        (true, false, true) => 6,
        (false, true, false) => 3,
        (true, true, false) => 4,
        (false, true, true) => 7,
        (true, true, true) => 8,
        (true, false, false) => 2,
        _ => 1,
    }
}

fn function_key(n: u8) -> Vec<u8> {
    match n {
        1 => vec![0x1b, b'O', b'P'],
        2 => vec![0x1b, b'O', b'Q'],
        3 => vec![0x1b, b'O', b'R'],
        4 => vec![0x1b, b'O', b'S'],
        5 => vec![0x1b, b'[', b'1', b'5', b'~'],
        6 => vec![0x1b, b'[', b'1', b'7', b'~'],
        7 => vec![0x1b, b'[', b'1', b'8', b'~'],
        8 => vec![0x1b, b'[', b'1', b'9', b'~'],
        9 => vec![0x1b, b'[', b'2', b'0', b'~'],
        10 => vec![0x1b, b'[', b'2', b'1', b'~'],
        11 => vec![0x1b, b'[', b'2', b'3', b'~'],
        12 => vec![0x1b, b'[', b'2', b'4', b'~'],
        _ => vec![],
    }
}

fn encode_char(c: char, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    if ctrl {
        let code = ctrl_code(c);
        out.push(code);
    } else {
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    out
}

/// Control code for a Ctrl+letter / Ctrl+symbol chord.
fn ctrl_code(c: char) -> u8 {
    let lc = c.to_ascii_lowercase();
    if ('a'..='z').contains(&lc) {
        (lc as u8) & 0x1f
    } else {
        match c {
            ' ' => 0x00,
            '@' => 0x00,
            '[' => 0x1b,
            '\\' => 0x1c,
            ']' => 0x1d,
            '^' => 0x1e,
            '_' => 0x1f,
            '?' => 0x7f,
            _ => 0x00,
        }
    }
}
