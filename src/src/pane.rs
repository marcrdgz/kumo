use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::Result;
use kumo_core::pty::{Pty, PtySpec};
use portable_pty::PtySize;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier};

use crate::vt::{self, ColorRgb};

/// Catppuccin mocha ANSI 16-color palette.
const PALETTE: [ColorRgb; 16] = [
    ColorRgb::new(0x45, 0x47, 0x5a), // Black
    ColorRgb::new(0xf3, 0x8b, 0xa8), // Red
    ColorRgb::new(0xa6, 0xe3, 0xa1), // Green
    ColorRgb::new(0xf9, 0xe2, 0xaf), // Yellow
    ColorRgb::new(0x89, 0xb4, 0xfa), // Blue
    ColorRgb::new(0xf5, 0xc2, 0xe7), // Magenta
    ColorRgb::new(0x94, 0xe2, 0xd5), // Cyan
    ColorRgb::new(0xba, 0xc2, 0xde), // White
    ColorRgb::new(0x58, 0x5b, 0x70), // BrightBlack
    ColorRgb::new(0xf3, 0x8b, 0xa8), // BrightRed
    ColorRgb::new(0xa6, 0xe3, 0xa1), // BrightGreen
    ColorRgb::new(0xf9, 0xe2, 0xaf), // BrightYellow
    ColorRgb::new(0x89, 0xb4, 0xfa), // BrightBlue
    ColorRgb::new(0xf5, 0xc2, 0xe7), // BrightMagenta
    ColorRgb::new(0x94, 0xe2, 0xd5), // BrightCyan
    ColorRgb::new(0xcd, 0xd6, 0xf4), // BrightWhite
];

pub const FG: RColor = RColor::Rgb(0xcd, 0xd6, 0xf4);
pub const ACCENT: RColor = RColor::Rgb(0xb4, 0xbe, 0xfe);

/// A live pane: PTY + a real terminal emulator (libghostty-vt) fed from it.
#[allow(dead_code)]
pub struct Pane {
    pub id: u64,
    pub session_id: u64,
    pub is_ai: bool,
    pub pty: Pty,
    pub vt: vt::Terminal,
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

        let vt = vt::Terminal::new(cols.max(1), rows.max(1), 10_000, &PALETTE)?;

        let mut pane = Pane {
            id,
            session_id,
            is_ai,
            pty,
            vt,
            dead: false,
        };

        // Route query responses (DA, size reports, etc.) back to the PTY.
        pane.vt.set_write_sink(&mut *pane.pty.writer as *mut (dyn std::io::Write + Send));

        let reader = pane.pty.master.try_clone_reader()?;
        let tx = events_tx.clone();
        let pid = id;
        Pty::read_loop(reader, move |data| {
            let _ = tx.send(PtyEvent::Output { pane_id: pid, data });
        });

