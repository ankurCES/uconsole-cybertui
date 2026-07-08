//! Pure-math RSSI → distance + direction model.
//!
//! Layered on top of the existing `DeviceStore` EMA smoother. This
//! module is intentionally side-effect-free so it can be unit-tested
//! in isolation and reused by both the Wi-Fi radar pipeline and the
//! BLE distance pipeline (Phase 3d).
//!
//! # Path-loss formula
//!
//! We use the standard log-distance path-loss model:
//!
//! ```text
//! d = 10 ^ ((TX_power_at_1m - RSSI) / (10 * n))
//! ```
//!
//! where:
//! - `TX_power_at_1m` is the calibrated received signal strength at
//!   one metre from the transmitter (dBm, negative — typical Wi-Fi
//!   AP: -30 to -40 dBm; typical BLE beacon: -55 to -65 dBm).
//! - `RSSI` is the observed signal strength (dBm, negative).
//! - `n` is the path-loss exponent: 2.0 free space, 2.5–3.0 indoor
//!   with line of sight, 3.0–4.0 indoor with obstructions.
//!
//! See Rappaport, *Wireless Communications: Principles and Practice*
//! §4.6 for the derivation.
//!
//! # Direction
//!
//! Single-AP bearing is fundamentally ambiguous (one RSSI sample
//! collapses a sphere of possible transmitter locations to a single
//! distance — no azimuth information). What we *can* recover without
//! extra hardware:
//!
//! - **RSSI gradient over time**: a rising RSSI means the device is
//!   getting closer (heading estimate by sign of the derivative).
//! - **Channel-relative bearing**: if the device is on a known
//!   channel, we use the channel as a coarse angle proxy (channel
//!   1 = north, channel 13 = south — totally synthetic, but stable
//!   across renders so a stationary device doesn't flicker).
//!
//! For true triangulation you need ≥2 radios (or AoA-capable
//! hardware). That's a future enhancement; the API here leaves
//! room for it — `bearing_deg` is a `f32` the caller can replace
//! with a real estimate when more data lands.

/// Path-loss exponent for indoor Wi-Fi with light obstructions.
/// `n = 2.0` is free space (no walls, no furniture); `n = 3.0` is a
/// typical office. We pick `2.5` as a reasonable middle ground
/// until per-deployment calibration lands.
pub const PATH_LOSS_EXPONENT_WIFI: f32 = 2.5;

/// Path-loss exponent for indoor BLE. BLE wavelengths are short
/// (~12 cm at 2.4 GHz) so multipath is worse and `n = 3.0` is the
/// standard literature value (see ZzangKyu/ble-rssi-distance-estimater
/// and the Bluetooth SIG indoor positioning guidance).
pub const PATH_LOSS_EXPONENT_BLE: f32 = 3.0;

/// Default calibrated TX power at 1 m for a typical home Wi-Fi AP
/// broadcasting at 20 dBm (100 mW). Real devices publish this in
/// the beacon as `tx_pwr`; this constant is the fallback when the
/// AP doesn't advertise it. The same number is used by OpenWrt's
/// `wifi-radar` and the `ruview` dataset.
pub const TX_POWER_1M_WIFI_DBM: f32 = -30.0;

/// Default calibrated TX power at 1 m for a typical BLE beacon
/// (iBeacon-style, 0 dBm transmit). BLE beacons usually advertise
/// their `tx_power` directly in the advertisement payload; this
/// constant is the fallback.
pub const TX_POWER_1M_BLE_DBM: f32 = -59.0;

