//! Smart terminal client: a "dumb viewport with chrome" for the kumo daemon.
//!
//! Connects to the daemon socket, subscribes to the semantic layout + per-pane
//! content, computes its own geometry, and draws ALL chrome (borders, sidebar,
//! status bar, menus, popups) with ratatui — exactly like the desktop app, but
//! in a host terminal. The daemon never renders chrome.

use std::io;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
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

/// Bound daemon-event memory while keeping the socket reader independent from
/// terminal rendering speed. The reader retries full sends, so events remain
/// FIFO and no pane-frame deltas are discarded.
const EVENT_QUEUE_CAP: usize = 256;
/// Maximum daemon events applied before local input gets a turn.
const EVENT_BATCH_MAX: usize = 128;
/// Maximum time spent applying one daemon-event batch before local input and
/// rendering are serviced.
const EVENT_BATCH_BUDGET: Duration = Duration::from_millis(4);
/// Retry interval while the bounded event queue is full. The stop flag is
/// checked between retries so teardown cannot wait on a blocked producer.
const EVENT_QUEUE_RETRY: Duration = Duration::from_millis(1);
/// Bound local input memory while keeping the crossterm reader independent
/// from render speed. Key events are retried in order when this fills.
const INPUT_QUEUE_CAP: usize = 128;
/// Retry interval while the bounded input queue is full.
const INPUT_QUEUE_RETRY: Duration = Duration::from_millis(1);
/// Maximum local input events handled before daemon events get another turn.
const INPUT_BATCH_MAX: usize = 128;
/// Maximum time spent handling one local input batch.
const INPUT_BATCH_BUDGET: Duration = Duration::from_millis(4);
/// How often a stopped input reader checks its stop flag while no key arrives.
const INPUT_POLL_TIMEOUT: Duration = Duration::from_millis(10);
/// Keep client-local timers (clock, spinner and expiring overlays) moving.
const RENDER_WAKE: Duration = Duration::from_millis(8);

