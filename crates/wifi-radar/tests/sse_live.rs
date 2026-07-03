//! Live-SSE end-to-end test.
//!
//! Spins up the full app on a real loopback port, fires synthetic events
//! into the broadcast channel, dials `/api/events` over a real TCP socket,
//! and asserts the SSE bytes actually arrive at the client. This is the
//! only test that would have caught the "stuck on connecting…" regression
//! caused by a hand-rolled `BroadcastStream` that didn't register a waker
//! with the broadcast sender.

use std::sync::Arc;
use std::time::Duration;

use axum::http::header;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};

use wifi_radar::api::{router as api_router, sse_router, AppState};
use wifi_radar::devices::DeviceStore;
use wifi_radar::frames::{DeviceEvent, FrameKind};
use wifi_radar::run::build_router;
use wifi_radar::tags::TagDb;

#[tokio::test]
async fn sse_endpoint_streams_events_over_real_socket() {
    // 1. Assemble the full app with a small, empty broadcast buffer.
    let tags = Arc::new(TagDb::load(std::env::temp_dir().join("wifi-radar-sse-live-tags.json").as_path()).unwrap_or_else(|_| {
        // TagDb::load on a missing file gives a placeholder; the path is
        // mostly incidental in this test.
        TagDb::load(std::env::temp_dir().join("wifi-radar-sse-live-missing.json").as_path()).unwrap()
    }));
    let store = Arc::new(DeviceStore::new());
    let (events_tx, _) = broadcast::channel::<DeviceEvent>(16);
    let (scanner_tx, _scanner_rx) = mpsc::channel::<DeviceEvent>(16);
    let state = Arc::new(AppState {
        store: store.clone(),
        tags: tags.clone(),
        events_tx: events_tx.clone(),
        scanner_tx,
    });
    let static_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web");
    let app = build_router(state.clone(), static_dir);

    // 2. Bind to a random loopback port and serve in the background.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 3. Dial the SSE endpoint over a real TCP socket.
    let _url = format!("http://{local_addr}/api/events");
    let host = local_addr.ip().to_string();
    let port = local_addr.port();
    let mut stream = TcpStream::connect((host.as_str(), port)).await.unwrap();
    let req = format!(
        "GET /api/events HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    // 4. Push an event onto the broadcast channel *after* the client
    //    has subscribed but *before* the SSE stream has had time to fire
    //    its keep-alive pulse (15 s).
    let ev_tx = events_tx.clone();
    let pusher = tokio::spawn(async move {
        // Give the client a tick to send its request, then push.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = ev_tx.send(DeviceEvent {
            mac: "aa:bb:cc:dd:ee:99".into(),
            kind: FrameKind::Beacon,
            rssi_dbm: -42,
            channel: 6,
        });
    });

    // 5. Read bytes until we see "data: " (an SSE event payload line).
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_marker = b"text/event-stream";
    let data_marker = b"data: ";
    let found_data = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                panic!("server closed before sending any data");
            }
            buf.extend_from_slice(&tmp[..n]);
            // Sanity: server advertised the right content-type.
            if buf.windows(header_marker.len()).any(|w| w == header_marker)
                && buf.windows(data_marker.len()).any(|w| w == data_marker)
            {
                return true;
            }
            if buf.len() > 64 * 1024 {
                return false;
            }
        }
    })
    .await
    .unwrap_or(false);

    pusher.abort();
    server.abort();

    assert!(
        found_data,
        "never received an SSE `data:` line; raw bytes so far: {:?}",
        String::from_utf8_lossy(&buf)
    );

    // Silence unused imports if `api_router`/`sse_router` happen to be
    // trimmed out of the test in the future.
    let _ = (api_router, sse_router, header::CONNECTION);
}
