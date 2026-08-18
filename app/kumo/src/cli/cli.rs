//! `kumo` control CLI: a thin client that sends [`Command`]s to the daemon over
//! the unix socket and prints the reply. Everything a tmux-style CLI can do:
//! `kumo session ...`, `kumo pane ...`, `kumo agent ...`.
//!
//! The TUI (`kumo` / `kumo attach`) is a separate, richer client; this module
//! only drives the daemon and prints structured replies.

use std::io::{self, IsTerminal};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use kumo_protocol::{
    Command, DaemonEvent, SplitDir, WireKeyCode, WireKeyEvent, WireModifiers,
};

/// One parsed CLI invocation.
enum CliCmd {
    List,
    SessionNew { name: Option<String>, workspace: Option<PathBuf> },
    SessionKill { name: String },
    SessionAttach { name: String },
    PaneSplit { session: Option<String>, dir: SplitDir, ai: bool },
    PaneClose { session: Option<String>, pane: Option<u64> },
    PaneFocus { session: Option<String>, pane: u64 },
    PaneSendKeys { session: Option<String>, pane: Option<u64>, keys: String },
    AgentSpawn { session: Option<String>, program: Option<String> },
    AgentStatus,
    AgentKill { session: Option<String>, pane: u64 },
    Kill,
    Reload,
    Restart,
}

pub fn run(args: &[String]) -> Result<()> {
    let cmd = parse(args)?;
    let mut stream = connect_daemon()?;

    let command = match cmd {
        CliCmd::List => Command::SessionList,
        CliCmd::Kill => Command::KillServer,
        CliCmd::Reload => Command::ReloadConfig,
        CliCmd::Restart => Command::Restart,
        CliCmd::SessionNew { name, workspace } => {
            Command::SessionNew { name, workspace }
        }
        CliCmd::SessionKill { name } => Command::SessionKill { name },
        CliCmd::SessionAttach { name } => Command::SessionFocus { name },
        CliCmd::PaneSplit { session, dir, ai } => Command::PaneSplit {
            session: resolve_session(&mut stream, session)?,
            dir,
            is_ai: ai,
        },
        CliCmd::PaneClose { session, pane } => Command::PaneClose {
            session: resolve_session(&mut stream, session)?,
            pane_id: pane,
        },
        CliCmd::PaneFocus { session, pane } => Command::PaneFocus {
            session: resolve_session(&mut stream, session)?,
            pane_id: pane,
        },
        CliCmd::PaneSendKeys { session, pane, keys } => Command::PaneSendKeys {
            session: resolve_session(&mut stream, session)?,
            pane_id: pane,
            keys: parse_keys(&keys),
        },
        CliCmd::AgentSpawn { session, program } => Command::AgentSpawn {
            session: resolve_session(&mut stream, session)?,
            program,
        },
        CliCmd::AgentStatus => Command::AgentStatus,
        CliCmd::AgentKill { session, pane } => Command::AgentKill {
            session: resolve_session(&mut stream, session)?,
            pane_id: pane,
        },
    };

    kumo_core::protocol::write_framed(&mut stream, &command)?;
    read_reply(&mut stream)
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

fn need(args: &[String], at: usize, what: &str) -> Result<String> {
    args.get(at)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing {what}"))
}

fn parse(args: &[String]) -> Result<CliCmd> {
    let Some(domain) = args.first() else {
        anyhow::bail!("missing command (try `kumo session list`)");
    };
    let rest = &args[1..];
    match domain.as_str() {
        "session" => parse_session(rest),
        "pane" => parse_pane(rest),
        "agent" => parse_agent(rest),
        "ls" | "list" => Ok(CliCmd::List),
        "kill" => Ok(CliCmd::Kill),
        "reload" => Ok(CliCmd::Reload),
        "server" if rest.first().map(|s| s.as_str()) == Some("restart") => Ok(CliCmd::Restart),
        other => anyhow::bail!("unknown command {other:?}"),
    }
}

fn parse_session(args: &[String]) -> Result<CliCmd> {
    let Some(sub) = args.first().map(|s| s.as_str()) else {
        anyhow::bail!("usage: kumo session [list|new [DIR] [--name NAME]|kill NAME|attach NAME]");
    };
    match sub {
        "list" => Ok(CliCmd::List),
        "new" => {
            let mut name = None;
            let mut workspace = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--name" => {
                        name = Some(need(args, i + 1, "a name after --name")?);
                        i += 2;
                    }
                    arg => {
                        workspace = Some(PathBuf::from(arg));
                        i += 1;
                    }
                }
            }
            Ok(CliCmd::SessionNew { name, workspace })
        }
        "kill" => Ok(CliCmd::SessionKill { name: need(args, 1, "a session name")? }),
        "attach" => Ok(CliCmd::SessionAttach { name: need(args, 1, "a session name")? }),
        other => anyhow::bail!("unknown session subcommand {other:?}"),
    }
}

