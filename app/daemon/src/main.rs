//! Headless daemon entry: `kumo-daemon [WORKSPACE]` or `kumo-daemon --resume
//! <file>`. Spawned detached by the `kumo` client and the desktop app; also
//! available to start manually. `--resume <file>` makes it adopt the live PTY
//! masters inherited from a `kumo update` restart.

mod agents;
mod alert;
mod app;
mod frames;
mod keys;
mod pane;
mod pty;
mod state;
mod vt;
mod xtgettcap;

use std::path::PathBuf;

use anyhow::Result;

use kumo_core::Launch;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut workspace = args.first().map(PathBuf::from);
    let mut resume = None;
    if workspace.as_deref().and_then(|w| w.to_str()) == Some("--resume") {
        resume = args.get(1).map(PathBuf::from);
        workspace = None;
    }
    #[cfg(unix)]
    {
        let launch = match resume {
            Some(path) => Launch::Resume(path),
            None => Launch::New(workspace),
        };
        app::server::run_daemon(launch)
    }
    #[cfg(not(unix))]
    {
        let _ = (workspace, resume);
        anyhow::bail!("the kumo daemon is unix-only for now")
    }
}
