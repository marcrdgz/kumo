use std::path::PathBuf;
use std::sync::mpsc::Sender;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{test::TermSize, Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use anyhow::Result;
use neomux_core::pty::{Pty, PtySpec};
use portable_pty::PtySize;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier};

/// Catppuccin mocha ANSI 16-color palette.
const PALETTE: [RColor; 16] = [
    RColor::Rgb(0x45, 0x47, 0x5a), // Black
    RColor::Rgb(0xf3, 0x8b, 0xa8), // Red
    RColor::Rgb(0xa6, 0xe3, 0xa1), // Green
    RColor::Rgb(0xf9, 0xe2, 0xaf), // Yellow
    RColor::Rgb(0x89, 0xb4, 0xfa), // Blue
    RColor::Rgb(0xf5, 0xc2, 0xe7), // Magenta
    RColor::Rgb(0x94, 0xe2, 0xd5), // Cyan
    RColor::Rgb(0xba, 0xc2, 0xde), // White
    RColor::Rgb(0x58, 0x5b, 0x70), // BrightBlack
    RColor::Rgb(0xf3, 0x8b, 0xa8), // BrightRed
    RColor::Rgb(0xa6, 0xe3, 0xa1), // BrightGreen
    RColor::Rgb(0xf9, 0xe2, 0xaf), // BrightYellow
    RColor::Rgb(0x89, 0xb4, 0xfa), // BrightBlue
    RColor::Rgb(0xf5, 0xc2, 0xe7), // BrightMagenta
    RColor::Rgb(0x94, 0xe2, 0xd5), // BrightCyan
    RColor::Rgb(0xcd, 0xd6, 0xf4), // BrightWhite
];

pub const FG: RColor = RColor::Rgb(0xcd, 0xd6, 0xf4);
pub const BG: RColor = RColor::Rgb(0x1e, 0x1e, 0x2e);
pub const ACCENT: RColor = RColor::Rgb(0xb4, 0xbe, 0xfe);

/// A live pane: PTY + a real terminal emulator (alacritty) fed from it.
#[allow(dead_code)]
pub struct Pane {
    pub id: u64,
    pub session_id: u64,
    pub is_ai: bool,
    pub pty: Pty,
    pub term: Term<VoidListener>,
    parser: Processor,
    pub dead: bool,
}

#[derive(Clone)]
pub enum PtyEvent {
    Output { pane_id: u64, data: Vec<u8> },
}

impl Pane {
    pub fn spawn(
        session_id: u64,
        id: u64,
        shell: String,
        program: Option<(String, Vec<String>)>,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        is_ai: bool,
        events_tx: Sender<PtyEvent>,
    ) -> Result<Pane> {
        let pty = Pty::spawn(&PtySpec {
            shell,
            program,
            cwd,
            cols,
            rows,
        })?;

        let mut config = Config::default();
        config.scrolling_history = 10_000;
        let size = TermSize::new(cols as usize, rows as usize);
        let term = Term::new(config, &size, VoidListener);

        let pane = Pane {
            id,
            session_id,
            is_ai,
            pty,
            term,
            parser: Processor::new(),
            dead: false,
        };

        let reader = pane.pty.master.try_clone_reader()?;
        let tx = events_tx.clone();
        let pid = id;
        Pty::read_loop(reader, move |data| {
            let _ = tx.send(PtyEvent::Output { pane_id: pid, data });
        });

        Ok(pane)
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.parser.advance(&mut self.term, data);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = TermSize::new(cols as usize, rows as usize);
        self.term.resize(size);
        let _ = self.pty.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    pub fn write(&mut self, data: &[u8]) {
        let _ = self.pty.write(data);
    }

    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
    }

    pub fn has_mouse_reporting(&self) -> bool {
        self.term.mode().contains(TermMode::MOUSE_MODE)
    }

