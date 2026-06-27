//! Safe async wrapper around `tokio::process::Command`.
//!
//! Centralises:
//!   * a uniform 30 s timeout (overridable per call),
//!   * stderr capture and folding into [`CoreError::Command`],
//!   * opt-in privilege escalation via `pkexec` so the same code path works
//!     for root and unprivileged users (apt, systemctl, mount, etc.),
//!   * tracing at DEBUG with the full argv.

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use crate::{CoreError, CoreResult};

/// Default per-call timeout. Most `nmcli`/`systemctl` calls are sub-second;
/// 30 s gives slow `apt update` and `journalctl` tails plenty of room.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Captured output of a finished child process.
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

/// Privilege handling for an external command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    /// Run as the current user (the usual case).
    User,
    /// Prefix the command with `pkexec` so the user is prompted graphically.
    /// This is the safe default for anything that writes to /etc or talks to
    /// systemd. No-op if the process is already root.
    Sudo,
}

/// Run `argv` and wait for it to complete.
///
/// `argv` must be a non-empty slice; the first element is the program. We do
/// not use a shell, so no quoting issues and no injection from user-typed
/// strings — but callers must still validate inputs (e.g. SSIDs, PIDs) before
/// they end up here.
pub async fn run<I, S>(argv: I, privilege: Privilege) -> CoreResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_with_timeout(argv, privilege, DEFAULT_TIMEOUT).await
}

pub async fn run_with_timeout<I, S>(
    argv: I,
    privilege: Privilege,
    dur: Duration,
) -> CoreResult<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parts: Vec<String> = argv.into_iter().map(|s| s.as_ref().to_string()).collect();
    if parts.is_empty() {
        return Err(CoreError::Invalid("empty argv".into()));
    }

    // Detect root via /proc — avoids a libc FFI dependency for one call.
    let already_root = tokio::fs::read_to_string("/proc/self/status")
        .await
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u32>().ok())
        })
        .map(|uid| uid == 0)
        .unwrap_or(false);
    if matches!(privilege, Privilege::Sudo) && !already_root {
        parts.insert(0, "pkexec".to_string());
    }

    let program = parts.remove(0);
    let args = parts;

    tracing::debug!(program = %program, args = ?args, "shell::run");

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let label = format!("{} {}", program, args.join(" "));

    let fut = async {
        let out = cmd.output().await.map_err(CoreError::from)?;
        Ok::<Output, CoreError>(Output {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            status: out.status.code().unwrap_or(-1),
        })
    };

    match timeout(dur, fut).await {
        Ok(Ok(out)) if out.success() => Ok(out),
        Ok(Ok(out)) => Err(CoreError::Command {
            cmd: label,
            detail: if out.stderr.trim().is_empty() {
                format!("exit status {}", out.status)
            } else {
                out.stderr.trim().to_string()
            },
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(CoreError::Timeout {
            cmd: label,
            secs: dur.as_secs(),
        }),
    }
}

/// Read a single file under /sys or /proc and trim trailing whitespace.
pub async fn read_sysfs(path: &str) -> CoreResult<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(s) => Ok(s.trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CoreError::NotFound(path.into())),
        Err(e) => Err(e.into()),
    }
}
