//! PTY backend. Thin wrapper around `portable-pty` that exposes the
//! minimum the WM needs:
//!   * `try_read`  — non-blocking read of whatever the child has emitted.
//!   * `write`     — push keystrokes into the child.
//!   * `resize`    — tell the kernel about a new (rows, cols).
//!   * `is_alive` / `try_wait` / `kill` / `exit_status` — lifecycle.
//!
//! Reads are blocking-style but called from a `tokio::task::spawn_blocking`
//! loop in the broadcaster (see `broadcaster()`). The render loop never
//! touches this directly — it subscribes to the broadcaster's channel.

//! Phase-2 module: PTY backend (portable-pty wrapper). Wired up by the
//! pane-grid work in `wm/mod.rs` once we reach the terminal pane milestone
//! (see ROADMAP.md).
#![allow(dead_code)]

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Handle to a spawned child inside a real PTY. Cheap to clone — the inner
/// state is shared behind a `Mutex`.
#[derive(Clone)]
pub struct Pty {
    inner: Arc<Inner>,
}

struct Inner {
    /// Wrapped in a `Mutex` because `MasterPty` is `Send` but not `Sync` —
    /// without the mutex, `Arc<Inner>` isn't `Send` and we can't move a
    /// `Pty` into a `spawn_blocking` worker (e.g. the broadcaster). The
    /// lock is uncontended in practice: the broadcaster worker is the
    /// only thing that ever touches the master, and PTY ops are quick.
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

impl Pty {
    /// Spawn `cmd` (a `CommandBuilder` — argv-style, with env / cwd if set)
    /// inside a fresh PTY of the given size. Returns an error if the pty
    /// can't be opened or the child can't be spawned.
    pub fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> io::Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty: {e}")))?;

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn: {e}")))?;
        // slave is dropped here, which detaches it from the master fd;
        // the master keeps the pty open until the child exits.

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take_writer: {e}")))?;

        Ok(Self {
            inner: Arc::new(Inner {
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                child: Mutex::new(child),
            }),
        })
    }

    /// Clone a fresh readable handle. Each call returns a new handle that
    /// reads from the same pty — used by the broadcaster task. Multiple
    /// readers on a single pty will interleave; v0 only ever has one, so
    /// this is fine.
    pub fn try_clone_reader(&self) -> io::Result<Box<dyn Read + Send>> {
        self.inner
            .master
            .lock()
            .expect("master mutex poisoned")
            .try_clone_reader()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone_reader: {e}")))
    }
    /// Non-blocking read of whatever the child has emitted so far.
    /// Returns `Ok(0)` if there's nothing to read right now (not EOF).
    /// For the real "is the pty done" check, call `is_alive`.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut reader = self.inner.master.lock()
            .expect("master mutex poisoned")
            .try_clone_reader()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone_reader: {e}")))?;
        // We don't have a real "would block" probe from portable-pty; the
        // simplest cross-platform approach is a short blocking read with a
        // small buffer and accept that an idle pty will return WouldBlock
        // once the kernel pipe is drained. For v0 this is fine because
        // we call it from `spawn_blocking` so it can't stall the runtime.
        match reader.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Push bytes into the pty — keystrokes, paste, mouse escapes, etc.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut w = self.inner.writer.lock().expect("writer mutex poisoned");
        w.write_all(bytes)?;
        w.flush()
    }

    /// Tell the kernel (and thus the child) that the visible area changed.
    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        self.inner
            .master
            .lock()
            .expect("master mutex poisoned")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("resize: {e}")))
    }

    /// Has the child exited?
    pub fn is_alive(&self) -> bool {
        let mut c = self.inner.child.lock().expect("child mutex poisoned");
        match c.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }

    /// Block (in this thread) until the child exits. Returns the exit
    /// status. Use from `spawn_blocking` only.
    pub fn wait(&self) -> io::Result<portable_pty::ExitStatus> {
        let mut c = self.inner.child.lock().expect("child mutex poisoned");
        c.wait()
    }

    /// Try once to reap the child without blocking. Returns `Some(status)`
    /// if it has exited, `None` otherwise.
    pub fn try_wait(&self) -> io::Result<Option<portable_pty::ExitStatus>> {
        let mut c = self.inner.child.lock().expect("child mutex poisoned");
        c.try_wait()
    }

    /// Send SIGKILL. The child is reaped lazily on the next `try_wait`.
    pub fn kill(&self) -> io::Result<()> {
        let mut c = self.inner.child.lock().expect("child mutex poisoned");
        c.kill()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_cmd() -> CommandBuilder {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "printf 'hello\\nworld\\n'"]);
        cmd
    }

    #[test]
    fn spawn_and_read() {
        let pty = Pty::spawn(echo_cmd(), 24, 80).expect("spawn");
        // Drain a few times. /bin/sh prints ~12 bytes, so two reads is plenty.
        let mut buf = [0u8; 128];
        let mut got = Vec::new();
        // /bin/sh + printf exits fast; the read loop should drain it within
        // ~200 ms. 50 tries × 10 ms = 500 ms upper bound.
        for _ in 0..50 {
            match pty.try_read(&mut buf) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(n) => {
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(12).any(|w| w == b"hello\nworld\n") {
                        break;
                    }
                }
                Err(e) => panic!("read: {e}"),
            }
        }
        let s = String::from_utf8_lossy(&got);
        assert!(s.contains("hello"), "got: {s:?}");
        assert!(s.contains("world"), "got: {s:?}");

        // Process should have exited by now.
        let status = pty.wait().expect("wait");
        assert!(status.success(), "exit: {status}");
    }

    #[test]
    fn write_and_read_roundtrip() {
        // Run `cat` so we can write into stdin and read back what we sent.
        let cmd = CommandBuilder::new("/bin/cat");
        let pty = Pty::spawn(cmd, 24, 80).expect("spawn");
        // Give cat a moment to start.
        std::thread::sleep(std::time::Duration::from_millis(50));
        pty.write(b"ping\n").expect("write");
        let mut buf = [0u8; 64];
        let mut got = Vec::new();
        for _ in 0..50 {
            match pty.try_read(&mut buf) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(n) => {
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(4).any(|w| w == b"ping") {
                        break;
                    }
                }
                Err(e) => panic!("read: {e}"),
            }
        }
        let s = String::from_utf8_lossy(&got);
        assert!(s.contains("ping"), "got: {s:?}");
        pty.kill().ok();
    }
}
