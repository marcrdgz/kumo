//! Headless kumo daemon: the single source of truth.
//!
//! Owns the `App` (PTYs, ghostty emulators, the semantic layout tree, agent
//! metadata) and serves every client over the unix socket. It never renders
//! chrome: clients receive the semantic [`Layout`] (splits in ratios) and
//! per-pane [`PaneFrame`]s, and drive everything else through [`Command`]s —
//! sessions, panes, agents, input, resizes, and chrome actions (rename,
//! worktrees, theme). Every client draws its own chrome.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::buffer::Buffer;

use super::{App, Launch};
use crate::daemon::frames;
use kumo_core::protocol::{ClientKind, Command, DaemonEvent, Layout, PROTOCOL_VERSION};

/// One connected client. Reads happen on a per-client reader thread; outgoing
/// messages go through a per-client writer thread with a bounded queue, so a
/// slow (or unread) client never blocks the daemon loop. A lagging client is
/// flagged (layout / pane-frame retry) instead of being dropped; a genuinely
/// dead connection is cleaned up by its reader thread (EOF -> `Detach`).
struct Client {
    tx: mpsc::SyncSender<DaemonEvent>,
    welcomed: bool,
    kind: ClientKind,
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
            wants_layout: false,
            pending_layout: false,
            panes_subscribed: HashSet::new(),
            pane_needs_full: HashSet::new(),
        }
    }

    fn send_msg(&mut self, msg: DaemonEvent) -> SendOutcome {
        match self.tx.try_send(msg) {
            Ok(()) => SendOutcome::Ok,
            Err(mpsc::TrySendError::Full(_)) => SendOutcome::Lagging,
            Err(mpsc::TrySendError::Disconnected(_)) => SendOutcome::Disconnected,
        }
    }
}

pub fn run_daemon(launch: Launch) -> Result<()> {
    run_daemon_at(kumo_core::config::ipc_socket_path(), launch)
}