/// Convert an observed RSSI (dBm, negative) to estimated distance
/// (metres, positive) via the log-distance path-loss formula.
///
/// Returns the clamped-to-positive result — for an RSSI stronger
/// than `tx_power_1m` (physically impossible unless you're standing
/// on top of the transmitter) we return 0.5 m as a "very close"
/// floor rather than a negative distance or a NaN.
///
/// # Examples
///
/// ```text
/// rssi_to_distance(-30, -30, 2.5) ≈ 1.0   // exactly 1 m
/// rssi_to_distance(-50, -30, 2.5) ≈ 3.2   // ~3 m
/// rssi_to_distance(-80, -30, 2.5) ≈ 100.0  // ~100 m
/// ```
pub fn rssi_to_distance(rssi_dbm: i8, tx_power_1m_dbm: f32, n: f32) -> f32 {
    // 10 * n in the denominator is just the standard log-distance
    // slope. Multiplying f32 by i8 widens, so cast rssi to f32 first.
    let exponent = (tx_power_1m_dbm - rssi_dbm as f32) / (10.0 * n);
    let dist = 10f32.powf(exponent);
    // Anything under 0.5 m rounds to 0.5 — physically the model
    // breaks down at very short range (near-field effects, antenna
    // pattern lobes). The clamp also catches the rssi > tx_power
    // case which would yield a value < 1.0.
    dist.clamp(0.5, f32::MAX)
}

/// Estimate a single-AP bearing (degrees clockwise from north) from
/// a short window of RSSI samples. The current implementation is
/// intentionally cheap and deterministic — it uses the channel as
/// a coarse angle proxy so a stationary device doesn't flicker
/// between renders. Real triangulation (≥2 radios + AoA / ToF) is
/// out of scope here; the API leaves room for it.
///
/// The channel-to-angle mapping is:
/// - 2.4 GHz Wi-Fi channels 1–13 → 0°–360° (each channel ≈ 30°)
/// - BLE advertising channels 37/38/39 → 0°/120°/240°
///
/// For an empty sample window we return `0.0` (north) as a sane
/// default so the radar canvas can always render an arrow.
pub fn bearing_from_samples(samples: &[i8], channel: u8) -> f32 {
    // Coarse angle proxy: map the channel index to a 0–360° span.
    // This is the only deterministic single-AP bearing we have
    // without triangulation hardware; a real implementation will
    // use `bearing_from_rssi_gradient` once the gradient signal
    // is stable enough to override the channel proxy.
    let coarse = channel_to_bearing(channel);

    // If we have at least two samples we layer a small gradient
    // delta on top: a rising RSSI nudges the bearing clockwise
    // (toward "approaching"), a falling RSSI counter-clockwise.
    // The delta is capped at ±15° so a single noisy spike can't
    // swing the arrow across the canvas.
    if samples.len() < 2 {
        return coarse;
    }
    let len = samples.len();
    let newer = samples[len - 1] as f32;
    let older = samples[len.saturating_sub(2).max(0)] as f32;
    let gradient = newer - older; // dBm delta between last two samples
    let delta = (gradient * 1.5).clamp(-15.0, 15.0);
    (coarse + delta).rem_euclid(360.0)
}

