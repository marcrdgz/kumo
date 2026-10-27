//! Headless daemon entry: `kumo daemon [WORKSPACE]` or `kumo daemon --resume
//! <file>`. Spawned detached by the `kumo` client and the desktop app; also
//! available to start manually. `--resume <file>` makes it adopt the live PTY
//! masters inherited from a `kumo update` restart.

pub mod agents;
pub mod alert;
pub mod app;
pub mod frames;
pub mod keys;
pub mod pane;
pub mod pty;
pub mod state;
pub mod vt;
pub mod xtgettcap;

use std::path::PathBuf;

use anyhow::Result;

use kumo_core::Launch;

pub fn run(args: &[String]) -> Result<()> {
    let mut workspace = args.first().map(PathBuf::from);
    let mut resume = None;
    if workspace.as_deref().and_then(|w| w.to_str()) == Some("--resume") {
        resume = args.get(1).map(PathBuf::from);
        workspace = None;
    }
    #[cfg(unix)]
    {
        install_panic_hook(kumo_core::config::state_dir().join("daemon.log"));
        let launch = match resume {
            Some(path) => Launch::Resume(path),
            None if kumo_core::config::persistence().restore => {
                Launch::Restore(kumo_core::config::state_file(), workspace)
            }
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

/// Append every panic (any thread) to `log` before the default hook runs. The
/// daemon runs detached with stderr on `/dev/null` and release builds abort on
/// panic, so otherwise a crash takes every session down without a trace.
#[cfg(unix)]
fn install_panic_hook(log: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let report = format!(
            "[{}] kumo {} daemon (pid {}) thread '{}' {info}\n{}\n",
            chrono::Local::now().to_rfc3339(),
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            thread.name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture(),
        );
        let _ = append_report(&log, &report);
        default_hook(info);
    }));
}

#[cfg(unix)]
fn append_report(log: &std::path::Path, report: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?
        .write_all(report.as_bytes())
}

#[cfg(all(test, unix))]
mod tests {
    use super::append_report;

    #[test]
    fn panic_reports_accumulate_in_the_log() {
        let dir = std::env::temp_dir().join(format!("kumo-panic-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = dir.join("state").join("daemon.log");
        append_report(&log, "first\n").unwrap();
        append_report(&log, "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "first\nsecond\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
