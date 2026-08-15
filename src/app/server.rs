//! Headless kumo daemon: the single source of truth.
//!
//! Owns the `App` (PTYs, ghostty emulators, the semantic layout tree, agent
//! metadata) and serves every client over the unix socket. It never renders
//! chrome: clients receive the semantic [`Layout`] (splits in ratios) and
//! per-pane [`PaneFrame`]s, and drive everything else through [`Command`]s —
//! sessions, panes, agents, input, resizes.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

use super::{App, Launch};
use crate::frames;
use crate::protocol::{ClientKind, Command, DaemonEvent, Layout, PROTOCOL_VERSION};

/// One connected client. Reads happen on a per-client reader thread; outgoing
/// messages go through a per-client writer thread with a bounded queue, so a
/// slow (or unread) client never blocks the daemon loop. A lagging client is
/// flagged (layout / pane-frame retry) instead of being dropped; a genuinely
/// dead connection is cleaned up by its reader thread (EOF -> `Detach`).
struct Client {
    tx: mpsc::SyncSender<DaemonEvent>,
    welcomed: bool,
    kind: ClientKind,
    /// True until this client has received one full composed frame (its first
    /// attach); it needs a full repaint even if the daemon's frame is a diff.
    needs_full: bool,
    /// `SubscribeLayout`: push `DaemonEvent::Layout` whenever it changes.
    wants_layout: bool,
    /// A layout push was dropped (writer queue full) and must be re-sent.
    pending_layout: bool,
    /// Panes this client subscribed to via `SubscribePane`.
    panes_subscribed: HashSet<u64>,
    /// Panes for which the client has not yet received a full `PaneFrame`
    /// (first subscribe or after a resize).
    pane_needs_full: HashSet<u64>,
}

/// Outcome of queueing one message to a client's writer thread.
enum SendOutcome {
    /// Queued successfully.
    Ok,
    /// The writer queue is full: the client is not reading. The message was
    /// dropped; the caller flags it for a retry.
    Lagging,
    /// The client's connection is gone; it must be dropped.
    Disconnected,
}

impl Client {
    fn new(tx: mpsc::SyncSender<DaemonEvent>) -> Self {
        Self {
            tx,
            welcomed: false,
            kind: ClientKind::Terminal,
            needs_full: true,
            wants_layout: false,
            pending_layout: false,
            panes_subscribed: HashSet::new(),
            pane_needs_full: HashSet::new(),
        }
    }

    fn send_msg(&mut self, msg: DaemonEvent) -> SendOutcome {
        match self.tx.try_send(msg) {
            Ok(()) => SendOutcome::Ok,
            Err(mpsc::TrySendError::Full(_)) => {
                self.needs_full = true;
                SendOutcome::Lagging
            }
            Err(mpsc::TrySendError::Disconnected(_)) => SendOutcome::Disconnected,
        }
    }
}

pub fn run_daemon(launch: Launch) -> Result<()> {
    run_daemon_at(crate::config::ipc_socket_path(), launch)
}

