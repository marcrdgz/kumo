mod agents;
mod alert;
mod app;
#[cfg(unix)]
mod client;
mod config;
mod frames;
mod keys;
mod layout;
mod pane;
mod protocol;
mod pty;
mod state;
mod theme;
mod update;
mod vt;
mod xtgettcap;

use anyhow::Result;
use std::path::PathBuf;

use crate::app::Launch;

#[cfg(not(unix))]
use {
    crossterm::event::{DisableMouseCapture, EnableMouseCapture},
    crossterm::execute,
    crossterm::terminal::{enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ratatui::backend::CrosstermBackend,
    ratatui::Terminal,
    std::io::stdout,
};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!(
            "kumo {} ({})",
            env!("CARGO_PKG_VERSION"),
            update::current_channel_label()
        );
        return Ok(());
    }
    if args.first().map(|s| s.as_str()) == Some("update") {
        let opts = update::parse_args(&args[1..])?;
        match update::update(&opts) {
            Ok(outcome) => {
                if opts.check && outcome == update::Outcome::Available {
                    std::process::exit(1);
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("kumo update: {e:#}");
                std::process::exit(1);
            }
        }
    }
    if args.first().map(|s| s.as_str()) == Some("daemon") {
        // Hidden subcommand: run the headless daemon. Spawned detached by the
        // client; also available to start manually. `--resume <file>` makes it
        // adopt the live PTY masters inherited from a `kumo update` restart.
        let mut workspace = args.get(1).map(PathBuf::from);
        let mut resume = None;
        if workspace.as_deref().and_then(|w| w.to_str()) == Some("--resume") {
            resume = args.get(2).map(PathBuf::from);
            workspace = None;
        }
        #[cfg(unix)]
        {
            let launch = match resume {
                Some(path) => app::Launch::Resume(path),
                None => app::Launch::New(workspace),
            };
            return app::server::run_daemon(launch);
        }
        #[cfg(not(unix))]
        {
            let _ = (workspace, resume);
            anyhow::bail!("the kumo daemon is unix-only for now");
        }
    }

    match args.first().map(|s| s.as_str()) {
        Some("ls") => {
            #[cfg(unix)]
            {
                return client::list_sessions();
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("`kumo ls` needs the unix daemon");
            }
        }
        Some("kill") => {
            #[cfg(unix)]
            {
                return client::kill_server();
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("`kumo kill` needs the unix daemon");
            }
        }
        Some("reload") => {
            #[cfg(unix)]
            {
                return client::reload();
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("`kumo reload` needs the unix daemon");
            }
        }
        Some("server") => {
            #[cfg(unix)]
            {
                match args.get(1).map(|s| s.as_str()) {
                    Some("restart") => return client::server_restart(),
                    Some(other) => anyhow::bail!(
                        "unknown kumo server subcommand {other:?} (try `kumo server restart`)"
                    ),
                    None => anyhow::bail!("missing kumo server subcommand (try `kumo server restart`)"),
                }
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("`kumo server restart` needs the unix daemon");
            }
        }
        _ => {}
    }

    // tmux-style launch: `kumo` attaches to the daemon if present (else starts
    // one), `kumo attach` requires a running daemon, `kumo new [dir]` and the
    // back-compat `kumo [dir]` start fresh.
    let launch = match args.first().map(|s| s.as_str()) {
        Some("attach") => Launch::Attach,
        Some("new") => Launch::New(args.get(1).map(PathBuf::from)),
        Some(ws) if !ws.starts_with('-') => Launch::New(Some(PathBuf::from(ws))),
        _ => Launch::Auto,
    };

    #[cfg(unix)]
    {
        client::run(launch)
    }
    #[cfg(not(unix))]
    {
        run_foreground(launch)
    }
}

/// Foreground TUI (non-unix fallback until daemon parity lands).
#[cfg(not(unix))]
fn run_foreground(launch: Launch) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, crossterm::event::EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = app::run(&mut terminal, launch);

    // Restore the terminal regardless of how the app exits.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, crossterm::event::DisableBracketedPaste);
    let _ = terminal.show_cursor();
    let _ = terminal.flush();

    result
}

fn print_help() {
    println!("kumo {} — terminal multiplexer", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE:");
    println!("    kumo                       attach to the daemon (start it if needed)");
    println!("    kumo attach                attach to the running daemon");
    println!("    kumo new [WORKSPACE]       start a fresh session");
    println!("    kumo [WORKSPACE]           start fresh inside this directory");
    println!("    kumo ls                    list the daemon's sessions");
    println!("    kumo kill                  stop the daemon (and its panes)");
    println!("    kumo reload                re-read the config and apply it live");
    println!("    kumo server restart        restart the daemon in place (panes survive)");
    println!("    kumo update [--nightly] [--check]");
    println!();
    println!("ARGS:");
    println!("    [WORKSPACE]    Start kumo inside this directory");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help     Print help information");
    println!("    -v, --version  Print version and channel (stable / nightly / dev)");
    println!();
    println!("COMMANDS:");
    println!("    attach         Attach a terminal to the daemon (no daemon = error)");
    println!("    new            Start a fresh session in the daemon");
    println!("    ls             List the daemon's sessions");
    println!("    kill           Stop the daemon (kills its panes)");
    println!("    reload         Re-read the config and apply it live");
    println!("    server restart Restart the daemon in place (execs the current binary, panes survive)");
    println!("    update         Update to the latest release (needs gh)");
    println!("                   --nightly  update to the latest nightly build");
    println!("                   --check    report availability (exit 0 = up to date, 1 = update)");
    println!();
    println!("The daemon runs in the background and owns your panes; `leader+d`");
    println!("detaches this terminal, leaving everything running. Re-attach with `kumo`.");
}