/// Run the daemon serving `path` (the socket). Split out so tests can drive a
/// daemon on a scratch socket without spawning a subprocess.
fn run_daemon_at(path: std::path::PathBuf, launch: Launch) -> Result<()> {
    // Create the app first: it resolves the shell/config and spawns panes. The
    // socket appears only once that's done, so callers (and tests) know the
    // daemon is fully up when they can connect.
    let mut app = App::new(launch)?;

    prepare_socket(&path)?;
    let listener = UnixListener::bind(&path)?;
    set_socket_perms(&path)?;
    listener.set_nonblocking(true)?;

    let (input_tx, input_rx) = mpsc::channel::<(usize, Command)>();
    let mut clients: HashMap<usize, Client> = HashMap::new();
    let mut next_id = 0usize;
    let mut last_layout: Option<Layout> = None;
    // Previous frame buffer per pane, used for diffing. Updated in-place each
    // tick to avoid cloning. When a pane is not dirty, this stays unchanged.
    let mut pane_bufs: HashMap<u64, Buffer> = HashMap::new();
    let mut pane_cursors: HashMap<u64, Option<(u16, u16)>> = HashMap::new();
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
                Command::Attach { protocol, kind, .. } => {
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
                    // Push the active theme so the client can draw its chrome
                    // before the first layout arrives.
                    let _ = send_to(&mut clients, id, &DaemonEvent::Theme { idx: app.theme_idx });
                    let notice = app.update_status();
                    let _ = send_to(&mut clients, id, &DaemonEvent::UpdateNotice { notice });
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
                            let resume = kumo_core::config::resume_file();
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
                    // Passthrough for every client: the client owns the keymap
                    // and sends leader actions as explicit commands.
                    app.write_key(key.to_crossterm());
                }
                Command::Paste { text } => {
                    app.paste(&text);
                }
                Command::Mouse { event } => {
                    app.on_pane_mouse(event.to_crossterm());
                }
                Command::PaneResize { pane_id, cols, rows } => {
                    app.resize_pane(pane_id, cols, rows);
                }
                Command::PaneRename { session, pane_id, name } => {
                    let reply = app.rename_pane_in_session(&session, pane_id, &name).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::SessionRename { session, new_name } => {
                    let reply = app.rename_session(&session, &new_name).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::WorktreeList { session } => {
                    match app.worktree_list(&session) {
                        Ok(items) => {
                            let _ = send_to(&mut clients, id, &DaemonEvent::Worktrees { items });
                        }
                        Err(e) => {
                            let _ = send_to(&mut clients, id, &DaemonEvent::Worktrees { items: Vec::new() });
                            let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: format!("error: {e:#}") });
                        }
                    }
                }
                Command::WorktreeCreate { session, branch } => {
                    let reply = app.worktree_create(&session, &branch).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::WorktreeOpen { session, path } => {
                    let reply = app.worktree_open(&session, &path).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::SetTheme { idx } => {
                    let reply = app.set_theme(idx).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                    // Broadcast the new chrome colors so every client re-colors.
                    for client in clients.values_mut() {
                        let _ = client.tx.try_send(DaemonEvent::Theme { idx: app.theme_idx });
                    }
                }
                Command::OpenConfig { session } => {
                    let reply = app.open_config_in_session(&session).unwrap_or_else(|e| format!("error: {e:#}"));
                    let _ = send_to(&mut clients, id, &DaemonEvent::Reply { message: reply });
                }
                Command::PaneWrite { pane_id, bytes } => {
                    app.pane_write(pane_id, &bytes);
                }
                Command::PaneScroll { pane_id, up } => {
                    app.scroll_pane(pane_id, up);
                }
                Command::UpdateStatus => {
                    let notice = app.update_status();
                    let _ = send_to(&mut clients, id, &DaemonEvent::UpdateNotice { notice });
                }
                Command::UpdateDismiss { key } => {
                    app.dismiss_update(&key);
                    // Tell every client the banner is gone.
                    for client in clients.values_mut() {
                        let _ = client.tx.try_send(DaemonEvent::UpdateNotice { notice: None });
                    }
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
                Command::PaneResizeTo { session, split_id, ratio } => {
                    // Fire-and-forget: drags stream these and the Reply would
                    // otherwise flash the client's status bar on every move.
                    let _ = app.set_split_ratio_in_session(&session, split_id, ratio);
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
        }

        // PTY output and background results.
        while let Ok(ev) = app.events_rx.try_recv() {
            app.on_pty_event(ev);
        }
        while let Ok(notice) = app.update_rx.try_recv() {
            app.update_notice = notice;
            let notice = app.update_status();
            for client in clients.values_mut() {
                let _ = client.tx.try_send(DaemonEvent::UpdateNotice { notice: notice.clone() });
            }
        }

        // Render dirty pane content into the caches, then stream what changed.
        let changed: HashSet<u64> = app.tick().into_iter().collect();

        // Prune per-pane server caches for panes that no longer exist (closed by
        // user command or process exit). Without this `pane_bufs`/`pane_cursors`
        // would grow monotonically for the lifetime of the daemon.
        pane_bufs.retain(|id, _| app.panes.contains_key(id));
        pane_cursors.retain(|id, _| app.panes.contains_key(id));
        for client in clients.values_mut() {
            client.panes_subscribed.retain(|id| app.panes.contains_key(id));
            client.pane_needs_full.retain(|id| app.panes.contains_key(id));
        }

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
            let pane_ids: Vec<u64> = client.panes_subscribed.iter().copied().collect();
            for pid in pane_ids {
                let was_pending = client.pane_needs_full.remove(&pid);
                let Some(cached) = app.pane_cache.get(&pid) else { continue };
                let resized = pane_bufs
                    .get(&pid)
                    .map(|l| l.area != cached.area)
                    .unwrap_or(true);
                if !was_pending && !resized && !changed.contains(&pid) {
                    continue;
                }
                let pane = app.panes.get(&pid);
                let pane_cursor = pane.and_then(|p| {
                    if p.vt.cursor_visible() {
                        p.vt.cursor_pos()
                    } else {
                        None
                    }
                });
                let scroll = pane.map(|p| super::ui::scroll_state(p.scrollbar_data()));
                let palette = &app.theme.palette;
                let frame = if was_pending || resized {
                    // Full frame: no previous buffer to diff against
                    let buf = frames::detach_buffer(cached);
                    let frame = frames::pane_frame(pid, &buf, None, pane_cursor, palette, pane, scroll);
                    pane_bufs.insert(pid, buf);
                    frame
                } else {
                    // Partial frame: diff against the previous buffer
                    let prev = pane_bufs.get(&pid);
                    let buf = frames::detach_buffer(cached);
                    let frame = frames::pane_frame(pid, &buf, prev, pane_cursor, palette, pane, scroll);
                    pane_bufs.insert(pid, buf);
                    frame
                };
                let cursor_changed = pane_cursors.get(&pid) != Some(&pane_cursor);
                if !frame.full && frame.rows_dirty.is_empty() && !cursor_changed {
                    continue;
                }
                pane_cursors.insert(pid, pane_cursor);
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

        // Every session closed (explicit `kill`, or auto-stop when the last
        // session closes): stop the daemon.
        if app.quit || kill {
            for client in clients.values_mut() {
                let _ = client.tx.try_send(DaemonEvent::Shutdown);
            }
            break;
        }

        std::thread::sleep(Duration::from_millis(4));
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
    let path = kumo_core::config::resume_file();
    crate::daemon::state::save(&path, &state)?;
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
fn client_write_loop(stream: UnixStream, rx: mpsc::Receiver<DaemonEvent>) {
    let mut writer = std::io::BufWriter::new(stream);
    while let Ok(msg) = rx.recv() {
        if kumo_core::protocol::write_framed(&mut writer, &msg).is_err() {
            break;
        }
        // Flush after each message to ensure timely delivery. The BufWriter
        // reduces syscalls by buffering small writes, but we still want each
        // logical frame to reach the client promptly.
        let _ = writer.flush();
    }
}

/// Read loop for one client: decodes frames and forwards them to the daemon's
/// main loop (tagged with the client id). A closed socket yields a synthetic
/// `Detach` so the writer is dropped.
fn client_read_loop(mut stream: UnixStream, tx: mpsc::Sender<(usize, Command)>, id: usize) {
    let mut reader = kumo_core::protocol::FrameReader::default();
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
        // No peer-credential API available: fail closed rather than trusting
        // socket perms alone (runtime dir may be world-visible like /tmp).
        let _ = fd;
        let _ = our_uid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_core::protocol::{self, LayoutNode, SplitDir};
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
            match kumo_core::protocol::read_framed::<DaemonEvent>(stream) {
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

    /// Whether any pane in the tree carries `needle` in its title.
    fn pane_title_contains(node: &LayoutNode, needle: &str) -> bool {
        match node {
            LayoutNode::Pane(p) => p.title.contains(needle),
            LayoutNode::Split { a, b, .. } => {
                pane_title_contains(a, needle) || pane_title_contains(b, needle)
            }
        }
    }

    /// End-to-end: the daemon is driven purely by commands over the socket and
    /// streams the semantic layout + per-pane content (never a composed grid).
    #[test]
    fn daemon_is_command_driven() {
        let cfg = scratch("cmd-cfg");
        std::fs::write(cfg.join("config"), "shell = /bin/sh\nupdate-check = false\nnew-cwd = current\n").unwrap();
        let _lock = kumo_core::config::TEST_ENV_LOCK.lock().unwrap();
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

        // Attach as a viewer: Welcome + the initial Theme, then the layout.
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
        let theme = loop {
            match next_event(&mut stream, Duration::from_secs(10), "Theme") {
                DaemonEvent::Theme { idx } => break idx,
                _ => continue,
            }
        };
        assert_eq!(theme, kumo_core::theme::DEFAULT_THEME_IDX, "attach pushes the active theme");

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
            LayoutNode::Split { dir, ratio, a, b, .. } => {
                assert_eq!(*dir, SplitDir::Vertical);
                assert!((0.0..=1.0).contains(ratio));
                assert!(matches!(a.as_ref(), LayoutNode::Pane(_)));
                assert!(matches!(b.as_ref(), LayoutNode::Pane(_)));
            }
            _ => panic!("expected a vertical split, got {root:?}"),
        }

        // Subscribe a pane and resize it; the PaneFrame streams at that size.
        protocol::write_framed(&mut stream, &Command::SubscribePane { pane_id: two_pane }).unwrap();
        protocol::write_framed(&mut stream, &Command::PaneResize { pane_id: two_pane, cols: 60, rows: 20 }).unwrap();
        let frame = loop {
            match next_event(&mut stream, Duration::from_secs(10), "PaneFrame") {
                DaemonEvent::PaneFrame { frame } if frame.pane_id == two_pane => break frame,
                DaemonEvent::Layout { .. } => continue,
                other => panic!("unexpected event while waiting for a PaneFrame: {other:?}"),
            }
        };
        assert_eq!((frame.cols, frame.rows), (60, 20), "the pane streams at its requested size");

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

        // Chrome actions over the wire: rename the pane, then confirm the
        // Layout title changed.
        protocol::write_framed(&mut stream, &Command::PaneRename {
            session: "two".into(),
            pane_id: two_pane,
            name: "editor".into(),
        })
        .unwrap();
        let layout = loop {
            match next_event(&mut stream, Duration::from_secs(10), "renamed Layout") {
                DaemonEvent::Layout { layout } => {
                    let two = layout.sessions.iter().find(|s| s.name == "two").unwrap();
                    if let Some(root) = &two.root {
                        if pane_title_contains(root, "editor") {
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
        assert!(
            pane_title_contains(root, "editor"),
            "the pane title must carry the custom name"
        );

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
