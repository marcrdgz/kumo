/// Minimal file-based logger for development diagnostics.
use std::io::Write;

pub fn log(msg: &str) {
    let path = std::env::var("NEOMUX_DEBUG_LOG").unwrap_or_else(|_| "/tmp/neomux_debug.log".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}
