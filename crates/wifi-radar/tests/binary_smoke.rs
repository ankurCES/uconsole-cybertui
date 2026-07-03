//! Live-binary smoke test.
//!
//! Spawns the freshly built `wifi-radar --dev` binary on a random
//! loopback port, dials `/api/events` over a real TCP socket, asserts
//! a `data:` line arrives within 5 s, then kills the process.
//!
//! Why this exists: the in-process tests (router via `oneshot`,
//! socket-bound test in `sse_live.rs` against the lib's `build_router`)
//! all assume the *binary*'s wiring is also correct. If someone rebuilds
//! only the lib, or the lib's `build_router` ever diverges from what
//! `main.rs` actually mounts, the tests pass but a real `cargo run`
//! shows a broken page. This test guards against that.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn binary_path() -> std::path::PathBuf {
    // CARGO_BIN_EXE_wifi-radar is set by `cargo test` for the bin target;
    // fall back to the workspace-relative debug build for `cargo test --release`.
    if let Some(p) = option_env!("CARGO_BIN_EXE_wifi-radar") {
        return std::path::PathBuf::from(p);
    }
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace root
    p.push("target");
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    p.push(profile);
    p.push("wifi-radar");
    p
}

#[tokio::test]
async fn dev_binary_streams_sse_to_a_real_socket() {
    // 1. Bind a random loopback port for the binary to claim.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    // 2. Spawn the binary with --dev into the just-picked port.
    let bin = binary_path();
    assert!(
        bin.exists(),
        "wifi-radar binary not built at {} — run `cargo build -p wifi-radar` first",
        bin.display()
    );
    let mut child = Command::new(&bin)
        .arg("--dev")
        .arg("--bind")
        .arg(format!("127.0.0.1:{port}"))
        // Quiet: don't litter the test log with the binary's own tracing.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", bin.display()));

    // 3. Wait until the port starts accepting (max 5 s). The binary logs
    //    "wifi-radar listening" once bound; polling the loopback is simpler
    //    than scraping journald here.
    let start = Instant::now();
    let mut connected = None;
    while start.elapsed() < Duration::from_secs(5) {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(s) => {
                connected = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let mut stream = connected.expect("wifi-radar never opened a TCP socket within 5 s");

    // 4. Send a real GET /api/events request and read until we see `data:`.
    let req = format!(
        "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let data_marker = b"data: ";
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break, // server closed
            Ok(Ok(n)) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(data_marker.len()).any(|w| w == data_marker) {
                    found = true;
                    break;
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }

    // 5. Tear down the binary whether or not we found a `data:` line.
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        found,
        "wifi-radar did not deliver any SSE data: line within 5 s. raw bytes: {:?}",
        String::from_utf8_lossy(&buf)
    );
}