    /// Render the emulator viewport into `buf` at `area`. If `cursor`, draw the
    /// terminal cursor as an inverted cell.
    pub fn render(&self, area: Rect, focused: bool, buf: &mut Buffer) {
        let area_x = area.x;
        let area_y = area.y;
        let aw = area.width as i32;
        let ah = area.height as i32;

        // Prefill with the background color.
        for y in area_y..(area_y + area.height) {
            for x in area_x..(area_x + area.width) {
                buf.cell_mut((x, y)).unwrap()
                    .set_char(' ')
                    .set_fg(FG)
                    .set_bg(BG);
            }
        }

        let content = self.term.renderable_content();
        let display_offset = content.display_offset as i32;

        for cellref in content.display_iter {
            let point = cellref.point;
            let row = point.line.0 + display_offset;
            let col = point.column.0 as i32;
            if row < 0 || row >= ah || col < 0 || col >= aw {
                continue;
            }
            let cell = cellref.cell;
            let spacer = cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER);

            let bcell = buf.cell_mut((area_x + col as u16, area_y + row as u16)).unwrap();
            if spacer {
                bcell.set_char(' ').set_fg(to_color(cell.fg)).set_bg(to_color(cell.bg));
                continue;
            }
            let mut mods = Modifier::empty();
            if cell.flags.contains(Flags::BOLD) {
                mods |= Modifier::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                mods |= Modifier::ITALIC;
            }
            if cell.flags.contains(Flags::UNDERLINE) {
                mods |= Modifier::UNDERLINED;
            }
            if cell.flags.contains(Flags::INVERSE) {
                mods |= Modifier::REVERSED;
            }
            if cell.flags.contains(Flags::DIM) {
                mods |= Modifier::DIM;
            }
            bcell
                .set_char(cell.c)
                .set_fg(to_color(cell.fg))
                .set_bg(to_color(cell.bg));
            bcell.modifier = mods;
        }

        if focused && self.term.mode().contains(TermMode::SHOW_CURSOR) {
            let cp = content.cursor.point;
            let row = cp.line.0 + display_offset;
            let col = cp.column.0 as i32;
            if row >= 0 && row < ah && col >= 0 && col < aw {
                let bcell = buf.cell_mut((area_x + col as u16, area_y + row as u16)).unwrap();
                let fg = bcell.fg;
                let bg = bcell.bg;
                bcell.set_fg(bg).set_bg(fg);
                bcell.modifier = Modifier::REVERSED;
            }
        }
    }

    /// Viewport position of the terminal cursor, relative to the pane origin.
    pub fn cursor_pos(&self) -> Option<(u16, u16)> {
        if !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let content = self.term.renderable_content();
        let offset = content.display_offset as i32;
        let cp = content.cursor.point;
        let row = cp.line.0 + offset;
        let col = cp.column.0 as i32;
        if row < 0 || col < 0 {
            return None;
        }
        Some((col as u16, row as u16))
    }
}

fn to_color(c: Color) -> RColor {
    match c {
        Color::Named(n) => match n {
            NamedColor::Black => PALETTE[0],
            NamedColor::Red => PALETTE[1],
            NamedColor::Green => PALETTE[2],
            NamedColor::Yellow => PALETTE[3],
            NamedColor::Blue => PALETTE[4],
            NamedColor::Magenta => PALETTE[5],
            NamedColor::Cyan => PALETTE[6],
            NamedColor::White => PALETTE[7],
            NamedColor::BrightBlack => PALETTE[8],
            NamedColor::BrightRed => PALETTE[9],
            NamedColor::BrightGreen => PALETTE[10],
            NamedColor::BrightYellow => PALETTE[11],
            NamedColor::BrightBlue => PALETTE[12],
            NamedColor::BrightMagenta => PALETTE[13],
            NamedColor::BrightCyan => PALETTE[14],
            NamedColor::BrightWhite => PALETTE[15],
            NamedColor::Foreground => FG,
            NamedColor::Background => BG,
            NamedColor::Cursor => ACCENT,
            _ => FG,
        },
        Color::Spec(rgb) => RColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => RColor::Indexed(i),
    }
}

pub fn sgr_mouse(button: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let b = if release { button | 3 } else { button };
    format!("\x1b[<{b};{col};{row}{}\x1b[0m", if release { "m" } else { "M" }).into_bytes()
}
