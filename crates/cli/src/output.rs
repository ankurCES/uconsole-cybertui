//! Output formatting for the CLI. Mirrors herdr's two-mode design:
//! human-readable tables / messages by default, `--json` for scripts.

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

impl OutputMode {
    pub fn is_json(&self) -> bool {
        matches!(self, OutputMode::Json)
    }
}

/// Print a single serializable value. In Human mode, JSON-serialise it and
/// pretty-print; in Json mode, emit compact JSON.
pub fn print<T: Serialize>(mode: OutputMode, value: &T) -> Result<()> {
    match mode {
        OutputMode::Json => {
            let s = serde_json::to_string(value).context("serialising JSON")?;
            println!("{s}");
        }
        OutputMode::Human => {
            let s = serde_json::to_string_pretty(value).context("serialising JSON")?;
            println!("{s}");
        }
    }
    Ok(())
}

/// Print a list of records as a human-readable table. In Json mode this
/// just serialises the slice as a JSON array.
pub fn print_table<T: Serialize>(mode: OutputMode, rows: &[T]) -> Result<()> {
    print(mode, &rows)
}

/// Confirmation message (Human) or {"ok": true} (Json).
pub fn print_ok(mode: OutputMode, ok: bool) -> Result<()> {
    if mode.is_json() {
        print(mode, &serde_json::json!({ "ok": ok }))
    } else {
        if ok {
            println!("OK");
        } else {
            println!("FAILED");
        }
        Ok(())
    }
}
