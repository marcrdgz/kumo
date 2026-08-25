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
    TabList { session: Option<String> },
    TabNew { session: Option<String>, name: Option<String>, workspace: Option<PathBuf> },
    TabClose { session: Option<String>, tab: String },
    TabFocus { session: Option<String>, tab: String },
    TabRename { session: Option<String>, tab: String, new_name: String },
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
    // `-h/--help` anywhere in a domain invocation prints that domain's usage
    // and exits without touching the daemon.
    if let Some(domain) = args.first() {
        if args[1..].iter().any(|a| a == "-h" || a == "--help") {
            print!("{}", domain_help(domain));
            return Ok(());
        }
    }
    let cmd = parse(args)?;
    let mut stream = connect_daemon()?;

    // Tab list is special: it needs SessionList filtering
    if let CliCmd::TabList { session } = cmd {
        let target = resolve_session(&mut stream, session)?;
        kumo_core::protocol::write_framed(&mut stream, &Command::SessionList)?;
        stream.set_read_timeout(Some(Duration::from_millis(800)))?;
        loop {
            match kumo_core::protocol::read_framed::<DaemonEvent>(&mut stream) {
                Ok(DaemonEvent::SessionList { sessions }) => {
                    print_tab_list(&sessions, &target);
                    return Ok(());
                }
                Ok(_) => continue,
                Err(e) if is_timeout(&e) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }

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
        CliCmd::TabList { .. } => unreachable!(),
        CliCmd::TabNew { session, name, workspace } => Command::TabNew {
            session: resolve_session(&mut stream, session)?,
            name,
            workspace,
        },
        CliCmd::TabClose { session, tab } => Command::TabClose {
            session: resolve_session(&mut stream, session)?,
            tab: Some(tab),
        },
        CliCmd::TabFocus { session, tab } => Command::TabFocus {
            session: resolve_session(&mut stream, session)?,
            tab,
        },
        CliCmd::TabRename { session, tab, new_name } => Command::TabRename {
            session: resolve_session(&mut stream, session)?,
            tab,
            new_name,
        },
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
        "tab" => parse_tab(rest),
        "ls" | "list" => Ok(CliCmd::List),
        "kill" => Ok(CliCmd::Kill),
        "reload" => Ok(CliCmd::Reload),
        "server" if rest.first().map(|s| s.as_str()) == Some("restart") => Ok(CliCmd::Restart),
        other => anyhow::bail!("unknown command {other:?}"),
    }
}

