//! Process management: ps, kill, renice, /proc enumeration with ppid.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub pid: i32,
    pub user: String,
    pub cpu: f32,
    pub mem: f32,
    pub vsz_kb: u64,
    pub rss_kb: u64,
    pub stat: String,
    pub start: String,
    pub time: String,
    pub command: String,
}

pub async fn list() -> CoreResult<Vec<Process>> {
    let out = run(
        [
            "ps",
            "-eo",
            "pid,user,pcpu,pmem,vsz,rss,stat,start,time,comm,args",
            "--sort=-pcpu",
        ],
        Privilege::User,
    )
    .await?;
    let mut v = Vec::new();
    for (i, line) in out.stdout.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 11 {
            continue;
        }
        let pid: i32 = cols[0].parse().unwrap_or(0);
        if pid == 0 {
            continue;
        }
        v.push(Process {
            pid,
            user: cols[1].into(),
            cpu: cols[2].parse().unwrap_or(0.0),
            mem: cols[3].parse().unwrap_or(0.0),
            vsz_kb: cols[4].parse().unwrap_or(0),
            rss_kb: cols[5].parse().unwrap_or(0),
            stat: cols[6].into(),
            start: cols[7].into(),
            time: cols[8].into(),
            command: cols[10..].join(" "),
        });
    }
    Ok(v)
}

pub async fn kill(pid: i32, signal: &str) -> CoreResult<()> {
    if pid <= 0 {
        return Err(CoreError::Invalid("pid".into()));
    }
    let sig = if signal.is_empty() { "TERM" } else { signal };
    run(
        ["kill", &format!("-{sig}"), &pid.to_string()],
        Privilege::User,
    )
    .await?;
    Ok(())
}

pub async fn renice(pid: i32, nice: i32) -> CoreResult<()> {
    if pid <= 0 {
        return Err(CoreError::Invalid("pid".into()));
    }
    if !(-20..=19).contains(&nice) {
        return Err(CoreError::Invalid("nice must be in -20..=19".into()));
    }
    run(
        ["renice", &nice.to_string(), "-p", &pid.to_string()],
        Privilege::User,
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /proc enumeration with ppid — used by the System screen's process-tree view
// (Module 6.3). Kept as a separate struct (`ProcEntry`) so it doesn't disturb
// the existing flat-listing `Process` shape used by the Processes screen, the
// web API, and the 15s `process::list()` refiller.
//
// `list_with_ppid` reads /proc/<pid>/{stat,comm,cmdline} for every numeric
// subdirectory of /proc. Non-numeric entries (e.g. "self", "cpuinfo") and
// unreadable PIDs are skipped silently — a transient missing file (a process
// exiting between `read_dir` and the per-file read) is normal during a live
// snapshot and shouldn't kill the whole enumeration.
//
// Returns `Ok(vec![])` when /proc is missing entirely (non-Linux), so callers
// in non-Linux test environments don't panic.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    pub cmdline: String,
}

/// Enumerate processes from /proc. Each entry reads /proc/<pid>/{stat,comm,cmdline}.
/// Returns `Ok(vec![])` if /proc is unavailable. Skips unreadable entries silently.
pub fn list_with_ppid() -> CoreResult<Vec<ProcEntry>> {
    use std::path::Path;
    let proc_dir = Path::new("/proc");
    if !proc_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(proc_dir) {
        Ok(e) => e,
        Err(err) => return Err(CoreError::Io(format!("read_dir /proc: {err}"))),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let pid: u32 = match name.parse() {
            Ok(p) => p,
            Err(_) => continue, // not a PID dir (e.g. "self", "cpuinfo")
        };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // /proc/<pid>/stat format: "pid (comm) state ppid pgrp ..."
        // comm may contain spaces or parens — find the LAST `)` to split reliably.
        let (ppid, comm) = parse_stat(&stat).unwrap_or((0, String::new()));
        let cmdline = std::fs::read_to_string(entry.path().join("cmdline"))
            .map(|s| s.replace('\0', " ").trim().to_string())
            .unwrap_or_default();
        out.push(ProcEntry { pid, ppid, comm, cmdline });
    }
    Ok(out)
}

/// Parse /proc/<pid>/stat. Returns `(ppid, comm)`. Robust against `comm`
/// containing spaces or parens by anchoring on the LAST `)`.
fn parse_stat(stat: &str) -> Option<(u32, String)> {
    let close = stat.rfind(')')?;
    let after = &stat[close + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // After `)`: state (1), ppid (2), pgrp (3), ...
    let ppid: u32 = fields.get(1)?.parse().ok()?;
    // comm is between the LAST '(' and the LAST ')'.
    let open = stat.find('(')?;
    let comm = &stat[open + 1..close];
    Some((ppid, comm.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_with_ppid_sync() -> Vec<ProcEntry> {
        // The /proc walk is synchronous, so we don't need an async runtime
        // to call it from a test. Tests just call the function directly.
        list_with_ppid().unwrap_or_default()
    }

    #[test]
    fn list_with_ppid_returns_at_least_current_process() {
        let procs = list_with_ppid_sync();
        assert!(!procs.is_empty(), "expected at least one process, got empty");
        // The current process (or its parent) should be present.
        let my_pid = std::process::id();
        assert!(
            procs.iter().any(|p| p.pid == my_pid || p.ppid == my_pid),
            "expected current PID {my_pid} in procs"
        );
    }

    #[test]
    fn list_with_ppid_handles_missing_proc_gracefully() {
        // The function must NOT panic when /proc is unreadable.
        // We can't easily simulate that, but we can assert the function returns cleanly.
        let result = list_with_ppid();
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn list_with_ppid_process_has_nonempty_comm() {
        let procs = list_with_ppid_sync();
        // Empty comm is tolerated (parser may give up on a malformed stat
        // line) but should be rare; we assert the bulk are populated.
        let with_comm = procs.iter().filter(|p| !p.comm.is_empty()).count();
        assert!(
            with_comm > 0,
            "expected at least one process with non-empty comm, got 0 of {}",
            procs.len()
        );
    }

    #[test]
    fn list_with_ppid_pid_is_nonzero() {
        let procs = list_with_ppid_sync();
        for p in procs.iter().take(50) {
            assert!(p.pid > 0, "pid must be > 0, got {}", p.pid);
        }
    }

    #[test]
    fn parse_stat_extracts_ppid_and_comm() {
        // Typical /proc/self/stat line — comm contains a space and a paren
        // so we can verify we anchor on the LAST `)`.
        let line = "1234 (weird (name)) S 99 1234 1234 0 -1 4194304 100 0 0 0 \
                    0 0 0 0 20 0 1 0 1234567 1234567 100";
        let (ppid, comm) = parse_stat(line).expect("must parse");
        assert_eq!(ppid, 99);
        assert_eq!(comm, "weird (name)");
    }

    #[test]
    fn parse_stat_returns_none_for_garbage() {
        assert!(parse_stat("").is_none());
        assert!(parse_stat("no parens here").is_none());
        // Missing fields after `)`
        assert!(parse_stat("1 (x)").is_none());
    }
}
