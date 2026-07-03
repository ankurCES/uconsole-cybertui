//! CSI-based human vital-sign sensing (breathing + heartbeat + presence).
//!
//! This is the piece the crate's "ruview-style surveillance" tagline always
//! implied but never shipped. RSSI (see [`crate::frames`]) tells you a device
//! is *in the room*; it can't see a chest rising or a pulse. That needs
//! **Channel State Information** — per-subcarrier amplitude+phase — which the
//! ClockworkPi uConsole's Broadcom BCM43455c0 can produce once patched with
//! [nexmon_csi](https://github.com/seemoo-lab/nexmon_csi).
//!
//! Data path on the CM4:
//!
//! ```text
//!   nexmon_csi firmware  ──UDP:5500──▶  this module  ──▶  /api/vitals  ──▶  UI
//!     (CSI per frame)      (int16 I/Q)     (DSP)          (JSON)
//! ```
//!
//! ruview (`wifi-densepose`) has no nexmon ingestion path, so we parse the
//! UDP frames ourselves and run a compact DSP that mirrors ruview's
//! `wifi-densepose-vitals` algorithm: static-clutter removal, an IIR bandpass
//! per vital band, then zero-crossing (breathing) / autocorrelation (heart)
//! frequency estimation. It is deliberately dependency-free — no FFT crate —
//! because a couple of biquads and one autocorrelation loop cover it.
//!
//! ponytail: amplitude-only. ruview also fuses unwrapped *phase* for the
//! heart-rate band, which is more robust to weak returns. Add phase fusion if
//! heart-rate confidence is too low in practice — the parser already keeps
//! phase per subcarrier ([`CsiFrame::phases`]).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Instant, SystemTime};

use serde::Serialize;

/// Byte offset where CSI int16 pairs begin — the nexmon metadata header is
/// 18 bytes: magic(2) rssi(1) fctl(1) mac(6) seq(2) css(2) chanspec(2)
/// chipver(2). Verified against nexmonster/nexcsi `interleaved.py`.
const NEXMON_HDR_LEN: usize = 18;

/// Valid subcarrier counts = bandwidth(MHz) * 3.2 → 20/40/80/160 MHz.
/// A payload whose CSI length doesn't map to one of these isn't a CSI frame
/// (or is from an unsupported chip) and is dropped.
const VALID_NSUB: [usize; 4] = [64, 128, 256, 512];

/// Ethernet(14) + IPv4(20, no options) + UDP(8). nexmon_csi injects CSI as
/// IPv4/UDP-over-Ethernet, so a `tcpdump`-captured packet has 42 bytes before
/// the nexmon payload. (nexcsi's "nbytes_before_csi = 60" = 42 + 18-byte
/// nexmon header confirms this.)
const L2L4_HDR: usize = 14 + 20 + 8;

/// How many seconds of history the vitals window holds. Breathing needs
/// ≥10 s to resolve 0.1 Hz; heart rate benefits from a longer view.
const WINDOW_SECS: f32 = 20.0;

/// Hard cap on buffered CSI frames (memory ceiling). At the highest realistic
/// CSI rate (~200 Hz) this is ~20 s; slower rates just fill less of it.
const MAX_FRAMES: usize = 4096;

/// Minimum frames before we'll report anything. Below this the window is too
/// short for a stable estimate.
const MIN_FRAMES: usize = 128;

/// Breathing band (Hz) → 6–30 breaths/min. Matches ruview's `breathing.rs`.
const BREATH_LO: f32 = 0.1;
const BREATH_HI: f32 = 0.5;
/// Heart band (Hz) → 48–120 bpm. ruview uses 0.8–2.0; we keep the same.
const HEART_LO: f32 = 0.8;
const HEART_HI: f32 = 2.0;

/// Presence threshold on the combined-signal motion score (std of the
/// z-scored chest signal). ponytail: this is the one knob that depends on
/// your room / antenna / distance — tune it on real hardware, it can't be
/// derived from a minimal model. Override with `--csi-motion-threshold`.
pub const DEFAULT_MOTION_THRESHOLD: f32 = 0.15;

