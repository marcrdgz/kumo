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

use crate::app::Launch;
use crate::client_view::View;
use crate::protocol::{self, ClientKind, Command, DaemonEvent};

pub fn run(launch: Launch) -> Result<()> {
    let path = crate::config::ipc_socket_path();
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

    // Daemon-event reader thread: forwards every frame the daemon pushes. When
    // the daemon exits (or closes the connection on detach) the read returns
    // EOF and the thread ends, which the main loop sees as a clean exit.
    let write_half = stream.try_clone()?;
    let (ev_tx, ev_rx) = mpsc::channel::<DaemonEvent>();
    let reader = std::thread::spawn(move || reader_loop(write_half, ev_tx));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;

    let mut view = View::new(stream.try_clone()?, cols, rows);
    view.render_now(&mut terminal)?;

    let result: Result<Exit> = (|| {
        loop {
            // Daemon events (16ms timeout so local input stays responsive).
            match ev_rx.recv_timeout(Duration::from_millis(16)) {
                Ok(ev) => {
                    let exit = match ev {
                        DaemonEvent::Detach => Some(Exit::Clean),
                        DaemonEvent::Restarting => Some(Exit::Restarting),
                        DaemonEvent::Shutdown => Some(Exit::Clean),
                        other => {
                            view.on_event(other);
                            None
                        }
                    };
                    if let Some(exit) = exit {
                        return Ok(exit);
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
            if crossterm::event::poll(Duration::from_millis(0))? {
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
            if view.dirty() || view.has_transient() {
                view.render_now(&mut terminal)?;
            }
        }
    })();

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

/// Read daemon events off the socket and forward them over the channel. The
/// reader blocks on the socket; when the daemon closes the connection (exit,
/// detach, restart) the read returns EOF/Err and the thread ends, closing the
/// channel.
fn reader_loop(mut stream: UnixStream, tx: mpsc::Sender<DaemonEvent>) {
    let mut reader = protocol::FrameReader::default();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Err(_) => break,
            Ok(n) => {
                let mut frames = Vec::new();
                reader.push(&buf[..n], &mut frames);
                for f in frames {
                    let Ok((msg, _)) =
                        bincode::serde::decode_from_slice::<DaemonEvent, _>(&f, bincode::config::standard())
                    else {
                        break;
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
    let path = crate::config::ipc_socket_path();
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

/// Launch the daemon as a detached process (own session, no stdio) so it
/// survives the client terminal closing.
fn spawn_daemon(workspace: Option<PathBuf>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
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
    cmd.spawn()?;
    Ok(())
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
