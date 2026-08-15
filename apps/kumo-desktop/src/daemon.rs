//! Background connection to the kumo daemon.
//!
//! The daemon owns everything (PTYs, terminal emulators, sessions, agents) and
//! streams frames/snapshots over a unix socket. This thread is the app's window
//! into that: it spawns the daemon when none is running, handshakes (declaring
//! itself a `Desktop` client), and pumps messages both ways over two channels —
//! `ServerMsg`s to the view, `ClientMsg`s from the view. On a daemon restart
//! (`kumo update`) it reconnects transparently.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kumo_protocol::{self as protocol, ClientKind, ClientMsg, ServerMsg, PROTOCOL_VERSION};

/// Channels the view uses to talk to the background connection thread.
pub struct Connection {
    /// The view polls this for `ServerMsg`s.
    pub to_view: mpsc::Receiver<ServerMsg>,
    /// The view sends `ClientMsg`s (input, resize, subscribe, focus) here.
    pub from_view: mpsc::Sender<ClientMsg>,
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

fn run(to_view: mpsc::Sender<ServerMsg>, from_view: mpsc::Receiver<ClientMsg>) {
    let mut spawned = false;
    loop {
        match serve(&to_view, &from_view, &mut spawned) {
            ServeOutcome::Restart => std::thread::sleep(Duration::from_millis(200)),
            ServeOutcome::Exited => return,
            ServeOutcome::Retry => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

fn serve(to_view: &mpsc::Sender<ServerMsg>, from_view: &mpsc::Receiver<ClientMsg>, spawned: &mut bool) -> ServeOutcome {
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
    let hello = ClientMsg::Hello { protocol: PROTOCOL_VERSION, kind: ClientKind::Desktop, cols: 80, rows: 24 };
    if protocol::write_framed(&mut stream, &hello).is_err() {
        return ServeOutcome::Restart;
    }
    // The daemon pushes a frame every ~250ms even when idle, so a short read
    // timeout lets us poll the outbox (resize/input) without a second thread.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));

    loop {
        while let Ok(msg) = from_view.try_recv() {
            if protocol::write_framed(&mut stream, &msg).is_err() {
                return ServeOutcome::Restart;
            }
        }
        match protocol::read_framed::<ServerMsg>(&mut stream) {
            Ok(ServerMsg::Restarting) => return ServeOutcome::Restart,
            Ok(ServerMsg::Shutdown) => return ServeOutcome::Exited,
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
/// `/tmp/kumo/kumo.sock` — the same path `src/config.rs` in the kumo crate uses.
fn ipc_socket_path() -> PathBuf {
    runtime_dir().join("kumo.sock")
}

fn runtime_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(runtime).join("kumo");
    }
    if let Some(tmp) = std::env::var_os("TMPDIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(tmp).join("kumo");
    }
    PathBuf::from("/tmp").join("kumo")
}

/// Launch `kumo daemon` detached (own session, no stdio) so it survives this
/// app closing. Tries `kumo` on `PATH` first, then the sibling `kumo` binary
/// next to this app (e.g. `target/debug/kumo` in a cargo workspace).
fn spawn_daemon() -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut cmd = if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().map(|p| p.join("kumo")).filter(|p| p.is_file());
        match sibling {
            Some(sib) => std::process::Command::new(sib),
            None => std::process::Command::new("kumo"),
        }
    } else {
        std::process::Command::new("kumo")
    };
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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
