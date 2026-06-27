//! Glue between a [`Pty`] and the rest of the WM.
//!
//! Every external pane owns one `PaneOutput` + one `PtyWriter` pair, created
//! by [`spawn`]:
//!
//! * `PaneOutput::subscribe` returns a `tokio::sync::broadcast::Receiver`
//!   the renderer pulls raw bytes from. Each chunk is fed through an
//!   `AnsiParser` to paint the grid.
//! * `PtyWriter::send` hands bytes (keystrokes, paste, mouse escapes) to a
//!   `tokio::sync::mpsc` channel that the blocking worker drains into the
//!   PTY master. Using `tokio::mpsc` (not `crossbeam_channel`) so the
//!   sender doesn't need any unsafe code: `crossbeam_channel::Sender` is
//!   `Send` but the channel itself isn't `Sync`, so a `Send + 'static`
//!   closure can't capture one. Tokio's mpsc works fine here.
//!
//! Both directions share a single [`tokio::task::spawn_blocking`] worker
//! because [`Pty`]'s `MasterPty` is `Send` but not `Sync` — you can't have
//! two threads calling methods on the same `Pty` concurrently. The blocking
//! worker selects between read and write (channel + read-with-timeout) so
//! an idle pane doesn't peg a CPU and keystrokes don't get stuck behind a
//! long read.

//! Phase-2 module: PTY ↔ WM glue (broadcaster). Wired up when panes land
//! (see ROADMAP.md).
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::wm::pty::Pty;

/// Monotonic per-pane ID. Used by the WM tree so a `PaneId` stays unique
/// even after panes close and new ones reuse the same slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

impl PaneId {
    pub(crate) fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Broadcast handle for one pane's raw output. Cheap to clone; the inner
/// `Sender` is itself an `Arc`.
#[derive(Clone)]
pub struct PaneOutput {
    id: PaneId,
    tx: broadcast::Sender<Vec<u8>>,
}

impl PaneOutput {
    /// New receiver for this pane's output stream. Receivers that lag more
    /// than `capacity` chunks will see a `RecvError::Lagged` — the renderer
    /// must handle that and re-paint from the latest grid.
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.tx.subscribe()
    }

    /// Pane ID. Stable for the life of the pane.
    pub fn id(&self) -> PaneId { self.id }

    /// Push a chunk directly into the broadcast — used by tests and by
    /// future features (e.g. remote PTY, replay). The read loop does NOT
    /// call this; it uses `tx` directly.
    #[allow(dead_code)] // used by tests
    pub(crate) fn emit(&self, chunk: Vec<u8>) {
        let _ = self.tx.send(chunk);
    }
}

/// Clone-able handle for sending bytes back to the PTY. The renderer's
/// input pipeline keeps one of these.
#[derive(Clone)]
pub struct PtyWriter {
    tx: mpsc::Sender<Vec<u8>>,
}

impl PtyWriter {
    /// Queue `bytes` for the writer task to feed into the PTY. Returns
    /// `Err` only if the worker task has been dropped (i.e. pane killed).
    pub async fn send(&self, bytes: Vec<u8>) -> Result<(), mpsc::error::SendError<Vec<u8>>> {
        self.tx.send(bytes).await
    }

    /// Convenience for byte literals — caller already has a slice.
    pub async fn send_slice(&self, bytes: &[u8]) -> Result<(), mpsc::error::SendError<Vec<u8>>> {
        self.tx.send(bytes.to_vec()).await
    }

    /// Try-once variant for the hot path where the caller can't await.
    pub fn try_send(&self, bytes: Vec<u8>) -> Result<(), mpsc::error::TrySendError<Vec<u8>>> {
        self.tx.try_send(bytes)
    }
}

/// Handle to the background task spawned for a pane. Only one — both read
/// and write are multiplexed onto a single blocking worker.
pub struct PaneTasks {
    /// Abort this to kill the pane.
    pub worker: JoinHandle<()>,
}

/// Spawn the read+write worker for `pty`. Returns the broadcast output
/// handle, the writer, and the task handle.
///
/// Capacity choices:
/// * `broadcast` is 256 chunks — at 4 KiB each, that's 1 MiB of in-flight
///   output. Plenty for any single subscriber; lag will only matter if a
///   subscriber stalls for seconds at a time.
/// * `tokio::mpsc` (input) is 64 chunks — 64 keystroke batches of pending
///   input, which is far more than a human can type ahead.
pub fn spawn(pty: Pty) -> (PaneOutput, PtyWriter, PaneTasks) {
    let (tx, _) = broadcast::channel::<Vec<u8>>(256);
    let out = PaneOutput { id: PaneId::fresh(), tx: tx.clone() };
    let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(64);
    let writer = PtyWriter { tx: writer_tx };

    let worker = {
        let tx = tx.clone();
        tokio::task::spawn_blocking(move || worker_loop(pty, tx, &mut writer_rx))
    };
    (out, writer, PaneTasks { worker })
}

