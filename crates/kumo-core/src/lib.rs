//! Shared logic between the kumo daemon and its clients (CLI, desktop):
//! configuration, the semantic layout tree, themes, the update check, and git
//! worktrees. Deliberately free of the terminal emulator and the UI so both
//! the lightweight client and the daemon can pull it in.
//!
//! The wire protocol lives in the pure `kumo-protocol` crate and is re-exported
//! here as `protocol`.

pub mod color;
pub mod config;
pub mod daemon;
pub mod launch;
pub mod layout;
pub mod protocol;
pub mod theme;
pub mod update;
pub mod updater;
pub mod worktree_meta;
pub mod worktrees;

pub use launch::Launch;
