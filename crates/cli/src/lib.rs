//! `cyberdeck` CLI. A thin adapter over `cyberdeck-daemon`'s RPC envelope:
//! every verb maps 1:1 to a `Method` variant. When the daemon is running
//! the CLI sends a JSON-RPC request to the local socket; otherwise it
//! falls back to calling `cyberdeck-core` directly (mirrors `herdr`'s CLI).

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

pub mod commands;
pub mod output;

use crate::commands::{
    audio::AudioCmd, bluetooth::BluetoothCmd, completion::CompletionCmd, config_cmd::ConfigCmd,
    daemon::DaemonCmd, display::DisplayCmd, logs::LogsCmd, net::NetCmd, packages::PackagesCmd,
    power::PowerCmd, process::ProcessCmd, services::ServicesCmd, storage::StorageCmd,
    system::SystemCmd, update::UpdateCmd, workspace::WorkspaceCmd, wm::WmCmd,
};
use crate::output::OutputMode;

#[derive(Debug, Parser)]
#[command(
    name = "cyberdeck",
    version,
    about = "Cyberdeck control CLI — local sysadmin, in the spirit of herdr."
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of human tables.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
    Net {
        #[command(subcommand)]
        cmd: NetCmd,
    },
    Bluetooth {
        #[command(subcommand)]
        cmd: BluetoothCmd,
    },
    Audio {
        #[command(subcommand)]
        cmd: AudioCmd,
    },
    Display {
        #[command(subcommand)]
        cmd: DisplayCmd,
    },
    Power {
        #[command(subcommand)]
        cmd: PowerCmd,
    },
    Storage {
        #[command(subcommand)]
        cmd: StorageCmd,
    },
    Services {
        #[command(subcommand)]
        cmd: ServicesCmd,
    },
    Packages {
        #[command(subcommand)]
        cmd: PackagesCmd,
    },
    Process {
        #[command(subcommand)]
        cmd: ProcessCmd,
    },
    Logs {
        #[command(subcommand)]
        cmd: LogsCmd,
    },
    Sys {
        #[command(subcommand)]
        cmd: SystemCmd,
    },
    Workspace {
        #[command(subcommand)]
        cmd: WorkspaceCmd,
    },
    Wm {
        #[command(subcommand)]
        cmd: WmCmd,
    },
    Completion {
        #[command(subcommand)]
        cmd: CompletionCmd,
    },
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    Update {
        #[command(subcommand)]
        cmd: UpdateCmd,
    },
}

pub fn run() -> Result<i32> {
    let cli = Cli::parse();
    let mode = if cli.json { OutputMode::Json } else { OutputMode::Human };
    match cli.cmd {
        Cmd::Daemon { cmd } => commands::daemon::run(cmd, mode),
        Cmd::Net { cmd } => commands::net::run(cmd, mode),
        Cmd::Bluetooth { cmd } => commands::bluetooth::run(cmd, mode),
        Cmd::Audio { cmd } => commands::audio::run(cmd, mode),
        Cmd::Display { cmd } => commands::display::run(cmd, mode),
        Cmd::Power { cmd } => commands::power::run(cmd, mode),
        Cmd::Storage { cmd } => commands::storage::run(cmd, mode),
        Cmd::Services { cmd } => commands::services::run(cmd, mode),
        Cmd::Packages { cmd } => commands::packages::run(cmd, mode),
        Cmd::Process { cmd } => commands::process::run(cmd, mode),
        Cmd::Logs { cmd } => commands::logs::run(cmd, mode),
        Cmd::Sys { cmd } => commands::system::run(cmd, mode),
        Cmd::Workspace { cmd } => commands::workspace::run(cmd, mode),
        Cmd::Wm { cmd } => commands::wm::run(cmd, mode),
        Cmd::Completion { cmd } => commands::completion::run(cmd, mode),
        Cmd::Config { cmd } => commands::config_cmd::run(cmd, mode),
        Cmd::Update { cmd } => commands::update::run(cmd, mode),
    }
}

pub fn main() -> Result<i32> {
    // Best-effort tracing init; not fatal if it fails.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
    let rc = match run() {
        Ok(rc) => rc,
        Err(e) => {
            eprintln!("error: {e:#}");
            // Print the cause chain so users can see where it broke.
            for cause in e.chain().skip(1) {
                eprintln!("caused by: {cause}");
            }
            1
        }
    };
    std::process::exit(rc);
    // unreachable but satisfies the type checker
}

/// Convenience helper: read the output of a successful unit verb and print
/// it according to `mode`. Used by every `run()` arm.
pub fn print_ok(mode: OutputMode, value: serde_json::Value) -> Result<()> {
    output::print(mode, &value).context("printing response")
}
