//! Smart terminal client: a "dumb viewport with chrome" for the kumo daemon.
//!
//! Connects to the daemon socket, subscribes to the semantic layout + per-pane
//! content, computes its own geometry, and draws ALL chrome (borders, sidebar,
//! status bar, menus, popups) with ratatui — exactly like the desktop app, but
//! in a host terminal. The daemon never renders chrome.

use std::io;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::Hide;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};

use kumo_core::Launch;
use crate::cli::client_view::View;
use kumo_core::protocol::{self, ClientKind, Command, DaemonEvent};

pub fn run(launch: Launch) -> Result<()> {
    let path = kumo_core::config::ipc_socket_path();
    let mut spawned = false;
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => match launch {
            Launch::Attach => {
                anyhow::bail!("no kumo daemon is running (start with `kumo` or `kumo new`)")
            }
            _ => {
                spawn_daemon(workspace_for(&launch))?;
                wait_for_daemon(&path)?;
                spawned = true;
                UnixStream::connect(&path)?
            }
        },
    };
    // `kumo new` against an already-running daemon: create the fresh session
    // instead of silently attaching to the existing one.
    let pre: Vec<Command> = if !spawned && matches!(launch, Launch::New(_)) {
        let workspace = workspace_for(&launch).or_else(|| std::env::current_dir().ok());
        vec![Command::SessionNew { name: None, workspace }]
    } else {
        Vec::new()
    };
    client_loop(stream, &pre)
}

fn workspace_for(launch: &Launch) -> Option<PathBuf> {
    match launch {
        Launch::New(Some(p)) => Some(p.clone()),
        _ => None,
    }
}

fn client_loop(mut stream: UnixStream, pre: &[Command]) -> Result<()> {
    let mut pre = pre.to_vec();
    loop {
        match client_once(&mut stream, &pre) {
            Ok(Exit::Clean) => return Ok(()),
            Ok(Exit::Restarting) => {
                // The daemon exec'd a new binary for `kumo update`; reconnect
                // (with retries) and re-handshake, keeping the TUI intact.
                stream = reconnect()?;
                pre.clear();
            }
            Err(e) => return Err(e),
        }
    }
}

/// Outcome of one attach (one handshake + render loop) to the daemon.
enum Exit {
    /// `leader+d` detach or a graceful daemon stop (last session / `kumo kill`).
    Clean,
    /// The daemon is restarting itself (`kumo update`); drop the socket and
    /// reconnect.
    Restarting,
}