/// One parsed CSI measurement.
#[derive(Debug, Clone)]
pub struct CsiFrame {
    /// Per-subcarrier amplitude (magnitude of the complex CSI value).
    pub amplitudes: Vec<f32>,
    /// Per-subcarrier phase in radians (kept for a future phase-fusion HR path).
    pub phases: Vec<f32>,
    /// RSSI of the frame the CSI was measured on (dBm, signed).
    pub rssi_dbm: i8,
    /// 802.11 sequence number (useful for spotting dropped frames).
    pub seq: u16,
    /// Local arrival time — used to estimate the effective sample rate.
    pub at: Instant,
}

/// Parse the UDP payload of a nexmon_csi frame (bcm43455c0 interleaved
/// format). Returns `None` for anything that isn't a well-formed CSI frame.
///
/// `payload` must start at the nexmon `magic` field, i.e. the raw bytes a
/// UDP socket bound to port 5500 hands back (eth/ip/udp already stripped).
pub fn parse_nexmon_csi(payload: &[u8]) -> Option<CsiFrame> {
    if payload.len() < NEXMON_HDR_LEN + 4 {
        return None;
    }
    let csi_bytes = payload.len() - NEXMON_HDR_LEN;
    // Each subcarrier is a complex int16 pair = 4 bytes.
    if csi_bytes % 4 != 0 {
        return None;
    }
    let nsub = csi_bytes / 4;
    if !VALID_NSUB.contains(&nsub) {
        return None;
    }

    let rssi_dbm = payload[2] as i8;
    let seq = u16::from_le_bytes([payload[10], payload[11]]);

    let mut amplitudes = Vec::with_capacity(nsub);
    let mut phases = Vec::with_capacity(nsub);
    let mut off = NEXMON_HDR_LEN;
    for _ in 0..nsub {
        // Real first, then imag (nexcsi views the int16 pair as complex64).
        let re = i16::from_le_bytes([payload[off], payload[off + 1]]) as f32;
        let im = i16::from_le_bytes([payload[off + 2], payload[off + 3]]) as f32;
        amplitudes.push(re.hypot(im));
        phases.push(im.atan2(re));
        off += 4;
    }

    Some(CsiFrame {
        amplitudes,
        phases,
        rssi_dbm,
        seq,
        at: Instant::now(),
    })
}

/// Parse a packet as captured by `tcpdump` on the CSI interface: try the
/// nexmon payload at the post-Ethernet/IP/UDP offset first, then at offset 0
/// (in case the capture is already L2-stripped, e.g. a UDP socket). The strict
/// subcarrier-count check in [`parse_nexmon_csi`] makes a wrong-offset match
/// vanishingly unlikely, so trying both is safe.
pub fn parse_captured(pkt: &[u8]) -> Option<CsiFrame> {
    if pkt.len() > L2L4_HDR {
        if let Some(f) = parse_nexmon_csi(&pkt[L2L4_HDR..]) {
            return Some(f);
        }
    }
    parse_nexmon_csi(pkt)
}

/// What `GET /api/vitals` returns. Field names mirror ruview's `SensingUpdate`
/// where they line up, so a UI written against either speaks the same shape.
#[derive(Debug, Clone, Serialize)]
pub struct VitalReading {
    /// Someone is present in the monitored link.
    pub presence: bool,
    /// Coarse motion label: `"none"` | `"low"` | `"high"`.
    pub motion_level: &'static str,
    /// Raw motion score (std of the z-scored chest signal) behind `motion_level`.
    pub motion_score: f32,
    /// Estimated respiration rate, breaths per minute (0 if unavailable).
    pub breathing_rate_bpm: f32,
    /// Confidence in the breathing estimate, 0..1.
    pub breathing_confidence: f32,
    /// Estimated heart rate, beats per minute (0 if unavailable).
    pub heart_rate_bpm: f32,
    /// Confidence in the heart-rate estimate, 0..1.
    pub heartbeat_confidence: f32,
    /// Number of subcarriers in the last CSI frame.
    pub subcarrier_count: usize,
    /// Effective CSI sample rate (Hz) estimated from arrival times.
    pub sample_rate_hz: f32,
    /// How many frames are currently in the analysis window.
    pub frames_in_window: usize,
    /// Wall-clock seconds since epoch when this reading was produced.
    pub updated_unix: u64,
}

