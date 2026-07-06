//! CLI stub for the logs subcommand. Real verb dispatch lands in a
//! follow-up commit (Tasks 10-13). For now, every variant prints a hint.

use anyhow::Result;
use clap::Subcommand;

use crate::output::OutputMode;

#[derive(Debug, Subcommand)]
pub enum LogsCmd {
    List,
    Status,
}

pub fn run(_cmd: LogsCmd, _mode: OutputMode) -> Result<i32> {
    println!("logs: not yet wired (Tasks 10-13)");
    Ok(0)
}