/// One attach session: handshake, render loop, teardown. On `Restarting` the
/// terminal is left in raw mode so the reconnect is seamless (no flicker).
fn client_once(stream: &mut UnixStream, pre: &[Command]) -> Result<Exit> {
    let (cols, rows) = crossterm::terminal::size()?;
    protocol::write_framed(
        stream,
        &Command::Attach { protocol: protocol::PROTOCOL_VERSION, kind: ClientKind::Terminal, cols, rows },
    )?;
    // Messages to send right after the handshake (e.g. the `kumo new` session
    // request), before entering the render loop.
    for msg in pre {
        protocol::write_framed(stream, msg)?;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        EnableBracketedPaste,
        Hide,
        Clear(ClearType::All),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )?;

    // Daemon-event reader thread: forwards every frame the daemon pushes. It
    // wakes on a read timeout to check the stop flag, because the client's own
    // socket clones keep the write end open — the reader would otherwise block
    // forever on a clean detach (the daemon closes its end, but our clones
    // keep the connection "alive" from the reader's perspective).
    let write_half = stream.try_clone()?;
    write_half.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let (ev_tx, ev_rx) = mpsc::channel::<DaemonEvent>();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let reader = std::thread::spawn(move || reader_loop(write_half, ev_tx, stop2));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;

    let mut view = View::new(stream.try_clone()?, cols, rows);
    view.render_now(&mut terminal)?;

    let result: Result<Exit> = (|| {
        loop {
            // Daemon events: block for the first one (8ms keeps local input
            // responsive), then drain everything already queued so a burst of
            // pane frames costs a single render instead of one render per frame.
            match ev_rx.recv_timeout(Duration::from_millis(8)) {
                Ok(ev) => {
                    if let Some(exit) = apply_daemon_event(&mut view, ev) {
                        return Ok(exit);
                    }
                    while let Ok(ev) = ev_rx.try_recv() {
                        if let Some(exit) = apply_daemon_event(&mut view, ev) {
                            return Ok(exit);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // The daemon closed the socket without a Shutdown frame
                    // (abrupt exit): nothing left to render.
                    return Ok(Exit::Clean);
                }
            }
            if view.detach_requested() {
                return Ok(Exit::Clean);
            }
            // Same for local input: apply every pending crossterm event, then
            // render once for the whole batch.
            while crossterm::event::poll(Duration::from_millis(0))? {
                match crossterm::event::read()? {
                    crossterm::event::Event::Key(k) => view.on_key(k)?,
                    crossterm::event::Event::Paste(text) => view.on_paste(&text),
                    crossterm::event::Event::Mouse(m) => view.on_mouse(m)?,
                    crossterm::event::Event::Resize(w, h) => {
                        view.on_resize(w, h)?;
                        terminal.resize(ratatui::layout::Rect::new(0, 0, w.max(2), h.max(2)))?;
                    }
                    _ => {}
                }
            }
            view.flush_wheel()?;
            if view.dirty() || view.has_transient() {
                view.render_now(&mut terminal)?;
            }
        }
    })();

    // Release the reader thread before a reconnect spawns a new one.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = reader.join();

    match result {
        Ok(Exit::Restarting) => {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
            let _ = stdout.flush();
            Ok(Exit::Restarting)
        }
        other => {
            let _ = execute!(
                stdout,
                crossterm::cursor::Show,
                crossterm::event::DisableMouseCapture,
                DisableBracketedPaste,
                PopKeyboardEnhancementFlags,
                LeaveAlternateScreen
            );
            let _ = disable_raw_mode();
            let _ = stdout.flush();
            other
        }
    }
}

/// Apply one daemon event; `Some(exit)` when the render loop should stop.
fn apply_daemon_event(view: &mut View, ev: DaemonEvent) -> Option<Exit> {
    match ev {
        DaemonEvent::Detach => Some(Exit::Clean),
        DaemonEvent::Restarting => Some(Exit::Restarting),
        DaemonEvent::Shutdown => Some(Exit::Clean),
        other => {
            view.on_event(other);
            None
        }
    }
}

/// Read daemon events off the socket and forward them over the channel. The
/// reader wakes every `read_timeout` to check `stop` (the client's own socket
/// clones keep the write end open, so a clean detach never yields EOF here).
fn reader_loop(
    mut stream: UnixStream,
    tx: mpsc::Sender<DaemonEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let mut reader = protocol::FrameReader::default();
    let mut buf = [0u8; 8192];
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        match stream.read(&mut buf) {
            Ok(0) => return,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => continue,
            Err(_) => return,
            Ok(n) => {
                let mut frames = Vec::new();
                let is_err = reader.push(&buf[..n], &mut frames);
                if is_err {
                    return;
                }
                for f in frames {
                    let Ok((msg, _)) =
                        bincode::serde::decode_from_slice::<DaemonEvent, _>(&f, bincode::config::standard())
                    else {
                        return;
                    };
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Reconnect to the daemon socket, retrying while the restarted daemon comes
/// back up (it rebinds the socket shortly after the `kumo update` exec).
fn reconnect() -> Result<UnixStream> {
    let path = kumo_core::config::ipc_socket_path();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(s) = UnixStream::connect(&path) {
            return Ok(s);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("kumo daemon did not come back after the update restart");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Launch the `kumo daemon` process detached (own session, no stdio) so it
/// survives the client terminal closing. The daemon is the same `kumo` binary
/// (a sibling next to this executable in a cargo workspace, else `kumo` on
/// `PATH`).
fn spawn_daemon(workspace: Option<PathBuf>) -> Result<()> {
    kumo_core::daemon::spawn_detached(workspace)
        .map_err(|e| anyhow::anyhow!("failed to start the kumo daemon: {e}"))
}

/// Wait (up to a few seconds) for the freshly spawned daemon to bind its socket.
fn wait_for_daemon(path: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if UnixStream::connect(path).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("kumo daemon did not start in time")
}
