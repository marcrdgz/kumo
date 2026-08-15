//! Headless kumo daemon: owns the `App` (PTYs + terminal emulators + the whole
//! UI), renders it into a `TestBackend`, and streams the resulting frames to
//! attached terminal clients over the unix socket. Detach only closes the
//! client connection; the daemon keeps running until the last session closes.

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
use crate::protocol::{self, AgentInfo, ClientKind, ClientMsg, PaneInfo, ServerMsg};

impl From<crate::agents::AgentStatus> for protocol::AgentStatus {
    fn from(status: crate::agents::AgentStatus) -> Self {
        match status {
            crate::agents::AgentStatus::Working => protocol::AgentStatus::Working,
            crate::agents::AgentStatus::Blocked => protocol::AgentStatus::Blocked,
            crate::agents::AgentStatus::Idle => protocol::AgentStatus::Idle,
        }
    }
}

/// One connected client. The read half lives in a per-client reader thread;
/// outgoing messages go through a per-client writer thread with a bounded
/// queue, so a slow (or unread) client never blocks the daemon loop — a
/// lagging client is paused (and later caught up with a full frame) instead of
/// stalling everyone. A client is only dropped when its connection is
/// genuinely gone (writer dead or the reader thread saw EOF).
struct Client {
    tx: mpsc::SyncSender<ServerMsg>,
    welcomed: bool,
    /// What kind of client this is (drives which channels it gets).
    kind: ClientKind,
    /// True until this client has received one full frame (its first attach);
    /// it needs a full repaint even if the daemon's own frame is a diff.
    needs_full: bool,
    /// `SubscribeSnapshot`: push `ServerMsg::Snapshot` whenever it changes.
    wants_snapshot: bool,
    /// Panes this client subscribed to via `SubscribePane`.
    panes_subscribed: HashSet<u64>,
    /// Panes for which the client has not yet received a full `PaneFrame`
    /// (first subscribe or after a resize).
    pane_needs_full: HashSet<u64>,
    /// A snapshot was dropped (writer queue full) and must be re-sent. Unlike
    /// frames, snapshots are only produced when the state *changes*, so a
    /// dropped one would otherwise be lost forever.
    pending_snapshot: bool,
}

/// Outcome of queueing one message to a client's writer thread.
enum SendOutcome {
    /// Queued successfully.
    Ok,
    /// The writer queue is full: the client is not reading (backgrounded tab,
    /// asleep machine). The message was dropped; the caller decides whether and
    /// how to retry.
    Lagging,
    /// The client's connection is gone; it must be dropped.
    Disconnected,
}

impl Client {
    /// Queue a message to the client's writer thread.
    ///
    /// A full queue is *not* fatal: it means the client stopped reading (tab
    /// backgrounded, terminal busy, machine asleep) and its writer thread is
    /// blocked writing to the socket. Dropping the client there would orphan a
    /// live connection — the client stays frozen on its last frame, unable to
    /// detach, with no EOF to wake it. Instead the message is dropped and the
    /// caller flags it for a retry (full frame, pending snapshot, pane full
    /// frame). Dead clients are cleaned up by their reader thread
    /// (EOF -> `Detach`).
    fn send_msg(&mut self, msg: ServerMsg) -> SendOutcome {
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

    let (input_tx, input_rx) = mpsc::channel::<(usize, ClientMsg)>();
    let mut clients: HashMap<usize, Client> = HashMap::new();
    let mut next_id = 0usize;
    let mut last_buffer: Option<Buffer> = None;
    // Last snapshot sent to subscribers (diffed against to avoid spam).
    let mut last_snapshot: Option<Vec<protocol::SessionInfo>> = None;
    // Last per-pane buffer serialized to subscribers (pane-frame diff baseline).
    let mut last_pane_bufs: HashMap<u64, Buffer> = HashMap::new();
    let mut kill = false;

    let mut render_dirty = true;
    let mut last_forced = Instant::now();

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
            let (writer_tx, writer_rx) = mpsc::sync_channel::<ServerMsg>(1);
            std::thread::spawn(move || client_write_loop(stream, writer_rx));
            let tx = input_tx.clone();
            let id = next_id;
            next_id += 1;
            std::thread::spawn(move || client_read_loop(read_half, tx, id));
            clients.insert(
                id,
                Client {
                    tx: writer_tx,
                    welcomed: false,
                    kind: ClientKind::Terminal,
                    needs_full: true,
                    wants_snapshot: false,
                    panes_subscribed: HashSet::new(),
                    pane_needs_full: HashSet::new(),
                    pending_snapshot: false,
                },
            );
            render_dirty = true;
        }

