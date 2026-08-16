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
}
