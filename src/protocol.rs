//! Re-exports of the wire protocol, now shared with other clients via the
//! `kumo-protocol` crate.
//!
//! The protocol types live in `crates/kumo-protocol` (pure `serde`/`bincode`).
//! Host conversions are kept in the kumo crate:
//! - `crate::wireconv`: `crossterm` key/mouse events <-> wire events.
//! - `crate::frames`: `ratatui` buffers -> `FrameMsg`/`PaneFrame`.
//!
//! This module re-exports the wire types so existing call sites keep using
//! `crate::protocol::*` unchanged.

pub use kumo_protocol::*;
