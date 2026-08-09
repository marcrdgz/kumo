mod alert;
mod app;
mod config;
mod keys;
mod layout;
mod pane;
mod pty;
mod update;
mod vt;
mod xtgettcap;

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    let workspace = args.first().map(|s| s.as_str());

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = app::run(&mut terminal, workspace);

    // Restore the terminal regardless of how the app exits.
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture);
    let _ = terminal.show_cursor();
    let _ = terminal.flush();

    result
}
