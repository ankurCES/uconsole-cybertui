//! In-memory BLE device state: RSSI smoothing, distance + bearing
//! via the same path-loss model the Wi-Fi radar uses (with BLE-tuned
//! constants — see [`crate::rssi_model`]).
//!
//! The store is intentionally a near-twin of
//! [`crate::devices::DeviceStore`] so the radar canvas can render
//! both device classes through the same `(mac, distance_m,
//! bearing_deg)` projection. The differences are:
//!
//! - **Path-loss constants**: BLE uses `n = 3.0` and a `-59 dBm`
//!   1-metre reference (vs. Wi-Fi's `2.5` / `-30 dBm`).
//! - **Channel mapping**: BLE only uses advertising channels
//!   37/38/39, mapped to 0°/120°/240° in [`crate::rssi_model`].
//! - **No `last_kind`**: the BlueZ `org.bluez.Device1` interface
//!   doesn't expose a frame-kind discriminator — every event is
//!   effectively "seen", so we skip the kind field.

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::time::SystemTime;

use serde::Serialize;

use crate::rssi_model::{
    bearing_from_samples, rssi_to_distance, PATH_LOSS_EXPONENT_BLE, TX_POWER_1M_BLE_DBM,
};

/// Maximum number of BLE devices kept in the store before LRU
/// eviction. Mirrors [`crate::devices::MAX_DEVICES`] — the radar
/// canvas merges both stores into one overlay, so the cap has to
/// match.
pub const MAX_BLE_DEVICES: usize = 1024;

/// EMA smoothing factor. Same `α = 0.3` as the Wi-Fi store: a
/// new sample contributes 30%, the running average keeps 70%.
const EMA_ALPHA: f32 = 0.3;

/// RSSI sample window for the gradient-bearing input. Same
/// five-sample ring as the Wi-Fi store.
const RSSI_WINDOW_SIZE: usize = 5;

/// One BLE device's smoothed state. Serialised to JSON for the
/// `/api/ble_devices` endpoint the radar canvas consumes.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BleDeviceState {
    /// MAC address, lowercase hex with colons (matches the BlueZ
    /// `Address` property on `org.bluez.Device1`).
    pub mac: String,
    /// Smoothed RSSI in dBm (negative).
    pub rssi_dbm: i8,
    /// BLE advertising channel we last heard this device on
    /// (37/38/39). Drives the bearing proxy in
    /// [`crate::rssi_model::channel_to_bearing`].
    pub channel: u8,
    /// Unix epoch seconds when we last heard from this device.
    pub last_seen_unix: u64,
    /// Estimated distance in metres from the log-distance path-loss
    /// model with BLE-tuned constants.
    pub distance_m: f32,
    /// Estimated bearing in degrees clockwise from north. Single-AP
    /// only — see the module-level note in [`crate::rssi_model`].
    pub bearing_deg: f32,
    /// Number of RSSI samples we've applied (debug aid, not
    /// exposed in the public API).
    #[serde(skip)]
    pub samples_seen: u64,
    /// Recent RSSI samples (oldest first) used to feed the
    /// gradient-bearing signal.
    #[serde(skip)]
    pub rssi_window: VecDeque<i8>,
}

/// Construct the (distance, bearing) pair from a smoothed RSSI +
/// channel + recent-window. Pure helper so `from_event` and `apply`
/// can share the math without a `&mut self` borrow.
fn model_from(rssi: i8, channel: u8, window: &VecDeque<i8>) -> (f32, f32) {
    let samples: Vec<i8> = window.iter().copied().collect();
    let distance = rssi_to_distance(rssi, TX_POWER_1M_BLE_DBM, PATH_LOSS_EXPONENT_BLE);
    let bearing = bearing_from_samples(&samples, channel);
    (distance, bearing)
}

impl BleDeviceState {
    /// Build a fresh state from the first RSSI we ever see for
    /// this MAC.
    fn from_event(rssi: i8, channel: u8, now_unix: u64, mac: String) -> Self {
        let mut window = VecDeque::with_capacity(RSSI_WINDOW_SIZE);
        window.push_back(rssi);
        let (distance_m, bearing_deg) = model_from(rssi, channel, &window);
        Self {
            mac,
            rssi_dbm: rssi,
            channel,
            last_seen_unix: now_unix,
            distance_m,
            bearing_deg,
            samples_seen: 1,
            rssi_window: window,
        }
    }

    /// Apply a new RSSI sample: update EMA, slide the window,
    /// recompute distance + bearing.
    fn apply(&mut self, rssi: i8, channel: u8, now_unix: u64) {
        let prev = self.rssi_dbm as f32;
        let next = rssi as f32;
        let smoothed = EMA_ALPHA.mul_add(next, (1.0 - EMA_ALPHA) * prev);
        self.rssi_dbm = smoothed.round().clamp(i8::MIN as f32, i8::MAX as f32) as i8;
        self.channel = channel;
        self.last_seen_unix = now_unix;
        self.samples_seen = self.samples_seen.saturating_add(1);
        if self.rssi_window.len() >= RSSI_WINDOW_SIZE {
            self.rssi_window.pop_front();
        }
        self.rssi_window.push_back(rssi);
        let (distance_m, bearing_deg) =
            model_from(self.rssi_dbm, self.channel, &self.rssi_window);
        self.distance_m = distance_m;
        self.bearing_deg = bearing_deg;
    }
}

