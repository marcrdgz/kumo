//! Single-line text editing shared by the client's popup fields (name popup,
//! worktree dialog, finder, command palette, copy-mode search).
//!
//! [`TextInput`] borrows the caller's `String` and char-indexed cursor, so each
//! popup keeps its own state layout and renderer; only the key handling is
//! shared. Callers handle their own Enter/Esc/navigation keys first and feed
//! the rest here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did to a [`TextInput`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextEdit {
    /// Not an editing key; the caller may handle it.
    Ignored,
    /// An editing key that left the text as is (cursor move, or a delete
    /// with nothing to delete).
    Moved,
    /// The text changed.
    Changed,
}

/// Readline-style editing over a borrowed `String` and its cursor (a char
/// index, not a byte offset).
pub(crate) struct TextInput<'a> {
    text: &'a mut String,
    cursor: &'a mut usize,
}

impl<'a> TextInput<'a> {
    pub(crate) fn new(text: &'a mut String, cursor: &'a mut usize) -> Self {
        *cursor = (*cursor).min(text.chars().count());
        TextInput { text, cursor }
    }

    /// Apply one editing key: printable chars, Backspace/Delete (word-wise
    /// with Ctrl/Alt/Super), Ctrl+H/W/U, and Left/Right/Home/End.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> TextEdit {
        let word = key.modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let changed = match key.code {
            KeyCode::Backspace if word => self.delete_word_backward(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Char('h') if ctrl => self.backspace(),
            KeyCode::Char('w') if ctrl => self.delete_word_backward(),
            KeyCode::Char('u') if ctrl => self.delete_to_start(),
            KeyCode::Delete if word => self.delete_word_forward(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Left => {
                *self.cursor = self.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                *self.cursor = (*self.cursor + 1).min(self.len());
                false
            }
            KeyCode::Home => {
                *self.cursor = 0;
                false
            }
            KeyCode::End => {
                *self.cursor = self.len();
                false
            }
            KeyCode::Char(c) if !ctrl => {
                self.insert_str(&c.to_string());
                true
            }
            _ => return TextEdit::Ignored,
        };
        // An editing key with nothing to act on (e.g. Backspace at 0) still
        // counts as consumed so it never leaks to the caller's fallbacks.
        if changed { TextEdit::Changed } else { TextEdit::Moved }
    }

    /// Insert `s` at the cursor and move past it.
    pub(crate) fn insert_str(&mut self, s: &str) {
        let b = char_idx_to_byte(self.text, *self.cursor);
        self.text.insert_str(b, s);
        *self.cursor += s.chars().count();
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn backspace(&mut self) -> bool {
        if *self.cursor == 0 {
            return false;
        }
        let end = char_idx_to_byte(self.text, *self.cursor);
        let start = char_idx_to_byte(self.text, *self.cursor - 1);
        self.text.replace_range(start..end, "");
        *self.cursor -= 1;
        true
    }

    fn delete_forward(&mut self) -> bool {
        if *self.cursor >= self.len() {
            return false;
        }
        let start = char_idx_to_byte(self.text, *self.cursor);
        let end = char_idx_to_byte(self.text, *self.cursor + 1);
        self.text.replace_range(start..end, "");
        true
    }

    fn delete_to_start(&mut self) -> bool {
        if *self.cursor == 0 {
            return false;
        }
        let b = char_idx_to_byte(self.text, *self.cursor);
        self.text.replace_range(..b, "");
        *self.cursor = 0;
        true
    }

    /// Delete the whitespace-delimited word before the cursor.
    fn delete_word_backward(&mut self) -> bool {
        let chars: Vec<char> = self.text.chars().collect();
        let end = *self.cursor;
        let mut start = end;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == end {
            return false;
        }
        let (sb, eb) = (char_idx_to_byte(self.text, start), char_idx_to_byte(self.text, end));
        self.text.replace_range(sb..eb, "");
        *self.cursor = start;
        true
    }

    /// Delete the whitespace-delimited word after the cursor.
    fn delete_word_forward(&mut self) -> bool {
        let chars: Vec<char> = self.text.chars().collect();
        let start = *self.cursor;
        let mut end = start;
        while end < chars.len() && chars[end].is_whitespace() {
            end += 1;
        }
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }
        if start == end {
            return false;
        }
        let (sb, eb) = (char_idx_to_byte(self.text, start), char_idx_to_byte(self.text, end));
        self.text.replace_range(sb..eb, "");
        true
    }
}

/// Byte offset of the `ci`-th char in `s` (or `s.len()` past the end).
fn char_idx_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Feed `keys` to a field holding `text` with the cursor at `cursor`.
    fn edit(text: &str, cursor: usize, keys: &[KeyEvent]) -> (String, usize) {
        let (mut text, mut cursor) = (text.to_string(), cursor);
        let mut input = TextInput::new(&mut text, &mut cursor);
        for k in keys {
            input.handle_key(*k);
        }
        (text, cursor)
    }

    #[test]
    fn inserts_and_deletes_around_multibyte_chars() {
        assert_eq!(edit("cfé", 1, &[key(KeyCode::Char('a'))]), ("café".to_string(), 2));
        assert_eq!(edit("café", 4, &[key(KeyCode::Backspace)]), ("caf".to_string(), 3));
        assert_eq!(edit("界面x", 0, &[key(KeyCode::Delete)]), ("面x".to_string(), 0));
        assert_eq!(edit("ab", 0, &[key(KeyCode::Backspace)]), ("ab".to_string(), 0));
        assert_eq!(edit("ab", 2, &[key(KeyCode::Delete)]), ("ab".to_string(), 2));
    }

    #[test]
    fn readline_word_and_line_deletes() {
        assert_eq!(edit("foo bar", 7, &[ctrl('w')]), ("foo ".to_string(), 4));
        assert_eq!(edit("foo bar  ", 9, &[KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT)]), ("foo ".to_string(), 4));
        assert_eq!(edit("foo bar", 4, &[KeyEvent::new(KeyCode::Delete, KeyModifiers::ALT)]), ("foo ".to_string(), 4));
        assert_eq!(edit("foo bar", 4, &[ctrl('u')]), ("bar".to_string(), 0));
        assert_eq!(edit("foo", 3, &[ctrl('h')]), ("fo".to_string(), 2));
    }

    #[test]
    fn cursor_moves_stay_in_bounds() {
        assert_eq!(edit("ab", 0, &[key(KeyCode::Left)]).1, 0);
        assert_eq!(edit("ab", 2, &[key(KeyCode::Right)]).1, 2);
        assert_eq!(edit("ab", 1, &[key(KeyCode::Home)]).1, 0);
        assert_eq!(edit("ab", 0, &[key(KeyCode::End)]).1, 2);
        // A stale cursor past the end is clamped instead of panicking.
        assert_eq!(edit("ab", 9, &[key(KeyCode::Char('c'))]), ("abc".to_string(), 3));
    }

    #[test]
    fn reports_what_changed() {
        let (mut text, mut cursor) = ("ab".to_string(), 1);
        let mut input = TextInput::new(&mut text, &mut cursor);
        assert_eq!(input.handle_key(key(KeyCode::Left)), TextEdit::Moved);
        assert_eq!(input.handle_key(key(KeyCode::Char('x'))), TextEdit::Changed);
        assert_eq!(input.handle_key(key(KeyCode::Enter)), TextEdit::Ignored);
        assert_eq!(input.handle_key(ctrl('a')), TextEdit::Ignored);
    }
}
