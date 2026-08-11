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
        loop {
            match protocol::read_framed::<ServerMsg>(stream)? {
                ServerMsg::Welcome { .. } => {}
                ServerMsg::Frame { frame } => blit(&mut stdout, &frame)?,
                ServerMsg::Detach => break,
                ServerMsg::Shutdown => break,
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

/// Phase 1 blit: clear and redraw every non-empty cell each frame. Correct but
/// not diff-efficient; dirty-row streaming lands with the protocol v2 work.
fn blit(out: &mut io::Stdout, f: &FrameMsg) -> io::Result<()> {
    let mut buf = String::with_capacity(f.cells.len() * 8);
    buf.push_str("\x1b[2J\x1b[H");
    let mut prev_style: Option<StyleKey> = None;
    for y in 0..f.rows {
        for x in 0..f.cols {
            let cell = &f.cells[(y as usize) * f.cols as usize + (x as usize)];
            if cell.text.is_empty() {
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
                push_sgr(&mut buf, &style);
                prev_style = Some(style);
            }
            buf.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
            buf.push_str(&cell.text);
        }
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
