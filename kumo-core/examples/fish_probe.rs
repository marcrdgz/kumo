use kumo_core::pty::{Pty, PtySpec};
use std::io::Read;
use std::time::Duration;

fn main() {
    let mut pty = Pty::spawn(&PtySpec {
        shell: "/opt/homebrew/bin/fish".into(),
        program: None,
        cwd: None,
        cols: 100,
        rows: 30,
    })
    .unwrap();

    let mut reader = pty.master.try_clone_reader().unwrap();
    let mut buf = Vec::new();
    let start = std::time::Instant::now();
    let mut tmp = [0u8; 8192];
    while start.elapsed() < Duration::from_secs(2) {
        if reader.read(&mut tmp).unwrap() > 0 {
            buf.extend_from_slice(&tmp[..]);
            let _ = buf.iter().position(|_| false);
            if buf.windows(2).any(|w| w == b"\r\n" || w == b"\n") {
                if buf.len() > 2000 { break; }
            }
        }
    }
    let text = String::from_utf8_lossy(&buf);
    println!("=== RAW ({} bytes) ===", buf.len());
    println!("{}", text);
    pty.kill();
}