impl VitalReading {
    /// The "no data yet" reading — what `/api/vitals` returns before any CSI
    /// has arrived (e.g. nexmon not running, or the first 20 s of warm-up).
    pub fn empty() -> Self {
        Self {
            presence: false,
            motion_level: "none",
            motion_score: 0.0,
            breathing_rate_bpm: 0.0,
            breathing_confidence: 0.0,
            heart_rate_bpm: 0.0,
            heartbeat_confidence: 0.0,
            subcarrier_count: 0,
            sample_rate_hz: 0.0,
            frames_in_window: 0,
            updated_unix: 0,
        }
    }
}

/// Thread-safe latest-reading holder read by the `/api/vitals` handler.
#[derive(Debug)]
pub struct VitalsStore {
    inner: RwLock<VitalReading>,
}

impl Default for VitalsStore {
    fn default() -> Self {
        Self {
            inner: RwLock::new(VitalReading::empty()),
        }
    }
}

impl VitalsStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, r: VitalReading) {
        *self.inner.write().expect("vitals store poisoned") = r;
    }
    pub fn get(&self) -> VitalReading {
        self.inner.read().expect("vitals store poisoned").clone()
    }
}

/// Sliding-window CSI analyser. Holds recent amplitude vectors and turns them
/// into a [`VitalReading`] on demand.
pub struct VitalsEngine {
    frames: std::collections::VecDeque<CsiFrame>,
    /// Optional fixed sample rate (`--csi-rate`). When `None` we estimate it
    /// from arrival times. Hardware ping rate drifts, so the override exists.
    fixed_rate_hz: Option<f32>,
    motion_threshold: f32,
}

impl VitalsEngine {
    pub fn new(fixed_rate_hz: Option<f32>, motion_threshold: f32) -> Self {
        Self {
            frames: std::collections::VecDeque::with_capacity(MAX_FRAMES),
            fixed_rate_hz,
            motion_threshold,
        }
    }

