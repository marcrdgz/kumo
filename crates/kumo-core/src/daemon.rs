//! Locating the `kumo-daemon` binary, shared by the CLI client and the desktop
//! app. The daemon is a separate binary from the `kumo` client, so a client
//! that needs to start one looks for a sibling `kumo-daemon` (the usual cargo
//! workspace layout) and falls back to `kumo-daemon` on `PATH`.

use std::path::PathBuf;

/// The `kumo-daemon` binary: the sibling of this executable first (e.g.
/// `target/debug/kumo-daemon` next to `target/debug/kumo`), then `kumo-daemon`
/// on `PATH`.
pub fn binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe
            .parent()
            .map(|p| p.join("kumo-daemon"))
            .filter(|p| p.is_file());
        if let Some(sib) = sibling {
            return Some(sib);
        }
    }
    which("kumo-daemon")
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