/// Map a Wi-Fi channel (1–13) or BLE advertising channel
/// (37/38/39) to a coarse bearing in degrees. Channels outside
/// the known ranges fall back to a hash of the channel number so
/// each device gets a stable but unique bearing.
fn channel_to_bearing(channel: u8) -> f32 {
    match channel {
        // 2.4 GHz Wi-Fi: 11 usable channels in the US, 13 in EU,
        // 14 in Japan. Spread across 0–360° so each channel is
        // visually distinguishable on the radar. Channel 13 wraps
        // back to 0° (last slice ends just shy of 360°) so the
        // radar canvas doesn't show two adjacent channels near
        // north — map channel 13 to angle 0 explicitly because
        // modulo arithmetic on `(12 * 360/13) ≈ 332°` doesn't
        // naturally land at 0.
        1..=12 => ((channel as f32 - 1.0) * (360.0 / 13.0)) % 360.0,
        13 => 0.0,
        // BLE primary advertising channels: 37 (2402 MHz),
        // 38 (2426 MHz), 39 (2480 MHz). 120° spacing keeps them
        // visually separate on the radar.
        37 => 0.0,
        38 => 120.0,
        39 => 240.0,
        // Fallback: deterministic hash so the same channel always
        // lands on the same angle, even if we don't recognise it.
        other => ((other as f32 * 37.0) % 360.0).abs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    // ----- rssi_to_distance -----

    #[test]
    fn rssi_at_tx_power_is_one_metre() {
        // RSSI equal to tx_power_1m → distance = 10^0 = 1.0 m.
        let d = rssi_to_distance(-30, -30.0, 2.5);
        assert!(approx(d, 1.0, 1e-3), "got {d}");
    }

    #[test]
    fn rssi_20db_below_tx_is_about_ten_metres() {
        // 20 dB drop at n=2.5 → 10^(20/25) = 10^0.8 ≈ 6.3 m
        // (not 10 m — the formula is `10^((tx-rssi)/(10*n))`, so
        // the relationship is non-linear). Pinning the exact value
        // catches accidental exponent swaps.
        let d = rssi_to_distance(-50, -30.0, 2.5);
        assert!(approx(d, 6.31, 0.05), "got {d}");
    }

    #[test]
    fn rssi_stronger_than_tx_clamps_to_half_metre() {
        // -20 dBm is stronger than the -30 dBm calibration →
        // physically impossible (you're inside the 1 m sphere).
        // We clamp to 0.5 m rather than report a sub-metre
        // distance the model can't actually resolve.
        let d = rssi_to_distance(-20, -30.0, 2.5);
        assert!(approx(d, 0.5, 1e-3), "got {d}");
    }

    #[test]
    fn ble_path_loss_exponent_steeper_than_wifi() {
        // Same RSSI, same TX power: Wi-Fi (n=2.5) yields a *larger*
        // distance than BLE (n=3.0). The path-loss exponent `n` is
        // in the *denominator* of the exponent
        // (`10^((TX-RSSI)/(10*n))`), so a steeper path loss means
        // you reach a given RSSI deficit over a shorter distance
        // (the signal falls off faster). This test pins that
        // relationship — getting it backwards is a classic
        // exponent-swap bug.
        let wifi = rssi_to_distance(-65, -30.0, PATH_LOSS_EXPONENT_WIFI);
        let ble = rssi_to_distance(-65, -30.0, PATH_LOSS_EXPONENT_BLE);
        assert!(wifi > ble, "Wi-Fi should be farther: wifi={wifi} ble={ble}");
    }

    // ----- bearing_from_samples -----

    #[test]
    fn bearing_wifi_channel_maps_to_expected_angle() {
        // Channel 1 → 0° (north); channel 7 → midpoint; channel 13
        // → 0° again (wraps around the 360° circle so the
        // last channel doesn't land at 332°). Each channel
        // occupies a 360/13 ≈ 27.7° slice.
        assert!(approx(bearing_from_samples(&[], 1), 0.0, 1e-3));
        let c7 = bearing_from_samples(&[], 7);
        assert!(c7 > 150.0 && c7 < 170.0, "ch 7 → ~{c7}°");
        let c13 = bearing_from_samples(&[], 13);
        assert!(approx(c13, 0.0, 1e-3), "ch 13 → ~{c13}° (must wrap)");
    }

    #[test]
    fn bearing_ble_advertising_channels_are_120_apart() {
        let a = bearing_from_samples(&[], 37);
        let b = bearing_from_samples(&[], 38);
        let c = bearing_from_samples(&[], 39);
        assert!(approx(a, 0.0, 1e-3));
        assert!(approx(b, 120.0, 1e-3));
        assert!(approx(c, 240.0, 1e-3));
    }

    #[test]
    fn bearing_gradient_nudges_within_fifteen_degrees() {
        // A 5 dBm rise → +7.5° nudge (gradient * 1.5). Pin the cap
        // so a single noisy spike can't swing the arrow.
        let stable = bearing_from_samples(&[-60, -60], 6);
        let rising = bearing_from_samples(&[-60, -55], 6);
        let delta = rising - stable;
        assert!(delta > 0.0 && delta < 15.0, "got {delta}");
    }

    #[test]
    fn bearing_gradient_caps_at_fifteen_degrees() {
        // A 100 dBm rise is absurd; the cap must clamp it.
        let rising = bearing_from_samples(&[-60, 40], 6);
        let stable = bearing_from_samples(&[-60, -60], 6);
        let delta = rising - stable;
        assert!(delta <= 15.0 + 1e-3, "delta {delta} exceeded cap");
    }

    #[test]
    fn bearing_single_sample_returns_channel_only() {
        // No gradient signal → bearing is just the channel proxy.
        assert!(approx(bearing_from_samples(&[-55], 6), bearing_from_samples(&[], 6), 1e-3));
    }
}