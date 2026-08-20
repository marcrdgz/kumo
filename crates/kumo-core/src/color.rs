//! RGB color value shared by the terminal emulator FFI (`vt`) and the theme.
//!
//! Kept here — not in the daemon's `vt` module — so the theme (which clients
//! use to draw their chrome) does not drag in the whole terminal emulator.

/// RGB color value.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorRgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Packed `0xRRGGBB`.
    pub fn hex(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

/// Parse a hex color string into `ColorRgb`.
///
/// Accepts `#rrggbb`, `rrggbb`, `#rgb` (expanded), `0xrrggbb`, with optional
/// surrounding whitespace. Returns `None` on invalid input.
pub fn parse_hex(s: &str) -> Option<ColorRgb> {
    let s = s.trim().trim_start_matches('#').trim_start_matches("0x").trim_start_matches("0X");
    if s.len() == 3 {
        let r = u8::from_str_radix(&s[0..1], 16).ok()? * 17;
        let g = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
        let b = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
        return Some(ColorRgb::new(r, g, b));
    }
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(ColorRgb::new(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_variants() {
        assert_eq!(parse_hex("#ff2a5f"), Some(ColorRgb::new(0xff, 0x2a, 0x5f)));
        assert_eq!(parse_hex("ff2a5f"), Some(ColorRgb::new(0xff, 0x2a, 0x5f)));
        assert_eq!(parse_hex("  #FF2A5F  "), Some(ColorRgb::new(0xff, 0x2a, 0x5f)));
        assert_eq!(parse_hex("#abc"), Some(ColorRgb::new(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex("0x1e1e2e"), Some(ColorRgb::new(0x1e, 0x1e, 0x2e)));
        assert_eq!(parse_hex("zzzzzz"), None);
        assert_eq!(parse_hex("#12"), None);
    }
}
