//! Local socket path resolution. Linux/macOS use a Unix domain socket at
//! `$XDG_RUNTIME_DIR/cyberdeck.sock` (falling back to `/tmp/cyberdeck.sock`).
//! Windows uses a named pipe `\\.\pipe\cyberdeck`. The CLI and the TUI
//! use the same helper so they always agree on the address.

use std::path::PathBuf;

#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok();
    resolve(xdg.as_deref())
}

#[cfg(windows)]
pub fn socket_path() -> PathBuf {
    PathBuf::from(r"\\.\pipe\cyberdeck")
}

/// Pure helper: resolves a socket path given an optional XDG override string.
///
/// On Unix: returns `<xdg>/cyberdeck.sock` if `xdg` is `Some(non-empty)`,
/// otherwise `/tmp/cyberdeck.sock`.
///
/// On Windows: always returns `\\.\pipe\cyberdeck` (the `xdg` argument is
/// ignored on Windows).
pub fn resolve(xdg: Option<&str>) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = xdg;
        return PathBuf::from(r"\\.\pipe\cyberdeck");
    }
    #[cfg(unix)]
    {
        if let Some(dir) = xdg {
            if !dir.is_empty() {
                return PathBuf::from(dir).join("cyberdeck.sock");
            }
        }
        PathBuf::from("/tmp/cyberdeck.sock")
    }
}

/// `cyberdeck daemon start` writes this file alongside the socket so the
/// CLI can verify the running daemon's PID without stat-ing the socket.
pub fn pidfile_path() -> PathBuf {
    let mut p = socket_path();
    p.set_extension("pid");
    p
}

/// Returns the string form of the socket path for display / logging.
pub fn display() -> String {
    socket_path().display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_nonempty() {
        // Whatever the platform, the path must be non-empty so the CLI
        // never hands back a zero-length connect target.
        assert!(!socket_path().as_os_str().is_empty());
    }

    #[test]
    fn pidfile_is_socket_with_different_extension() {
        let sock = socket_path();
        let pid = pidfile_path();
        assert_eq!(sock.parent(), pid.parent());
        assert_ne!(sock.extension(), pid.extension());
    }

    #[test]
    #[cfg(unix)]
    fn resolve_uses_xdg_override_when_set() {
        let p = resolve(Some("/run/user/1000"));
        assert_eq!(p, PathBuf::from("/run/user/1000/cyberdeck.sock"));
    }

    #[test]
    #[cfg(unix)]
    fn resolve_falls_back_when_xdg_unset() {
        let p = resolve(None);
        assert_eq!(p, PathBuf::from("/tmp/cyberdeck.sock"));
    }

    #[test]
    #[cfg(unix)]
    fn resolve_falls_back_when_xdg_is_empty() {
        // Some implementations or test fixtures may set XDG_RUNTIME_DIR=""
        // to signal "not configured" — treat that the same as None.
        let p = resolve(Some(""));
        assert_eq!(p, PathBuf::from("/tmp/cyberdeck.sock"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_path_is_named_pipe() {
        assert_eq!(socket_path(), PathBuf::from(r"\\.\pipe\cyberdeck"));
        assert_eq!(
            resolve(Some("ignored")),
            PathBuf::from(r"\\.\pipe\cyberdeck")
        );
    }
}