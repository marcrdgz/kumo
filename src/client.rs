//! Thin terminal client: connects to the daemon socket, renders the frames the
//! daemon streams, and forwards input. It has no terminal emulator — the daemon
//! renders the whole UI (panes, chrome, sidebar) and ships rendered cells.

use std::io::{self, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};

use crate::app::Launch;
use crate::protocol::{self, ClientKind, ClientMsg, FrameMsg, ServerMsg};

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
    // instead of silently attaching to the existing one. The daemon owns the
    // sessions, so the client resolves the workspace (its own cwd when no
    // explicit dir was given) and ships it over the wire.
    let pre: Vec<ClientMsg> = if !spawned && matches!(launch, Launch::New(_)) {
        let workspace = workspace_for(&launch).or_else(|| std::env::current_dir().ok());
        vec![ClientMsg::NewSession { workspace }]
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

/// Connect to a running daemon, or fail with a friendly error.
fn connect_daemon() -> Result<UnixStream> {
    let path = crate::config::ipc_socket_path();
    UnixStream::connect(&path).map_err(|_| {
        anyhow::anyhow!("no kumo daemon is running (start with `kumo` or `kumo new`)")
    })
}

/// `kumo ls`: list the daemon's sessions and exit.
pub fn list_sessions() -> Result<()> {
    let mut stream = connect_daemon()?;
    protocol::write_framed(&mut stream, &ClientMsg::ListSessions)?;
    match read_server(&mut stream)? {
        ServerMsg::SessionList { sessions } => {
            for s in &sessions {
                let mark = if s.active { "* " } else { "  " };
                let pane_word = if s.panes.len() == 1 { "pane" } else { "panes" };
                println!(
                    "{mark}{}: {} {} · {}{}",
                    s.name,
                    s.panes.len(),
                    pane_word,
                    s.workspace.display(),
                    if s.zoomed { " (zoomed)" } else { "" }
                );
                // One indented line per pane (title + cwd), with the AI CLI's
                // status highlighted, so a blocked agent is noticeable from
                // outside the TUI.
                let color = io::stdout().is_terminal();
                for pane in &s.panes {
                    let mut line = format!("    {} ({})", pane.title.trim(), pane.cwd.display());
                    if let Some(agent) = &pane.agent {
                        line.push_str(&format!(" — {}", agent_line(&agent.name, agent.status, color)));
                    }
                    println!("{line}");
                }
            }
            if sessions.is_empty() {
                println!("(no sessions)");
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!("unexpected daemon reply: {other:?}")),
    }
}

/// ANSI truecolor for the agent status word in `kumo ls` output, matching the
/// sidebar's palette. `None` = leave the terminal's default color (idle).
fn agent_status_color(status: protocol::AgentStatus) -> Option<(u8, u8, u8)> {
    match status {
        protocol::AgentStatus::Blocked => Some((0xff, 0xb8, 0x4d)), // amber
        protocol::AgentStatus::Working => Some((0x2e, 0xe0, 0x6b)), // green
        protocol::AgentStatus::Idle => None,
    }
}

/// One `kumo ls` agent line, e.g. `    opencode · blocked`. Colored (amber
/// blocked, green working) only when `color` is set, so piped output stays plain.
fn agent_line(name: &str, status: protocol::AgentStatus, color: bool) -> String {
    let label = status.label();
    match (color, agent_status_color(status)) {
        (true, Some((r, g, b))) => format!("    {name} · \x1b[38;2;{r};{g};{b}m{label}\x1b[0m"),
        _ => format!("    {name} · {label}"),
    }
}

/// Read one `ServerMsg`, mapping a decode failure (a reply from an older,
/// incompatible daemon) to a friendly restart hint instead of a raw bincode
/// error. `kumo ls` never handshakes, so it cannot rely on the `Hello` version
/// check the attach path uses. A pre-restart daemon's `SessionList` (variant
/// index 4, five-field `SessionInfo`) happens to decode as `Restarting` in the
/// current protocol, so that reply is treated as the same stale-daemon case.
fn read_server(stream: &mut UnixStream) -> Result<ServerMsg> {
    let stale = |e: anyhow::Error| {
        anyhow::anyhow!(
            "the running kumo daemon is from an older, incompatible kumo.\n\
             Restart it with:  pkill -f 'kumo daemon'\n\
             ({e})"
        )
    };
    match protocol::read_framed::<ServerMsg>(stream) {
        Ok(ServerMsg::Restarting) => Err(stale(anyhow::anyhow!("stale session list reply"))),
        Ok(msg) => Ok(msg),
        Err(e) => Err(stale(e)),
    }
}

/// `kumo kill`: stop the daemon (and the processes in its panes).
pub fn kill_server() -> Result<()> {
    let mut stream = connect_daemon()?;
    protocol::write_framed(&mut stream, &ClientMsg::KillServer)?;
    Ok(())
}

/// `kumo reload`: re-read the config on the daemon and apply it live.
pub fn reload() -> Result<()> {
    let mut stream = connect_daemon()?;
    protocol::write_framed(&mut stream, &ClientMsg::ReloadConfig)?;
    match read_server(&mut stream)? {
        ServerMsg::ConfigReloaded { notice } => {
            println!("{notice}");
            Ok(())
        }
        other => Err(anyhow::anyhow!("unexpected daemon reply: {other:?}")),
    }
}

/// `kumo server restart`: ask the daemon to restart itself in place — exec the
/// current binary (which `cargo build` / `kumo update` may have just replaced)
/// with `--resume`, inheriting the live PTY masters so panes and agents
/// survive. Attached terminals reconnect automatically.
pub fn server_restart() -> Result<()> {
    let mut stream = connect_daemon()?;
    protocol::write_framed(&mut stream, &ClientMsg::Restart)?;
    Ok(())
}

fn client_loop(mut stream: UnixStream, pre: &[ClientMsg]) -> Result<()> {
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
fn client_once(stream: &mut UnixStream, pre: &[ClientMsg]) -> Result<Exit> {
    let (cols, rows) = crossterm::terminal::size()?;
    protocol::write_framed(
        stream,
        &ClientMsg::Hello {
            protocol: protocol::PROTOCOL_VERSION,
            kind: ClientKind::Terminal,
            cols,
            rows,
        },
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
        // CSI-u (kitty) keyboard protocol: without it the host terminal reports
        // `cmd`/`ctrl`+backspace as a bare DEL (0x7f), indistinguishable from a
        // plain backspace. Disambiguated escape codes let the daemon tell them
        // apart (e.g. word-delete in the popups) and keep the leader chord.
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )?;

    // Input thread: reads crossterm events and writes them straight to the
    // daemon over its own socket clone. Stopped via the flag so a restart can
    // hand the event reader back to the next connection.
    let write_half = stream.try_clone()?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let input = std::thread::spawn(move || input_loop(write_half, stop2));

    // The daemon pushes a frame every ~250ms even when idle, so a read timeout
    // (2s of silence) always means the connection is stuck: the daemon flagged
    // this client as lagging (tab backgrounded, machine asleep) and stopped
    // serving it. Reconnecting instead of blocking on the last frame forever
    // (which would leave the UI frozen with no way to detach).
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;

    let result: Result<Exit> = (|| {
        let mut saw_frame = false;
        loop {
            match protocol::read_framed::<ServerMsg>(stream) {
                Ok(ServerMsg::Welcome { .. }) => {}
                Ok(ServerMsg::Frame { frame }) => {
                    saw_frame = true;
                    blit(&mut stdout, &frame)?;
                }
                Ok(ServerMsg::SessionList { .. }) => {}
                Ok(ServerMsg::ConfigReloaded { .. }) => {}
                Ok(ServerMsg::Snapshot { .. }) => {}
                Ok(ServerMsg::PaneFrame { .. }) => {}
                Ok(ServerMsg::Detach) => return Ok(Exit::Clean),
                Ok(ServerMsg::Restarting) => return Ok(Exit::Restarting),
                Ok(ServerMsg::Shutdown) => {
                    // A shutdown before any frame means the daemon rejected the
                    // handshake (protocol mismatch with an old lingering daemon)
                    // rather than a clean auto-stop.
                    if !saw_frame {
                        anyhow::bail!(
                            "the running kumo daemon is from an older, incompatible kumo.\n\
                             Restart it with:  pkill -f 'kumo daemon'\nthen run `kumo` again"
                        );
                    }
                    return Ok(Exit::Clean);
                }
                Err(e) if is_read_timeout(&e) => return Ok(Exit::Restarting),
                Err(e) => return Err(e),
            }
        }
    })();

    // Release the crossterm event reader before a reconnect spawns a new one.
    stop.store(true, Ordering::Relaxed);
    let _ = input.join();

    match result {
        Ok(Exit::Restarting) => {
            // Leave the terminal in raw mode so the reconnect is seamless, but
            // pop the keyboard enhancement so the next `client_once` push is
            // balanced (kitty's protocol stack would otherwise grow unbounded).
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
            let _ = stdout.flush();
            Ok(Exit::Restarting)
        }
        other => {
            let _ = execute!(stdout, Show, crossterm::event::DisableMouseCapture, DisableBracketedPaste, PopKeyboardEnhancementFlags, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            let _ = stdout.flush();
            other
        }
    }
}

/// Whether an error is a socket read timeout: no data arrived within the
/// client's read-timeout window. The daemon sends a frame every ~250ms even
/// when idle, so prolonged silence always means a lost/stalled connection.
fn is_read_timeout(e: &anyhow::Error) -> bool {
    e.downcast_ref::<io::Error>()
        .map(|ioe| matches!(ioe.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
        .unwrap_or(false)
}

/// Read crossterm events and forward them to the daemon. Exits when `stop` is
/// set (restart/reconnect) or once the socket becomes unwritable (daemon gone).
fn input_loop(mut stream: UnixStream, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        if !crossterm::event::poll(Duration::from_millis(50)).unwrap_or(false) {
            continue;
        }
        let ok = match crossterm::event::read() {
            Ok(crossterm::event::Event::Key(k)) => {
                protocol::write_framed(&mut stream, &ClientMsg::Input { key: k.into() })
            }
            Ok(crossterm::event::Event::Paste(text)) => {
                protocol::write_framed(&mut stream, &ClientMsg::Paste { text })
            }
            Ok(crossterm::event::Event::Mouse(m)) => {
                protocol::write_framed(&mut stream, &ClientMsg::Mouse { event: m.into() })
            }
            Ok(crossterm::event::Event::Resize(w, h)) => {
                protocol::write_framed(&mut stream, &ClientMsg::Resize { cols: w, rows: h })
            }
            _ => continue,
        };
        if ok.is_err() {
            return;
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

/// The subset of a cell's styling that decides whether to re-emit SGR codes.
type StyleKey = (Option<u32>, Option<u32>, bool, bool, bool, bool, bool);

/// Blit a frame: repaint only the dirty rows (`full` frames clear first), so
/// unchanged parts of the terminal never flicker.
fn blit(out: &mut io::Stdout, f: &FrameMsg) -> io::Result<()> {
    let mut buf = String::with_capacity(4096);
    if f.full {
        buf.push_str("\x1b[2J\x1b[H");
    }
    for patch in &f.rows_dirty {
        write_row(&mut buf, patch.row, &patch.cells);
    }
    match f.cursor {
        Some((x, y)) => {
            buf.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
            buf.push_str("\x1b[?25h");
        }
        None => buf.push_str("\x1b[?25l"),
    }
    out.write_all(buf.as_bytes())?;
    out.flush()
}

/// Rewrite one row (column-major cells), then erase the tail to the end of the
/// line so stale cells beyond the row's content are cleared. Continuation cells
/// after a wide grapheme (`cell_width == 0`) are skipped so wide characters
/// are not overwritten.
fn write_row(buf: &mut String, row: u16, cells: &[protocol::WireCell]) {
    let last = cells
        .iter()
        .rposition(|c| !c.text.trim().is_empty())
        .unwrap_or(usize::MAX);
    let mut prev_style: Option<StyleKey> = None;
    // Physical (0-indexed) column the terminal cursor lands on after the last
    // written cell; used to erase only the stale tail beyond the content.
    let mut cursor_col: u16 = 0;
    let mut wrote_any = false;
    for (col, cell) in cells.iter().enumerate() {
        if col > last {
            break;
        }
        if cell.cell_width == 0 {
            continue;
        }
        let style = (
            cell.fg,
            cell.bg,
            cell.bold,
            cell.italic,
            cell.underline,
            cell.inverse,
            cell.faint,
        );
        if prev_style != Some(style) {
            buf.push_str("\x1b[0m");
            push_sgr(buf, &style);
            prev_style = Some(style);
        }
        buf.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
        // Blank cells still reset to a space so previously drawn content in the
        // row is cleared.
        buf.push_str(if cell.text.trim().is_empty() { " " } else { &cell.text });
        cursor_col = col as u16 + cell.cell_width;
        wrote_any = true;
    }
    // Erase the stale tail only when the content does not already reach the
    // row's end: `\x1b[K` clears from the cursor to the end of the line
    // *inclusive*, so emitting it while the cursor is on the last column (the
    // pane's right border) would erase that border cell.
    if wrote_any && cursor_col < cells.len() as u16 {
        buf.push_str(&format!("\x1b[{};{}H", row + 1, cursor_col + 1));
        buf.push_str("\x1b[K");
    }
}

fn push_sgr(
    buf: &mut String,
    &(fg, bg, bold, italic, underline, inverse, faint): &(
        Option<u32>,
        Option<u32>,
        bool,
        bool,
        bool,
        bool,
        bool,
    ),
) {
    if bold {
        buf.push_str("\x1b[1m");
    }
    if faint {
        buf.push_str("\x1b[2m");
    }
    if italic {
        buf.push_str("\x1b[3m");
    }
    if underline {
        buf.push_str("\x1b[4m");
    }
    if inverse {
        buf.push_str("\x1b[7m");
    }
    if let Some(c) = fg {
        buf.push_str(&format!("\x1b[38;2;{};{};{}m", (c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff));
    }
    if let Some(c) = bg {
        buf.push_str(&format!("\x1b[48;2;{};{};{}m", (c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, width: u16) -> protocol::WireCell {
        protocol::WireCell {
            text: text.to_string(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
            faint: false,
            cell_width: width,
        }
    }

    #[test]
    fn read_timeout_signals_a_stalled_connection() {
        use std::os::unix::net::UnixStream;
        let (mut a, _b) = UnixStream::pair().unwrap();
        a.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        // Nothing is ever written: the read must time out, and the client must
        // treat that as a lost connection (reconnect), not as a hard error.
        let err = protocol::read_framed::<ServerMsg>(&mut a).unwrap_err();
        assert!(
            is_read_timeout(&err),
            "a silent socket must be a stalled connection, got: {err:#}"
        );
    }

    #[test]
    fn non_timeout_errors_are_not_reconnects() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(!is_read_timeout(&err), "a plain error must not trigger a reconnect");
    }

    #[test]
    fn read_server_maps_old_daemon_reply_to_friendly_error() {
        // An old daemon's SessionList (5-field SessionInfo) is undecodable; the
        // user must see the restart hint, not a raw bincode error.
        use serde::Serialize;
        use std::os::unix::net::UnixStream;

        #[derive(Serialize)]
        struct OldSessionInfo {
            name: String,
            workspace: PathBuf,
            panes: usize,
            zoomed: bool,
            active: bool,
        }
        // Mirrors the OLD `ServerMsg`: `SessionList` is the 5th variant (index
        // 4) — exactly the bytes a pre-`Restarting` daemon sends — so the
        // current client must treat that decode as a stale, incompatible
        // daemon. Only `SessionList` is ever constructed here; the leading
        // variants exist solely to match its variant index.
        #[allow(dead_code)]
        #[derive(Serialize)]
        enum OldServerMsg {
            Welcome,
            Frame,
            Detach,
            Shutdown,
            SessionList { sessions: Vec<OldSessionInfo> },
        }
        let old = OldServerMsg::SessionList {
            sessions: vec![OldSessionInfo {
                name: "session-1".into(),
                workspace: PathBuf::from("/tmp"),
                panes: 1,
                zoomed: false,
                active: true,
            }],
        };
        let (mut a, mut b) = UnixStream::pair().unwrap();
        protocol::write_framed(&mut a, &old).unwrap();

        let err = read_server(&mut b).unwrap_err();
        assert!(
            err.to_string().contains("older, incompatible kumo"),
            "expected the restart hint, got: {err:#}"
        );
    }

    #[test]
    fn agent_line_colors_blocked_amber() {
        assert_eq!(
            agent_line("opencode", protocol::AgentStatus::Blocked, true),
            "    opencode · \x1b[38;2;255;184;77mblocked\x1b[0m"
        );
    }

    #[test]
    fn agent_line_colors_working_green() {
        assert_eq!(
            agent_line("claude", protocol::AgentStatus::Working, true),
            "    claude · \x1b[38;2;46;224;107mworking\x1b[0m"
        );
    }

    #[test]
    fn agent_line_leaves_idle_plain() {
        assert_eq!(agent_line("opencode", protocol::AgentStatus::Idle, true), "    opencode · idle");
    }

    #[test]
    fn agent_line_plain_when_not_a_terminal() {
        assert_eq!(agent_line("opencode", protocol::AgentStatus::Blocked, false), "    opencode · blocked");
    }

    #[test]
    fn write_row_emits_full_emoji_grapheme() {
        // Row: wide emoji (width 2), then its continuation cell (width 0),
        // then plain text.
        let row = vec![
            cell("\u{1f1ea}\u{1f1f8}", 2), // 🇪🇸
            cell(" ", 0),                  // continuation, must be skipped
            cell("hi", 1),
        ];
        let mut out = String::new();
        write_row(&mut out, 0, &row);
        assert!(
            out.contains("\u{1f1ea}\u{1f1f8}"),
            "emoji missing from client bytes: {out:?}"
        );
        // The continuation cell must not write over the emoji's right half.
        assert!(
            !out.contains("\x1b[1;2H"),
            "continuation cell emitted a position: {out:?}"
        );
    }

    #[test]
    fn write_row_skips_continuation_after_wide_char() {
        // The emoji at col 0 occupies cols 1-2 (1-indexed); the next real cell
        // at col 2 must be positioned at col 3, past the emoji.
        let row = vec![
            cell("\u{1f600}", 2),
            cell(" ", 0),
            cell("x", 1),
        ];
        let mut out = String::new();
        write_row(&mut out, 3, &row);
        assert!(out.contains("\x1b[4;1H\u{1f600}"), "wide cell wrong: {out:?}");
        assert!(out.contains("\x1b[4;3Hx"), "text after emoji mispositioned: {out:?}");
    }

    #[test]
    fn write_row_does_not_erase_last_column_border() {
        // Full-width row whose last non-blank cell is the right border at the
        // final column. `\x1b[K` erases from the cursor to EOL inclusive, so it
        // must NOT be emitted here or it would delete the border we just wrote.
        let mut row = Vec::new();
        for _ in 0..79 {
            row.push(cell(" ", 1));
        }
        row.push(cell("\u{2502}", 1)); // │ border at the last column (80th)
        let mut out = String::new();
        write_row(&mut out, 2, &row);
        assert!(out.contains("\x1b[3;80H\u{2502}"), "border not written: {out:?}");
        assert!(
            !out.contains("\x1b[K"),
            "trailing erase must be skipped when content fills the row: {out:?}"
        );
    }

    #[test]
    fn write_row_erases_tail_after_short_content() {
        // Content ends at col 2 of 4; the stale tail (cols 3-4) must be erased.
        let row = vec![cell("a", 1), cell("b", 1), cell("c", 1), cell(" ", 1)];
        let mut out = String::new();
        write_row(&mut out, 0, &row);
        assert!(out.contains("\x1b[1;4H\x1b[K"), "tail not erased after content: {out:?}");
    }
}
