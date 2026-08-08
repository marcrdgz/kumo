//! Shared PTY and configuration layer for neomux (GUI and TUI frontends).

pub mod config;
pub mod pty;

pub use config::resolve_program;