/// Run the daemon serving `path` (the socket). Split out so tests can drive a
/// daemon on a scratch socket without spawning a subprocess.
fn run_daemon_at(path: std::path::PathBuf, launch: Launch) -> Result<()> {
    // Create the app first: it resolves the shell/config and spawns panes. The
    // socket appears only once that's done, so callers (and tests) know the
    // daemon is fully up when they can connect.
    let mut app = App::new(launch)?;
    let mut terminal = Terminal::new(TestBackend::new(80, 24))?;

    prepare_socket(&path)?;
    let listener = UnixListener::bind(&path)?;
    set_socket_perms(&path)?;
    listener.set_nonblocking(true)?;

    let (input_tx, input_rx) = mpsc::channel::<(usize, Command)>();
    let mut clients: HashMap<usize, Client> = HashMap::new();
    let mut next_id = 0usize;
    let mut last_layout: Option<Layout> = None;
    let mut last_pane_bufs: HashMap<u64, Buffer> = HashMap::new();
    // Composed-grid state for full-attach TUI clients.
    let mut last_buffer: Option<Buffer> = None;
    let mut render_dirty = true;
    let mut last_forced = Instant::now();
    let mut kill = false;

    loop {
        // Accept new clients (each gets a reader thread + a writer thread).
        while let Ok((stream, _)) = listener.accept() {
            if !peer_owned_by_same_user(&stream) {
                log::warn!("daemon: rejecting connection from a different user");
                continue;
            }
            // A socket accepted from a nonblocking listener can inherit
            // O_NONBLOCK on some platforms; force blocking mode so the reader
            // thread actually blocks (not spin on EAGAIN).
            let _ = stream.set_nonblocking(false);
            let Ok(read_half) = stream.try_clone() else { continue };
            let (writer_tx, writer_rx) = mpsc::sync_channel::<DaemonEvent>(8);
            std::thread::spawn(move || client_write_loop(stream, writer_rx));
            let tx = input_tx.clone();
            let id = next_id;
            next_id += 1;
            std::thread::spawn(move || client_read_loop(read_half, tx, id));
            clients.insert(id, Client::new(writer_tx));
        }

        // Commands from clients.
        while let Ok((id, cmd)) = input_rx.try_recv() {
            match cmd {
                Command::Attach { protocol, kind, cols, rows } => {
                    if protocol != PROTOCOL_VERSION {
                        let _ = send_to(&mut clients, id, &DaemonEvent::Shutdown);
                        clients.remove(&id);
                        continue;
                    }
                    let _ = send_to(
                        &mut clients,
                        id,
                        &DaemonEvent::Welcome { protocol: PROTOCOL_VERSION },
                    );
                    if let Some(c) = clients.get_mut(&id) {
                        c.welcomed = true;
                        c.kind = kind;
                    }
                    // A full-attach client sizes the composed grid immediately
                    // (before its first `Resize` on terminal change), so the
                    // TUI does not start at the 80x24 default.
                    resize_terminal(&mut terminal, cols, rows);
                }
                Command::Detach => {
                    clients.remove(&id);
                }
                Command::KillServer => {
                    kill = true;
                }
                Command::ReloadConfig => {
                    app.reload_config();
                    let _ = send_to(
                        &mut clients,
                        id,
                        &DaemonEvent::ConfigReloaded { notice: "config reloaded".to_string() },
                    );
                }
                Command::Restart => {
                    // `kumo update` swapped the binary on disk: restart this
                    // process so the new version serves the sessions, inheriting
                    // the live PTY masters so panes and agents survive.
                    match restart_daemon(&app) {
                        Ok(()) => {
                            for client in clients.values_mut() {
                                let _ = client.tx.try_send(DaemonEvent::Restarting);
                            }
                            std::thread::sleep(Duration::from_millis(100));
                            let resume = crate::config::resume_file();
                            if let Err(e) = exec_new_binary(&resume) {
                                log::warn!("daemon: restart exec failed: {e:#}");
                                let _ = std::fs::remove_file(&resume);
                            }
                        }
                        Err(e) => log::warn!("daemon: restart prep failed: {e:#}"),
                    }
                }
                Command::SubscribeLayout => {
                    if let Some(c) = clients.get_mut(&id) {
                        c.wants_layout = true;
                        c.pending_layout = true;
                    }
                }
                Command::SubscribePane { pane_id } => {
                    if let Some(c) = clients.get_mut(&id) {
                        if c.panes_subscribed.insert(pane_id) {
                            c.pane_needs_full.insert(pane_id);
                        }
                    }
                }
                Command::UnsubscribePane { pane_id } => {
                    if let Some(c) = clients.get_mut(&id) {
                        c.panes_subscribed.remove(&pane_id);
                        c.pane_needs_full.remove(&pane_id);
                    }
                }
                Command::Input { key } => {
                    let key = key.to_crossterm();
                    // Terminal clients send raw keys: the daemon interprets the
                    // leader keymap, popups, and menus (the classic TUI). Other
                    // clients forward keys straight to the focused pane.
                    let is_terminal = clients.get(&id).map(|c| c.kind == ClientKind::Terminal).unwrap_or(false);
                    let result = if is_terminal {
                        app.on_key(key)
                    } else {
                        app.write_key(key);
                        Ok(())
                    };
                    if let Err(e) = result {
                        log::warn!("daemon: input error: {e:#}");
                    }
                }
                Command::Paste { text } => {
                    app.on_paste(&text);
                }
                Command::Mouse { event } => {
                    let event = event.to_crossterm();
                    let is_terminal = clients.get(&id).map(|c| c.kind == ClientKind::Terminal).unwrap_or(false);
                    let result = if is_terminal {
                        app.on_mouse(event)
                    } else {
                        app.on_pane_mouse(event);
                        Ok(())
                    };
                    if let Err(e) = result {
                        log::warn!("daemon: mouse error: {e:#}");
                    }
                }
                Command::Resize { cols, rows } => {
                    resize_terminal(&mut terminal, cols, rows);
                }
                Command::PaneResize { pane_id, cols, rows } => {
                    app.resize_pane(pane_id, cols, rows);
                }
                Command::SessionList => {
                    let sessions = app.session_info_list();
                    let _ = send_to(&mut clients, id, &DaemonEvent::SessionList { sessions });
                }
                Command::SessionNew { name, workspace } => {
                    let reply = app.new_session_command(name.as_deref(), workspace.as_ref()).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::SessionKill { name } => {
                    let reply = app.kill_session_named(&name).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::SessionFocus { name } => {
                    let _ = app.focus_session_named(&name);
                }
                Command::PaneSplit { session, dir, is_ai } => {
                    let reply = app.split_in_session(&session, dir, is_ai).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::PaneClose { session, pane_id } => {
                    let reply = app.close_pane_in_session(&session, pane_id).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::PaneFocus { session, pane_id } => {
                    let _ = app.focus_pane_in_session(&session, pane_id);
                }
                Command::PaneResizeRatio { session, dir } => {
                    let reply = app.resize_split_in_session(&session, dir).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::PaneSwap { session } => {
                    let reply = app.swap_focused(&session).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::LayoutRotate { session } => {
                    let reply = app.rotate_layout(&session).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::SessionZoom { session } => {
                    let reply = app.zoom_session(&session).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::PaneSendKeys { session, pane_id, keys } => {
                    let reply = app.send_keys(&session, pane_id, &keys).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::AgentSpawn { session, program } => {
                    let reply = app.agent_spawn(&session, program.as_deref()).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::AgentStatus => {
                    let agents = app.agent_status_lines();
                    let _ = send_to(&mut clients, id, &DaemonEvent::AgentStatus { agents });
                }
                Command::AgentKill { session, pane_id } => {
                    let reply = app.close_pane_in_session(&session, Some(pane_id)).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
            }
            // Any command may have mutated state; re-render next cycle.
            render_dirty = true;
        }

        // PTY output and background results.
        while let Ok(ev) = app.events_rx.try_recv() {
            app.on_pty_event(ev);
            render_dirty = true;
        }
        while let Ok(notice) = app.update_rx.try_recv() {
            app.update_notice = notice;
            render_dirty = true;
        }
        if last_forced.elapsed() >= Duration::from_millis(250) {
            // Periodic frame so the composed TUI stays fresh even without input.
            last_forced = Instant::now();
            render_dirty = true;
        }

        // Render the composed UI (this also fills each pane's content cache).
        if render_dirty {
            render_dirty = false;
            // Which panes were dirty *before* rendering (frame() clears the
            // flags); used to decide which pane frames changed.
            let dirty_before: HashSet<u64> = app
                .panes
                .iter()
                .filter(|(_, p)| p.dirty || p.full_redraw)
                .map(|(id, _)| *id)
                .collect();
            app.frame(&mut terminal)?;
            let (cx, cy) = {
                let pos = terminal.get_cursor_position()?;
                (pos.x, pos.y)
            };
            let new_buf = terminal.backend().buffer().clone();
            let cursor = Some((cx, cy));
            let area_changed = last_buffer.as_ref().map(|l| l.area != new_buf.area).unwrap_or(true);

            // Composed frames for full-attach TUI clients, and the semantic
            // layout + per-pane content for clients that draw their own chrome.
            let mut full_msg: Option<DaemonEvent> = None;
            let mut diff_msg: Option<DaemonEvent> = None;

            let layout_needed = clients.values().any(|c| c.wants_layout);
            let layout: Option<Layout> = if layout_needed { Some(app.layout()) } else { None };
            let layout_changed = layout_needed && last_layout.as_ref() != layout.as_ref();
            if layout_changed {
                last_layout = layout.clone();
            }

            let mut dead = Vec::new();
            for (id, client) in clients.iter_mut() {
                if !client.welcomed {
                    continue;
                }
                if client.kind == ClientKind::Terminal {
                    // Full-attach TUI: stream the composed grid.
                    let send_full = client.needs_full || area_changed;
                    let msg = if send_full {
                        client.needs_full = false;
                        full_msg.get_or_insert_with(|| {
                            DaemonEvent::Composed {
                                frame: frames::full_frame(&new_buf, cursor, &app.theme.palette),
                            }
                        })
                    } else {
                        let Some(last) = &last_buffer else { continue };
                        diff_msg.get_or_insert_with(|| {
                            DaemonEvent::Composed {
                                frame: frames::diff_frame(&new_buf, last, cursor, &app.theme.palette),
                            }
                        })
                    };
                    if matches!(client.send_msg(msg.clone()), SendOutcome::Disconnected) {
                        dead.push(*id);
                    }
                    continue;
                }

                // Semantic layout for desktop/mobile/CLI subscribers.
                if let Some(layout) = &layout {
                    if client.wants_layout && (layout_changed || client.pending_layout) {
                        match client.send_msg(DaemonEvent::Layout { layout: layout.clone() }) {
                            SendOutcome::Ok => client.pending_layout = false,
                            SendOutcome::Lagging => client.pending_layout = true,
                            SendOutcome::Disconnected => {
                                dead.push(*id);
                                continue;
                            }
                        }
                    }
                }
                // Per-pane content for pane subscribers.
                let pane_ids: Vec<u64> = client.panes_subscribed.iter().copied().collect();
                for pid in pane_ids {
                    let was_pending = client.pane_needs_full.remove(&pid);
                    let Some(cached) = app.pane_cache.get(&pid) else { continue };
                    let resized = last_pane_bufs
                        .get(&pid)
                        .map(|l| l.area != cached.area)
                        .unwrap_or(true);
                    if !was_pending && !resized && !dirty_before.contains(&pid) {
                        continue;
                    }
                    let buf = frames::detach_buffer(cached);
                    let pane_cursor = app.panes.get(&pid).and_then(|p| {
                        if p.vt.cursor_visible() {
                            p.vt.cursor_pos()
                        } else {
                            None
                        }
                    });
                    let palette = &app.theme.palette;
                    let frame = if was_pending || resized {
                        frames::pane_frame(pid, &buf, None, pane_cursor, palette)
                    } else {
                        frames::pane_frame(pid, &buf, last_pane_bufs.get(&pid), pane_cursor, palette)
                    };
                    last_pane_bufs.insert(pid, buf);
                    if !frame.full && frame.rows_dirty.is_empty() {
                        continue;
                    }
                    match client.send_msg(DaemonEvent::PaneFrame { frame }) {
                        SendOutcome::Ok => {}
                        SendOutcome::Lagging => {
                            client.pane_needs_full.insert(pid);
                        }
                        SendOutcome::Disconnected => {
                            dead.push(*id);
                            break;
                        }
                    }
                }
            }
            for id in dead {
                clients.remove(&id);
            }
            last_buffer = Some(new_buf);
        }

        // `leader+d` / MENU detach: tell every client to disconnect and keep
        // the daemon (and the panes) alive.
        if app.detach_requested {
            app.detach_requested = false;
            app.quit = false;
            for client in clients.values_mut() {
                let _ = client.tx.try_send(DaemonEvent::Detach);
            }
        }

        // Every session closed (explicit `kill`, or auto-stop when the last
        // session closes): stop the daemon.
        if app.quit || kill {
            for client in clients.values_mut() {
                let _ = client.tx.try_send(DaemonEvent::Shutdown);
            }
            break;
        }

        std::thread::sleep(Duration::from_millis(16));
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// Resize both the `TestBackend` buffer (which `Terminal::size()` reports) and
/// the `Terminal`'s own buffers + viewport. `Terminal::resize` alone only does
/// the latter, leaving the frame stuck at the initial size.
fn resize_terminal(terminal: &mut Terminal<TestBackend>, cols: u16, rows: u16) {
    let w = cols.max(1);
    let h = rows.max(1);
    terminal.backend_mut().resize(w, h);
    let _ = terminal.resize(Rect::new(0, 0, w, h));
}

/// Prepare an in-place daemon restart for `kumo update`: snapshot the live
/// sessions into the resume file (each pane's PTY master descriptor + child
/// pid) and clear `FD_CLOEXEC` on those descriptors so they survive the exec./// `portable-pty` sets the flag at openpty; without clearing it, the masters
/// would close at exec and the panes (and their processes) would die.
fn restart_daemon(app: &App) -> Result<()> {
    let Some(state) = app.to_resume_state() else {
        anyhow::bail!("nothing to resume (no live sessions)");
    };
    let path = crate::config::resume_file();
    crate::state::save(&path, &state)?;
    for pane in app.panes.values() {
        if let Some(fd) = pane.pty.raw_fd() {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags >= 0 {
                let _ = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
            }
        }
    }
    Ok(())
}

/// Replace the current process image with the `kumo` binary at the current
/// executable path — which `kumo update` already swapped for the new release —
/// asking it to resume the sessions from `resume`. The open PTY master
/// descriptors (now without `FD_CLOEXEC`) are inherited. Never returns on
/// success; `Err` means exec failed and the old daemon keeps running.
fn exec_new_binary(resume: &std::path::Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("cannot determine the current executable path")?;
    let err = std::process::Command::new(&exe)
        .arg("daemon")
        .arg("--resume")
        .arg(resume)
        .exec();
    Err(err.into())
}

/// Queue a message to one client (non-blocking).
fn send_to(clients: &mut HashMap<usize, Client>, id: usize, msg: &DaemonEvent) -> Result<()> {
    if let Some(c) = clients.get_mut(&id) {
        c.tx.try_send(msg.clone())?;
    }
    Ok(())
}

/// Writer loop for one client: drains its outgoing queue and writes frames to
/// the socket. A client that stops reading blocks here (isolated in this
/// thread) while the daemon loop keeps running and drops frames for it.
fn client_write_loop(mut stream: UnixStream, rx: mpsc::Receiver<DaemonEvent>) {
    while let Ok(msg) = rx.recv() {
        if crate::protocol::write_framed(&mut stream, &msg).is_err() {
            break;
        }
    }
}

/// Read loop for one client: decodes frames and forwards them to the daemon's
/// main loop (tagged with the client id). A closed socket yields a synthetic
/// `Detach` so the writer is dropped.
fn client_read_loop(mut stream: UnixStream, tx: mpsc::Sender<(usize, Command)>, id: usize) {
    let mut reader = crate::protocol::FrameReader::default();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
            Ok(n) => {
                let mut frames = Vec::new();
                reader.push(&buf[..n], &mut frames);
                for f in frames {
                    let Ok((msg, _)) = bincode::serde::decode_from_slice::<Command, _>(
                        &f,
                        bincode::config::standard(),
                    ) else {
                        return;
                    };
                    if tx.send((id, msg)).is_err() {
                        return;
                    }
                }
            }
        }
    }
    let _ = tx.send((id, Command::Detach));
}

/// Bind the socket: remove a stale file, reject a live daemon, create parents.
fn prepare_socket(path: &Path) -> Result<()> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            anyhow::bail!("kumo daemon is already running ({})", path.display());
        }
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Restrict the socket file to the owner (0o600).
fn set_socket_perms(path: &Path) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Whether the peer behind an accepted connection is the same user that runs
/// the daemon. The socket file perms alone are not enough: the runtime dir can
/// sit somewhere world-visible (e.g. `/tmp`), and any user able to reach the
/// path must not be able to drive the daemon's panes. Linux exposes the peer
/// credentials via `SO_PEERCRED`; the BSDs (macOS included) use `getpeereid()`.
/// A failed or unavailable probe rejects the connection (fail closed).
fn peer_owned_by_same_user(stream: &UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let our_uid = unsafe { libc::geteuid() };
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            )
        };
        rc == 0 && cred.uid == our_uid
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        rc == 0 && uid == our_uid
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )))]
    {
        // No peer-credential API; the socket perms are the only protection.
        let _ = fd;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{self, LayoutNode, SplitDir};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kumo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn wait_for_socket(path: &std::path::Path, timeout: Duration) -> UnixStream {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(s) = UnixStream::connect(path) {
                return s;
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared at {}", path.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Read the next event of interest (skipping others) within a deadline.
    fn next_event(stream: &mut UnixStream, deadline: Duration, what: &str) -> DaemonEvent {
        let deadline = Instant::now() + deadline;
        loop {
            assert!(Instant::now() < deadline, "no {what} within deadline");
            match crate::protocol::read_framed::<DaemonEvent>(stream) {
                Ok(ev) => return ev,
                Err(e) => {
                    if e.downcast_ref::<std::io::Error>()
                        .map(|e| matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    panic!("daemon connection failed: {e:#}");
                }
            }
        }
    }

    /// End-to-end: the daemon is driven purely by commands over the socket and
    /// streams the semantic layout + per-pane content (never a composed grid).
    #[test]
    fn daemon_is_command_driven() {
        let cfg = scratch("cmd-cfg");
        std::fs::write(cfg.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let _lock = crate::config::TEST_ENV_LOCK.lock().unwrap();
        let prev_cfg = std::env::var("KUMO_CONFIG_DIR").ok();
        let prev_update = std::env::var("KUMO_NO_UPDATE").ok();
        std::env::set_var("KUMO_CONFIG_DIR", &cfg);
        std::env::set_var("KUMO_NO_UPDATE", "1");

        let rt = scratch("cmd-rt");
        let sock = rt.join("kumo").join("kumo.sock");
        let s = sock.clone();
        std::thread::spawn(move || {
            let _ = run_daemon_at(s, Launch::New(None));
        });
        let _ = wait_for_socket(&sock, Duration::from_secs(10));
        match prev_cfg {
            Some(v) => std::env::set_var("KUMO_CONFIG_DIR", v),
            None => std::env::remove_var("KUMO_CONFIG_DIR"),
        }
        match prev_update {
            Some(v) => std::env::set_var("KUMO_NO_UPDATE", v),
            None => std::env::remove_var("KUMO_NO_UPDATE"),
        }
        drop(_lock);

        let mut stream = wait_for_socket(&sock, Duration::from_secs(10));
        stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();

        // Attach as a desktop viewer: Welcome, then the initial layout.
        protocol::write_framed(&mut stream, &Command::Attach {
            protocol: PROTOCOL_VERSION,
            kind: ClientKind::Desktop,
            cols: 100,
            rows: 30,
        })
        .unwrap();
        protocol::write_framed(&mut stream, &Command::SubscribeLayout).unwrap();
        loop {
            match next_event(&mut stream, Duration::from_secs(10), "Welcome") {
                DaemonEvent::Welcome { protocol: v } => {
                    assert_eq!(v, PROTOCOL_VERSION);
                    break;
                }
                _ => continue,
            }
        }
        let layout = loop {
            match next_event(&mut stream, Duration::from_secs(10), "initial Layout") {
                DaemonEvent::Layout { layout } => break layout,
                _ => continue,
            }
        };
        assert_eq!(layout.sessions.len(), 1, "one initial session");
        let root = layout.sessions[0].root.as_deref().unwrap();
        assert!(
            matches!(root, LayoutNode::Pane(_)),
            "the semantic tree must be plain panes/splits, never a composed grid"
        );

        // `kumo session new --name two`: creates and focuses a second session.
        protocol::write_framed(&mut stream, &Command::SessionNew {
            name: Some("two".into()),
            workspace: Some(PathBuf::from("/tmp")),
        })
        .unwrap();
        let layout = loop {
            match next_event(&mut stream, Duration::from_secs(10), "two-session Layout") {
                DaemonEvent::Layout { layout } if layout.sessions.len() >= 2 => break layout,
                _ => continue,
            }
        };
        assert_eq!(layout.active.as_deref(), Some("two"));
        let two = layout.sessions.iter().find(|s| s.name == "two").unwrap();
        assert_eq!(two.workspace, PathBuf::from("/tmp"));
        let two_pane = match two.root.as_deref().unwrap() {
            LayoutNode::Pane(p) => p.id,
            _ => panic!("expected a single pane"),
        };

        // `kumo pane split -s two`: the tree becomes a semantic split.
        protocol::write_framed(&mut stream, &Command::PaneSplit {
            session: "two".into(),
            dir: SplitDir::Vertical,
            is_ai: false,
        })
        .unwrap();
        let layout = loop {
            match next_event(&mut stream, Duration::from_secs(10), "split Layout") {
                DaemonEvent::Layout { layout } => {
                    let two = layout.sessions.iter().find(|s| s.name == "two").unwrap();
                    if let Some(root) = &two.root {
                        if matches!(root.as_ref(), LayoutNode::Split { .. }) {
                            break layout;
                        }
                    }
                    continue;
                }
                _ => continue,
            }
        };
        let two = layout.sessions.iter().find(|s| s.name == "two").unwrap();
        let root = two.root.as_deref().unwrap();
        match root {
            LayoutNode::Split { dir, ratio, a, b } => {
                assert_eq!(*dir, SplitDir::Vertical);
                assert!((0.0..=1.0).contains(ratio));
                assert!(matches!(a.as_ref(), LayoutNode::Pane(_)));
                assert!(matches!(b.as_ref(), LayoutNode::Pane(_)));
            }
            _ => panic!("expected a vertical split, got {root:?}"),
        }

        // Subscribe a pane and resize the composed grid; the pane's PaneFrame
        // streams at its composed inner size.
        protocol::write_framed(&mut stream, &Command::SubscribePane { pane_id: two_pane }).unwrap();
        protocol::write_framed(&mut stream, &Command::Resize { cols: 60, rows: 20 }).unwrap();
        let frame = loop {
            match next_event(&mut stream, Duration::from_secs(10), "PaneFrame") {
                DaemonEvent::PaneFrame { frame } if frame.pane_id == two_pane => break frame,
                DaemonEvent::Layout { .. } => continue,
                other => panic!("unexpected event while waiting for a PaneFrame: {other:?}"),
            }
        };
        // 60x20 grid: the pane streams at its composed inner size (the daemon's
        // chrome — sidebar/status — shrinks the pane area, as in the TUI).
        assert!(
            frame.cols > 0 && frame.rows > 0,
            "the pane must stream at a real composed size, got {}x{}",
            frame.cols,
            frame.rows
        );

        // `kumo session list` returns metadata (no geometry/rects).
        protocol::write_framed(&mut stream, &Command::SessionList).unwrap();
        let sessions = loop {
            match next_event(&mut stream, Duration::from_secs(10), "SessionList") {
                DaemonEvent::SessionList { sessions } => break sessions,
                _ => continue,
            }
        };
        assert_eq!(sessions.len(), 2);
        let two_info = sessions.iter().find(|s| s.name == "two").unwrap();
        assert_eq!(two_info.pane_count, 2, "the split added a pane");

        // A full-attach TUI client gets the composed grid — with box-drawing
        // borders in it, exactly like the classic TUI.
        let mut tstream = wait_for_socket(&sock, Duration::from_secs(10));
        tstream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        protocol::write_framed(&mut tstream, &Command::Attach {
            protocol: PROTOCOL_VERSION,
            kind: ClientKind::Terminal,
            cols: 80,
            rows: 24,
        })
        .unwrap();
        let composed = loop {
            match next_event(&mut tstream, Duration::from_secs(10), "Composed") {
                DaemonEvent::Composed { frame } if frame.full => break frame,
                _ => continue,
            }
        };
        assert!(!composed.rows_dirty.is_empty(), "a full composed frame has rows");
        let has_box_chars = composed.rows_dirty.iter().flat_map(|p| p.cells.iter()).any(|c| {
            c.text.chars().any(|ch| matches!(ch, '│' | '─' | '┌' | '┐' | '└' | '┘'))
        });
        assert!(has_box_chars, "the composed grid must include the pane borders the daemon draws for the TUI");

        // `kumo session kill two`: back to one session.
        protocol::write_framed(&mut stream, &Command::SessionKill { name: "two".into() }).unwrap();
        loop {
            match next_event(&mut stream, Duration::from_secs(10), "post-kill Layout") {
                DaemonEvent::Layout { layout } => {
                    if layout.sessions.len() == 1 {
                        break;
                    }
                }
                _ => continue,
            }
        }

        protocol::write_framed(&mut stream, &Command::KillServer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while sock.exists() {
            assert!(Instant::now() < deadline, "daemon socket not removed after kill");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&rt);
    }
}
