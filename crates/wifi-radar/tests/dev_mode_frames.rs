//! Test the dev-mode synthetic stream: spawn the scanner, give it a tick,
//! confirm the store has devices and the SSE channel received events.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use wifi_radar::devices::DeviceStore;
use wifi_radar::frames::DeviceEvent;
use wifi_radar::scanner::{spawn, ScannerSource};

#[tokio::test]
async fn dev_mode_emits_deterministic_stream_into_store_and_channel() {
    let store = Arc::new(DeviceStore::new());
    let (events_tx, _) = broadcast::channel::<DeviceEvent>(64);
    let (scanner_tx, mut scanner_rx) = mpsc::channel::<DeviceEvent>(64);

    // Forward mpsc → broadcast so the test can subscribe.
    let fanout_tx = events_tx.clone();
    let fanout = tokio::spawn(async move {
        while let Some(ev) = scanner_rx.recv().await {
            let _ = fanout_tx.send(ev);
        }
    });

    let handle = spawn(store.clone(), scanner_tx, ScannerSource::Dev);

    // Let the scanner tick a few times (250ms per tick, 6 ticks ≈ 1.5s).
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Store should hold all DEV_DEVICE_COUNT synthetic MACs.
    assert!(store.len() >= 4, "expected ≥4 devices, got {}", store.len());

    // The SSE broadcast should have received at least one event.
    let mut rx = events_tx.subscribe();
    let first = tokio::time::timeout(Duration::from_millis(200), rx.recv())
        .await
        .expect("timeout waiting for first SSE event")
        .expect("broadcast closed");
    assert!(!first.mac.is_empty());

    handle.stop().await;
    fanout.abort();
}