    /// Add a frame, evicting the oldest if we're at the memory cap.
    pub fn push(&mut self, frame: CsiFrame) {
        if self.frames.len() >= MAX_FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    /// Effective sample rate: the override if set, else estimated from the
    /// span of arrival times across the window.
    fn sample_rate(&self) -> f32 {
        if let Some(r) = self.fixed_rate_hz {
            return r;
        }
        let n = self.frames.len();
        if n < 2 {
            return 0.0;
        }
        let span = self.frames[n - 1]
            .at
            .duration_since(self.frames[0].at)
            .as_secs_f32();
        if span <= 0.0 {
            0.0
        } else {
            (n - 1) as f32 / span
        }
    }

    /// Compute the current vitals reading from the window.
    pub fn read(&self) -> VitalReading {
        let n = self.frames.len();
        let subcarrier_count = self.frames.back().map(|f| f.amplitudes.len()).unwrap_or(0);
        let fs = self.sample_rate();
        if n < MIN_FRAMES || fs <= 0.0 {
            let mut r = VitalReading::empty();
            r.subcarrier_count = subcarrier_count;
            r.sample_rate_hz = fs;
            r.frames_in_window = n;
            r.updated_unix = now_unix();
            return r;
        }

        // Keep only the last WINDOW_SECS worth of frames for the estimate.
        let want = ((fs * WINDOW_SECS) as usize).clamp(MIN_FRAMES, n);
        let start = n - want;

        let combined = combined_chest_signal(
            self.frames.iter().skip(start).map(|f| f.amplitudes.as_slice()),
            want,
            subcarrier_count,
        );

        let motion_score = std_dev(&combined);
        let (breathing_rate_bpm, breathing_confidence) =
            estimate_breathing(&combined, fs);
        let (heart_rate_bpm, heartbeat_confidence) = estimate_heart(&combined, fs);

        // Presence: sustained motion in the chest-signal, OR a confident
        // breathing lock (a still, breathing person has low broadband motion
        // but a clear respiration peak).
        let presence =
            motion_score > self.motion_threshold || breathing_confidence > 0.5;
        let motion_level = if motion_score > self.motion_threshold * 3.0 {
            "high"
        } else if motion_score > self.motion_threshold {
            "low"
        } else {
            "none"
        };

        VitalReading {
            presence,
            motion_level,
            motion_score,
            breathing_rate_bpm,
            breathing_confidence,
            heart_rate_bpm,
            heartbeat_confidence,
            subcarrier_count,
            sample_rate_hz: fs,
            frames_in_window: want,
            updated_unix: now_unix(),
        }
    }
}

/// Build one chest-motion signal from the window's amplitude vectors.
///
/// Chest movement modulates a *few* subcarriers strongly and leaves the rest
/// (and the null/pilot subcarriers, which sit near zero) flat. We pick the
/// highest-variance subcarriers, z-score each, and average them. Averaging a
/// handful lifts SNR over any single subcarrier, and picking by variance
/// automatically skips the null subcarriers without hardcoding their indices.
fn combined_chest_signal<'a>(
    rows: impl Iterator<Item = &'a [f32]>,
    nrows: usize,
    nsub: usize,
) -> Vec<f32> {
    if nsub == 0 || nrows == 0 {
        return Vec::new();
    }
    // Column-major: columns[sub] = time series for that subcarrier.
    let mut columns = vec![Vec::with_capacity(nrows); nsub];
    for row in rows {
        for (s, col) in columns.iter_mut().enumerate() {
            col.push(row.get(s).copied().unwrap_or(0.0));
        }
    }

    // Rank subcarriers by variance, take the top few.
    const TOP_K: usize = 3;
    let mut ranked: Vec<(usize, f32)> = columns
        .iter()
        .enumerate()
        .map(|(s, col)| (s, variance(col)))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut combined = vec![0.0f32; nrows];
    let mut used = 0;
    for &(s, var) in ranked.iter().take(TOP_K) {
        if var <= f32::EPSILON {
            continue;
        }
        let z = zscore(&columns[s]);
        for (c, zi) in combined.iter_mut().zip(z) {
            *c += zi;
        }
        used += 1;
    }
    if used > 1 {
        for c in combined.iter_mut() {
            *c /= used as f32;
        }
    }
    combined
}

/// Breathing estimate: bandpass to the respiration band, then count zero
/// crossings of the filtered signal to get frequency. Returns `(bpm, conf)`.
pub fn estimate_breathing(signal: &[f32], fs: f32) -> (f32, f32) {
    let f0 = (BREATH_LO + BREATH_HI) / 2.0;
    let q = f0 / (BREATH_HI - BREATH_LO);
    band_estimate_zero_cross(signal, fs, f0, q, BREATH_LO, BREATH_HI, 6.0, 30.0)
}

/// Heart-rate estimate: bandpass to the cardiac band, then autocorrelation
/// peak. Returns `(bpm, conf)`.
pub fn estimate_heart(signal: &[f32], fs: f32) -> (f32, f32) {
    let f0 = (HEART_LO + HEART_HI) / 2.0;
    let q = f0 / (HEART_HI - HEART_LO);
    let filtered = apply_biquad(&biquad_bandpass(f0, q, fs), signal);
    let conf_energy = energy_ratio(signal, &filtered);

    let min_lag = (fs / HEART_HI).floor().max(2.0) as usize;
    let max_lag = (fs / HEART_LO).ceil() as usize;
    let Some((lag, peak)) = autocorr_peak(&filtered, min_lag, max_lag) else {
        return (0.0, 0.0);
    };
    let bpm = 60.0 * fs / lag as f32;
    if !(40.0..=180.0).contains(&bpm) {
        return (0.0, 0.0);
    }
    // Confidence blends the autocorrelation peak sharpness with in-band energy.
    let conf = (peak.clamp(0.0, 1.0) * conf_energy).clamp(0.0, 1.0);
    (bpm, conf)
}