fn parse_pane(args: &[String]) -> Result<CliCmd> {
    let Some(sub) = args.first().map(|s| s.as_str()) else {
        anyhow::bail!("usage: kumo pane [split|close|focus|send-keys]");
    };
    // `-s SESSION` / `-p PANE` options anywhere in the args.
    let (session, pane_id, positional) = split_options(args);
    match sub {
        "split" => {
            let ai = positional.iter().any(|a| a == "--ai");
            let dir = if positional.iter().any(|a| a == "--horizontal" || a == "-h") {
                SplitDir::Horizontal
            } else {
                SplitDir::Vertical
            };
            Ok(CliCmd::PaneSplit { session, dir, ai })
        }
        "close" => Ok(CliCmd::PaneClose { session, pane: pane_id }),
        "focus" => Ok(CliCmd::PaneFocus {
            session,
            pane: pane_id.ok_or_else(|| anyhow::anyhow!("pane focus needs -p PANE_ID"))?,
        }),
        "send-keys" => Ok(CliCmd::PaneSendKeys {
            session,
            pane: pane_id,
            keys: positional.join(" "),
        }),
        other => anyhow::bail!("unknown pane subcommand {other:?}"),
    }
}

fn parse_agent(args: &[String]) -> Result<CliCmd> {
    let Some(sub) = args.first().map(|s| s.as_str()) else {
        anyhow::bail!("usage: kumo agent [spawn|status|kill]");
    };
    let (session, pane_id, positional) = split_options(args);
    match sub {
        "spawn" => Ok(CliCmd::AgentSpawn {
            session,
            program: positional.first().cloned(),
        }),
        "status" => Ok(CliCmd::AgentStatus),
        "kill" => Ok(CliCmd::AgentKill {
            session,
            pane: pane_id.ok_or_else(|| anyhow::anyhow!("agent kill needs -p PANE_ID"))?,
        }),
        other => anyhow::bail!("unknown agent subcommand {other:?}"),
    }
}

