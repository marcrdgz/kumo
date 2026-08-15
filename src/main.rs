mod agents;
mod alert;
mod app;
mod cli;
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

    // Control CLI: `kumo session|pane|agent ...` (and the legacy aliases
    // `ls`/`kill`/`reload`/`server restart`).
    match args.first().map(|s| s.as_str()) {
        Some("session") | Some("pane") | Some("agent") | Some("ls") | Some("list")
        | Some("kill") | Some("reload") | Some("server") => {
            #[cfg(unix)]
            {
                return cli::run(&args);
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("kumo commands need the unix daemon");
            }
        }
        _ => {}
    }

    // tmux-style launch: `kumo` attaches to the daemon if present (else starts
    // one), `kumo attach` requires a running daemon, `kumo new [dir]` starts a
    // fresh session.
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
        anyhow::bail!("kumo needs a unix daemon (the TUI client is unix-only)")
    }
}

fn print_help() {
    println!("kumo {} — terminal multiplexer", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE:");
    println!("    kumo                       attach to the daemon (start it if needed)");
    println!("    kumo attach                attach to the running daemon");
    println!("    kumo new [WORKSPACE]       start a fresh session");
    println!("    kumo [WORKSPACE]           start fresh inside this directory");
    println!();
    println!("SESSIONS:");
    println!("    kumo session list");
    println!("    kumo session new [DIR] [--name NAME]");
    println!("    kumo session kill NAME");
    println!("    kumo session attach NAME");
    println!();
    println!("PANES:");
    println!("    kumo pane split [-s SESSION] [--horizontal] [--ai]");
    println!("    kumo pane close [-s SESSION] [-p PANE_ID]");
    println!("    kumo pane focus -p PANE_ID [-s SESSION]");
    println!("    kumo pane send-keys [-s SESSION] [-p PANE_ID] KEYS...");
    println!();
    println!("AGENTS:");
    println!("    kumo agent spawn [-s SESSION] [PROGRAM]");
    println!("    kumo agent status");
    println!("    kumo agent kill -p PANE_ID [-s SESSION]");
    println!();
    println!("OTHER:");
    println!("    kumo ls / kill / reload / server restart / update");
    println!();
    println!("The daemon runs in the background and owns your panes; the TUI is a");
    println!("client to it, so several terminals and the desktop app can attach at once.");
}
