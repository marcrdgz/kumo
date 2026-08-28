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

/// A pane selector: the stable numeric id, or a composite `s1:t2:p3` /
/// `kumo:t2:p3` spec (1-based indexes; the session part may be a name).
/// Composite specs are resolved client-side via a `SessionList` round trip —
/// the daemon always gets the canonical `u64`.
enum PaneRef {
    Id(u64),
    Spec(String),
}

/// A composite position `s1:t2:p3`, `kumo:t2:p1`, `t2:p1` (session from `-s`),
/// or `t2` (tab-only, for `kumo pane list`). Components may carry an optional
/// `s`/`t`/`p` letter; the session part is a name or a 1-based index. Positions
/// are indexes in the visible session list / tab list / pane list ordering.
#[derive(Debug, Clone)]
struct CompositeSpec {
    session: Option<String>,
    session_index: Option<usize>,
    tab: Option<usize>,
    pane: Option<usize>,
}

impl CompositeSpec {
    /// Parse a 1- to 3-part `:`-separated spec.
    fn parse(text: &str) -> Result<CompositeSpec> {
        let parts: Vec<&str> = text.split(':').map(str::trim).collect();
        let seg = |i: usize| match parts.get(i) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => anyhow::bail!("bad pane spec {text:?}: empty component"),
        };
        let indexed = |s: &str, label: char, what: &str| -> Result<Option<usize>> {
            let v = s.strip_prefix(label).unwrap_or(s);
            if v.is_empty() {
                anyhow::bail!("bad {what}: {s:?} (expects a 1-based index)");
            }
            v.parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("bad {what}: {s:?} (expects a 1-based index)"))
        };
        match parts.len() {
            1 => Ok(CompositeSpec {
                session: None,
                session_index: None,
                tab: indexed(seg(0)?, 't', "tab index")?,
                pane: None,
            }),
            2 => Ok(CompositeSpec {
                session: None,
                session_index: None,
                tab: indexed(seg(0)?, 't', "tab index")?,
                pane: indexed(seg(1)?, 'p', "pane index")?,
            }),
            3 => {
                let s = seg(0)?;
                // Session part: a 1-based index (`s2` — the `s` label only
                // counts when the remainder is numeric, so a session literally
                // named "session-1" is never mangled), or a name (`kumo`).
                let (session, session_index) =
                    match s.strip_prefix('s').and_then(|r| r.parse::<usize>().ok()) {
                        Some(n) if n >= 1 => (None, Some(n)),
                        _ => match s.parse::<usize>() {
                            Ok(n) if n >= 1 => (None, Some(n)),
                            Ok(_) => anyhow::bail!("bad session index: {s:?} (indexes are 1-based)"),
                            Err(_) => (Some(s.to_string()), None),
                        },
                    };
                Ok(CompositeSpec {
                    session,
                    session_index,
                    tab: indexed(seg(1)?, 't', "tab index")?,
                    pane: indexed(seg(2)?, 'p', "pane index")?,
                })
            }
            _ => anyhow::bail!("bad pane spec {text:?}: expected [s:]t[:p], e.g. s1:t2:p1"),
        }
    }
}

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
    PaneClose { session: Option<String>, pane: Option<PaneRef> },
    PaneFocus { session: Option<String>, pane: PaneRef },
    PaneSendKeys { session: Option<String>, pane: Option<PaneRef>, keys: String },
    PaneList { session: Option<String>, tab: Option<PaneRef> },
    AgentSpawn { session: Option<String>, program: Option<String> },
    AgentStatus,
    AgentKill { session: Option<String>, pane: PaneRef },
    AgentExplain { session: Option<String>, pane: Option<PaneRef> },
    Kill,
    Reload,
    Restart,
}

/// `-p` value or a positional selector: all-digits = stable numeric id,
/// anything with `:` = composite spec.
fn parse_pane_ref(v: &str) -> Option<PaneRef> {
    if let Ok(n) = v.parse::<u64>() {
        Some(PaneRef::Id(n))
    } else if v.contains(':') {
        Some(PaneRef::Spec(v.to_string()))
    } else {
        None
    }
}

pub fn run(args: &[String]) -> Result<()> {
    run_inner(args).map_err(friendly_protocol_error)
}

