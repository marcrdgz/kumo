#![cfg(unix)]

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kumo_protocol::{read_event, write_framed, ClientKind, Command, DaemonEvent, PROTOCOL_VERSION};

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
                if matches!(read_event(&mut stream), Ok(DaemonEvent::Welcome { .. })) {
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
            if let DaemonEvent::SessionList { sessions } = read_event(stream).unwrap() {
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
        if let DaemonEvent::PaneFrame { frame } = read_event(stream).unwrap() {
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
#[ignore = "local end-to-end latency benchmark; run with --ignored --nocapture"]
fn benchmark_pane_write_to_incremental_frame() {
    const SAMPLES: usize = 32;

    let mut daemon = Daemon::start();
    let mut stream = daemon.connect();
    let pane_id = daemon.pane(&mut stream);
    send(&mut stream, Command::SubscribePane { pane_id });

    // Consume the initial full frame so each sample measures only the command
    // write, PTY echo/output, daemon tick, serialization, and socket receipt.
    await_frame(&mut stream, true, "");

    let mut samples = Vec::with_capacity(SAMPLES);
    for index in 0..SAMPLES {
        let token = format!("KUMO_LATENCY_{index}_{}", std::process::id());
        let command = format!("printf '%s\\n' '{token}'\n");
        let started = Instant::now();
        send(
            &mut stream,
            Command::PaneWrite { pane_id, bytes: command.into_bytes() },
        );
        await_frame(&mut stream, false, &token);
        samples.push(started.elapsed());
    }

    samples.sort_unstable();
    let mean = samples.iter().copied().sum::<Duration>() / SAMPLES as u32;
    let p50 = samples[SAMPLES / 2];
    let p95 = samples[SAMPLES * 95 / 100];
    eprintln!(
        "pane write -> incremental frame latency (microseconds): mean={} p50={} p95={} max={}",
        mean.as_micros(),
        p50.as_micros(),
        p95.as_micros(),
        samples.last().unwrap().as_micros(),
    );

    send(&mut stream, Command::KillServer);
    daemon.wait_exit();
}

/// Lines written into a pane to overflow the hard-coded `10_000`-line
/// scrollback, so a "full" pane holds its maximum history.
const SCROLLBACK_FILL_LINES: usize = 12_000;

/// Fills `pane_id`'s scrollback and waits for the trailing marker in that
/// pane's frames (the stream may carry other subscribed panes). The marker is
/// split by quotes in the typed command so only the output matches it.
fn fill_scrollback(stream: &mut UnixStream, pane_id: u64) {
    let command = format!("seq 1 {SCROLLBACK_FILL_LINES}; printf 'FIL''LED\\n'\n");
    send(stream, Command::PaneWrite { pane_id, bytes: command.into_bytes() });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "pane {pane_id} never printed its fill marker");
        if let DaemonEvent::PaneFrame { frame } = read_event(stream).unwrap() {
            let text: String = frame.rows_dirty.iter().flat_map(|r| r.cells.iter().map(|c| c.text.as_str())).collect();
            if frame.pane_id == pane_id && text.contains("FILLED") { return; }
        }
    }
}

/// Resident set size of `pid` in KiB (`ps` reports KiB on macOS and Linux).
fn rss_kib(pid: u32) -> u64 {
    let out = ProcessCommand::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().parse().unwrap()
}

#[test]
#[ignore = "local end-to-end latency benchmark; run with --ignored --nocapture"]
fn benchmark_attach_to_first_full_frame() {
    const SAMPLES: usize = 32;

    let mut daemon = Daemon::start();
    let mut setup = daemon.connect();
    let pane_id = daemon.pane(&mut setup);
    send(&mut setup, Command::SubscribePane { pane_id });
    await_frame(&mut setup, true, "");
    // Attach to a pane with full history: the worst case a reattach serves.
    fill_scrollback(&mut setup, pane_id);

    let mut welcome = Vec::with_capacity(SAMPLES);
    let mut first_frame = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        // Each sample is a fresh client: socket connect, handshake, then the
        // subscription a TUI sends for its focused pane.
        let started = Instant::now();
        let mut stream = UnixStream::connect(daemon.root.join("kumo/kumo.sock")).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        send(&mut stream, Command::Attach { protocol: PROTOCOL_VERSION, kind: ClientKind::Terminal, cols: 80, rows: 24 });
        assert!(matches!(read_event(&mut stream), Ok(DaemonEvent::Welcome { .. })));
        welcome.push(started.elapsed());
        send(&mut stream, Command::SubscribePane { pane_id });
        await_frame(&mut stream, true, "");
        first_frame.push(started.elapsed());
    }

    for (label, samples) in [("connect -> welcome", &mut welcome), ("connect -> first full frame", &mut first_frame)] {
        samples.sort_unstable();
        eprintln!(
            "attach {label} latency (microseconds): p50={} p95={} max={}",
            samples[SAMPLES / 2].as_micros(),
            samples[SAMPLES * 95 / 100].as_micros(),
            samples.last().unwrap().as_micros(),
        );
    }

    send(&mut setup, Command::KillServer);
    daemon.wait_exit();
}

