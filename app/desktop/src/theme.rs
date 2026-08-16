//! Design system for the Kumo desktop client — the "comet" frost.
//!
//! Modeled on zeron/comet's glass recipe (`crates/ui/src/theme.rs`): the whole
//! window is one translucent **frost** scrim painted over the blurred desktop
//! (macOS `Blurred` window background), the sidebar/titlebar chrome is
//! *transparent* so the glass reads through it, and content planes are inset
//! translucent cards. Opaque fills would bury the blur — surfaces here are
//! deliberate low-alpha tints.
//!
//! Palette follows comet's dark theme: near-black neutral scale
//! (`#060606`/`#0d0d0d`) with an indigo accent (`#7c86ff`), hairline
//! `rgba(255,255,255,0.08)` borders, and emerald/amber status tones.

#![allow(dead_code)]

use gpui::{BoxShadow, Corners, Hsla, Pixels, px, point, rgba};

use kumo_protocol::AgentStatus;

/// The chrome palette. `Copy` so it rides along in render state without borrow
/// juggling.
#[derive(Clone, Copy, Debug)]
pub struct Chrome {
    /// Frost scrim painted over the blurred window background (`#080808` at 80% on
    /// macOS — see [`Chrome::glass`]).
    pub glass: u32,
    pub glass_alpha: u32,
    /// Shell / sidebar surface (`#0d0d0d`).
    pub surface: u32,
    /// Raised surface: opaque pills and chips that sit proud of the panel.
    pub surface_raised: u32,
    /// Terminal-card tone; painted translucent ([`Chrome::card`]) so the frost
    /// shows through the panes.
    pub card: u32,
    pub card_alpha: u32,
    /// Indigo accent (focus ring, active session).
    pub accent: u32,
    /// Primary text.
    pub text: u32,
    /// Muted text.
    pub muted: u32,
    /// Faint text / idle dots.
    pub faint: u32,
    /// Working status dot — emerald.
    pub working: u32,
    /// Blocked status dot — amber.
    pub blocked: u32,
    /// Idle status dot.
    pub idle: u32,
}

impl Chrome {
    /// The frost tint over the blurred window background (macOS glass). Dark
    /// `#080808` at 80% — darker than `surface`, matched by eye to comet's
    /// reference vibrancy scrim so the blur reads as frosted glass, not a
    /// washed-out backdrop.
    pub fn glass(&self) -> Hsla {
        rgba_hsla((self.glass << 8) | self.glass_alpha)
    }

    /// Shell / sidebar surface (`#0d0d0d`). On glass, this is translucent so the
    /// frost reads through it.
    pub fn surface(&self) -> Hsla {
        rgba_hsla((self.surface << 8) | 0xff)
    }

    /// Translucent surface for glass compositing — the sidebar on macOS.
    pub fn surface_glass(&self) -> Hsla {
        rgba_hsla((self.surface << 8) | 0x80) // 50% alpha
    }

    /// Raised plate tone (hover/active fills, pills that sit proud).
    pub fn surface_raised(&self) -> Hsla {
        rgba_hsla((self.surface_raised << 8) | 0xff)
    }

    /// Terminal-card fill: translucent so the frosted backdrop shows through
    /// the panes themselves.
    pub fn card(&self) -> Hsla {
        rgba_hsla((self.card << 8) | self.card_alpha)
    }

    /// Primary accent (focus rings, active session).
    pub fn accent(&self) -> Hsla {
        rgba_hsla((self.accent << 8) | 0xff)
    }

    /// Soft accent wash (selected rows, avatar wells).
    pub fn accent_soft(&self) -> Hsla {
        rgba_hsla((self.accent << 8) | 0x2e)
    }

    /// Primary text.
    pub fn text(&self) -> Hsla {
        rgba_hsla((self.text << 8) | 0xff)
    }

    /// Muted text.
    pub fn muted(&self) -> Hsla {
        rgba_hsla((self.muted << 8) | 0xff)
    }

    /// Faint text.
    pub fn faint(&self) -> Hsla {
        rgba_hsla((self.faint << 8) | 0xff)
    }

    /// Status-dot color for an agent lifecycle state.
    pub fn status(&self, status: AgentStatus) -> Hsla {
        let hex = match status {
            AgentStatus::Working => self.working,
            AgentStatus::Blocked => self.blocked,
            AgentStatus::Idle => self.idle,
        };
        rgba_hsla((hex << 8) | 0xff)
    }

    /// Working status-dot color.
    pub fn working(&self) -> Hsla {
        rgba_hsla((self.working << 8) | 0xff)
    }

    /// Idle status-dot color.
    pub fn idle(&self) -> Hsla {
        rgba_hsla((self.faint << 8) | 0xff)
    }
}

/// The comet-style chrome palette (single theme; the daemon's `Theme` events
/// still re-color the terminal panes themselves).
pub const DEFAULT: Chrome = Chrome {
    glass: 0x08_08_08,
    glass_alpha: 0x80, // 50% — translúcido para que el blur del desktop se vea
    surface: 0x0d_0d_0d,
    surface_raised: 0x24_24_24,
    card: 0x1a_1a_1a,
    card_alpha: 0x99, // 60% — translúcido pero visible
    accent: 0x7c_86_ff, // indigo-400
    text: 0xeb_eb_eb,
    muted: 0xb4_b4_b4,
    faint: 0x8e_8e_8e,
    working: 0x4a_de_80, // emerald-400
    blocked: 0xff_b9_00, // amber-400
    idle: 0x63_63_66,
};

/// The chrome palette (comet frost; index kept for call-site parity with the
/// daemon's `Theme` events).
pub fn chrome(_idx: usize) -> &'static Chrome {
    &DEFAULT
}

// ---------------------------------------------------------------------------
// Radii
// ---------------------------------------------------------------------------

/// Medium radius (agent rows, chips): 10px.
pub const RADIUS_MD: f32 = 10.0;

/// All corners at `radius` px.
pub fn corners(radius: f32) -> Corners<Pixels> {
    Corners::all(px(radius))
}

// ---------------------------------------------------------------------------
// Theme-independent chrome
// ---------------------------------------------------------------------------

fn rgba_hsla(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// 1px hairline `rgba(255,255,255,0.08)`.
pub fn hairline() -> Hsla {
    rgba_hsla((0xffffff_u32 << 8) | 0x14)
}

/// A translucent white wash used for chips and wells.
pub fn wash(alpha: u32) -> Hsla {
    rgba_hsla((0xffffff_u32 << 8) | (alpha & 0xff))
}

// ---------------------------------------------------------------------------
// Shadows
// ---------------------------------------------------------------------------

/// Soft depth shadow that lifts a floating card off the frost.
pub fn card_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: hsla_black(0.35),
            offset: point(px(0.0), px(8.0)),
            blur_radius: px(24.0),
            spread_radius: px(-4.0),
        },
        BoxShadow {
            color: hsla_black(0.20),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(3.0),
            spread_radius: px(-1.0),
        },
    ]
}

fn hsla_black(alpha: f32) -> Hsla {
    let mut c: Hsla = rgba_hsla(0x0000_00ff);
    c.a = alpha;
    c
}
