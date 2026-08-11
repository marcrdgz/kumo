//! Thin terminal client: connects to the daemon socket, renders the frames the
//! daemon streams, and forwards input. It has no terminal emulator — the daemon
//! renders the whole UI (panes, chrome, sidebar) and ships rendered cells.

use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};

use crate::app::Launch;
use crate::protocol::{self, ClientMsg, FrameMsg, ServerMsg};

pub fn run(launch: Launch) -> Result<()> {
    let path = crate::config::ipc_socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => match launch {
            Launch::Attach => {
                anyhow::bail!("no kumo daemon is running (start with `kumo` or `kumo new`)")
            }
            _ => {
                spawn_daemon(workspace_for(&launch))?;
                wait_for_daemon(&path)?;
                UnixStream::connect(&path)?
            }
        },
    };
    client_loop(&mut stream)
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
    match protocol::read_framed::<ServerMsg>(&mut stream)? {
        ServerMsg::SessionList { sessions } => {
            for s in &sessions {
                let mark = if s.active { "* " } else { "  " };
                let pane_word = if s.panes == 1 { "pane" } else { "panes" };
                println!(
                    "{mark}{}: {} {} · {}{}",
                    s.name,
                    s.panes,
                    pane_word,
                    s.workspace.display(),
                    if s.zoomed { " (zoomed)" } else { "" }
                );
            }
            if sessions.is_empty() {
                println!("(no sessions)");
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!("unexpected daemon reply: {other:?}")),
    }
}

/// `kumo kill`: stop the daemon (and the processes in its panes).
pub fn kill_server() -> Result<()> {
    let mut stream = connect_daemon()?;
    protocol::write_framed(&mut stream, &ClientMsg::KillServer)?;
    Ok(())
}

fn client_loop(stream: &mut UnixStream) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    protocol::write_framed(
        stream,
        &ClientMsg::Hello { protocol: protocol::PROTOCOL_VERSION, cols, rows },
    )?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::event::EnableMouseCapture, Hide, Clear(ClearType::All))?;

    // Input thread: reads crossterm events and writes them straight to the
    // daemon over its own socket clone.
    let write_half = stream.try_clone()?;
    std::thread::spawn(move || input_loop(write_half));

    let result: Result<()> = (|| {
        let mut saw_frame = false;
        loop {
            match protocol::read_framed::<ServerMsg>(stream)? {
                ServerMsg::Welcome { .. } => {}
                ServerMsg::Frame { frame } => {
                    saw_frame = true;
                    blit(&mut stdout, &frame)?;
                }
                ServerMsg::SessionList { .. } => {}
                ServerMsg::Detach => break,
                ServerMsg::Shutdown => {
                    // A shutdown before any frame means the daemon rejected the
                    // handshake (protocol mismatch with an old lingering daemon)
                    // rather than a clean auto-stop.
                    if !saw_frame {
                        anyhow::bail!(
                            "the running kumo daemon is from an older, incompatible kumo.\n\
                             Restart it with:  pkill -f 'kumo daemon'\nthen run `kumo` again"
                        );
                    }
                    break;
                }
            }
        }
        Ok(())
    })();

    let _ = execute!(stdout, Show, crossterm::event::DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = stdout.flush();
    result
}

/// Read crossterm events and forward them to the daemon.
fn input_loop(mut stream: UnixStream) {
    while let Ok(ev) = crossterm::event::read() {
        match ev {
            crossterm::event::Event::Key(k) => {
                let _ = protocol::write_framed(&mut stream, &ClientMsg::Input { key: k.into() });
            }
            crossterm::event::Event::Mouse(m) => {
                let _ = protocol::write_framed(&mut stream, &ClientMsg::Mouse { event: m.into() });
            }
            crossterm::event::Event::Resize(w, h) => {
                let _ = protocol::write_framed(&mut stream, &ClientMsg::Resize { cols: w, rows: h });
            }
            _ => {}
        }
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
    }
    buf.push_str("\x1b[K");
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
