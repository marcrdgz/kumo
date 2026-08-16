//! How to start kumo: the launch mode shared by the CLI client and the daemon.

use std::path::PathBuf;

/// How to start kumo (0.3.0 tmux-style CLI).
#[derive(Clone, Debug)]
pub enum Launch {
    /// `kumo`: attach to the last saved state if present, else fresh in cwd.
    Auto,
    /// `kumo attach`: restore the saved state; error if none exists.
    Attach,
    /// `kumo new [WORKSPACE]` / `kumo [WORKSPACE]`: start fresh, never attach.
    New(Option<PathBuf>),
    /// Daemon restarted by `kumo update` (`daemon --resume <file>`): adopt the
    /// inherited PTY masters recorded in the resume file.
    Resume(PathBuf),
}
