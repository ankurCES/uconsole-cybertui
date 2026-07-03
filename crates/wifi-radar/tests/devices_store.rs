//! Public-API tests for the `DeviceStore`.
//!
//! Lives as a separate `tests/` file (not only the `#[cfg(test)] mod` inside
//! `devices.rs`) so external code can verify the store's public contract
//! without poking private fields.

use wifi_radar::devices::DeviceStore;
use wifi_radar::frames::{DeviceEvent, FrameKind};

fn ev(mac: &str, rssi: i8) -> DeviceEvent {
    DeviceEvent {
        mac: mac.to_string(),
        kind: FrameKind::Data,
        rssi_dbm: rssi,
        channel: 6,
    }
}

#[test]
fn public_apply_then_snapshot_returns_inserted_device() {
    let store = DeviceStore::new();
    store.apply(&ev("aa:bb:cc:dd:ee:01", -55));
    let snap = store.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].mac, "aa:bb:cc:dd:ee:01");
    assert_eq!(snap[0].rssi_dbm, -55);
}

#[test]
fn public_apply_is_idempotent_for_same_mac() {
    let store = DeviceStore::new();
    store.apply(&ev("aa:bb:cc:dd:ee:02", -60));
    store.apply(&ev("aa:bb:cc:dd:ee:02", -60));
    store.apply(&ev("aa:bb:cc:dd:ee:02", -60));
    let snap = store.snapshot();
    assert_eq!(snap.len(), 1, "same MAC must collapse to one row");
}

#[test]
fn public_store_reports_empty_when_new() {
    let store = DeviceStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
}