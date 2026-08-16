//! Background connection to the kumo daemon.
//!
//! The daemon owns everything and never renders chrome. This thread is the
//! app's window into it: it spawns the daemon when none is running, attaches
//! as a `Desktop` client, subscribes to the semantic layout, and pumps
//! `Command`s/`DaemonEvent`s over two channels. On a daemon restart
//! (`kumo update`) it reconnects transparently.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kumo_protocol::{self as protocol, ClientKind, Command, DaemonEvent, PROTOCOL_VERSION};

/// Channels the view uses to talk to the background connection thread.
pub struct Connection {
    /// The view polls this for `DaemonEvent`s.
    pub to_view: mpsc::Receiver<DaemonEvent>,
    /// The view sends `Command`s (input, pane resizes, focus, subscribe) here.
    pub from_view: mpsc::Sender<Command>,
}

/// Start the background connection thread.
pub fn spawn() -> Connection {
    let (to_view_tx, to_view) = mpsc::channel();
    let (from_view, from_view_rx) = mpsc::channel();
    std::thread::spawn(move || run(to_view_tx, from_view_rx));
    Connection { to_view, from_view }
}

enum ServeOutcome {
    /// Reconnect (the daemon restarted itself, or the socket dropped).
    Restart,
    /// The daemon asked this client to detach / shut down.
    Exited,
    /// Nothing was listening and the daemon did not come up; try again.
    Retry,
}

fn run(to_view: mpsc::Sender<DaemonEvent>, from_view: mpsc::Receiver<Command>) {
    let mut spawned = false;
    loop {
        match serve(&to_view, &from_view, &mut spawned) {
            ServeOutcome::Restart => std::thread::sleep(Duration::from_millis(200)),
            ServeOutcome::Exited => return,
            ServeOutcome::Retry => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

fn serve(to_view: &mpsc::Sender<DaemonEvent>, from_view: &mpsc::Receiver<Command>, spawned: &mut bool) -> ServeOutcome {
    let path = ipc_socket_path();
    if UnixStream::connect(&path).is_err() {
        if !*spawned {
            let _ = spawn_daemon();
            *spawned = true;
        }
        if !wait_for_socket(&path, Duration::from_secs(10)) {
            return ServeOutcome::Retry;
        }
    }
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => return ServeOutcome::Retry,
    };

    // Handshake: announce ourselves as a desktop client. The daemon rejects a
    // mismatched protocol version with `Shutdown`.
    let attach = Command::Attach { protocol: PROTOCOL_VERSION, kind: ClientKind::Desktop, cols: 80, rows: 24 };
    if protocol::write_framed(&mut stream, &attach).is_err() {
        return ServeOutcome::Restart;
    }
    let _ = protocol::write_framed(&mut stream, &Command::SubscribeLayout);
    // A short read timeout lets us poll the outbox (resize/input) without a
    // second thread.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));

    loop {
        while let Ok(msg) = from_view.try_recv() {
            if protocol::write_framed(&mut stream, &msg).is_err() {
                return ServeOutcome::Restart;
            }
        }
        match protocol::read_framed::<DaemonEvent>(&mut stream) {
            Ok(DaemonEvent::Restarting) => return ServeOutcome::Restart,
            Ok(DaemonEvent::Shutdown) => return ServeOutcome::Exited,
            Ok(msg) => {
                if to_view.send(msg).is_err() {
                    return ServeOutcome::Exited;
                }
            }
            Err(e) if is_read_timeout(&e) => continue,
            Err(_) => return ServeOutcome::Restart,
        }
    }
}

/// `$XDG_RUNTIME_DIR/kumo/kumo.sock`, else `$TMPDIR/kumo/kumo.sock`, else
/// `/tmp/kumo/kumo.sock` — the same path `kumo_core::config` uses.
fn ipc_socket_path() -> PathBuf {
    let dir = if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        PathBuf::from(runtime)
    } else if let Some(tmp) = std::env::var_os("TMPDIR").filter(|v| !v.is_empty()) {
        PathBuf::from(tmp)
    } else {
        PathBuf::from("/tmp")
    };
    dir.join("kumo").join("kumo.sock")
}

/// Launch the `kumo-daemon` binary detached (own session, no stdio) so it
/// survives this app closing. The daemon is a separate binary: this app looks
/// for a sibling `kumo-daemon` next to it (e.g. `target/debug/kumo-daemon` in
/// a cargo workspace) and falls back to `kumo-daemon` on `PATH`.
fn spawn_daemon() -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let Some(bin) = kumo_core::daemon::binary() else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "kumo-daemon binary not found"));
    };
    let mut cmd = std::process::Command::new(bin);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ())
}

/// Wait (up to `timeout`) for a socket to start accepting connections.
fn wait_for_socket(path: &std::path::Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if UnixStream::connect(path).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// Whether an error is a socket read timeout (no data within the window).
fn is_read_timeout(e: &anyhow::Error) -> bool {
    e.downcast_ref::<io::Error>()
        .map(|ioe| matches!(ioe.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
        .unwrap_or(false)
}
