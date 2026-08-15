//! Re-exports of the wire protocol, shared with other clients via the
//! `kumo-protocol` crate.
//!
//! The protocol types live in `crates/kumo-protocol` (pure `serde`/`bincode`):
//! `Command` (client → daemon), `DaemonEvent` (daemon → client), the semantic
//! `LayoutNode`/`Layout` tree, and per-pane `PaneFrame`s. Host conversions are
//! kept in the kumo crate:
//! - `crate::wireconv` (via the protocol crate's `crossterm` feature):
//!   `crossterm` key/mouse events <-> wire events.
//! - `crate::frames`: `ratatui` buffers -> `PaneFrame`.
//!
//! This module re-exports the wire types so existing call sites keep using
//! `crate::protocol::*` unchanged.

pub use kumo_protocol::*;
