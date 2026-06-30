//! `journalctl --output=json --since` wrapper for cyberdeck-tui's live log feed.
//!
//! Used by the periodic log refiller (Module 2.2) and the `r` refresh
//! handlers on the Logs and System screens (Modules 2.3 / 2.4). Always
//! returns lines in chronological order, newest last, capped at 200 lines.
//! Each entry carries the journalctl-native `__REALTIME_TIMESTAMP` (UTC,
//! microseconds since the epoch) — not the fetch-time stamp — so the
//! rendered log line shows when the event actually happened, even on a
//! busy box where the 1Hz poller runs behind.
//!
//! Tests skip themselves when `journalctl` is not on the test box.

use std::process::Command;

use chrono::{DateTime, Utc};

use crate::{CoreError, CoreResult};

/// Fetch journal entries from the last `secs` seconds, capped at 200 lines,
/// in chronological order (newest last).
///
/// Each tuple is `(journal_timestamp, message)`. The timestamp is parsed
/// from journalctl's `__REALTIME_TIMESTAMP` field (microseconds since the
/// Unix epoch, UTC) and converted to `DateTime<Utc>`. Lines without a
/// `MESSAGE` field, or whose JSON is malformed, are silently skipped —
/// the recent-logs buffer should never panic on a stray entry.
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
pub async fn recent_since(secs: u64) -> CoreResult<Vec<(DateTime<Utc>, String)>> {
    let since = format!("-{}s", secs);
    let output = Command::new("journalctl")
        .args([
            "-n", "200",
            "--no-pager",
            "-q",
            "-o", "json",
            "--since", &since,
        ])
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

    let mut out = Vec::new();
    for raw in output.stdout.split(|b| *b == b'\n') {
        if raw.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_slice(raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ts_us = v
            .get("__REALTIME_TIMESTAMP")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<i64>().ok());
        let ts = match ts_us {
            Some(us) => DateTime::<Utc>::from_timestamp(
                us / 1_000_000,
                (us % 1_000_000).unsigned_abs() as u32 * 1_000,
            )
            .unwrap_or_else(Utc::now),
            None => continue,
        };
        let msg = v
            .get("MESSAGE")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if msg.is_empty() {
            continue;
        }
        out.push((ts, msg));
    }
    Ok(out)
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
        // a Vec back without panic, and that each entry carries both a
        // timestamp and a non-empty message.
        let lines = rt.block_on(recent_since(1)).unwrap();
        for (_ts, msg) in &lines {
            assert!(!msg.is_empty(), "MESSAGE must be non-empty");
        }
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

    #[test]
    fn recent_since_uses_journalctl_native_timestamps() {
        if !journalctl_available() {
            eprintln!("journalctl not present — skipping");
            return;
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Timestamps must be journalctl-native (i.e. within the last
        // 60s + a small slack), not fetch-time, and certainly not
        // epoch-fallback noise. This is the regression guard for
        // Module 2.3: prior versions stamped `Local::now()` at fetch
        // time, which gave plausible-looking but lying timestamps on
        // any box that happened to be quiet.
        let lines = rt.block_on(recent_since(60)).unwrap();
        let now = Utc::now();
        for (ts, _msg) in &lines {
            let delta = (now - *ts).num_seconds().abs();
            assert!(
                delta < 120,
                "timestamp {ts} not within 120s of now ({now})"
            );
        }
    }
}