/// Worker loop. Holds the single `Pty` exclusively. Reads whatever the
/// child has emitted and broadcasts it. Drains pending keystrokes from
/// the mpsc every iteration. Exits when:
///   * the child has died AND the pipe is drained, OR
///   * the mpsc is closed (all writers dropped — pane was killed).
fn worker_loop(
    pty: Pty,
    tx: broadcast::Sender<Vec<u8>>,
    writer_rx: &mut mpsc::Receiver<Vec<u8>>,
) {
    let mut buf = [0u8; 4096];
    // Idle sleep so an idle pty doesn't peg a CPU. 5 ms ≈ 200 Hz.
    let idle = Duration::from_millis(5);

    loop {
        // 1. Drain any pending keystrokes first. Non-blocking; if there's
        //    nothing to do, just move on.
        loop {
            match writer_rx.try_recv() {
                Ok(chunk) => {
                    if let Err(e) = pty.write(&chunk) {
                        eprintln!("wm::broadcaster write error: {e}");
                        return;
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // All writers dropped. Best-effort drain anything
                    // still in the child, then exit.
                    drain_and_exit(&pty, &tx, &mut buf);
                    return;
                }
            }
        }

        // 2. Read whatever the child has emitted.
        match pty.try_read(&mut buf) {
            Ok(0) => {
                if !pty.is_alive() {
                    // Child exited and pipe is drained.
                    return;
                }
                std::thread::sleep(idle);
            }
            Ok(n) => {
                // Ignore Lagged/Recv errors: it just means no subscribers
                // right now. The data is gone but the next read will deliver
                // more.
                let _ = tx.send(buf[..n].to_vec());
            }
            Err(e) => {
                eprintln!("wm::broadcaster read error: {e}");
                return;
            }
        }
    }
}

/// Best-effort drain of whatever's still in the pipe before exit. Bound
/// to a fixed iteration count so a chatty dying child can't pin us here.
fn drain_and_exit(
    pty: &Pty,
    tx: &broadcast::Sender<Vec<u8>>,
    buf: &mut [u8],
) {
    for _ in 0..64 {
        match pty.try_read(buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let _ = tx.send(buf[..n].to_vec());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::CommandBuilder;
    use std::time::Instant;

    /// Blocking helper: receive one chunk from a broadcast receiver with
    /// a timeout. Used by tests that want to drain without `.await`.
    fn recv_chunk(
        rx: &mut broadcast::Receiver<Vec<u8>>,
        timeout: Duration,
    ) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(chunk) => return Some(chunk),
                Err(broadcast::error::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(e) => panic!("recv: {e}"),
            }
        }
    }

    #[test]
    fn pane_id_is_monotonic() {
        let a = PaneId::fresh();
        let b = PaneId::fresh();
        assert!(b.0 > a.0, "fresh IDs must be strictly increasing: a={} b={}", a.0, b.0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn roundtrip_echo_via_broadcaster() {
        // Spawn `cat` so whatever we send comes back.
        let cmd = CommandBuilder::new("/bin/cat");
        let pty = Pty::spawn(cmd, 24, 80).expect("spawn cat");
        let (out, writer, tasks) = spawn(pty);

        let mut rx = out.subscribe();
        writer.send_slice(b"ping\n").await.expect("send");

        // Drain for up to 1 s and concatenate.
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < deadline && got.windows(4).all(|w| w != b"ping") {
            if let Some(chunk) = recv_chunk(&mut rx, Duration::from_millis(50)) {
                got.extend_from_slice(&chunk);
            }
        }
        assert!(
            String::from_utf8_lossy(&got).contains("ping"),
            "got: {:?}",
            String::from_utf8_lossy(&got)
        );

        // Drop the writer — the worker should notice and drain+exit.
        drop(writer);
        // Give it a moment to notice the disconnect.
        tokio::time::sleep(Duration::from_millis(50)).await;
        tasks.worker.abort();
        drop(out);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn echo_emits_into_ansi_grid() {
        // End-to-end: shell prints "hello\nworld\n", broadcaster delivers,
        // AnsiParser paints it into a Grid. Asserts row 0 == "hello" and
        // row 1 == "world".
        use crate::wm::ansi::{AnsiParser, Grid};

        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "printf 'hello\\nworld\\n'"]);
        let pty = Pty::spawn(cmd, 24, 80).expect("spawn sh");
        let (out, _writer, tasks) = spawn(pty);

        let mut rx = out.subscribe();
        let mut raw = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < deadline && raw.windows(12).all(|w| w != b"hello\nworld\n") {
            if let Some(chunk) = recv_chunk(&mut rx, Duration::from_millis(50)) {
                raw.extend_from_slice(&chunk);
            }
        }

        tasks.worker.abort();
        drop(_writer);
        drop(out);

        let mut grid = Grid::new(3, 10);
        let mut parser = AnsiParser::new();
        parser.advance(&mut grid, &raw);

        let row = |r: u16| -> String {
            (0..grid.cols as usize)
                .map(|c| grid.cells()[r as usize * grid.cols as usize + c].ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        assert_eq!(row(0), "hello", "raw: {:?}", String::from_utf8_lossy(&raw));
        assert_eq!(row(1), "world", "raw: {:?}", String::from_utf8_lossy(&raw));
    }
}
