//! cyberdeck-tui — entry point.
//!
//! Responsibilities:
//!   1. Initialise tracing (env-filter, `RUST_LOG=debug` is friendly).
//!   2. Parse CLI flags (`--web` to also host the LAN server).
//!   3. Bring up the tokio runtime, the App, and the live data refreshers.
//!   4. Drive the ratatui event/render loop until the user quits or Ctrl-C.

use cyberdeck_tui::app;
use std::io::stdout;
use std::path::PathBuf;

use anyhow::Context;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

#[derive(Debug, Default)]
struct Args {
    web: bool,
    web_bind: Option<String>,
    config: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--web" => a.web = true,
            "--web-bind" => a.web_bind = it.next(),
            "--config" => a.config = it.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("cyberdeck-tui — a rich TUI for OS-level control on a cyberdeck.\n\n\
                          USAGE: cyberdeck-tui [OPTIONS]\n\n\
                          OPTIONS:\n  \
                            --web            Also start the LAN web server (default 127.0.0.1:7878)\n  \
                            --web-bind ADDR  Bind address for the web server (e.g. 127.0.0.1:9000)\n  \
                            --config PATH    Path to a config file (optional)\n  \
                            -h, --help       Show this help\n");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    a
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    );
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // Set up panic hook so the terminal is restored even on panic.
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        orig_hook(info);
    }));

    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .context("enter alt screen")?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend).context("init terminal")?;

    // --web / --web-bind flags are wired through run_v2 in a future story.
    let _ = &args;

    let res = app::run::run_v2(&mut terminal).await;

    restore_terminal();
    terminal.show_cursor().ok();
    if let Err(e) = res {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}
