//! `journalctl --since` wrapper for cyberdeck-tui's live log feed.
//!
//! Used by the periodic log refiller (Module 2.2) and the `r` refresh
//! handlers on the Logs and System screens (Modules 2.3 / 2.4). Always
//! returns lines in chronological order, newest last, capped at 200 lines.
//!
//! Tests skip themselves when `journalctl` is not on the test box.

use std::process::Command;

use crate::{CoreError, CoreResult};

/// Fetch journal entries from the last `secs` seconds, capped at 200 lines,
/// in chronological order (newest last).
///
/// Returns `Err(CoreError::Command { .. })` if `journalctl` exits non-zero
/// (e.g. permission denied) and `Err(CoreError::Io(..))` if the binary
/// cannot be spawned at all. Returns `Ok(vec![])` if journalctl ran but
/// produced no output (quiet box, absurd `--since`, etc.).
///
/// Implementation note: this uses the synchronous `std::process::Command`
/// even though the function is `async`. `journalctl -n 200 --since=-Ns` is
/// normally sub-100 ms, and the call sites are already on a periodic
/// poll / explicit refresh — not in a hot path. Switching to
/// `tokio::task::spawn_blocking` would be the right move if this ever
/// becomes latency-sensitive.
pub async fn recent_since(secs: u64) -> CoreResult<Vec<String>> {
    let since = format!("-{}s", secs);
    let output = Command::new("journalctl")
        .args(["-n", "200", "--no-pager", "-q", "--since", &since])
        .output()
        .map_err(|e| CoreError::Io(format!("journalctl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit status {}", output.status.code().unwrap_or(-1))
        } else {
            stderr
        };
        return Err(CoreError::Command {
            cmd: "journalctl".into(),
            detail,
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journalctl_available() -> bool {
        Command::new("which")
            .arg("journalctl")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn recent_since_returns_recent_lines() {
        if !journalctl_available() {
            eprintln!("journalctl not present — skipping");
            return;
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Last 1s of journal — likely empty on a quiet box; assert we got
        // a Vec back without panic.
        let lines = rt.block_on(recent_since(1)).unwrap();
        let _ = lines;
    }

    #[test]
    fn recent_since_handles_absurd_since_without_panic() {
        if !journalctl_available() {
            eprintln!("journalctl not present — skipping");
            return;
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        // 1 year of journal — should not panic. May succeed with a huge
        // Vec or be capped at 200 by `-n 200`. Either way, contract is
        // "Ok, no panic".
        let res = rt.block_on(recent_since(60 * 60 * 24 * 365));
        assert!(res.is_ok(), "recent_since must not panic on large --since");
        let lines = res.unwrap();
        assert!(lines.len() <= 200, "-n 200 cap must hold");
    }
}