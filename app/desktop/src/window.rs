//! Frameless-window chrome helpers.
//!
//! gpui 0.2's `WindowControlArea::Drag` and `start_window_move` are no-ops on
//! macOS, so a custom titlebar can't move the window through gpui. Instead we
//! hand the drag to AppKit directly: `performWindowDragWithEvent:` on the key
//! window, given the current mouse event — the standard technique for custom
//! titlebar drags, and exactly what newer gpui's `start_window_move` wraps.

// The `objc` crate's msg_send! macros expand `#[cfg(feature = "cargo-clippy")]`
// gates that newer rustc flags as unexpected cfgs; quieten just this module.
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub fn start_system_move() {
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: *mut objc::runtime::Object = msg_send![class!(NSApplication), sharedApplication];
        let window: *mut objc::runtime::Object = msg_send![app, keyWindow];
        let window = if window.is_null() {
            msg_send![app, mainWindow]
        } else {
            window
        };
        if !window.is_null() {
            let event: *mut objc::runtime::Object = msg_send![app, currentEvent];
            if !event.is_null() {
                let _: () = msg_send![window, performWindowDragWithEvent: event];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn start_system_move() {
    // Nothing to do off-macOS: the native titlebar is left in place there.
}