#[derive(Debug)]
enum InputMessage {
    Event(crossterm::event::Event),
    Error(String),
}

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

    // Finish all fallible terminal/view setup before starting worker threads.
    // This keeps an initialization error from orphaning a producer that is
    // already blocked on the socket or terminal input.
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let mut view = View::new(stream.try_clone()?, cols, rows);
    view.render_now(&mut terminal)?;

    // Daemon-event reader thread: forwards every frame the daemon pushes. It
    // wakes on a read timeout to check the stop flag, because the client's own
    // socket clones keep the write end open — the reader would otherwise block
    // forever on a clean detach (the daemon closes its end, but our clones
    // keep the connection "alive" from the reader's perspective).
    let write_half = stream.try_clone()?;
    write_half.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let (ev_tx, ev_rx) = mpsc::sync_channel::<DaemonEvent>(EVENT_QUEUE_CAP);
    let (input_tx, input_rx) = mpsc::sync_channel::<InputMessage>(INPUT_QUEUE_CAP);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let main_thread = std::sync::Arc::new(std::thread::current());
    let stop2 = stop.clone();
    let daemon_wake = main_thread.clone();
    let reader = std::thread::spawn(move || {
        reader_loop(write_half, ev_tx, stop2, daemon_wake.clone());
        // Wake even when the socket closes without a final protocol frame.
        wake_render_thread(&daemon_wake);
    });

    // Keep terminal input off the render thread. The reader wakes the render
    // thread as soon as crossterm reports an event, so daemon traffic cannot
    // add a fixed input polling delay.
    let input_stop = stop.clone();
    let input_wake = main_thread.clone();
    let input_reader = std::thread::spawn(move || input_loop(input_tx, input_stop, input_wake));

    let result: Result<Exit> = (|| {
        loop {
            // Daemon events and local input are both drained without blocking.
            // Bounded batches preserve responsiveness when a pane is emitting
            // output or a key is held down.
            let daemon_started = Instant::now();
            let mut daemon_count = 0usize;
            let mut daemon_disconnected = false;
            while daemon_count < EVENT_BATCH_MAX && daemon_started.elapsed() < EVENT_BATCH_BUDGET {
                match ev_rx.try_recv() {
                    Ok(ev) => {
                        daemon_count += 1;
                        if let Some(exit) = apply_daemon_event(&mut view, ev) {
                            return Ok(exit);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        daemon_disconnected = true;
                        break;
                    }
                }
            }
            let daemon_budget_hit = daemon_count == EVENT_BATCH_MAX || daemon_started.elapsed() >= EVENT_BATCH_BUDGET;
            if daemon_disconnected {
                return Ok(Exit::Clean);
            }

            let input_started = Instant::now();
            let mut input_count = 0usize;
            while input_count < INPUT_BATCH_MAX && input_started.elapsed() < INPUT_BATCH_BUDGET {
                let Ok(msg) = input_rx.try_recv() else { break };
                input_count += 1;
                match msg {
                    InputMessage::Event(crossterm::event::Event::Key(k)) => view.on_key(k)?,
                    InputMessage::Event(crossterm::event::Event::Paste(text)) => view.on_paste(&text),
                    InputMessage::Event(crossterm::event::Event::Mouse(m)) => view.on_mouse(m)?,
                    InputMessage::Event(crossterm::event::Event::Resize(w, h)) => {
                        view.on_resize(w, h)?;
                        terminal.resize(ratatui::layout::Rect::new(0, 0, w.max(2), h.max(2)))?;
                    }
                    InputMessage::Event(_) => {}
                    InputMessage::Error(message) => return Err(anyhow::anyhow!(message)),
                }
            }
            let input_budget_hit = input_count == INPUT_BATCH_MAX || input_started.elapsed() >= INPUT_BATCH_BUDGET;
            view.flush_wheel()?;
            let transient = view.has_transient();
            if view.dirty() || transient {
                view.render_now(&mut terminal)?;
            }
            if view.detach_requested() {
                return Ok(Exit::Clean);
            }

            // A producer may have filled a batch while we were rendering. Do
            // not park in that case; the next bounded batch gives the other
            // queue a chance before more input is consumed.
            if daemon_budget_hit || input_budget_hit {
                continue;
            }
            // Keep the existing 8 ms timer cadence for client-local state.
            // `has_transient` only reports currently visible overlays; it does
            // not account for future clock/spinner deadlines or background
            // results that can make the next frame dirty.
            std::thread::park_timeout(RENDER_WAKE);
        }
    })();

    // Close all client socket clones before joining the reader. The bounded
    // producer checks `stop` between retries, while shutdown also wakes any
    // pending socket read; both keep reconnect and clean detach deterministic.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    drop(view);
    let _ = stream.shutdown(Shutdown::Both);
    main_thread.unpark();
    let _ = reader.join();
    let _ = input_reader.join();

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
    tx: SyncSender<DaemonEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    wake: std::sync::Arc<std::thread::Thread>,
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
                    if !send_event_and_wake(&tx, msg, &stop, &wake) {
                        return;
                    }
                }
            }
        }
    }
}

/// Read crossterm input independently of rendering. `poll` wakes immediately
/// for a key; its timeout only bounds how long teardown waits when the terminal
/// is quiet so the stop flag is observed promptly.
fn input_loop(
    tx: mpsc::SyncSender<InputMessage>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    wake: std::sync::Arc<std::thread::Thread>,
) {
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        match crossterm::event::poll(INPUT_POLL_TIMEOUT) {
            Ok(false) => continue,
            Ok(true) => match crossterm::event::read() {
                Ok(event) => {
                    if !send_input_and_wake(&tx, InputMessage::Event(event), &stop, &wake) {
                        return;
                    }
                }
                Err(error) => {
                    let message = format!("terminal input failed: {error}");
                    let _ = send_input_and_wake(&tx, InputMessage::Error(message), &stop, &wake);
                    return;
                }
            },
            Err(error) => {
                let message = format!("terminal input polling failed: {error}");
                let _ = send_input_and_wake(&tx, InputMessage::Error(message), &stop, &wake);
                return;
            }
        }
    }
}

fn wake_render_thread(wake: &std::thread::Thread) {
    wake.unpark();
}