#[test]
#[ignore = "local end-to-end memory benchmark; run with --ignored --nocapture"]
fn benchmark_daemon_memory_per_pane() {
    const PANES: usize = 16;

    let mut daemon = Daemon::start();
    let mut stream = daemon.connect();
    let first = daemon.pane(&mut stream);
    send(&mut stream, Command::SubscribePane { pane_id: first });
    await_frame(&mut stream, true, "");
    // Let allocator and shell startup settle before each RSS sample.
    let settle = || std::thread::sleep(Duration::from_millis(500));
    settle();
    let baseline = rss_kib(daemon.child.id());

    for index in 0..PANES {
        send(&mut stream, Command::SessionNew { name: Some(format!("bench-{index}")), workspace: None });
        while !matches!(read_event(&mut stream).unwrap(), DaemonEvent::Reply { .. }) {}
    }
    send(&mut stream, Command::SessionList);
    let panes: Vec<u64> = loop {
        if let DaemonEvent::SessionList { sessions } = read_event(&mut stream).unwrap() {
            break sessions.iter().filter(|s| s.name.starts_with("bench-")).map(|s| s.tabs[0].panes[0].id).collect();
        }
    };
    assert_eq!(panes.len(), PANES);
    for &pane_id in &panes {
        send(&mut stream, Command::SubscribePane { pane_id });
        await_frame(&mut stream, true, "");
    }
    settle();
    let empty = rss_kib(daemon.child.id());

    for &pane_id in &panes {
        fill_scrollback(&mut stream, pane_id);
    }
    settle();
    let full = rss_kib(daemon.child.id());

    eprintln!(
        "daemon RSS per pane (KiB, {PANES} panes, baseline {baseline}): empty={} full scrollback={}",
        empty.saturating_sub(baseline) / PANES as u64,
        full.saturating_sub(baseline) / PANES as u64,
    );

    send(&mut stream, Command::KillServer);
    daemon.wait_exit();
}

#[test]
fn real_exec_restart_reaps_exited_shell() {
    let mut daemon = Daemon::start();
    let mut stream = daemon.connect();
    let pid = daemon.pane(&mut stream);
    send(&mut stream, Command::Restart);
    loop {
        if read_event(&mut stream).is_err() { break; }
    }
    let mut resumed = daemon.connect();
    assert_eq!(daemon.pane(&mut resumed), pid);
    send(&mut resumed, Command::PaneWrite { pane_id: pid, bytes: b"exit 7\n".to_vec() });
    daemon.wait_exit();
    assert!(!daemon.root.join("kumo/kumo.sock").exists());
}

#[test]
fn daemon_survives_sighup_with_panes_intact() {
    let mut daemon = Daemon::start();
    let mut stream = daemon.connect();
    let pid = daemon.pane(&mut stream);
    unsafe { libc::kill(daemon.child.id() as i32, libc::SIGHUP) };
    std::thread::sleep(Duration::from_millis(200));
    assert!(daemon.child.try_wait().unwrap().is_none(), "SIGHUP must not stop the daemon");
    send(&mut stream, Command::SubscribePane { pane_id: pid });
    await_frame(&mut stream, true, "");
    send(&mut stream, Command::PaneWrite { pane_id: pid, bytes: b"printf 'SIGHUP_OK\\n'\n".to_vec() });
    await_frame(&mut stream, false, "SIGHUP_OK");
}

#[test]
fn config_file_changes_reload_the_running_daemon() {
    let mut daemon = Daemon::start();
    let mut stream = daemon.connect();
    std::fs::write(
        daemon.root.join("config"),
        "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\ntheme = dracula\n",
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reloaded = false;
    let mut theme_after_reload = false;
    while Instant::now() < deadline && !theme_after_reload {
        match read_event(&mut stream).unwrap() {
            DaemonEvent::ConfigReloaded { .. } => reloaded = true,
            DaemonEvent::Theme { .. } if reloaded => theme_after_reload = true,
            _ => {}
        }
    }
    assert!(reloaded, "the config watcher did not notify the client");
    assert!(theme_after_reload, "the changed theme was not applied live");

    send(&mut stream, Command::KillServer);
    daemon.wait_exit();
}

#[test]
fn control_cli_emits_parseable_json() {
    let mut daemon = Daemon::start();
    // Wait until the daemon has bound and completed its attach handshake.
    drop(daemon.connect());
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_kumo"))
        .args(["session", "list", "--json"])
        .env("KUMO_CONFIG_DIR", &daemon.root)
        .env("XDG_RUNTIME_DIR", &daemon.root)
        .env("XDG_STATE_HOME", &daemon.root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let sessions = value["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["active"], true);

    let mut stream = daemon.connect();
    send(&mut stream, Command::KillServer);
    daemon.wait_exit();
}