fn parse_session(args: &[String]) -> Result<CliCmd> {
    let Some(sub) = args.first().map(|s| s.as_str()) else {
        anyhow::bail!("missing session subcommand (see `kumo session -h`)");
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
        anyhow::bail!("missing pane subcommand (see `kumo pane -h`)");
    };
    // `-s SESSION` / `-p PANE` options anywhere in the args.
    let (session, pane_id, positional) = split_options(args);
    match sub {
        "split" => {
            let ai = positional.iter().any(|a| a == "--ai");
            let dir = if positional.iter().any(|a| a == "--horizontal") {
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
        anyhow::bail!("missing agent subcommand (see `kumo agent -h`)");
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

fn parse_tab(args: &[String]) -> Result<CliCmd> {
    let Some(sub) = args.first().map(|s| s.as_str()) else {
        anyhow::bail!("missing tab subcommand (see `kumo tab -h`)");
    };
    // Extract -s/--session wherever it appears
    let mut session: Option<String> = None;
    let mut filtered: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-s" | "--session" => {
                if let Some(v) = args.get(i+1) { session = Some(v.clone()); }
                i += 2;
            }
            _ => { filtered.push(args[i].clone()); i+=1; }
        }
    }
    match sub {
        "list" => Ok(CliCmd::TabList { session }),
        "new" => {
            let mut name = None;
            let mut workspace = None;
            let mut j = 0;
            while j < filtered.len() {
                match filtered[j].as_str() {
                    "--name" => {
                        name = Some(need(&filtered, j+1, "a name after --name")?);
                        j += 2;
                    }
                    arg if workspace.is_none() => {
                        workspace = Some(PathBuf::from(arg));
                        j += 1;
                    }
                    _ => { j += 1; }
                }
            }
            Ok(CliCmd::TabNew { session, name, workspace })
        }
        "close" | "kill" => {
            let tab = filtered.first().cloned().ok_or_else(|| anyhow::anyhow!("tab close needs TAB name/id"))?;
            Ok(CliCmd::TabClose { session, tab })
        }
        "focus" => {
            let tab = filtered.first().cloned().ok_or_else(|| anyhow::anyhow!("tab focus needs TAB name/id"))?;
            Ok(CliCmd::TabFocus { session, tab })
        }
        "rename" => {
            let tab = filtered.first().cloned().ok_or_else(|| anyhow::anyhow!("tab rename needs TAB and NEW_NAME"))?;
            let new_name = filtered.get(1).cloned().ok_or_else(|| anyhow::anyhow!("tab rename needs NEW_NAME"))?;
            Ok(CliCmd::TabRename { session, tab, new_name })
        }
        other => anyhow::bail!("unknown tab subcommand {other:?}"),
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
// Help text
// ---------------------------------------------------------------------------

fn domain_help(domain: &str) -> &'static str {
    match domain {
        "session" => SESSION_HELP,
        "pane" => PANE_HELP,
        "agent" => AGENT_HELP,
        "tab" => TAB_HELP,
        "server" => SERVER_HELP,
        "ls" | "list" | "kill" | "reload" => LEGACY_HELP,
        _ => "",
    }
}

const SESSION_HELP: &str = "\
kumo session — manage sessions

USAGE:
    kumo session list
    kumo session new [DIR] [--name NAME]
    kumo session kill NAME
    kumo session attach NAME

OPTIONS:
    --name NAME    name the new session (defaults to the workspace name)
";

const PANE_HELP: &str = "\
kumo pane — manage panes

USAGE:
    kumo pane split [-s SESSION] [--horizontal] [--ai]
    kumo pane close [-s SESSION] [-p PANE_ID]
    kumo pane focus -p PANE_ID [-s SESSION]
    kumo pane send-keys [-s SESSION] [-p PANE_ID] KEYS...

OPTIONS:
    -s, --session SESSION   target session (defaults to the active one)
    -p, --pane PANE_ID      target pane id (defaults to the active pane)
    --horizontal            split left/right instead of top/bottom
    --ai                    start the new pane with the AI agent program

send-keys: KEYS... are typed into the pane (plain text plus tokens such as
Enter, Tab, Esc, Left, Up, PageDown — see `kumo pane send-keys` KEYS).
";

const AGENT_HELP: &str = "\
kumo agent — manage AI agents

USAGE:
    kumo agent spawn [-s SESSION] [PROGRAM]
    kumo agent status
    kumo agent kill -p PANE_ID [-s SESSION]

OPTIONS:
    -s, --session SESSION   target session (defaults to the active one)
    -p, --pane PANE_ID      target pane id (the agent pane)

PROGRAM defaults to the configured AI program (see config).
";

const TAB_HELP: &str = "\
kumo tab — manage tabs (implicit tab bars inside a session)

USAGE:
    kumo tab list [-s SESSION]
    kumo tab new [WORKSPACE] [--name NAME] [-s SESSION]
    kumo tab focus TAB [-s SESSION]
    kumo tab kill TAB [-s SESSION]
    kumo tab rename TAB NEW_NAME [-s SESSION]

OPTIONS:
    -s, --session SESSION   target session (defaults to the active one)
    --name NAME             name the new tab (defaults to the workspace name)
";

const SERVER_HELP: &str = "\
kumo server — control the headless daemon

USAGE:
    kumo server restart

`kumo server restart` restarts the daemon in place; panes stay alive.
";

const LEGACY_HELP: &str = "\
kumo ls | list / kill / reload — legacy aliases

USAGE:
    kumo ls | list       list sessions (same as `kumo session list`)
    kumo kill            shut down the daemon
    kumo reload          reload the configuration on the daemon

Prefer the namespaced commands (`kumo session`, `kumo pane`, `kumo agent`).
";

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
        let tab_word = if s.tab_count == 1 { "tab" } else { "tabs" };
        println!(
            "{mark}{}: {} {tab_word} · {} {} · {}{}",
            s.name,
            s.tab_count,
            s.pane_count,
            pane_word,
            s.workspace.display(),
            if s.zoomed { " (zoomed)" } else { "" }
        );
        // tabs detail (for tab-aware ls)
        for tab in &s.tabs {
            let tmark = if tab.active { "  * " } else { "    " };
            let pw = if tab.pane_count == 1 { "pane" } else { "panes" };
            println!("{tmark}[{}] (id {}): {} {}{}", tab.name, tab.id, tab.pane_count, pw, if tab.zoomed { " (zoomed)" } else { "" });
        }
        let color = io::stdout().is_terminal();
        for agent in &s.agents {
            let label = agent.status.label();
            let line = if color {
                let (r, g, b) = kumo_core::theme::agent_status_color(agent.status);
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

fn print_tab_list(sessions: &[kumo_protocol::SessionInfo], target: &str) {
    let Some(s) = sessions.iter().find(|s| s.name == target) else {
        println!("no session {target:?}");
        return;
    };
    if s.tabs.is_empty() {
        println!("(no tabs)");
        return;
    }
    for tab in &s.tabs {
        let mark = if tab.active { "* " } else { "  " };
        let pw = if tab.pane_count == 1 { "pane" } else { "panes" };
        println!("{mark}{} (id {}): {} {}{}", tab.name, tab.id, tab.pane_count, pw, if tab.zoomed { " (zoomed)" } else { "" });
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