/// Queue local input without dropping or reordering events when rendering is
/// temporarily slower than a held key stream.
fn send_input(
    tx: &mpsc::SyncSender<InputMessage>,
    mut message: InputMessage,
    stop: &std::sync::atomic::AtomicBool,
) -> bool {
    loop {
        match tx.try_send(message) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
            Err(mpsc::TrySendError::Full(returned)) => {
                message = returned;
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return false;
                }
                std::thread::sleep(INPUT_QUEUE_RETRY);
            }
        }
    }
}

fn send_input_and_wake(
    tx: &mpsc::SyncSender<InputMessage>,
    message: InputMessage,
    stop: &std::sync::atomic::AtomicBool,
    wake: &std::thread::Thread,
) -> bool {
    if !send_input(tx, message, stop) {
        return false;
    }
    wake_render_thread(wake);
    true
}

/// Queue one daemon event without dropping it when the bounded FIFO is full.
/// Returning the event from `TrySendError::Full` keeps ownership through every
/// retry and preserves ordering across incremental pane frames and controls.
fn send_event(
    tx: &SyncSender<DaemonEvent>,
    mut event: DaemonEvent,
    stop: &std::sync::atomic::AtomicBool,
) -> bool {
    loop {
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return false;
                }
                std::thread::sleep(EVENT_QUEUE_RETRY);
            }
        }
    }
}

fn send_event_and_wake(
    tx: &SyncSender<DaemonEvent>,
    event: DaemonEvent,
    stop: &std::sync::atomic::AtomicBool,
    wake: &std::thread::Thread,
) -> bool {
    if !send_event(tx, event, stop) {
        return false;
    }
    wake_render_thread(wake);
    true
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn reply(n: usize) -> DaemonEvent {
        DaemonEvent::Reply { message: n.to_string() }
    }

    fn reply_number(event: DaemonEvent) -> usize {
        let DaemonEvent::Reply { message } = event else {
            panic!("test queue contained an unexpected event");
        };
        message.parse().expect("reply message is a test sequence number")
    }

    #[test]
    fn bounded_event_queue_retries_without_reordering_or_dropping() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(reply(0)).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let producer_stop = stop.clone();
        let producer = std::thread::spawn(move || {
            for n in 1..=64 {
                assert!(send_event(&tx, reply(n), &producer_stop));
            }
        });

        let mut received = Vec::new();
        for _ in 0..=64 {
            received.push(reply_number(rx.recv().unwrap()));
        }
        producer.join().unwrap();
        assert_eq!(received, (0..=64).collect::<Vec<_>>());
    }

    #[test]
    fn bounded_event_queue_stops_retrying_when_shutdown_is_requested() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(reply(0)).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let producer_stop = stop.clone();
        let producer = std::thread::spawn(move || send_event(&tx, reply(1), &producer_stop));

        std::thread::sleep(EVENT_QUEUE_RETRY * 2);
        stop.store(true, Ordering::Relaxed);
        assert!(!producer.join().unwrap(), "full-queue producer must exit on shutdown");
        assert_eq!(reply_number(rx.try_recv().unwrap()), 0);
    }

    #[test]
    fn bounded_input_queue_stops_retrying_when_shutdown_is_requested() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(InputMessage::Event(crossterm::event::Event::FocusGained)).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let producer_stop = stop.clone();
        let producer = std::thread::spawn(move || {
            send_input(
                &tx,
                InputMessage::Event(crossterm::event::Event::FocusLost),
                &producer_stop,
            )
        });

        std::thread::sleep(INPUT_QUEUE_RETRY * 2);
        stop.store(true, Ordering::Relaxed);
        assert!(!producer.join().unwrap(), "full input queue must exit on shutdown");
        assert!(matches!(rx.try_recv(), Ok(InputMessage::Event(crossterm::event::Event::FocusGained))));
    }

    #[test]
    fn producer_wakes_a_parked_render_thread() {
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            std::thread::park();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();

        let wake = waiter.thread().clone();
        let stop = AtomicBool::new(false);
        let (input_tx, _input_rx) = mpsc::sync_channel(1);
        assert!(send_input_and_wake(
            &input_tx,
            InputMessage::Event(crossterm::event::Event::FocusGained),
            &stop,
            &wake,
        ));
        done_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("queued daemon event must wake the render thread");
        waiter.join().unwrap();
    }
}