/// A bincode decode failure against a running daemon means a binary version
/// mismatch (the CLI and the daemon are the same binary — they ship and must
/// run together). Turn the bare bincode error into an actionable message.
fn friendly_protocol_error(e: anyhow::Error) -> anyhow::Error {
    let msg = format!("{e}");
    if msg.contains("UnexpectedEnd")
        || msg.contains("Unexpected end")
        || msg.contains("unexpected eof")
        || msg.contains("unexpected end")
    {
        anyhow::anyhow!(
            "the running kumo daemon is too old to speak this protocol — restart it with \
             `kumo server restart` after `kumo update` (or kill it and start the new binary: \
             `kumo daemon`)"
        )
    } else {
        e
    }
}

fn run_inner(args: &[String]) -> Result<()> {
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

    // Tab/pane list are special: they need a SessionList round trip and filter
    // client-side (the metadata travels in SessionInfo).
    match cmd {
        CliCmd::TabList { session } => {
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
        CliCmd::PaneList { session, tab } => {
            let mut target = resolve_session(&mut stream, session)?;
            let sessions = fetch_session_list(&mut stream)?;
            // The tab filter is client-side: a numeric id, or a composite
            // `s1:t2` / `t2` positional spec.
            let tab_id: Option<u64> = match &tab {
                None => None,
                Some(PaneRef::Id(id)) => Some(*id),
                Some(PaneRef::Spec(raw)) => {
                    let spec = CompositeSpec::parse(raw)?;
                    let s = find_spec_session(&sessions, &spec, &target)?;
                    target = s.name.clone();
                    match spec.tab {
                        Some(t) => match s.tabs.get(t - 1) {
                            Some(tab) => Some(tab.id),
                            None => anyhow::bail!("session {:?} has no tab {t}", s.name),
                        },
                        None => None,
                    }
                }
            };
            print_pane_list(&sessions, &target, tab_id);
            Ok(())
        }
        cmd => {
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
                CliCmd::PaneClose { session, pane } => {
                    let session = resolve_session(&mut stream, session)?;
                    let pane_id = match pane {
                        Some(p) => Some(resolve_pane_ref(&mut stream, &session, &p)?),
                        None => None,
                    };
                    Command::PaneClose { session, pane_id }
                }
                CliCmd::PaneFocus { session, pane } => {
                    let session = resolve_session(&mut stream, session)?;
                    let pane_id = resolve_pane_ref(&mut stream, &session, &pane)?;
                    Command::PaneFocus { session, pane_id }
                }
                CliCmd::PaneSendKeys { session, pane, keys } => {
                    let session = resolve_session(&mut stream, session)?;
                    let pane_id = match pane {
                        Some(p) => Some(resolve_pane_ref(&mut stream, &session, &p)?),
                        None => None,
                    };
                    Command::PaneSendKeys { session, pane_id, keys: parse_keys(&keys) }
                }
                CliCmd::PaneList { .. } => unreachable!(),
                CliCmd::AgentSpawn { session, program } => Command::AgentSpawn {
                    session: resolve_session(&mut stream, session)?,
                    program,
                },
                CliCmd::AgentStatus => Command::AgentStatus,
                CliCmd::AgentKill { session, pane } => {
                    let session = resolve_session(&mut stream, session)?;
                    let pane_id = resolve_pane_ref(&mut stream, &session, &pane)?;
                    Command::AgentKill { session, pane_id }
                }
                CliCmd::AgentExplain { session, pane } => {
                    let session = resolve_session(&mut stream, session)?;
                    let pane_id = match pane {
                        Some(p) => resolve_pane_ref(&mut stream, &session, &p)?,
                        // Default: the first AI pane of the target session
                        // (same data `kumo agent status` prints).
                        None => {
                            kumo_core::protocol::write_framed(&mut stream, &Command::AgentStatus)?;
                            let mut found = None;
                            loop {
                                match kumo_core::protocol::read_framed::<DaemonEvent>(&mut stream) {
                                    Ok(DaemonEvent::AgentStatus { agents }) => {
                                        found = agents
                                            .into_iter()
                                            .find(|a| a.session == session)
                                            .map(|a| a.pane_id);
                                        break;
                                    }
                                    Ok(_) => continue,
                                    Err(e) if is_timeout(&e) => break,
                                    Err(e) => return Err(e),
                                }
                            }
                            found.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "no AI agent in session {session:?} — pass a pane id (see `kumo pane list`)"
                                )
                            })?
                        }
                    };
                    Command::AgentExplain { session, pane_id }
                }
            };

            kumo_core::protocol::write_framed(&mut stream, &command)?;
            read_reply(&mut stream)
        }
    }
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
        "list" => {
            // Optional -t/--tab TAB_ID (numeric id) or a positional composite
            // `s1:t2[:p3]` / `t2` filter (split_options leaves both in the
            // positional args).
            let mut tab: Option<PaneRef> = None;
            let mut i = 0;
            while i < positional.len() {
                if positional[i] == "-t" || positional[i] == "--tab" {
                    tab = positional.get(i + 1).and_then(|v| v.parse::<u64>().ok().map(PaneRef::Id));
                    i += 2;
                } else {
                    if tab.is_some() {
                        anyhow::bail!("pane list takes one tab filter (got {:?} after -t)", positional[i]);
                    }
                    tab = parse_pane_ref(&positional[i]).or_else(|| {
                        // `t2` (tab-only composite) has no colon; accept it here.
                        CompositeSpec::parse(&positional[i]).ok().map(|_| PaneRef::Spec(positional[i].clone()))
                    });
                    i += 1;
                }
            }
            Ok(CliCmd::PaneList { session, tab })
        }
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
        "status" | "list" | "ls" => Ok(CliCmd::AgentStatus),
        "kill" => Ok(CliCmd::AgentKill {
            session,
            pane: pane_id
                .or_else(|| positional.first().and_then(|s| parse_pane_ref(s)))
                .ok_or_else(|| anyhow::anyhow!("agent kill needs a PANE (see `kumo pane -h`)"))?,
        }),
        "explain" => Ok(CliCmd::AgentExplain {
            session,
            pane: pane_id.or_else(|| positional.first().and_then(|s| parse_pane_ref(s))),
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

/// Extract `-s SESSION` and `-p PANE` options, returning the rest as
/// positional args. `-p` accepts a stable numeric id or a composite
/// `s1:t2:p3` / `kumo:t2:p1` spec.
fn split_options(args: &[String]) -> (Option<String>, Option<PaneRef>, Vec<String>) {
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
                    pane = match v.parse::<u64>() {
                        Ok(n) => Some(PaneRef::Id(n)),
                        Err(_) if v.contains(':') => Some(PaneRef::Spec(v.clone())),
                        Err(_) => None,
                    };
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
    kumo pane close [-s SESSION] [PANE]
    kumo pane focus [PANE] [-s SESSION]
    kumo pane send-keys [-s SESSION] [PANE] KEYS...
    kumo pane list [-s SESSION] [-t TAB_ID] [PANE]

OPTIONS:
    -s, --session SESSION   target session (defaults to the active one)
    -p, --pane PANE         target pane: a pane id or a composite position
    -t, --tab TAB_ID        filter `list` to one tab id
    --horizontal            split left/right instead of top/bottom
    --ai                    start the new pane with the AI agent program

send-keys: KEYS... are typed into the pane (plain text plus tokens such as
Enter, Tab, Esc, Left, Up, PageDown — see `kumo pane send-keys` KEYS).
list: prints every pane with its id + composite position (marking the
focused one) — the ids feed `kumo agent explain` / `kumo pane focus`.

PANE may be a stable numeric id, or a composite position (1-based indexes):
    s1:t2:p3    session 1, tab 2, pane 3
    kumo:t2:p1  the session named kumo, tab 2, pane 1
    t2:p1       tab 2, pane 1 (session from -s)
list also accepts `s1:t2` / `t2` as a tab filter.
";

const AGENT_HELP: &str = "\
kumo agent — manage AI agents

USAGE:
    kumo agent spawn [-s SESSION] [PROGRAM]
    kumo agent status          (aliases: list, ls)
    kumo agent kill [PANE] [-s SESSION]
    kumo agent explain [PANE] [-s SESSION]

OPTIONS:
    -s, --session SESSION   target session (defaults to the active one)
    -p, --pane PANE         target pane id (the agent pane)

PANE is a stable numeric id or a composite position (see `kumo pane -h`):
s1:t2:p3, kumo:t2:p1, or t2:p1 with -s.
PROGRAM defaults to the configured AI program (see config).
status: one line per running AI CLI with its pane id and position.
explain: why this pane reads the state it does — matched markers, evidence
region (screen/form/footer/title), and the idle-fallback reason, evaluated
live by the daemon. PANE may also be a composite position given as the
first positional, or omitted to pick the first AI pane of the session.
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

/// One `SessionList` round trip (the composite-spec resolver's data source).
fn fetch_session_list(stream: &mut UnixStream) -> Result<Vec<kumo_protocol::SessionInfo>> {
    kumo_core::protocol::write_framed(stream, &Command::SessionList)?;
    loop {
        match kumo_core::protocol::read_framed::<DaemonEvent>(stream) {
            Ok(DaemonEvent::SessionList { sessions }) => return Ok(sessions),
            Ok(_) => continue,
            Err(e) if is_timeout(&e) => anyhow::bail!("no reply from the daemon"),
            Err(e) => return Err(e),
        }
    }
}

/// Resolve a composite spec to the canonical `u64` pane id, plus a display
/// string (`name:t2:p3`) for printouts.
fn resolve_pane_ref(stream: &mut UnixStream, session: &str, pane: &PaneRef) -> Result<u64> {
    match pane {
        PaneRef::Id(id) => Ok(*id),
        PaneRef::Spec(raw) => {
            let spec = CompositeSpec::parse(raw)?;
            let sessions = fetch_session_list(stream)?;
            resolve_spec_pane(&sessions, &spec, session).map(|(id, _)| id)
        }
    }
}

/// Find the `SessionInfo` a spec targets: explicit name, session index, or
/// the default session.
fn find_spec_session<'a>(
    sessions: &'a [kumo_protocol::SessionInfo],
    spec: &CompositeSpec,
    default_session: &str,
) -> Result<&'a kumo_protocol::SessionInfo> {
    if let Some(name) = &spec.session {
        sessions
            .iter()
            .find(|s| &s.name == name)
            .ok_or_else(|| anyhow::anyhow!("no session {name:?}"))
    } else if let Some(n) = spec.session_index {
        sessions
            .get(n - 1)
            .ok_or_else(|| anyhow::anyhow!("no session {n} of {}", sessions.len()))
    } else {
        sessions
            .iter()
            .find(|s| s.name == default_session)
            .ok_or_else(|| anyhow::anyhow!("no session {default_session:?}"))
    }
}

/// Resolve a parsed composite spec against `SessionInfo` data: `(pane id,
/// display string)`. Pure — unit-testable without a socket.
fn resolve_spec_pane(
    sessions: &[kumo_protocol::SessionInfo],
    spec: &CompositeSpec,
    default_session: &str,
) -> Result<(u64, String)> {
    let tab = spec
        .tab
        .ok_or_else(|| anyhow::anyhow!("pane spec needs a tab index (t:n)"))?;
    let pane = spec
        .pane
        .ok_or_else(|| anyhow::anyhow!("pane spec needs a pane index (p:n)"))?;
    let s = find_spec_session(sessions, spec, default_session)?;
    let t = s
        .tabs
        .get(tab - 1)
        .ok_or_else(|| anyhow::anyhow!("session {:?} has {} tabs; no tab {tab}", s.name, s.tabs.len()))?;
    let p = t
        .panes
        .get(pane - 1)
        .ok_or_else(|| anyhow::anyhow!("tab {:?} has {} panes; no pane {pane}", t.name, t.panes.len()))?;
    Ok((p.id, format!("{}:t{tab}:p{pane}", s.name)))
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
            Ok(DaemonEvent::AgentExplain { report }) => {
                print_agent_explain(&report);
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
    for (i, s) in sessions.iter().enumerate() {
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
            let pos = if agent.tab_index > 0 && agent.pane_index > 0 {
                format!(" (pane {} · s{}:t{}:p{})", agent.pane_id, i + 1, agent.tab_index, agent.pane_index)
            } else {
                format!(" (pane {})", agent.pane_id)
            };
            let line = if color {
                let (r, g, b) = kumo_core::theme::agent_status_color(agent.status);
                format!("    {} · \x1b[38;2;{r};{g};{b}m{label}\x1b[0m{pos}", agent.name)
            } else {
                format!("    {} · {label}{pos}", agent.name)
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

fn print_pane_list(sessions: &[kumo_protocol::SessionInfo], target: &str, tab: Option<u64>) {
    let Some(si) = sessions.iter().position(|s| s.name == target) else {
        println!("no session {target:?}");
        return;
    };
    let s = &sessions[si];
    let mut any = false;
    for (ti, t) in s.tabs.iter().enumerate() {
        if tab.is_some() && tab != Some(t.id) {
            continue;
        }
        if t.panes.is_empty() {
            continue;
        }
        any = true;
        println!("[{}] (id {})", t.name, t.id);
        for (pi, p) in t.panes.iter().enumerate() {
            let mark = if p.active { "* " } else { "  " };
            println!("{mark}pane {} · s{}:t{}:p{} · {}", p.id, si + 1, ti + 1, pi + 1, p.label);
        }
    }
    if !any {
        println!("(no panes)");
    }
}

fn print_agent_status(agents: &[kumo_protocol::AgentStatusLine]) {
    for a in agents {
        let pos = if a.tab_index > 0 && a.pane_index > 0 {
            format!(" · {}:t{}:p{}", a.session, a.tab_index, a.pane_index)
        } else {
            String::new()
        };
        println!("{} · {} · {} (pane {}{})", a.session, a.name, a.status.label(), a.pane_id, pos);
    }
    if agents.is_empty() {
        println!("(no agents running)");
    }
}

fn print_agent_explain(r: &kumo_protocol::AgentExplainReport) {
    let color = io::stdout().is_terminal();
    let label = r.status.label();
    let status = if color {
        let (r_, g, b) = kumo_core::theme::agent_status_color(r.status);
        format!("\x1b[38;2;{r_};{g};{b}m{label}\x1b[0m")
    } else {
        label.to_string()
    };
    let pos = if r.tab_index > 0 && r.pane_index > 0 {
        format!(" ({}:t{}:p{})", r.session, r.tab_index, r.pane_index)
    } else {
        String::new()
    };
    println!("pane {}{} · {} · {}", r.pane_id, pos, r.cli, status);
    println!(
        "  session: {}    {}    os pid: {}    dead: {}",
        r.session,
        if r.is_ai_cli { "AI CLI" } else { "shell" },
        r.os_pid,
        r.dead
    );
    match r.prev_status {
        Some(prev) => println!(
            "  raw: {}    displayed: {}    prev observed: {}    focused: {}",
            r.raw_status.label(),
            r.status.label(),
            prev.label(),
            r.focused
        ),
        None => println!(
            "  raw: {}    displayed: {}    prev observed: none    focused: {}",
            r.raw_status.label(),
            r.status.label(),
            r.focused
        ),
    }
    println!(
        "  output silence: {} ms    cpu: {:.1}%    mem: {} KiB",
        r.last_output_age_ms, r.cpu, r.mem_kb
    );
    println!("  precedence: {}", r.precedence);
    println!("  reason: {}", r.idle_reason.label());
    if r.markers.is_empty() {
        println!("  no markers matched");
    } else {
        for m in &r.markers {
            println!("  marker: {} · {} · in {}", m.agent, m.marker, m.region.label());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_protocol::{AgentInfo, AgentStatus, PaneInfo};

    fn fixture_session(name: &str, panes: &[u64]) -> kumo_protocol::SessionInfo {
        kumo_protocol::SessionInfo {
            name: name.to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            tab_count: 1,
            pane_count: panes.len(),
            zoomed: false,
            active: name == "kumo",
            active_tab: Some("1".to_string()),
            focus: panes.first().copied(),
            tabs: vec![kumo_protocol::TabInfo {
                id: 7,
                name: "1".to_string(),
                pane_count: panes.len(),
                zoomed: false,
                active: true,
                focus: panes.first().copied(),
                panes: panes
                    .iter()
                    .enumerate()
                    .map(|(i, id)| PaneInfo { id: *id, label: format!("p{}", i + 1), active: i == 0 })
                    .collect(),
            }],
            agents: vec![AgentInfo {
                name: "opencode".into(),
                status: AgentStatus::Idle,
                cpu: 0.0,
                mem_kb: 0,
                pane_id: panes[0],
                pane_index: 1,
                tab_index: 1,
            }],
        }
    }

    #[test]
    fn composite_spec_parses_all_forms() {
        let s = CompositeSpec::parse("s1:t2:p1").unwrap();
        assert_eq!(s.session_index, Some(1));
        assert_eq!(s.tab, Some(2));
        assert_eq!(s.pane, Some(1));

        let s = CompositeSpec::parse("kumo:t2:p1").unwrap();
        assert_eq!(s.session.as_deref(), Some("kumo"));
        assert_eq!(s.tab, Some(2));

        let s = CompositeSpec::parse("2:1").unwrap();
        assert_eq!(s.session, None);
        assert_eq!(s.tab, Some(2));
        assert_eq!(s.pane, Some(1));

        let s = CompositeSpec::parse("t2").unwrap();
        assert_eq!(s.tab, Some(2));
        assert_eq!(s.pane, None);

        let s = CompositeSpec::parse("3:2:1").unwrap();
        assert_eq!(s.session_index, Some(3));
        assert_eq!(s.tab, Some(2));
        assert_eq!(s.pane, Some(1));
    }

    #[test]
    fn composite_spec_rejects_garbage() {
        assert!(CompositeSpec::parse("0:1").is_err(), "indexes are 1-based");
        assert!(CompositeSpec::parse("a:b").is_err(), "session name needs t:p");
        assert!(CompositeSpec::parse(":2:1").is_err(), "empty session");
        assert!(CompositeSpec::parse("1:2:3:4").is_err());
        assert!(CompositeSpec::parse("").is_err());
    }

    #[test]
    fn composite_spec_keeps_s_leading_session_names() {
        // A session named "session-1" must not lose its leading `s` to the
        // optional `s` label (regression: it parsed as "ession-1").
        let s = CompositeSpec::parse("session-1:t2:p3").unwrap();
        assert_eq!(s.session.as_deref(), Some("session-1"));
        assert_eq!(s.session_index, None);
        assert_eq!(s.tab, Some(2));
        assert_eq!(s.pane, Some(3));
        // The `s` label still works: `s2` = session index 2.
        let s = CompositeSpec::parse("s2:t1:p1").unwrap();
        assert_eq!(s.session_index, Some(2));
        assert_eq!(s.session, None);
        assert_eq!(s.tab, Some(1));
    }

    #[test]
    fn resolve_spec_pane_by_name_and_index() {
        let sessions = vec![fixture_session("kumo", &[10, 11]), fixture_session("two", &[20])];
        let spec = CompositeSpec::parse("kumo:t1:p2").unwrap();
        let (id, display) = resolve_spec_pane(&sessions, &spec, "kumo").unwrap();
        assert_eq!(id, 11);
        assert_eq!(display, "kumo:t1:p2");

        // Session index form (1-based into the session list).
        let spec = CompositeSpec::parse("2:1:1").unwrap();
        let (id, _) = resolve_spec_pane(&sessions, &spec, "kumo").unwrap();
        assert_eq!(id, 20);

        // Tab:pane form inherits the default session.
        let spec = CompositeSpec::parse("t1:p1").unwrap();
        let (id, _) = resolve_spec_pane(&sessions, &spec, "two").unwrap();
        assert_eq!(id, 20);
    }

    #[test]
    fn resolve_spec_pane_reports_oob_precisely() {
        let sessions = vec![fixture_session("kumo", &[10, 11])];
        assert!(resolve_spec_pane(&sessions, &CompositeSpec::parse("kumo:t1:p3").unwrap(), "kumo").is_err());
        assert!(resolve_spec_pane(&sessions, &CompositeSpec::parse("kumo:t2:p1").unwrap(), "kumo").is_err());
        assert!(resolve_spec_pane(&sessions, &CompositeSpec::parse("nope:t1:p1").unwrap(), "kumo").is_err());
    }
}
