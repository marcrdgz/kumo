//! Locating and spawning the `kumo` daemon, shared by the TUI client and the
//! desktop app. The daemon is the same `kumo` binary as the client (`kumo
//! daemon`), so a client that needs one looks for the `kumo` executable (the
//! current one, a sibling, or `PATH`) and spawns it detached with the `daemon`
//! subcommand.

use std::path::PathBuf;

/// The `kumo` executable that hosts the daemon: this very process when it was
/// launched as `kumo`, then a sibling `kumo` next to this executable (the
/// usual cargo workspace layout), then `kumo` on `PATH`.
pub fn binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if exe.file_name().map(|n| n == "kumo").unwrap_or(false) {
            return Some(exe);
        }
        let sibling = exe
            .parent()
            .map(|p| p.join("kumo"))
            .filter(|p| p.is_file());
        if let Some(sib) = sibling {
            return Some(sib);
        }
    }
    which("kumo")
}

/// Spawn the `kumo daemon` process detached (own session, no stdio) so it
/// survives the client closing. `workspace` seeds a fresh session when the
/// daemon starts with none.
#[cfg(unix)]
pub fn spawn_detached(workspace: Option<PathBuf>) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let Some(bin) = binary() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "kumo binary not found (install it with the kumo-installer, or `cargo install --path app/kumo`)",
        ));
    };
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("daemon");
    if let Some(ws) = workspace {
        cmd.arg(ws);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ())
}

/// Find `name` on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