        // Input from clients.
        while let Ok((id, msg)) = input_rx.try_recv() {
            match msg {
                ClientMsg::Hello { protocol, cols, rows, kind } => {
                    if protocol != protocol::PROTOCOL_VERSION {
                        let _ = send_to(&mut clients, id, &ServerMsg::Shutdown);
                        clients.remove(&id);
                        continue;
                    }
                    let _ = send_to(
                        &mut clients,
                        id,
                        &ServerMsg::Welcome { protocol: protocol::PROTOCOL_VERSION },
                    );
                    if let Some(c) = clients.get_mut(&id) {
                        c.welcomed = true;
                        c.kind = kind;
                    }
                    resize_terminal(&mut terminal, cols, rows);
                    render_dirty = true;
                }
                ClientMsg::Input { key } => {
                    if let Err(e) = app.on_key(key.to_crossterm()) {
                        log::warn!("daemon: input error: {e:#}");
                    }
                    render_dirty = true;
                }
                ClientMsg::Paste { text } => {
                    app.on_paste(&text);
                    render_dirty = true;
                }
                ClientMsg::Mouse { event } => {
                    if let Err(e) = app.on_mouse(event.to_crossterm()) {
                        log::warn!("daemon: mouse error: {e:#}");
                    }
                    render_dirty = true;
                }
                ClientMsg::Resize { cols, rows } => {
                    resize_terminal(&mut terminal, cols, rows);
                    render_dirty = true;
                }
                ClientMsg::Detach => {
                    clients.remove(&id);
                }
                ClientMsg::ListSessions => {
                    let sessions = session_list(&app);
                    let _ = send_to(&mut clients, id, &ServerMsg::SessionList { sessions });
                }
                ClientMsg::SubscribeSnapshot => {
                    if let Some(c) = clients.get_mut(&id) {
                        c.wants_snapshot = true;
                    }
                    // Respond immediately with the current state; a lagging
                    // client is flagged so the snapshot is re-sent.
                    let sessions = session_list(&app);
                    let snap = ServerMsg::Snapshot { sessions };
                    if let Some(c) = clients.get_mut(&id) {
                        match c.send_msg(snap) {
                            SendOutcome::Ok => c.pending_snapshot = false,
                            SendOutcome::Lagging => c.pending_snapshot = true,
                            SendOutcome::Disconnected => {}
                        }
                    }
                    render_dirty = true;
                }
                ClientMsg::SubscribePane { pane_id } => {
                    if let Some(c) = clients.get_mut(&id) {
                        if c.panes_subscribed.insert(pane_id) {
                            c.pane_needs_full.insert(pane_id);
                        }
                    }
                    render_dirty = true;
                }
                ClientMsg::UnsubscribePane { pane_id } => {
                    if let Some(c) = clients.get_mut(&id) {
                        c.panes_subscribed.remove(&pane_id);
                        c.pane_needs_full.remove(&pane_id);
                    }
                }
                ClientMsg::FocusSession { name } => {
                    if app.focus_session_named(&name) {
                        render_dirty = true;
                    }
                }
                ClientMsg::FocusPane { session, pane_id } => {
                    if app.focus_pane_in_session(&session, pane_id) {
                        render_dirty = true;
                    }
                }
                ClientMsg::SetSidebar { open } => {
                    app.sidebar_open = open;
                    render_dirty = true;
                }
                ClientMsg::KillServer => {
                    kill = true;
                }
                ClientMsg::ReloadConfig => {
                    app.reload_config();
                    let notice = "config reloaded".to_string();
                    let _ = send_to(&mut clients, id, &ServerMsg::ConfigReloaded { notice });
                    render_dirty = true;
                }
                ClientMsg::Restart => {
                    // `kumo update` swapped the binary on disk: restart this
                    // process so the new version serves the sessions, inheriting
                    // the live PTY masters so panes and agents survive.
                    match restart_daemon(&app) {
                        Ok(()) => {
                            // The prep is done: tell the attached terminals to
                            // reconnect, give their writer threads a beat to
                            // flush the message (local unix socket, effectively
                            // instant), then exec the new binary. A successful
                            // exec never returns to this loop.
                            for client in clients.values_mut() {
                                let _ = client.tx.try_send(ServerMsg::Restarting);
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
                    render_dirty = true;
                }
                ClientMsg::NewSession { workspace } => {
                    // `kumo new` against a running daemon: create a fresh
                    // session and focus it. The client resolved its own cwd
                    // when no dir was given; a path that is not a directory
                    // falls back to the daemon's own workspace.
                    let ws = match workspace {
                        Some(p) if p.is_dir() => p,
                        _ => app.workspace.clone(),
                    };
                    if let Err(e) = app.new_session_in(ws) {
                        log::warn!("daemon: new session error: {e:#}");
                    }
                    render_dirty = true;
                }
            }
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
            // Periodic frame so agent-status dots stay fresh even without input.
            last_forced = Instant::now();
            render_dirty = true;
        }

        if render_dirty {
            render_dirty = false;
            app.frame(&mut terminal)?;
            let (cx, cy) = {
                let pos = terminal.get_cursor_position()?;
                (pos.x, pos.y)
            };
            let new_buf = terminal.backend().buffer().clone();
            let cursor = Some((cx, cy));
            let area_changed = last_buffer.as_ref().map(|l| l.area != new_buf.area).unwrap_or(true);

            // Build the shared messages lazily: one full frame (if anything needs
            // it) and one row-diff. A client gets the full frame on its first
            // attach or after a resize, then the row diffs.
            let mut full_msg: Option<ServerMsg> = None;
            let mut diff_msg: Option<ServerMsg> = None;
            let mut dead = Vec::new();

            // Snapshot push: built only when a subscriber needs it — the state
            // changed, or a previous snapshot was dropped while the client
            // lagged (writer queue full). Diffed against the last sent snapshot
            // so idle cycles send nothing.
            let any_subscriber = clients.values().any(|c| c.wants_snapshot);
            let snapshot_msg: Option<ServerMsg> = if any_subscriber {
                let any_pending = clients.values().any(|c| c.wants_snapshot && c.pending_snapshot);
                let sessions = session_list(&app);
                if last_snapshot.as_ref() != Some(&sessions) || any_pending {
                    last_snapshot = Some(sessions.clone());
                    Some(ServerMsg::Snapshot { sessions })
                } else {
                    None
                }
            } else {
                None
            };

            // Pane-frame delivery: detach each subscribed pane's retained cache
            // to its own origin grid and precompute the full + diff variants.
            let mut pane_msgs: HashMap<u64, (ServerMsg, ServerMsg)> = HashMap::new();
            let mut subscribed: Vec<u64> = clients
                .values()
                .flat_map(|c| c.panes_subscribed.iter().copied())
                .collect();
            subscribed.sort_unstable();
            subscribed.dedup();
            for pid in subscribed {
                let Some(cached) = app.pane_cache.get(&pid) else { continue };
                let buf = frames::detach_buffer(cached);
                let cursor = app.panes.get(&pid).and_then(|p| {
                    if p.vt.cursor_visible() {
                        p.vt.cursor_pos()
                    } else {
                        None
                    }
                });
                let palette = &app.theme.palette;
                let full = frames::pane_frame(pid, &buf, None, cursor, palette);
                let diff = frames::pane_frame(pid, &buf, last_pane_bufs.get(&pid), cursor, palette);
                pane_msgs.insert(
                    pid,
                    (
                        ServerMsg::PaneFrame { frame: full },
                        ServerMsg::PaneFrame { frame: diff },
                    ),
                );
                last_pane_bufs.insert(pid, buf);
            }

            for (id, client) in clients.iter_mut() {
                if !client.welcomed {
                    continue;
                }
                // Non-terminal clients (desktop/mobile) never consume the
                // composed grid — they get snapshots + pane frames — so skip
                // the composed frame to keep their writer queue free for the
                // channels they actually use.
                let wants_composed = client.kind == ClientKind::Terminal;
                if wants_composed {
                    let send_full = client.needs_full || area_changed;
                    let msg = if send_full {
                        client.needs_full = false;
                        full_msg.get_or_insert_with(|| {
                            ServerMsg::Frame {
                                frame: frames::full_frame(&new_buf, cursor, &app.theme.palette),
                            }
                        })
                    } else {
                        let Some(last) = &last_buffer else { continue };
                        diff_msg.get_or_insert_with(|| {
                            ServerMsg::Frame {
                                frame: frames::diff_frame(&new_buf, last, cursor, &app.theme.palette),
                            }
                        })
                    };
                    // A full queue means the client is lagging (not reading),
                    // never that it is gone: keep it and let it catch up with a
                    // full frame. Only a disconnected writer drops the client.
                    if matches!(client.send_msg(msg.clone()), SendOutcome::Disconnected) {
                        dead.push(*id);
                        continue;
                    }
                }
                if client.wants_snapshot {
                    if let Some(snap) = &snapshot_msg {
                        match client.send_msg(snap.clone()) {
                            SendOutcome::Ok => client.pending_snapshot = false,
                            SendOutcome::Lagging => client.pending_snapshot = true,
                            SendOutcome::Disconnected => {
                                dead.push(*id);
                                continue;
                            }
                        }
                    }
                }
                let pane_ids: Vec<u64> = client.panes_subscribed.iter().copied().collect();
                for pid in pane_ids {
                    let Some((full, diff)) = pane_msgs.get(&pid) else { continue };
                    let was_pending = client.pane_needs_full.remove(&pid);
                    if !was_pending {
                        // Nothing changed for this pane since the last sent
                        // frame; skip it so it never starves other panes.
                        if let ServerMsg::PaneFrame { frame } = diff {
                            if frame.rows_dirty.is_empty() {
                                continue;
                            }
                        }
                    }
                    let msg = if was_pending { full } else { diff };
                    match client.send_msg(msg.clone()) {
                        SendOutcome::Ok => {}
                        SendOutcome::Lagging => {
                            // Dropped while the client lagged; re-flag so it is
                            // re-sent as a full frame when the client reads again.
                            client.pane_needs_full.insert(pid);
                            break;
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
                let _ = client.tx.try_send(ServerMsg::Detach);
            }
        }
        // Every session closed (explicit `kumo kill`, or auto-stop when the
        // last session closes): stop the daemon.
        if app.quit || kill {
            for client in clients.values_mut() {
                let _ = client.tx.try_send(ServerMsg::Shutdown);
            }
            break;
        }

        std::thread::sleep(Duration::from_millis(16));
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// Prepare an in-place daemon restart for `kumo update`: snapshot the live
/// sessions into the resume file (each pane's PTY master descriptor + child
/// pid) and clear `FD_CLOEXEC` on those descriptors so they survive the exec.
/// `portable-pty` sets the flag at openpty; without clearing it, the masters
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

/// Resize both the `TestBackend` buffer (which `Terminal::size()` reports) and
/// the `Terminal`'s own buffers + viewport. `Terminal::resize` alone only does
/// the latter, leaving the frame stuck at the initial size.
fn resize_terminal(terminal: &mut Terminal<TestBackend>, cols: u16, rows: u16) {
    let w = cols.max(1);
    let h = rows.max(1);
    terminal.backend_mut().resize(w, h);
    let _ = terminal.resize(Rect::new(0, 0, w, h));
}

/// Build the structured session list (used by `kumo ls`, `ListSessions`, and
/// snapshot pushes). Every field is data the daemon already keeps: session
/// name/workspace, per-pane title/cwd/AI marker, the agent lifecycle cache, and
/// each pane's geometry within its session's grid (so native clients can paint
/// panes themselves instead of showing the daemon's composed UI).
fn session_list(app: &App) -> Vec<protocol::SessionInfo> {
    // The pane area the daemon lays out within — the same rect the TUI chrome
    // uses, so the reported rects match the rendered pane caches exactly.
    let area = app.panes_area();
    app.sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // Geometry for this session's tree over the pane area. A zoomed
            // session shows only its focused pane full-size.
            let mut geom = crate::layout::TreeGeom::default();
            if let Some(root) = &s.tree.root {
                if s.zoom {
                    geom.panes.push(crate::layout::PaneGeom { pane_id: s.tree.focus, rect: area });
                } else {
                    crate::layout::compute_geometry(root, area, &mut geom);
                }
            }
            let rect_of = |pid: u64| {
                geom.panes
                    .iter()
                    .find(|p| p.pane_id == pid)
                    .map(|p| p.inner())
                    .map(|r| protocol::PaneRect { x: r.x, y: r.y, width: r.width, height: r.height })
            };
            protocol::SessionInfo {
                name: s.name.clone(),
                workspace: s.workspace.clone(),
                panes: s
                    .tree
                    .pane_ids()
                    .into_iter()
                    .filter_map(|pid| {
                        let pane = app.panes.get(&pid)?;
                        let is_ai = pane.is_ai_cli();
                        let agent = if is_ai {
                            Some(AgentInfo {
                                name: app.agent_label(pid),
                                status: app
                                    .agent_status_cache
                                    .get(&pid)
                                    .copied()
                                    .unwrap_or(crate::agents::AgentStatus::Idle)
                                    .into(),
                            })
                        } else {
                            None
                        };
                        Some(PaneInfo {
                            id: pid,
                            title: app.pane_label(pid),
                            cwd: pane.cwd.clone(),
                            is_ai,
                            agent,
                            rect: rect_of(pid)?,
                        })
                    })
                    .collect(),
                zoomed: s.zoom,
                active: i == app.active,
                focus: s.tree.pane_ids().contains(&s.tree.focus).then_some(s.tree.focus),
            }
        })
        .collect()
}

/// Queue a message to one client (non-blocking).
fn send_to(clients: &mut HashMap<usize, Client>, id: usize, msg: &ServerMsg) -> Result<()> {
    if let Some(c) = clients.get_mut(&id) {
        c.tx.try_send(msg.clone())?;
    }
    Ok(())
}

/// Writer loop for one client: drains its outgoing queue and writes frames to
/// the socket. A client that stops reading blocks here (isolated in this
/// thread) while the daemon loop keeps running and drops frames for it.
fn client_write_loop(mut stream: UnixStream, rx: mpsc::Receiver<ServerMsg>) {
    while let Ok(msg) = rx.recv() {
        if protocol::write_framed(&mut stream, &msg).is_err() {
            break;
        }
    }
}

/// Read loop for one client: decodes frames and forwards them to the daemon's
/// main loop (tagged with the client id). A closed socket yields a synthetic
/// `Detach` so the writer is dropped.
fn client_read_loop(mut stream: UnixStream, tx: mpsc::Sender<(usize, ClientMsg)>, id: usize) {
    let mut reader = protocol::FrameReader::default();
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
                    let Ok((msg, _)) =
                        bincode::serde::decode_from_slice::<ClientMsg, _>(&f, bincode::config::standard())
                    else {
                        return;
                    };
                    if tx.send((id, msg)).is_err() {
                        return;
                    }
                }
            }
        }
    }
    let _ = tx.send((id, ClientMsg::Detach));
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
    use crate::protocol::{WireKeyCode, WireKeyEvent, WireModifiers};
    use std::path::PathBuf;

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

    /// Read a frame (the daemon sends one every ~250ms even when idle).
    fn next_frame(stream: &mut UnixStream) -> protocol::FrameMsg {
        loop {
            match protocol::read_framed::<ServerMsg>(stream) {
                Ok(ServerMsg::Frame { frame }) => return frame,
                Ok(_) => {}
                Err(e) => {
                    let timed_out = e
                        .downcast_ref::<std::io::Error>()
                        .map(|e| matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
                        .unwrap_or(false);
                    if timed_out {
                        continue;
                    }
                    panic!("daemon connection failed: {e:#}");
                }
            }
        }
    }

    fn send_key(stream: &mut UnixStream, code: WireKeyCode) {
        protocol::write_framed(
            stream,
            &ClientMsg::Input { key: WireKeyEvent { code, modifiers: WireModifiers::default() } },
        )
        .unwrap();
    }

    /// Send a key with explicit modifiers (e.g. the Ctrl+B leader chord).
    fn send_key_mods(stream: &mut UnixStream, code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) {
        let key: WireKeyEvent = crossterm::event::KeyEvent::new(code, mods).into();
        protocol::write_framed(stream, &ClientMsg::Input { key }).unwrap();
    }

    /// The `kumo` binary this test's package builds: the unit-test harness runs
    /// from `target/debug/deps/kumo-<hash>`, the daemon executable sits one
    /// level up in `target/debug/kumo`.
    fn kumo_bin() -> PathBuf {
        let exe = std::env::current_exe().expect("current exe");
        exe.parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("kumo"))
            .expect("kumo binary next to the test harness")
    }

    /// Accumulate every dirty row seen so far into `rows` (row index -> text).
    /// Drains only the frames already buffered (the stream must have a read
    /// timeout set), so it returns promptly instead of blocking on the daemon's
    /// ~250ms idle frame stream.
    fn collect_rows(stream: &mut UnixStream, rows: &mut std::collections::HashMap<u16, String>) {
        loop {
            match protocol::read_framed::<ServerMsg>(stream) {
                Ok(ServerMsg::Frame { frame }) => {
                    for patch in &frame.rows_dirty {
                        let text: String = patch
                            .cells
                            .iter()
                            .filter(|c| c.cell_width != 0)
                            .map(|c| c.text.as_str())
                            .collect();
                        rows.insert(patch.row, text);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if e.downcast_ref::<std::io::Error>()
                        .map(|e| matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
                        .unwrap_or(false)
                    {
                        break;
                    }
                    break;
                }
            }
        }
    }

    /// Read frames until some rendered row contains `needle` (or the deadline).
    fn wait_for_text(stream: &mut UnixStream, needle: &str, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut rows = std::collections::HashMap::new();
        loop {
            collect_rows(stream, &mut rows);
            if rows.values().any(|r| r.contains(needle)) {
                return;
            }
            assert!(Instant::now() < deadline, "never saw {what} ({needle:?}) in frames");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn start_daemon(sock: &PathBuf) {
        let s = sock.clone();
        std::thread::spawn(move || {
            let _ = run_daemon_at(s, Launch::New(None));
        });
    }

    fn handshake(sock: &PathBuf) -> UnixStream {
        let mut stream = wait_for_socket(sock, Duration::from_secs(10));
        protocol::write_framed(
            &mut stream,
            &ClientMsg::Hello {
                protocol: protocol::PROTOCOL_VERSION,
                kind: ClientKind::Terminal,
                cols: 180,
                rows: 45,
            },
        )
        .unwrap();
        let msg: ServerMsg = protocol::read_framed(&mut stream).unwrap();
        assert!(matches!(msg, ServerMsg::Welcome { .. }), "expected Welcome, got {msg:?}");
        stream
    }

    /// Wait until the shell has painted real content into the pane area, so
    /// keys typed afterwards reach a ready shell.
    fn wait_for_prompt(stream: &mut UnixStream) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let f = next_frame(stream);
            let has_content = f
                .rows_dirty
                .iter()
                .flat_map(|p| p.cells.iter().enumerate())
                .any(|(col_in_row, c)| col_in_row >= 27 && !c.text.trim().is_empty());
            if has_content {
                return;
            }
            assert!(Instant::now() < deadline, "shell prompt never appeared");
        }
    }

    /// Read frames until the terminal cursor moves away from `start`.
    fn wait_cursor_move(stream: &mut UnixStream, start: (u16, u16), what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let f = next_frame(stream);
            if let Some((x, y)) = f.cursor {
                if (x, y) != start {
                    assert!((x, y).1 >= start.1, "cursor moved up unexpectedly");
                    return;
                }
            }
            assert!(Instant::now() < deadline, "{what}");
        }
    }

    #[test]
    fn agent_status_wire_mapping_is_lossless() {
        use crate::agents::AgentStatus as S;
        use protocol::AgentStatus as W;
        assert_eq!(W::from(S::Working), W::Working);
        assert_eq!(W::from(S::Blocked), W::Blocked);
        assert_eq!(W::from(S::Idle), W::Idle);
        assert_eq!(W::Working.label(), "working");
        assert_eq!(W::Blocked.label(), "blocked");
        assert_eq!(W::Idle.label(), "idle");
    }

    #[test]
    fn full_queue_marks_full_redraw_instead_of_dropping() {
        let (tx, rx) = mpsc::sync_channel::<ServerMsg>(1);
        let mut client = Client {
            tx,
            welcomed: true,
            kind: ClientKind::Terminal,
            needs_full: false,
            wants_snapshot: false,
            panes_subscribed: HashSet::new(),
            pane_needs_full: HashSet::new(),
            pending_snapshot: false,
        };
        // Fill the writer's channel so the next send hits Full — exactly what
        // happens while the writer thread is blocked on a client that stopped
        // reading (backgrounded tab, asleep machine).
        let filler = client.tx.clone();
        filler.try_send(ServerMsg::Detach).unwrap();
        assert!(
            matches!(client.send_msg(ServerMsg::Detach), SendOutcome::Lagging),
            "a lagging client must be reported as lagging, never dropped"
        );
        assert!(
            client.needs_full,
            "the lagging client must be flagged for a full-frame repaint"
        );

        // Once the receiver is gone (writer thread died = connection really
        // broken), the client is dropped.
        drop(rx);
        assert!(
            matches!(client.send_msg(ServerMsg::Detach), SendOutcome::Disconnected),
            "a disconnected client must be dropped"
        );
    }

    #[test]
    fn peer_owned_by_same_user_accepts_same_user_connection() {
        let dir = scratch("peercred");
        let sock = dir.join("peer.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let _stream = UnixStream::connect(&sock).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        assert!(
            peer_owned_by_same_user(&accepted),
            "a same-user connection must pass the owner check"
        );
        drop(accepted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end daemon behavior over the socket, in one sequential test so the
    /// real shells spawned never contend with each other:
    ///   attach -> echo -> detach (daemon lives) -> re-attach (session alive)
    ///   -> `exit\n` closes the last shell -> daemon auto-stops + cleans socket.
    #[test]
    fn daemon_serves_detach_reattach_and_auto_stops() {
        // Isolated config: `/bin/sh` pane shell (fast + deterministic, unlike
        // the user's interactive shell) and no update check. The env must be
        // mutated under the same lock config tests use, or they race.
        let cfg = scratch("daemon-cfg");
        std::fs::write(cfg.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let _lock = crate::config::TEST_ENV_LOCK.lock().unwrap();
        let prev_cfg = std::env::var("KUMO_CONFIG_DIR").ok();
        let prev_update = std::env::var("KUMO_NO_UPDATE").ok();
        std::env::set_var("KUMO_CONFIG_DIR", &cfg);
        std::env::set_var("KUMO_NO_UPDATE", "1");

        let dir = scratch("daemon-e2e");
        let sock = dir.join("kumo.sock");
        start_daemon(&sock);
        // Wait for the daemon to bind its socket: App::new (which resolves the
        // shell/config) has finished, so the env can be restored now.
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

        // Attach + echo: typing 'q' moves the shell cursor.
        let mut stream = handshake(&sock);
        wait_for_prompt(&mut stream);
        let before = next_frame(&mut stream);
        assert_eq!((before.cols, before.rows), (180, 45), "frame should render at the Hello size");
        let start_cursor = before.cursor.expect("cursor reported");
        send_key(&mut stream, WireKeyCode::Char('q'));
        wait_cursor_move(&mut stream, start_cursor, "typed 'q' never moved the cursor");

        // A resize on the wire must change the rendered frame size (the daemon
        // used to stay stuck at the initial 80x24).
        protocol::write_framed(&mut stream, &ClientMsg::Resize { cols: 120, rows: 30 }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let f = next_frame(&mut stream);
            if (f.cols, f.rows) == (120, 30) {
                break;
            }
            assert!(Instant::now() < deadline, "resize was never applied");
        }

        // Detach: the daemon (and the live session) keeps running.
        protocol::write_framed(&mut stream, &ClientMsg::Detach).unwrap();
        drop(stream);

        // Re-attach to the same live session and type again.
        let mut stream2 = handshake(&sock);
        wait_for_prompt(&mut stream2);
        let start2 = next_frame(&mut stream2).cursor.expect("cursor reported");
        send_key(&mut stream2, WireKeyCode::Char('w'));
        wait_cursor_move(&mut stream2, start2, "re-attached session did not accept input");

        // `kumo ls`: the daemon reports its sessions (frames interleave, so
        // skip them until the SessionList reply arrives).
        protocol::write_framed(&mut stream2, &ClientMsg::ListSessions).unwrap();
        let sessions = loop {
            match protocol::read_framed::<ServerMsg>(&mut stream2).unwrap() {
                ServerMsg::SessionList { sessions } => break sessions,
                _ => {}
            }
        };
        assert_eq!(sessions.len(), 1, "one session should be listed");
        assert_eq!(sessions[0].name, "session-1");
        assert!(sessions[0].active);
        assert!(!sessions[0].panes.is_empty(), "session should list its panes");
        let first = &sessions[0].panes[0];
        assert!(!first.title.is_empty(), "pane title must be reported");
        assert!(
            first.agent.is_none(),
            "a plain shell pane has no AI agent to report"
        );

        // `kumo reload`: the daemon re-reads its config and confirms live.
        protocol::write_framed(&mut stream2, &ClientMsg::ReloadConfig).unwrap();
        let notice = loop {
            match protocol::read_framed::<ServerMsg>(&mut stream2).unwrap() {
                ServerMsg::ConfigReloaded { notice } => break notice,
                _ => {}
            }
        };
        assert_eq!(notice, "config reloaded");

        // The echo checks left `qw` pending in the shell's line buffer; submit
        // it (a harmless "command not found") so the following `exit` runs.
        send_key(&mut stream2, WireKeyCode::Enter);
        std::thread::sleep(Duration::from_millis(100));

        // `exit\n` closes the only shell -> the daemon stops and cleans the socket.
        for code in "exit".chars().map(WireKeyCode::Char) {
            send_key(&mut stream2, code);
            std::thread::sleep(Duration::from_millis(30));
        }
        send_key(&mut stream2, WireKeyCode::Enter);
        let deadline = Instant::now() + Duration::from_secs(10);
        while sock.exists() {
            assert!(Instant::now() < deadline, "daemon socket not removed after last session closed");
            std::thread::sleep(Duration::from_millis(50));
        }

        // A freshly spawned daemon accepts `kumo new` over the wire: the
        // NewSession IPC message must create a second session and focus it.
        start_daemon(&sock);
        let _ = wait_for_socket(&sock, Duration::from_secs(10));
        let mut nstream = handshake(&sock);
        protocol::write_framed(
            &mut nstream,
            &ClientMsg::NewSession { workspace: Some(PathBuf::from("/tmp")) },
        )
        .unwrap();
        protocol::write_framed(&mut nstream, &ClientMsg::ListSessions).unwrap();
        let sessions = loop {
            match protocol::read_framed::<ServerMsg>(&mut nstream).unwrap() {
                ServerMsg::SessionList { sessions } => break sessions,
                _ => {}
            }
        };
        assert_eq!(sessions.len(), 2, "NewSession should create a second session");
        assert_eq!(sessions[1].name, "session-2");
        assert_eq!(sessions[1].workspace, PathBuf::from("/tmp"));
        assert!(sessions[1].active, "the new session should be focused");
        assert!(!sessions[0].active, "the original session should be unfocused");

        // `kumo kill` stops a daemon and cleans its socket.
        protocol::write_framed(&mut nstream, &ClientMsg::KillServer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while sock.exists() {
            assert!(Instant::now() < deadline, "daemon socket not removed after kumo kill");
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&cfg);
    }

    /// End-to-end `kumo server restart` against a real daemon subprocess: the
    /// resume must repaint *every* session's panes, not just the one active at
    /// the restart. Regression for the bug where the session that was inactive
    /// during the restart came back blank until a layout action (leader+o)
    /// forced a genuine PTY resize.
    #[test]
    fn daemon_restart_repaints_inactive_sessions() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use std::collections::HashMap;
        use std::process::Stdio;

        let cfg = scratch("restart-cfg");
        std::fs::write(cfg.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let rt = scratch("restart-rt");
        let sock = rt.join("kumo").join("kumo.sock");

        // A real daemon subprocess with an isolated runtime dir, so the in-place
        // restart exec rebinds the same socket / resume paths.
        let mut daemon = std::process::Command::new(kumo_bin())
            .arg("daemon")
            .env("KUMO_CONFIG_DIR", &cfg)
            .env("XDG_RUNTIME_DIR", &rt)
            .env("KUMO_NO_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon");

        // Attach, create a second session and give its shell a distinctive prompt.
        let mut stream = handshake(&sock);
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        wait_for_prompt(&mut stream);
        protocol::write_framed(&mut stream, &ClientMsg::NewSession { workspace: Some(cfg.clone()) }).unwrap();
        wait_for_prompt(&mut stream);
        for ch in "PS1='SESS2> '".chars() {
            send_key(&mut stream, WireKeyCode::Char(ch));
        }
        send_key(&mut stream, WireKeyCode::Enter);
        wait_for_text(&mut stream, "SESS2>", "the SESS2 prompt");

        // Switch back to session 1 so the restart snapshots session 2 as inactive.
        send_key_mods(&mut stream, KeyCode::Char('b'), KeyModifiers::CONTROL);
        send_key(&mut stream, WireKeyCode::Char('1'));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut rows = HashMap::new();
            collect_rows(&mut stream, &mut rows);
            let last = rows
                .iter()
                .filter(|(_, t)| !t.trim().is_empty())
                .max_by_key(|(&r, _)| r)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            if last.contains("session-1") {
                break;
            }
            assert!(Instant::now() < deadline, "session 1 never became active");
            std::thread::sleep(Duration::from_millis(20));
        }

        // Restart the daemon in place (exec + resume of the live PTYs).
        protocol::write_framed(&mut stream, &ClientMsg::Restart).unwrap();
        let _ = protocol::read_framed::<ServerMsg>(&mut stream);
        drop(stream);

        // The old daemon sleeps ~100ms before exec'ing; the new daemon (same
        // pid) rebinds the socket once its resume finishes. A reconnect in the
        // exec window is cut off, so just wait for the new daemon to settle.
        std::thread::sleep(Duration::from_millis(2000));

        // The new daemon re-binds the same socket; re-attach and re-handshake.
        let mut stream = handshake(&sock);
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        // The resumed active session (session 1) must repaint its prompt.
        wait_for_text(&mut stream, "session-1", "the resumed session-1 status bar");

        // Now the crux: switching to the session that was INACTIVE during the
        // restart must show its repainted prompt immediately, with no rotate.
        send_key_mods(&mut stream, KeyCode::Char('b'), KeyModifiers::CONTROL);
        send_key(&mut stream, WireKeyCode::Char('2'));
        wait_for_text(&mut stream, "SESS2>", "the repainted SESS2 prompt after restart");

        protocol::write_framed(&mut stream, &ClientMsg::KillServer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while sock.exists() {
            assert!(Instant::now() < deadline, "daemon socket not removed after kill");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = daemon.wait();

        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&rt);
    }

    /// The session-name popup must support word editing: `SUPER`/`CONTROL` +
    /// Backspace deletes the previous word, `SUPER`/`CONTROL` + Delete the next
    /// one. Regression for the popup-input-editing roadmap item (0.5.0).
    #[test]
    fn popup_word_editing() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let cfg = scratch("popup-cfg");
        std::fs::write(cfg.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let _lock = crate::config::TEST_ENV_LOCK.lock().unwrap();
        let prev_cfg = std::env::var("KUMO_CONFIG_DIR").ok();
        let prev_update = std::env::var("KUMO_NO_UPDATE").ok();
        std::env::set_var("KUMO_CONFIG_DIR", &cfg);
        std::env::set_var("KUMO_NO_UPDATE", "1");

        let rt = scratch("popup-rt");
        let sock = rt.join("kumo").join("kumo.sock");
        start_daemon(&sock);
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

        let mut stream = handshake(&sock);
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        wait_for_prompt(&mut stream);

        // Open the new-session popup (`leader+c`). It pre-fills the default
        // name, so clear it with a single word-delete first.
        send_key_mods(&mut stream, KeyCode::Char('b'), KeyModifiers::CONTROL);
        send_key(&mut stream, WireKeyCode::Char('c'));
        wait_for_text(&mut stream, "new session", "the new-session popup title");
        send_key_mods(&mut stream, KeyCode::Backspace, KeyModifiers::SUPER);

        // Type "hello world" then `cmd+backspace` -> the trailing word goes.
        for ch in "hello world".chars() {
            send_key(&mut stream, WireKeyCode::Char(ch));
        }
        send_key_mods(&mut stream, KeyCode::Backspace, KeyModifiers::SUPER);
        wait_for_text(&mut stream, "hello", "the popup after cmd+backspace");

        // `cmd+backspace` again -> "hello" goes too.
        send_key_mods(&mut stream, KeyCode::Backspace, KeyModifiers::SUPER);
        send_key_mods(&mut stream, KeyCode::Backspace, KeyModifiers::SUPER);

        // Type "one two", then `ctrl+delete` at the end does nothing; go back to
        // the word boundary and `ctrl+delete` removes the next word.
        for ch in "one two".chars() {
            send_key(&mut stream, WireKeyCode::Char(ch));
        }
        for _ in 0..4 {
            send_key(&mut stream, WireKeyCode::Left);
        }
        send_key_mods(&mut stream, KeyCode::Delete, KeyModifiers::CONTROL);
        wait_for_text(&mut stream, "one", "the popup after ctrl+delete");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut rows = std::collections::HashMap::new();
            collect_rows(&mut stream, &mut rows);
            if rows.values().any(|r| r.contains("one two")) {
                panic!("ctrl+delete should have removed the word after the cursor");
            }
            if Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        protocol::write_framed(&mut stream, &ClientMsg::KillServer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while sock.exists() {
            assert!(Instant::now() < deadline, "daemon socket not removed after kill");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&rt);
    }

    /// The v2 capability channels for non-terminal clients: a `Desktop` attach
    /// gets the composed frames, a snapshot push with per-pane info, works with
    /// `FocusSession`, and streams per-pane `PaneFrame`s.
    #[test]
    fn desktop_client_gets_snapshot_and_pane_frames() {
        let cfg = scratch("desktop-cfg");
        std::fs::write(cfg.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let _lock = crate::config::TEST_ENV_LOCK.lock().unwrap();
        let prev_cfg = std::env::var("KUMO_CONFIG_DIR").ok();
        let prev_update = std::env::var("KUMO_NO_UPDATE").ok();
        std::env::set_var("KUMO_CONFIG_DIR", &cfg);
        std::env::set_var("KUMO_NO_UPDATE", "1");

        let rt = scratch("desktop-rt");
        let sock = rt.join("kumo").join("kumo.sock");
        start_daemon(&sock);
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

        // A Desktop client handshakes with its kind and attaches.
        let mut stream = wait_for_socket(&sock, Duration::from_secs(10));
        stream.set_read_timeout(Some(Duration::from_millis(2000))).unwrap();
        protocol::write_framed(
            &mut stream,
            &ClientMsg::Hello { protocol: protocol::PROTOCOL_VERSION, kind: ClientKind::Desktop, cols: 120, rows: 36 },
        )
        .unwrap();
        let msg: ServerMsg = protocol::read_framed(&mut stream).unwrap();
        assert!(matches!(msg, ServerMsg::Welcome { .. }), "expected Welcome, got {msg:?}");

        // SubscribeSnapshot: the daemon answers immediately with the current
        // state (per-pane titles/cwd/agent info).
        protocol::write_framed(&mut stream, &ClientMsg::SubscribeSnapshot).unwrap();
        let sessions = loop {
            match protocol::read_framed::<ServerMsg>(&mut stream).unwrap() {
                ServerMsg::Snapshot { sessions } => break sessions,
                ServerMsg::Frame { .. } => continue,
                other => panic!("expected a Snapshot, got {other:?}"),
            }
        };
        assert_eq!(sessions.len(), 1, "one session should be reported");
        let first = &sessions[0];
        assert!(!first.panes.is_empty(), "session must report its panes");
        assert!(!first.panes[0].title.is_empty(), "pane title must be reported");
        assert!(!first.panes[0].cwd.as_os_str().is_empty(), "pane cwd must be reported");
        assert!(
            first.panes[0].rect.width > 0 && first.panes[0].rect.height > 0,
            "pane geometry must be reported"
        );
        assert_eq!(first.focus, Some(first.panes[0].id), "session must report its focused pane");
        let pid = first.panes[0].id;

        // FocusSession by name: the daemon must not crash and the session stays.
        protocol::write_framed(&mut stream, &ClientMsg::FocusSession { name: first.name.clone() }).unwrap();
        // A bogus name is a no-op, not an error.
        protocol::write_framed(&mut stream, &ClientMsg::FocusSession { name: "nope".into() }).unwrap();

        // FocusPane focuses a specific pane in the session (desktop click).
        protocol::write_framed(
            &mut stream,
            &ClientMsg::FocusPane { session: first.name.clone(), pane_id: pid },
        )
        .unwrap();
        // A bogus pane id is a no-op.
        protocol::write_framed(
            &mut stream,
            &ClientMsg::FocusPane { session: first.name.clone(), pane_id: 999_999 },
        )
        .unwrap();

        // SetSidebar closes the daemon's chrome so native clients get the full width.
        protocol::write_framed(&mut stream, &ClientMsg::SetSidebar { open: false }).unwrap();

        // SubscribePane: the daemon streams this pane's own grid (full first).
        protocol::write_framed(&mut stream, &ClientMsg::SubscribePane { pane_id: pid }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let pane_frame = loop {
            assert!(Instant::now() < deadline, "no PaneFrame for pane {pid}");
            match protocol::read_framed::<ServerMsg>(&mut stream).unwrap() {
                ServerMsg::PaneFrame { frame } if frame.pane_id == pid => break frame,
                ServerMsg::PaneFrame { frame } => panic!("unexpected PaneFrame for {}", frame.pane_id),
                ServerMsg::Frame { .. } | ServerMsg::Snapshot { .. } => continue,
                other => panic!("unexpected message while waiting for a PaneFrame: {other:?}"),
            }
        };
        assert!(pane_frame.full, "the first PaneFrame must be a full frame");
        assert!(pane_frame.rows > 0 && pane_frame.cols > 0, "pane frame must have a real size");
        assert_eq!(pane_frame.rows_dirty.len(), pane_frame.rows as usize, "a full frame includes every row");

        // Split the pane (leader+b then 'v'): the session now has two panes
        // with distinct geometry, and both pane streams flow.
        send_key_mods(&mut stream, crossterm::event::KeyCode::Char('b'), crossterm::event::KeyModifiers::CONTROL);
        send_key(&mut stream, WireKeyCode::Char('v'));
        let deadline = Instant::now() + Duration::from_secs(10);
        let (pid2, split_session) = loop {
            assert!(Instant::now() < deadline, "no two-pane snapshot after the split");
            match protocol::read_framed::<ServerMsg>(&mut stream).unwrap() {
                ServerMsg::Snapshot { sessions } => {
                    let active = sessions
                        .iter()
                        .find(|s| s.active)
                        .expect("an active session");
                    if active.panes.len() >= 2 {
                        break (active.panes[1].id, active.clone());
                    }
                }
                ServerMsg::Frame { .. } | ServerMsg::PaneFrame { .. } => continue,
                other => panic!("unexpected message while waiting for the split snapshot: {other:?}"),
            }
        };
        assert_ne!(pid, pid2, "the new pane must have its own id");
        protocol::write_framed(&mut stream, &ClientMsg::SubscribePane { pane_id: pid2 }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "no PaneFrame for the split pane {pid2}");
            match protocol::read_framed::<ServerMsg>(&mut stream).unwrap() {
                ServerMsg::PaneFrame { frame } if frame.pane_id == pid2 => break,
                // Other panes stay subscribed; ignore their frames.
                ServerMsg::PaneFrame { .. } | ServerMsg::Frame { .. } | ServerMsg::Snapshot { .. } => continue,
                other => panic!("unexpected message while waiting for the split PaneFrame: {other:?}"),
            }
        }
        // The two panes must report different geometry (side-by-side split).
        let (a, b) = (&split_session.panes[0], &split_session.panes[1]);
        assert_ne!(a.rect, b.rect, "split panes must have distinct geometry");

        // Unsubscribe and kill.
        protocol::write_framed(&mut stream, &ClientMsg::UnsubscribePane { pane_id: pid }).unwrap();
        protocol::write_framed(&mut stream, &ClientMsg::UnsubscribePane { pane_id: pid2 }).unwrap();
        protocol::write_framed(&mut stream, &ClientMsg::KillServer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while sock.exists() {
            assert!(Instant::now() < deadline, "daemon socket not removed after kill");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&rt);
    }
}