        Ok(pane)
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.vt.write(data);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.vt.resize(cols, rows);
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
        self.vt.scroll(delta);
    }

    pub fn has_mouse_reporting(&self) -> bool {
        self.vt.mouse_tracking()
    }

    /// Whether the alternate screen is active.
    pub fn in_alt_screen(&self) -> bool {
        self.vt.mode_get(vt::MODE_ALT_SCREEN)
    }

    /// Scrollbar state (total/offset/len rows), refreshed during `render`.
    pub fn scrollbar_data(&self) -> vt::TerminalScrollbar {
        self.vt.scrollbar()
    }

    /// Render the emulator viewport into `buf` at `area`. If `cursor`, draw the
    /// terminal cursor as an inverted cell.
    pub fn render(&mut self, area: Rect, focused: bool, buf: &mut Buffer) {
        let area_x = area.x;
        let area_y = area.y;
        let aw = area.width as i32;
        let ah = area.height as i32;

        self.vt.refresh();
        let fg = rgb(self.vt.default_fg());
        let bg = rgb(self.vt.default_bg());

        // Prefill with the default background color.
        for y in area_y..(area_y + area.height) {
            for x in area_x..(area_x + area.width) {
                buf.cell_mut((x, y)).unwrap().set_char(' ').set_fg(fg).set_bg(bg);
            }
        }

        self.vt.for_each_cell(|row, col, rc| {
            if row >= ah as usize || col >= aw as usize {
                return;
            }
            let bcell = buf.cell_mut((area_x + col as u16, area_y + row as u16)).unwrap();
            let mut mods = Modifier::empty();
            if rc.bold {
                mods |= Modifier::BOLD;
            }
            if rc.italic {
                mods |= Modifier::ITALIC;
            }
            if rc.underline {
                mods |= Modifier::UNDERLINED;
            }
            if rc.inverse {
                mods |= Modifier::REVERSED;
            }
            if rc.faint {
                mods |= Modifier::DIM;
            }
            if rc.text.is_empty() {
                bcell.set_char(' ').set_fg(rgb(rc.fg)).set_bg(rgb(rc.bg));
            } else {
                let ch = rc.text.chars().next().unwrap_or(' ');
                bcell.set_char(ch).set_fg(rgb(rc.fg)).set_bg(rgb(rc.bg));
            }
            bcell.modifier = mods;
        });

        if focused && self.vt.cursor_visible() {
            if let Some((cx, cy)) = self.vt.cursor_pos() {
                let x = area_x + cx;
                let y = area_y + cy;
                if x < area_x + area.width && y < area_y + area.height {
                    let bcell = buf.cell_mut((x, y)).unwrap();
                    let f = bcell.fg;
                    let b = bcell.bg;
                    bcell.set_fg(b).set_bg(f);
                    bcell.modifier = Modifier::REVERSED;
                }
            }
        }
    }

    /// Text of the viewport cells between two (col, row) points (inclusive),
    /// used to copy a mouse selection. Rows are joined with '\n'.
    pub fn selection_text(&mut self, a: (u16, u16), b: (u16, u16)) -> String {
        use std::collections::HashMap;
        let (r0, r1) = (a.1.min(b.1), a.1.max(b.1));
        self.vt.refresh();
        let mut cells_by_row: HashMap<i32, Vec<(i32, char)>> = HashMap::new();
        self.vt.for_each_cell(|row, col, rc| {
            if let Some(ch) = rc.text.chars().next() {
                cells_by_row.entry(row as i32).or_default().push((col as i32, ch));
            }
        });
        let mut out = String::new();
        for r in r0..=r1 {
            let (c_lo, c_hi) = if r0 == r1 {
                (a.0.min(b.0), a.0.max(b.0))
            } else if r == r0 {
                (a.0.min(b.0), u16::MAX)
            } else if r == r1 {
                (0, a.0.max(b.0))
            } else {
                (0, u16::MAX)
            };
            let mut line = String::new();
            if let Some(mut cells) = cells_by_row.get(&(r as i32)).cloned() {
                cells.sort_by_key(|(c, _)| *c);
                for (c, ch) in cells {
                    let cu = c as u16;
                    if cu >= c_lo && cu <= c_hi {
                        line.push(ch);
                    }
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.trim_end_matches('\n').to_string()
    }

    /// Viewport position of the terminal cursor, relative to the pane origin.
    /// Requires a prior `render` (which refreshes the render state).
    pub fn cursor_pos(&self) -> Option<(u16, u16)> {
        if self.vt.mode_get(vt::MODE_CURSOR_VISIBLE) {
            self.vt.cursor_pos()
        } else {
            None
        }
    }
}

fn rgb(c: ColorRgb) -> RColor {
    RColor::Rgb(c.r, c.g, c.b)
}

pub fn sgr_mouse(button: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let b = if release { button | 3 } else { button };
    format!("\x1b[<{b};{col};{row}{}\x1b[0m", if release { "m" } else { "M" }).into_bytes()
}