/// Shared band → zero-crossing frequency estimate for the breathing path.
#[allow(clippy::too_many_arguments)]
fn band_estimate_zero_cross(
    signal: &[f32],
    fs: f32,
    f0: f32,
    q: f32,
    _lo: f32,
    _hi: f32,
    bpm_min: f32,
    bpm_max: f32,
) -> (f32, f32) {
    if fs <= 0.0 || signal.len() < 4 {
        return (0.0, 0.0);
    }
    let filtered = apply_biquad(&biquad_bandpass(f0, q, fs), signal);
    let crossings = zero_crossings(&filtered);
    let secs = signal.len() as f32 / fs;
    if secs <= 0.0 || crossings < 2 {
        return (0.0, 0.0);
    }
    let freq = (crossings as f32 / 2.0) / secs;
    let bpm = freq * 60.0;
    if !(bpm_min..=bpm_max).contains(&bpm) {
        return (0.0, 0.0);
    }
    (bpm, energy_ratio(signal, &filtered))
}

/// RBJ constant-0 dB-peak bandpass biquad. Returns `[b0,b1,b2,a1,a2]` with
/// `a0` normalised to 1.
fn biquad_bandpass(f0: f32, q: f32, fs: f32) -> [f32; 5] {
    let w0 = 2.0 * std::f32::consts::PI * f0 / fs;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q);
    let a0 = 1.0 + alpha;
    [
        alpha / a0,      // b0
        0.0,             // b1
        -alpha / a0,     // b2
        -2.0 * cos / a0, // a1
        (1.0 - alpha) / a0, // a2
    ]
}

/// Direct-form-I biquad, single forward pass.
fn apply_biquad(c: &[f32; 5], x: &[f32]) -> Vec<f32> {
    let (b0, b1, b2, a1, a2) = (c[0], c[1], c[2], c[3], c[4]);
    let mut y = vec![0.0f32; x.len()];
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    for i in 0..x.len() {
        let out = b0 * x[i] + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        x2 = x1;
        x1 = x[i];
        y2 = y1;
        y1 = out;
        y[i] = out;
    }
    y
}

/// Count sign changes (zero crossings) in a signal.
fn zero_crossings(x: &[f32]) -> usize {
    let mut count = 0;
    let mut prev = 0.0f32;
    let mut have_prev = false;
    for &v in x {
        if v == 0.0 {
            continue;
        }
        if have_prev && (v > 0.0) != (prev > 0.0) {
            count += 1;
        }
        prev = v;
        have_prev = true;
    }
    count
}

/// Normalised autocorrelation peak in `[min_lag, max_lag]`.
/// Returns `(lag, peak/r0)` or `None` if the window is too short.
fn autocorr_peak(x: &[f32], min_lag: usize, max_lag: usize) -> Option<(usize, f32)> {
    let n = x.len();
    if min_lag < 1 || max_lag <= min_lag || max_lag >= n {
        return None;
    }
    let r0: f32 = x.iter().map(|v| v * v).sum();
    if r0 <= f32::EPSILON {
        return None;
    }
    let mut best = (0usize, f32::MIN);
    for lag in min_lag..=max_lag {
        let mut acc = 0.0f32;
        for i in lag..n {
            acc += x[i] * x[i - lag];
        }
        if acc > best.1 {
            best = (lag, acc);
        }
    }
    if best.1 <= 0.0 {
        return None;
    }
    Some((best.0, best.1 / r0))
}

/// Fraction of signal energy that survives the bandpass, 0..1. Used as a
/// crude confidence: an in-band oscillation keeps most of its energy.
fn energy_ratio(input: &[f32], filtered: &[f32]) -> f32 {
    let ein: f32 = input.iter().map(|v| v * v).sum();
    if ein <= f32::EPSILON {
        return 0.0;
    }
    let ef: f32 = filtered.iter().map(|v| v * v).sum();
    (ef / ein).clamp(0.0, 1.0)
}

fn mean(x: &[f32]) -> f32 {
    if x.is_empty() {
        0.0
    } else {
        x.iter().sum::<f32>() / x.len() as f32
    }
}

fn variance(x: &[f32]) -> f32 {
    if x.len() < 2 {
        return 0.0;
    }
    let m = mean(x);
    x.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / x.len() as f32
}

fn std_dev(x: &[f32]) -> f32 {
    variance(x).sqrt()
}

