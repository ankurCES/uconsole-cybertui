//! CLI stub for the update subcommand. Real verb dispatch lands in a
//! follow-up commit (Tasks 10-13). For now, every variant prints a hint.

use anyhow::Result;
use clap::Subcommand;

use crate::output::OutputMode;

#[derive(Debug, Subcommand)]
pub enum UpdateCmd {
    List,
    Status,
}

pub fn run(_cmd: UpdateCmd, _mode: OutputMode) -> Result<i32> {
    println!("update: not yet wired (Tasks 10-13)");
    Ok(0)
}
