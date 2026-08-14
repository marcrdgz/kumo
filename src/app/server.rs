//! Headless kumo daemon: owns the `App` (PTYs + terminal emulators + the whole
//! UI), renders it into a `TestBackend`, and streams the resulting frames to
//! attached terminal clients over the unix socket. Detach only closes the
//! client connection; the daemon keeps running until the last session closes.

use std::collections::HashMap;
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
use crate::protocol::{self, ClientMsg, ServerMsg};

impl From<crate::agents::AgentStatus> for protocol::AgentStatus {
    fn from(status: crate::agents::AgentStatus) -> Self {
        match status {
            crate::agents::AgentStatus::Working => protocol::AgentStatus::Working,
            crate::agents::AgentStatus::Blocked => protocol::AgentStatus::Blocked,
            crate::agents::AgentStatus::Idle => protocol::AgentStatus::Idle,
        }
    }
}

/// One connected terminal client. The read half lives in a per-client reader
/// thread; outgoing messages go through a per-client writer thread with a
/// bounded queue, so a slow (or unread) client never blocks the daemon loop —
/// frames are dropped for lagging clients instead of stalling everyone.
struct Client {
    tx: mpsc::SyncSender<ServerMsg>,
    welcomed: bool,
    /// True until this client has received one full frame (its first attach);
    /// it needs a full repaint even if the daemon's own frame is a diff.
    needs_full: bool,
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
            clients.insert(id, Client { tx: writer_tx, welcomed: false, needs_full: true });
            render_dirty = true;
        }

        // Input from clients.
        while let Ok((id, msg)) = input_rx.try_recv() {
            match msg {
                ClientMsg::Hello { protocol, cols, rows } => {
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
                    let sessions = app
                        .sessions
                        .iter()
                        .enumerate()
                        .map(|(i, s)| protocol::SessionInfo {
                            name: s.name.clone(),
                            workspace: s.workspace.clone(),
                            panes: s.tree.pane_count(),
                            zoomed: s.zoom,
                            active: i == app.active,
                            agents: app
                                .sessions[i]
                                .tree
                                .pane_ids()
                                .into_iter()
                                .filter(|pid| {
                                    app.panes.get(pid).map(|p| p.is_ai_cli()).unwrap_or(false)
                                })
                                .map(|pid| protocol::AgentInfo {
                                    name: app.agent_label(pid),
                                    status: app
                                        .agent_status_cache
                                        .get(&pid)
                                        .copied()
                                        .unwrap_or(crate::agents::AgentStatus::Idle)
                                        .into(),
                                })
                                .collect(),
                        })
                        .collect();
                    let _ = send_to(&mut clients, id, &ServerMsg::SessionList { sessions });
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
            for (id, client) in clients.iter_mut() {
                if !client.welcomed {
                    continue;
                }
                let send_full = client.needs_full || area_changed;
                let msg = if send_full {
                    client.needs_full = false;
                    full_msg.get_or_insert_with(|| {
                        ServerMsg::Frame { frame: protocol::FrameMsg::full_frame(&new_buf, cursor) }
                    })
                } else {
                    let Some(last) = &last_buffer else { continue };
                    diff_msg.get_or_insert_with(|| {
                        ServerMsg::Frame { frame: protocol::FrameMsg::diff_frame(&new_buf, last, cursor) }
                    })
                };
                if client.tx.try_send(msg.clone()).is_err() {
                    dead.push(*id);
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
            match protocol::read_framed::<ServerMsg>(stream).unwrap() {
                ServerMsg::Frame { frame } => return frame,
                _ => {}
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
            &ClientMsg::Hello { protocol: protocol::PROTOCOL_VERSION, cols: 180, rows: 45 },
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
        assert!(sessions[0].panes >= 1);
        assert!(
            sessions[0].agents.is_empty(),
            "a plain shell session has no AI agents to report"
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
}