/// Extract `-s SESSION` and `-p PANE_ID` options, returning the rest as
/// positional args.
fn split_options(args: &[String]) -> (Option<String>, Option<u64>, Vec<String>) {
    let mut session = None;
    let mut pane = None;
    let mut positional = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-s" | "--session" => {
                if let Some(v) = args.get(i + 1) {
                    session = Some(v.clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "-p" | "--pane" => {
                if let Some(v) = args.get(i + 1) {
                    pane = v.parse::<u64>().ok();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            arg => {
                positional.push(arg.to_string());
                i += 1;
            }
        }
    }
    (session, pane, positional)
}

/// Turn a send-keys string into wire key events: plain characters are typed as
///-is; recognized tokens map to special keys.
fn parse_keys(text: &str) -> Vec<WireKeyEvent> {
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        let code = match token {
            "Enter" | "RETURN" => WireKeyCode::Enter,
            "Space" => WireKeyCode::Char(' '),
            "Tab" => WireKeyCode::Tab,
            "Esc" | "Escape" => WireKeyCode::Esc,
            "Left" => WireKeyCode::Left,
            "Right" => WireKeyCode::Right,
            "Up" => WireKeyCode::Up,
            "Down" => WireKeyCode::Down,
            "Backspace" => WireKeyCode::Backspace,
            "Delete" => WireKeyCode::Delete,
            "PageUp" => WireKeyCode::PageUp,
            "PageDown" => WireKeyCode::PageDown,
            "Home" => WireKeyCode::Home,
            "End" => WireKeyCode::End,
            _ => {
                for c in token.chars() {
                    out.push(WireKeyEvent::new(WireKeyCode::Char(c), WireModifiers::none()));
                }
                continue;
            }
        };
        out.push(WireKeyEvent::new(code, WireModifiers::none()));
    }
    out
}

// ---------------------------------------------------------------------------
// Socket plumbing
// ---------------------------------------------------------------------------

fn connect_daemon() -> Result<UnixStream> {
    let path = kumo_core::config::ipc_socket_path();
    UnixStream::connect(&path).map_err(|_| {
        anyhow::anyhow!("no kumo daemon is running (start with `kumo` or `kumo new`)")
    })
}

/// Ask the daemon for the active session name (used when a command targets
/// "the current session" without `-s`).
fn resolve_session(stream: &mut UnixStream, session: Option<String>) -> Result<String> {
    if let Some(s) = session {
        return Ok(s);
    }
    kumo_core::protocol::write_framed(stream, &Command::SessionList)?;
    loop {
        match kumo_core::protocol::read_framed::<DaemonEvent>(stream) {
            Ok(DaemonEvent::SessionList { sessions }) => {
                return Ok(sessions
                    .iter()
                    .find(|s| s.active)
                    .or_else(|| sessions.first())
                    .map(|s| s.name.clone())
                    .unwrap_or_default());
            }
            Ok(_) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Read the daemon's reply and print it. A short read timeout covers commands
/// that produce no reply (focus/resize): those simply exit.
fn read_reply(stream: &mut UnixStream) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(800)))?;
    loop {
        match kumo_core::protocol::read_framed::<DaemonEvent>(stream) {
            Ok(DaemonEvent::Reply { message }) => {
                println!("{message}");
                return Ok(());
            }
            Ok(DaemonEvent::Restarting) => {
                println!("daemon restarting…");
                return Ok(());
            }
            Ok(DaemonEvent::ConfigReloaded { notice }) => {
                println!("{notice}");
                return Ok(());
            }
            Ok(DaemonEvent::SessionList { sessions }) => {
                print_session_list(&sessions);
                return Ok(());
            }
            Ok(DaemonEvent::AgentStatus { agents }) => {
                print_agent_status(&agents);
                return Ok(());
            }
            Ok(_) => continue,
            Err(e) if is_timeout(&e) => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

fn is_timeout(e: &anyhow::Error) -> bool {
    e.downcast_ref::<io::Error>()
        .map(|ioe| matches!(ioe.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
        .unwrap_or(false)
}

fn print_session_list(sessions: &[kumo_protocol::SessionInfo]) {
    for s in sessions {
        let mark = if s.active { "* " } else { "  " };
        let pane_word = if s.pane_count == 1 { "pane" } else { "panes" };
        println!(
            "{mark}{}: {} {} · {}{}",
            s.name,
            s.pane_count,
            pane_word,
            s.workspace.display(),
            if s.zoomed { " (zoomed)" } else { "" }
        );
        let color = io::stdout().is_terminal();
        for agent in &s.agents {
            let label = agent.status.label();
            let line = if color {
                let (r, g, b) = match agent.status {
                    kumo_protocol::AgentStatus::Blocked => (0xff, 0xb8, 0x4d),
                    kumo_protocol::AgentStatus::Working => (0x2e, 0xe0, 0x6b),
                    kumo_protocol::AgentStatus::Idle => (0x88, 0x88, 0x88),
                };
                format!("    {} · \x1b[38;2;{r};{g};{b}m{label}\x1b[0m", agent.name)
            } else {
                format!("    {} · {label}", agent.name)
            };
            println!("{line}");
        }
    }
    if sessions.is_empty() {
        println!("(no sessions)");
    }
}

fn print_agent_status(agents: &[kumo_protocol::AgentStatusLine]) {
    for a in agents {
        println!("{} · {} · {} (pane {})", a.session, a.name, a.status.label(), a.pane_id);
    }
    if agents.is_empty() {
        println!("(no agents running)");
    }
}