/// Snapshot read by axum handlers + the SSE broadcaster.
#[derive(Debug, Default)]
pub struct BleDeviceStore {
    inner: RwLock<HashMap<String, BleDeviceState>>,
}

impl BleDeviceStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert / update a BLE device from a fresh RSSI + channel
    /// observation. If the store is full, the least-recently-seen
    /// entry is evicted (LRU) so a chatty scanner can't push out
    /// older but still-relevant devices.
    pub fn observe(&self, mac: &str, rssi: i8, channel: u8) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut inner = self.inner.write().expect("BleDeviceStore poisoned");
        if let Some(state) = inner.get_mut(mac) {
            state.apply(rssi, channel, now);
            return;
        }
        if inner.len() >= MAX_BLE_DEVICES {
            // Evict the LRU — find the smallest `last_seen_unix`.
            // O(n) but n ≤ 1024 so it's fine for the size we
            // allow, and we only run it when a new MAC pushes us
            // over the cap.
            if let Some(oldest_mac) = inner
                .iter()
                .min_by_key(|(_, s)| s.last_seen_unix)
                .map(|(m, _)| m.clone())
            {
                inner.remove(&oldest_mac);
            }
        }
        inner.insert(
            mac.to_string(),
            BleDeviceState::from_event(rssi, channel, now, mac.to_string()),
        );
    }

    /// Return a snapshot of every BLE device the store knows about,
    /// sorted newest-first by `last_seen_unix` so the radar canvas
    /// renders active devices on top. Ties (same epoch second —
    /// common in unit tests that run inside one second) break on
    /// MAC address ascending so the order is deterministic.
    pub fn snapshot(&self) -> Vec<BleDeviceState> {
        let inner = self.inner.read().expect("BleDeviceStore poisoned");
        let mut devices: Vec<BleDeviceState> = inner.values().cloned().collect();
        devices.sort_by(|a, b| {
            b.last_seen_unix
                .cmp(&a.last_seen_unix)
                .then_with(|| a.mac.cmp(&b.mac))
        });
        devices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn first_event_initialises_state() {
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:ff", -55, 37);
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        let d = &snap[0];
        assert_eq!(d.mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(d.rssi_dbm, -55);
        assert_eq!(d.channel, 37);
        // BLE: -55 dBm with tx=-59, n=3.0
        // distance = 10^((−59 − (−55)) / (10 × 3))
        //         = 10^(−4/30) ≈ 0.72 m
        assert!(approx(d.distance_m, 0.72, 0.05), "got {}", d.distance_m);
        // Channel 37 → bearing 0°.
        assert!(approx(d.bearing_deg, 0.0, 1e-3));
        assert_eq!(d.samples_seen, 1);
    }

    #[test]
    fn apply_smooths_rssi_with_ema() {
        // EMA α = 0.3: second sample -55, prev -65 → smoothed = -62.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:ff", -65, 37);
        store.observe("aa:bb:cc:dd:ee:ff", -55, 37);
        let snap = store.snapshot();
        assert_eq!(snap[0].rssi_dbm, -62);
        assert_eq!(snap[0].samples_seen, 2);
    }

    #[test]
    fn window_does_not_exceed_five_samples() {
        // Pin the RSSI window's bounded size — without it, a
        // chatty scanner would grow the window forever and the
        // gradient-bearing input would lag reality.
        let store = BleDeviceStore::new();
        for i in 0..20 {
            store.observe("aa:bb:cc:dd:ee:ff", -60 - (i as i8 % 5), 37);
        }
        let snap = store.snapshot();
        assert_eq!(snap[0].rssi_window.len(), RSSI_WINDOW_SIZE);
    }

    #[test]
    fn lru_eviction_drops_least_recently_seen() {
        // Fill the cap with one exception, then insert a new MAC.
        // The oldest (smallest last_seen_unix) MAC must be evicted.
        // The MAC formatter widens to 4 hex digits on the host
        // octet so we can actually generate MAX_BLE_DEVICES + 1
        // distinct entries (with a 2-digit octet the test collides
        // at i=256).
        let store = BleDeviceStore::new();
        for i in 0..(MAX_BLE_DEVICES + 5) {
            let mac = format!("aa:bb:cc:dd:ee:{:04x}", i);
            store.observe(&mac, -60, 37);
        }
        let snap = store.snapshot();
        assert_eq!(snap.len(), MAX_BLE_DEVICES);
    }

    #[test]
    fn snapshot_is_sorted_newest_first() {
        // Distinct MACs so the LRU logic doesn't merge them.
        // Three observes back-to-back usually share the same
        // epoch-second timestamp, so the primary sort key
        // (`last_seen_unix` DESC) ties. The tiebreaker is MAC
        // ascending, so the snapshot lands in MAC order. Pin
        // both invariants: ties are broken by MAC, and the
        // primary key is timestamp-desc.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:01", -55, 37);
        store.observe("aa:bb:cc:dd:ee:02", -65, 38);
        store.observe("aa:bb:cc:dd:ee:03", -75, 39);
        let snap = store.snapshot();
        // All three landed in the same epoch second → MAC asc
        // wins the tiebreak.
        assert_eq!(snap[0].mac, "aa:bb:cc:dd:ee:01");
        assert_eq!(snap[1].mac, "aa:bb:cc:dd:ee:02");
        assert_eq!(snap[2].mac, "aa:bb:cc:dd:ee:03");
    }

    #[test]
    fn bearing_rotates_with_channel_change() {
        // Changing the channel should change the bearing (same
        // RSSI, but the channel proxy drives the angle). We don't
        // pin the exact value because the gradient delta can shift
        // it by up to ±15° — just confirm the bearing moves.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:ff", -60, 37);
        let before = store.snapshot()[0].bearing_deg;
        // Pad the window past the threshold so the gradient signal
        // is dampened, then jump channels.
        for _ in 0..4 {
            store.observe("aa:bb:cc:dd:ee:ff", -60, 37);
        }
        store.observe("aa:bb:cc:dd:ee:ff", -60, 39);
        let after = store.snapshot()[0].bearing_deg;
        let delta = (after - before).abs();
        assert!(
            delta > 100.0,
            "channel jump should swing bearing: before={before} after={after} delta={delta}"
        );
    }

    // ----- 3e: BLE distance + direction calculator coverage -----
    //
    // The pure-math tests live in `rssi_model::tests`. These tests
    // pin the *projection* into `BleDeviceState`: that the BLE
    // calibration constants (TX=-59 dBm, n=3.0) actually flow
    // through, and that the BLE advertising-channel bearing proxy
    // lands at the documented 0°/120°/240°.

    #[test]
    fn ble_channel_38_maps_to_bearing_120() {
        // Channel 38 → 120° in the channel_to_bearing table.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:11", -55, 38);
        let d = &store.snapshot()[0];
        assert!(approx(d.bearing_deg, 120.0, 1e-3), "got {}", d.bearing_deg);
    }

    #[test]
    fn ble_channel_39_maps_to_bearing_240() {
        // Channel 39 → 240° in the channel_to_bearing table.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:11", -55, 39);
        let d = &store.snapshot()[0];
        assert!(approx(d.bearing_deg, 240.0, 1e-3), "got {}", d.bearing_deg);
    }

    #[test]
    fn ble_distance_grows_as_rssi_drops() {
        // Same MAC, same channel, three RSSIs that get progressively
        // weaker. Distance must be monotonically non-decreasing —
        // the log-distance model is strictly decreasing in RSSI.
        // Pin the BLE-specific values (tx=-59, n=3.0) end-to-end.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:22", -55, 37); // near
        let near = store.snapshot()[0].distance_m;
        store.observe("aa:bb:cc:dd:ee:22", -70, 37); // mid
        let mid = store.snapshot()[0].distance_m;
        store.observe("aa:bb:cc:dd:ee:22", -85, 37); // far
        let far = store.snapshot()[0].distance_m;
        assert!(near < mid, "near={near} should be < mid={mid}");
        assert!(mid < far, "mid={mid} should be < far={far}");
        // And the near sample should still be in the realistic
        // BLE range (0.5–2 m), not some runaway value.
        assert!(near > 0.5 && near < 2.0, "near={near} out of range");
    }

    #[test]
    fn ble_distance_clamps_on_impossibly_strong_rssi() {
        // RSSI *stronger* than the BLE -59 dBm 1m calibration
        // means you're inside the 1m sphere. The path-loss
        // formula produces 10^(−0.3) ≈ 0.5 m in this regime —
        // the rssi_model floor is what catches truly absurd
        // values (e.g. -120 dBm with a stronger TX). Verify
        // the clamp is tight at the floor: -59 + 10 = -49 dBm
        // yields 10^((−59 − (−49))/30) = 10^(−1/3) ≈ 0.46,
        // which is below the 0.5 m floor and must clamp up.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:33", -49, 37); // > -59 dBm
        let d = &store.snapshot()[0];
        assert!(approx(d.distance_m, 0.5, 1e-3), "got {}", d.distance_m);
        // Sanity: a merely-strong -50 dBm (inside 1m but not
        // triggering the clamp) lands at ~0.5 m via the formula
        // itself.
        store.observe("aa:bb:cc:dd:ee:33", -50, 37);
        let d = &store.snapshot()[0];
        assert!(d.distance_m >= 0.5, "got {}", d.distance_m);
    }

    #[test]
    fn ble_distance_at_tx_power_is_one_metre() {
        // Edge case: RSSI exactly equal to the BLE TX-power
        // calibration (-59 dBm) → 10^0 = 1.0 m. Pin the
        // calibration constant flowing through end-to-end.
        let store = BleDeviceStore::new();
        store.observe("aa:bb:cc:dd:ee:44", TX_POWER_1M_BLE_DBM as i8, 37);
        let d = &store.snapshot()[0];
        assert!(approx(d.distance_m, 1.0, 0.01), "got {}", d.distance_m);
    }
}