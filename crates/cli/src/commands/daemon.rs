//! `cyberdeck daemon` — start/stop/inspect the local daemon.

use anyhow::Result;
use clap::Subcommand;

use crate::output::OutputMode;

#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Start the daemon. Use `--background` to detach and return immediately.
    Start {
        #[arg(long)]
        background: bool,
    },
    /// Stop the running daemon.
    Stop,
    /// Liveness probe (cheap; doesn't require a running daemon to verify the binary).
    Ping,
    /// Print whether a daemon is listening and on which socket.
    Status,
}

pub fn run(cmd: DaemonCmd, mode: OutputMode) -> Result<i32> {
    match cmd {
        DaemonCmd::Ping => {
            if mode.is_json() {
                println!("{{\"pong\":true,\"socket\":\"{}\"}}", cyberdeck_daemon::socket::display());
            } else {
                println!("pong");
                println!("socket: {}", cyberdeck_daemon::socket::display());
            }
            Ok(0)
        }
        DaemonCmd::Status => {
            let path = cyberdeck_daemon::socket::display();
            if mode.is_json() {
                println!("{{\"socket\":\"{path}\"}}");
            } else {
                println!("daemon socket: {path}");
                println!("(no live-probe in stub; use `ping` for liveness)");
            }
            Ok(0)
        }
        DaemonCmd::Start { background } => {
            eprintln!(
                "daemon start {} is a no-op in stub mode (Tasks 22-23 wire the real spawn)",
                if background { "(background)" } else { "" }
            );
            Ok(1)
        }
        DaemonCmd::Stop => {
            eprintln!("daemon stop not yet wired (Tasks 22-23)");
            Ok(1)
        }
    }
}
