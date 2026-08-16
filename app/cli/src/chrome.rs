//! Low-level ratatui cell helpers for drawing chrome (borders, status bar,
//! sidebar, popups). These lived in the daemon before the split; the client is
//! the only renderer now.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Style};

pub fn put(f: &mut Frame, x: u16, y: u16, ch: &str, style: Style) {
    let a = f.area();
    if x >= a.width || y >= a.height {
        return;
    }
    let c = f.buffer_mut().cell_mut((x, y)).unwrap();
    c.set_symbol(ch).set_style(style);
}

pub fn text(f: &mut Frame, x: u16, y: u16, s: &str, style: Style, max_width: u16) {
    for (i, ch) in s.chars().take(max_width as usize).enumerate() {
        put(f, x + i as u16, y, &ch.to_string(), style);
    }
}

pub fn fill(f: &mut Frame, area: Rect, color: RColor) {
    for y in area.y..(area.y + area.height) {
        for x in area.x..(area.x + area.width) {
            let a = f.area();
            if x >= a.width || y >= a.height {
                continue;
            }
            let c = f.buffer_mut().cell_mut((x, y)).unwrap();
            c.set_symbol(" ").set_style(Style::default().bg(color));
        }
    }
}