fn zscore(x: &[f32]) -> Vec<f32> {
    let m = mean(x);
    let sd = std_dev(x);
    if sd <= f32::EPSILON {
        return vec![0.0; x.len()];
    }
    x.iter().map(|v| (v - m) / sd).collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Bind a UDP socket and feed every parsed CSI frame into the engine, updating
/// `store` after each frame. Runs until the task is aborted.
pub async fn run_csi_udp(
    bind: SocketAddr,
    store: Arc<VitalsStore>,
    fixed_rate_hz: Option<f32>,
    motion_threshold: f32,
) -> anyhow::Result<()> {
    let sock = tokio::net::UdpSocket::bind(bind).await?;
    tracing::info!(%bind, "csi: listening for nexmon_csi UDP frames");
    let mut engine = VitalsEngine::new(fixed_rate_hz, motion_threshold);
    let mut buf = vec![0u8; 4096];
    // Recompute the reading at most ~4×/s; the DSP over a 20 s window is cheap
    // but there's no point running it on every single UDP datagram.
    let mut since_publish = 0usize;
    loop {
        let (len, _from) = sock.recv_from(&mut buf).await?;
        if let Some(frame) = parse_nexmon_csi(&buf[..len]) {
            engine.push(frame);
            since_publish += 1;
            if since_publish >= 16 {
                since_publish = 0;
                store.set(engine.read());
            }
        }
    }
}

/// Read nexmon CSI from a pcap stream (a file, or `-` for stdin) and feed the
/// engine. This is the reliable path for nexmon_csi on the CM4:
///
/// ```sh
/// tcpdump -i wlan0 -s 0 -U -w - 'udp port 5500' | wifi-radar --csi-pcap -
/// ```
///
/// pcap I/O is synchronous (`pcap-file`), so this runs on a blocking task and
/// publishes readings back through the shared store.
pub async fn run_csi_pcap(
    path: std::path::PathBuf,
    store: Arc<VitalsStore>,
    fixed_rate_hz: Option<f32>,
    motion_threshold: f32,
) -> anyhow::Result<()> {
    use pcap_file::pcap::PcapReader;
    use std::io::BufReader;

    tracing::info!(?path, "csi: reading nexmon CSI from pcap stream");
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut engine = VitalsEngine::new(fixed_rate_hz, motion_threshold);
        let mut since_publish = 0usize;

        // `-` means stdin (streams as tcpdump writes); anything else is a file.
        let reader: Box<dyn std::io::Read + Send> = if path.as_os_str() == "-" {
            Box::new(std::io::stdin())
        } else {
            Box::new(std::fs::File::open(&path)?)
        };
        let mut pcap = PcapReader::new(BufReader::new(reader))?;
        while let Some(pkt) = pcap.next_packet() {
            let pkt = pkt?;
            if let Some(frame) = parse_captured(&pkt.data) {
                engine.push(frame);
                since_publish += 1;
                if since_publish >= 16 {
                    since_publish = 0;
                    store.set(engine.read());
                }
            }
        }
        // Publish a final reading at EOF (e.g. reading a fixed capture file).
        store.set(engine.read());
        Ok(())
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic nexmon CSI UDP payload with `nsub` subcarriers where
    /// subcarrier `hot` carries `(re, im)` and the rest are zero.
    fn fake_payload(nsub: usize, rssi: i8, seq: u16, hot: usize, re: i16, im: i16) -> Vec<u8> {
        let mut p = vec![0u8; NEXMON_HDR_LEN + nsub * 4];
        p[0] = 0x11;
        p[1] = 0x11;
        p[2] = rssi as u8;
        p[10..12].copy_from_slice(&seq.to_le_bytes());
        let off = NEXMON_HDR_LEN + hot * 4;
        p[off..off + 2].copy_from_slice(&re.to_le_bytes());
        p[off + 2..off + 4].copy_from_slice(&im.to_le_bytes());
        p
    }

    #[test]
    fn parses_valid_csi_frame() {
        let p = fake_payload(64, -42, 7, 10, 30, 40); // |30+40i| = 50
        let f = parse_nexmon_csi(&p).expect("should parse");
        assert_eq!(f.amplitudes.len(), 64);
        assert_eq!(f.rssi_dbm, -42);
        assert_eq!(f.seq, 7);
        assert!((f.amplitudes[10] - 50.0).abs() < 1e-3);
        assert_eq!(f.amplitudes[0], 0.0);
    }

    #[test]
    fn parse_captured_strips_l2l4_header() {
        // A tcpdump-captured packet: 42 bytes of eth/ip/udp, then the nexmon
        // payload. parse_captured must find the CSI at offset 42.
        let payload = fake_payload(64, -55, 3, 5, 100, 0);
        let mut pkt = vec![0xabu8; L2L4_HDR];
        pkt.extend_from_slice(&payload);
        let f = parse_captured(&pkt).expect("should parse at offset 42");
        assert_eq!(f.rssi_dbm, -55);
        assert_eq!(f.amplitudes.len(), 64);
        // And a bare payload (already L2-stripped) still parses at offset 0.
        assert!(parse_captured(&payload).is_some());
    }

    #[test]
    fn rejects_bad_lengths() {
        assert!(parse_nexmon_csi(&[0u8; 10]).is_none()); // too short
        assert!(parse_nexmon_csi(&vec![0u8; NEXMON_HDR_LEN + 63 * 4]).is_none()); // 63 not valid
        assert!(parse_nexmon_csi(&vec![0u8; NEXMON_HDR_LEN + 65]).is_none()); // not %4
    }

    /// Feed a synthetic breathing signal (pure sine at a known rate) straight
    /// into the estimator and confirm we recover the rate. This is the check
    /// that fails loudly if the biquad or zero-crossing math regresses.
    #[test]
    fn recovers_known_breathing_rate() {
        let fs = 20.0; // Hz
        let bpm = 15.0; // 0.25 Hz
        let freq = bpm / 60.0;
        let n = (fs * WINDOW_SECS) as usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / fs).sin())
            .collect();
        let (got, conf) = estimate_breathing(&sig, fs);
        assert!((got - bpm).abs() < 1.5, "breathing bpm = {got}");
        assert!(conf > 0.5, "breathing conf = {conf}");
    }

    #[test]
    fn recovers_known_heart_rate() {
        let fs = 20.0;
        let bpm = 72.0; // 1.2 Hz
        let freq = bpm / 60.0;
        let n = (fs * WINDOW_SECS) as usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / fs).sin())
            .collect();
        let (got, conf) = estimate_heart(&sig, fs);
        assert!((got - bpm).abs() < 3.0, "heart bpm = {got}");
        assert!(conf > 0.2, "heart conf = {conf}");
    }

    #[test]
    fn flat_signal_reports_nothing() {
        let sig = vec![0.0f32; 400];
        let (bpm, conf) = estimate_breathing(&sig, 20.0);
        assert_eq!(bpm, 0.0);
        assert_eq!(conf, 0.0);
    }

    /// End-to-end through the engine: synthetic CSI frames where one
    /// subcarrier breathes at 12 bpm → engine reports ~12 bpm and presence.
    #[test]
    fn engine_reports_breathing_from_frames() {
        let fs = 20.0f32;
        let bpm = 12.0f32;
        let freq = bpm / 60.0;
        let n = (fs * WINDOW_SECS) as usize;
        let mut engine = VitalsEngine::new(Some(fs), DEFAULT_MOTION_THRESHOLD);
        for i in 0..n {
            let mut amps = vec![1.0f32; 64];
            // Subcarrier 20 oscillates; a static one (0) stays flat.
            amps[20] = 5.0 + 3.0 * (2.0 * std::f32::consts::PI * freq * i as f32 / fs).sin();
            engine.push(CsiFrame {
                amplitudes: amps,
                phases: vec![0.0; 64],
                rssi_dbm: -50,
                seq: i as u16,
                at: Instant::now(),
            });
        }
        let r = engine.read();
        assert_eq!(r.subcarrier_count, 64);
        assert!((r.breathing_rate_bpm - bpm).abs() < 2.0, "bpm = {}", r.breathing_rate_bpm);
        assert!(r.presence, "should detect presence");
    }
}
