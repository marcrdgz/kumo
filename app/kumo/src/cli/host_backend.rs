//! Host-terminal backend that stays silent when a frame changes nothing.
//!
//! ratatui re-sends "show cursor" + "move cursor" on every draw, even when the
//! diff has no cells. The client redraws on every daemon event, and terminals
//! such as Ghostty restart their cursor-blink cycle whenever output arrives, so
//! those redundant bytes made the cursor blink erratically fast. Tracking the
//! cursor state here lets an unchanged frame write nothing at all.

use std::io;

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

pub struct HostBackend<B> {
    inner: B,
    /// Last visibility sent; `None` until the first show/hide.
    visible: Option<bool>,
    /// Where the host cursor is known to be; `None` once a cell write or clear
    /// may have moved it.
    position: Option<Position>,
}

impl<B> HostBackend<B> {
    pub fn new(inner: B) -> Self {
        Self { inner, visible: None, position: None }
    }
}

impl<B: Backend> Backend for HostBackend<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            return Ok(());
        }
        self.position = None;
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.position = None;
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        if self.visible == Some(false) {
            return Ok(());
        }
        self.visible = Some(false);
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        if self.visible == Some(true) {
            return Ok(());
        }
        self.visible = Some(true);
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let position = position.into();
        if self.position == Some(position) {
            return Ok(());
        }
        self.position = Some(position);
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.position = None;
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.position = None;
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

/// The backend the TUI client draws through.
pub type StdoutBackend = HostBackend<ratatui::backend::CrosstermBackend<io::Stdout>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::Paragraph;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    /// Shared sink so the test can count the bytes the terminal emitted.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unchanged_frame_writes_nothing() {
        let sink = Sink::default();
        let backend = HostBackend::new(CrosstermBackend::new(sink.clone()));
        let viewport = Viewport::Fixed(Rect::new(0, 0, 10, 3));
        let mut term = Terminal::with_options(backend, TerminalOptions { viewport }).unwrap();
        let mut draw = |text: &str| {
            let before = sink.0.lock().unwrap().len();
            term.draw(|f| {
                f.render_widget(Paragraph::new(text.to_string()), f.area());
                f.set_cursor_position((2, 1));
            })
            .unwrap();
            sink.0.lock().unwrap().len() - before
        };
        assert!(draw("a") > 0);
        assert_eq!(draw("a"), 0, "an identical frame must not touch the cursor");
        assert!(draw("b") > 0, "a changed cell is still drawn and the cursor restored");
    }
}
