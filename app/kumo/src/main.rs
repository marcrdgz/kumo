mod daemon;
mod cli;

use anyhow::Result;
use std::path::PathBuf;

use kumo_core::update;
use kumo_core::Launch;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Ensure the agent skill is present for every install (idempotent).
    let _ = kumo_core::skill::ensure_installed();
    // Top-level flags only (per-command help lives in each command's parser).
    if args.first().map(|s| s.as_str()) == Some("-h")
        || args.first().map(|s| s.as_str()) == Some("--help")
    {
        print_help();
        return Ok(());
    }
    if args.first().map(|s| s.as_str()) == Some("-v")
        || args.first().map(|s| s.as_str()) == Some("--version")
    {
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
        if args[1..].iter().any(|a| a == "-h" || a == "--help") {
            print_daemon_help();
            return Ok(());
        }
        return daemon::run(&args[1..]);
    }
    if args.first().map(|s| s.as_str()) == Some("update") {
        if args[1..].iter().any(|a| a == "-h" || a == "--help") {
            update::print_help();
            return Ok(());
        }
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

    // Control CLI: `kumo session|pane|agent|tab|worktree ...` (and the legacy aliases
    // `ls`/`kill`/`reload`/`server restart`).
    match args.first().map(|s| s.as_str()) {
        Some("session") | Some("pane") | Some("agent") | Some("tab") | Some("worktree") | Some("ls")
        | Some("list") | Some("kill") | Some("reload") | Some("server") => {
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
            if args.iter().any(|a| a == "-h" || a == "--help") {
                print_attach_help();
                return Ok(());
            }
            if args.len() > 1 {
                anyhow::bail!("kumo attach takes no arguments");
            }
            Launch::Attach
        }
        Some("new") => {
            if args.iter().any(|a| a == "-h" || a == "--help") {
                print_new_help();
                return Ok(());
            }
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
    println!("    kumo update [-n] [-c] [--tag TAG]");
    println!();
    println!("SESSIONS:");
    println!("    kumo session list");
    println!("    kumo session new [DIR] [--name NAME]");
    println!("    kumo session kill NAME");
    println!("    kumo session attach NAME");
    println!();
    println!("PANES:");
    println!("    kumo pane split [-s SESSION] [--horizontal] [--ai]");
    println!("    kumo pane close [-s SESSION] [-p PANE]");
    println!("    kumo pane focus -p PANE [-s SESSION]");
    println!("    kumo pane send-keys [-s SESSION] [-p PANE] KEYS...");
    println!("    kumo pane list [-s SESSION] [-t TAB_ID] [S1:T2[:P3]]");
    println!("            PANE = pane id or s1:t2:p3 (kumo:t2:p1, t2:p1 with -s)");
    println!("TABS:");
    println!("    kumo tab list [-s SESSION]");
    println!("    kumo tab new [WORKSPACE] [--name NAME] [-s SESSION]");
    println!("    kumo tab focus TAB [-s SESSION]");
    println!("    kumo tab kill TAB [-s SESSION]");
    println!("    kumo tab rename TAB NEW_NAME [-s SESSION]");
    println!();
    println!("AGENTS:");
    println!("    kumo agent spawn [-s SESSION] [PROGRAM]");
    println!("    kumo agent status  (aliases: list, ls)");
    println!("    kumo agent kill -p PANE [-s SESSION]");
    println!("    kumo agent explain [PANE] [-s SESSION]");
    println!();
    println!("WORKTREES:");
    println!("    kumo worktree create [--ai] [NAME] [--branch BRANCH] [--from REF] [--note NOTE] [--agent AGENT] [-s SESSION] [--json]");
    println!("    kumo worktree open PATH [-s SESSION]");
    println!("    kumo worktree rm PATH [--force] [-s SESSION]");
    println!("    kumo worktree set [--path PATH] --comment COMMENT --status STATUS [-s SESSION] [--json]");
    println!("    kumo worktree current [--path PATH] [-s SESSION] [--json]");
    println!("    kumo worktree list [-s SESSION] [--json]");
    println!();
    println!("OTHER:");
    println!("    kumo ls / kill / reload / server restart");
    println!();
    println!("Add -h/--help to any command for its own usage, e.g. `kumo pane -h`.");
    println!("The daemon (`kumo daemon`) runs in the background and owns your panes;");
    println!("the TUI is a client to it, so several terminals and the desktop app can");
    println!("attach at once.");
}

fn print_attach_help() {
    println!("kumo attach — attach to the running daemon");
    println!();
    println!("USAGE:");
    println!("    kumo attach");
    println!();
    println!("Requires a running daemon (`kumo` or `kumo new` starts one if missing).");
}

fn print_new_help() {
    println!("kumo new — start a fresh session");
    println!();
    println!("USAGE:");
    println!("    kumo new [WORKSPACE]");
    println!();
    println!("WORKSPACE is the directory to start in; defaults to the current directory.");
}

fn print_daemon_help() {
    println!("kumo daemon — run the headless daemon in the foreground");
    println!();
    println!("USAGE:");
    println!("    kumo daemon [WORKSPACE]");
    println!("    kumo daemon --resume <file>");
    println!();
    println!("WORKSPACE is the starting directory (the TUI client and desktop app");
    println!("spawn this automatically). `--resume <file>` adopts the live PTY masters");
    println!("inherited from a `kumo update` restart, keeping panes alive.");
}
