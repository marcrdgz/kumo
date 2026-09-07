#![cfg(unix)]

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kumo_protocol::{read_framed, write_framed, ClientKind, Command, DaemonEvent, PROTOCOL_VERSION};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Daemon {
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!("kumo-runtime-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let child = ProcessCommand::new(env!("CARGO_BIN_EXE_kumo"))
            .arg("daemon").arg(&root)
            .env("KUMO_CONFIG_DIR", &root)
            .env("XDG_RUNTIME_DIR", &root)
            .env("XDG_STATE_HOME", &root)
            .env("KUMO_NO_UPDATE", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit())
            .spawn().unwrap();
        Self { child, root }
    }

    fn connect(&self) -> UnixStream {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(mut stream) = UnixStream::connect(self.root.join("kumo/kumo.sock")) {
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                send(&mut stream, Command::Attach { protocol: PROTOCOL_VERSION, kind: ClientKind::Terminal, cols: 80, rows: 24 });
                if matches!(read_framed::<DaemonEvent>(&mut stream), Ok(DaemonEvent::Welcome { .. })) {
                    return stream;
                }
            }
            assert!(Instant::now() < deadline, "daemon did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn pane(&self, stream: &mut UnixStream) -> u64 {
        send(stream, Command::SessionList);
        loop {
            if let DaemonEvent::SessionList { sessions } = read_framed(stream).unwrap() {
                return sessions[0].tabs[0].panes[0].id;
            }
        }
    }

    fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "daemon did not reap its last pane and exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn send(stream: &mut UnixStream, command: Command) {
    write_framed(stream, &command).unwrap();
}

fn await_frame(stream: &mut UnixStream, full: bool, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "missing frame containing {needle}");
        if let DaemonEvent::PaneFrame { frame } = read_framed(stream).unwrap() {
            let text: String = frame.rows_dirty.iter().flat_map(|r| r.cells.iter().map(|c| c.text.as_str())).collect();
            if frame.full == full && text.contains(needle) { return; }
        }
    }
}

#[test]
fn both_viewers_receive_incremental_output() {
    let mut daemon = Daemon::start();
    let mut first = daemon.connect();
    let pid = daemon.pane(&mut first);
    send(&mut first, Command::SubscribePane { pane_id: pid });
    await_frame(&mut first, true, "");
    let mut second = daemon.connect();
    send(&mut second, Command::SubscribePane { pane_id: pid });
    await_frame(&mut second, true, "");
    for token in ["FIRST_GENERATION", "SECOND_GENERATION", "THIRD_GENERATION"] {
        send(&mut first, Command::PaneWrite { pane_id: pid, bytes: format!("printf '{token}\\n'\n").into_bytes() });
        await_frame(&mut first, false, token);
        await_frame(&mut second, false, token);
    }
    send(&mut first, Command::KillServer);
    daemon.wait_exit();
}

#[test]
fn real_exec_restart_reaps_exited_shell() {
    let mut daemon = Daemon::start();
    let mut stream = daemon.connect();
    let pid = daemon.pane(&mut stream);
    send(&mut stream, Command::Restart);
    loop {
        if read_framed::<DaemonEvent>(&mut stream).is_err() { break; }
    }
    let mut resumed = daemon.connect();
    assert_eq!(daemon.pane(&mut resumed), pid);
    send(&mut resumed, Command::PaneWrite { pane_id: pid, bytes: b"exit 7\n".to_vec() });
    daemon.wait_exit();
    assert!(!daemon.root.join("kumo/kumo.sock").exists());
}
