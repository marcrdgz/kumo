mod daemon;
mod cli;

use anyhow::Result;
use std::path::PathBuf;

use kumo_core::update;
use kumo_core::Launch;

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
    // Headless daemon: `kumo daemon [--resume <file>] [WORKSPACE]`. Spawned
    // detached by the TUI client and the desktop app; also available to start
    // manually. `--resume <file>` makes it adopt the live PTY masters
    // inherited from a `kumo update` restart.
    if args.first().map(|s| s.as_str()) == Some("daemon") {
        return daemon::run(&args[1..]);
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

    // Control CLI: `kumo session|pane|agent ...` (and the legacy aliases
    // `ls`/`kill`/`reload`/`server restart`).
    match args.first().map(|s| s.as_str()) {
        Some("session") | Some("pane") | Some("agent") | Some("ls") | Some("list")
        | Some("kill") | Some("reload") | Some("server") => {
            #[cfg(unix)]
            {
                return cli::cli::run(&args);
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("kumo requires a unix daemon");
            }
        }
        _ => {}
    }

    // tmux-style launch: `kumo` attaches to the daemon if present (else starts
    // one), `kumo attach` requires a running daemon, `kumo new [dir]` starts a
    // fresh session. Bare `kumo [WORKSPACE]` is a shorthand for `kumo new [WORKSPACE]`
    // but only when the argument is an existing directory; otherwise it is an
    // unknown command (prevents `kumo lits` typo silently creating a session).
    let launch = match args.first().map(|s| s.as_str()) {
        Some("attach") => {
            if args.len() > 1 {
                anyhow::bail!("kumo attach takes no arguments");
            }
            Launch::Attach
        }
        Some("new") => {
            match args.get(1) {
                Some(dir) if dir.starts_with('-') => {
                    anyhow::bail!("unknown option for kumo new: {dir} (expected [WORKSPACE])");
                }
                Some(dir) => {
                    let p = PathBuf::from(dir);
                    if args.len() > 2 {
                        anyhow::bail!("kumo new takes at most one directory");
                    }
                    if !p.is_dir() {
                        anyhow::bail!("no such directory: {}", p.display());
                    }
                    Launch::New(Some(p))
                }
                None => Launch::New(None),
            }
        }
        Some(ws) if !ws.starts_with('-') => {
            let p = PathBuf::from(ws);
            if p.is_dir() {
                if args.len() > 1 {
                    anyhow::bail!("kumo [WORKSPACE] takes at most one argument");
                }
                Launch::New(Some(p))
            } else {
                anyhow::bail!("unknown command: {ws} (see `kumo --help`)");
            }
        }
        Some(arg) if arg.starts_with('-') => {
            anyhow::bail!("unknown option: {arg} (see `kumo --help`)");
        }
        Some(other) => {
            anyhow::bail!("unknown command: {other} (see `kumo --help`)");
        }
        None => Launch::Auto,
    };

    #[cfg(unix)]
    {
        cli::client::run(launch)
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("kumo requires a unix daemon");
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
    println!("    kumo daemon [WORKSPACE]    run the headless daemon in the foreground");
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
    println!("The daemon (`kumo daemon`) runs in the background and owns your panes;");
    println!("the TUI is a client to it, so several terminals and the desktop app can");
    println!("attach at once.");
}
