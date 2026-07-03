//! Background scanner: reads 802.11 frames and pushes `DeviceEvent`s into
//! the [`DeviceStore`].
//!
//! Two modes:
//!
//! - **Live**: read frames from a pcap file (or stdin) using `pcap-file`.
//!   The radiotap header is parsed for RSSI + channel, then the raw 802.11
//!   payload is fed into [`frames::parse_frame`].
//! - **Dev**: emit a deterministic synthetic stream so the UI is visible
//!   on machines that can't actually capture monitor-mode frames. The
//!   stream sweeps a fixed set of MACs through channel 6 with RSSIs that
//!   move like a person pacing around the room.
//!
//! [`frames::parse_frame`]: crate::frames::parse_frame

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::devices::DeviceStore;
use crate::frames::{parse_frame, DeviceEvent, FrameKind};

/// Fixed "tick" between synthetic dev frames.
const DEV_FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// Number of synthetic devices we cycle through in dev mode.
const DEV_DEVICE_COUNT: usize = 8;

/// Handle to a running scanner task. Drop it (or call [`stop`]) to cancel.
pub struct ScannerHandle {
    join: JoinHandle<()>,
}

impl ScannerHandle {
    /// Request shutdown and wait for the task to exit.
    pub async fn stop(self) {
        self.join.abort();
        let _ = self.join.await;
    }
}

/// What the scanner is reading from.
#[derive(Debug, Clone)]
pub enum ScannerSource {
    /// Live capture: a pcap file (e.g. `tcpdump -I -i wlan0 -w capture.pcap`).
    /// Pure-Rust `pcap-file` parses it; no libpcap C dep.
    PcapFile(std::path::PathBuf),
    /// Synthetic stream — for development and the test suite.
    Dev,
}

/// Spawn the background scanner. Frames flow into the given store AND into
/// the given SSE channel (so the browser gets a live tick).
pub fn spawn(
    store: Arc<DeviceStore>,
    tx: mpsc::Sender<DeviceEvent>,
    source: ScannerSource,
) -> ScannerHandle {
    let join = match source {
        ScannerSource::PcapFile(path) => {
            tokio::spawn(async move { run_pcap(store, tx, path).await })
        }
        ScannerSource::Dev => tokio::spawn(async move { run_dev(store, tx).await }),
    };
    ScannerHandle { join }
}

/// Live capture loop: read a pcap file (or stdin as `-`) and parse frames.
async fn run_pcap(
    store: Arc<DeviceStore>,
    tx: mpsc::Sender<DeviceEvent>,
    path: std::path::PathBuf,
) {
    tracing::info!(?path, "scanner: starting pcap reader");
    if let Err(e) = read_pcap_into(&path, &store, &tx).await {
        tracing::warn!(error = %e, "scanner: pcap reader exited");
    }
}

async fn read_pcap_into(
    path: &Path,
    store: &Arc<DeviceStore>,
    tx: &mpsc::Sender<DeviceEvent>,
) -> anyhow::Result<()> {
    use pcap_file::pcap::PcapReader;
    use std::fs::File;
    use std::io::BufReader;

    // Sync I/O in a blocking task — `pcap-file` is sync.
    let path = path.to_path_buf();
    let store = store.clone();
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let f = File::open(&path)?;
        let mut reader = PcapReader::new(BufReader::new(f))?;
        while let Some(pkt) = reader.next_packet() {
            let pkt = pkt?;
            let data = pkt.data;
            // The link type is a property of the file header, not the
            // packet. We assume LINKTYPE_IEEE802_11_RADIO (127) here,
            // which is what `tcpdump -I` writes. For LINKTYPE_IEEE802_11
            // (105) the radiotap header is absent and we'd parse data
            // directly; we treat anything we can't parse as a no-op.
            let (radiotap_len, payload) = strip_radiotap(&data);
            let (rssi, channel) = parse_radiotap_rssi_channel(&data[..radiotap_len]);
            if let Some(ev) = parse_frame(payload, rssi, channel) {
                store.apply(&ev);
                // Best-effort: drop the event if the SSE client is gone.
                let _ = tx.try_send(ev);
            }
        }
        Ok(())
    })
    .await??;
    Ok(())
}

/// Strip the radiotap header from a monitor-mode pcap packet.
/// Returns `(radiotap_len, payload)`. If the radiotap looks bogus we
/// fall back to treating the whole buffer as raw 802.11 (RSSI=0).
fn strip_radiotap(data: &[u8]) -> (usize, &[u8]) {
    if data.len() >= 4 && data[0] == 0 && data[1] == 0 {
        let len = u16::from_le_bytes([data[2], data[3]]) as usize;
        if (8..=data.len()).contains(&len) {
            return (len, &data[len..]);
        }
    }
    (0, data)
}

/// Pull RSSI + channel out of the radiotap header TLVs.
///
/// We scan for:
///   - TLV 22 (`IEEE80211_RADIOTAP_DB_ANTSIGNAL`), signed byte, dBm.
///   - TLV 18 (`IEEE80211_RADIOTAP_CHANNEL`), two bytes: (flags, channel).
///
/// Returns `(rssi_dbm, channel)`. Both default to 0 if not present.
fn parse_radiotap_rssi_channel(radiotap: &[u8]) -> (i8, u8) {
    if radiotap.len() < 8 {
        return (0, 0);
    }
    // Header: version(1), pad(1), length(2), present(4). Present words
    // start at offset 4. Walk them — bit 31 of each word signals "another
    // present word follows".
    let present_start = 4usize;
    let mut pos = present_start;
    let mut present: u32 = 0;
    loop {
        if pos + 4 > radiotap.len() {
            return (0, 0);
        }
        let word = u32::from_le_bytes([
            radiotap[pos],
            radiotap[pos + 1],
            radiotap[pos + 2],
            radiotap[pos + 3],
        ]);
        pos += 4;
        present |= word & !(1 << 31); // bits 0..30 only; bit 31 means "another word"
        if word & (1 << 31) == 0 {
            break;
        }
    }

    // Now walk the actual TLVs in order. We only care about TLVs 18 and
    // 22, but we still have to skip over the others (which have known
    // fixed sizes; this isn't a general radiotap parser).
    //
    // The set of present TLVs we'd typically see from a Linux `iw` driver:
    //   0 TSFT (8)
    //   1 Flags (1)
    //   2 Rate (1)
    //   3 Channel (2) + 2 bytes ChannelFlags
    //   4 FHSS (1)
    //   5 Antenna (1)
    //   6 DB Signal (1, signed)
    //   7 Noise (1, signed)
    //   ...
    // Modern radiotap can be variable-length, so we use the *standard*
    // ordering from the radiotap spec and known widths.
    let mut tlv_pos = pos;
    let mut rssi: i8 = 0;
    let mut channel: u8 = 0;
    for i in 0..32 {
        if present & (1 << i) == 0 {
            continue;
        }
        let (size, aligned_size) = tlv_size(i);
        if tlv_pos + size > radiotap.len() {
            break;
        }
        match i {
            3 => {
                // Channel: 2 bytes (freq MHz), then 2 bytes flags.
                if tlv_pos + 2 <= radiotap.len() {
                    let freq = u16::from_le_bytes([
                        radiotap[tlv_pos],
                        radiotap[tlv_pos + 1],
                    ]);
                    channel = freq_to_channel(freq);
                }
            }
            6 => {
                // DB antenna signal in dBm (signed byte).
                if tlv_pos < radiotap.len() {
                    rssi = radiotap[tlv_pos] as i8;
                }
            }
            _ => {}
        }
        tlv_pos += aligned_size;
    }
    (rssi, channel)
}

/// Fixed sizes for the radiotap TLVs we care about. Returns (size, aligned).
/// We only need the standard radiotap TLV widths here — see
/// https://www.radiotap.org/ for the full list.
fn tlv_size(bit: u32) -> (usize, usize) {
    match bit {
        0 => (8, 8),    // TSFT
        1 => (1, 1),    // Flags
        2 => (1, 1),    // Rate
        3 => (2, 4),    // Channel + flags
        4 => (1, 1),    // FHSS
        5 => (1, 1),    // Antenna
        6 => (1, 1),    // DB signal
        7 => (1, 1),    // Noise
        8 => (2, 2),    // lock quality
        9 => (2, 2),    // TX attenuation
        10 => (2, 2),   // DB TX attenuation
        11 => (1, 1),   // antenna noise
        12 => (4, 4),   // XChannel
        _ => (0, 0),
    }
}

/// Convert a frequency in MHz to a Wi-Fi channel number. Returns 0 if
/// the frequency isn't 2.4 GHz or 5 GHz Wi-Fi.
fn freq_to_channel(freq: u16) -> u8 {
    match freq {
        2412 => 1,
        2417 => 2,
        2422 => 3,
        2427 => 4,
        2432 => 5,
        2437 => 6,
        2442 => 7,
        2447 => 8,
        2452 => 9,
        2457 => 10,
        2462 => 11,
        2467 => 12,
        2472 => 13,
        2484 => 14,
        5160..=5885 => ((freq - 5000) / 5) as u8,
        _ => 0,
    }
}

/// Dev-mode: emit a deterministic synthetic stream that exercises the
/// EMA, MRU eviction, and SSE pipeline end-to-end.
async fn run_dev(store: Arc<DeviceStore>, tx: mpsc::Sender<DeviceEvent>) {
    tracing::info!("scanner: starting dev-mode synthetic stream");
    let mut tick: u64 = 0;
    loop {
        for i in 0..DEV_DEVICE_COUNT {
            // RSSI oscillates between -80 and -30 like a person pacing.
            let phase = (tick.wrapping_add(i as u64 * 7)) % 64;
            let rssi = -80 + (phase as i8) * 1; // -80..-16
            let rssi = rssi.clamp(-90, -20);
            let mac = format!(
                "02:00:00:00:{:02x}:{:02x}",
                (i >> 8) & 0xff,
                i & 0xff
            );
            let kind = match i % 3 {
                0 => FrameKind::Beacon,
                1 => FrameKind::Probe,
                _ => FrameKind::Data,
            };
            let ev = DeviceEvent {
                mac,
                kind,
                rssi_dbm: rssi,
                channel: 6,
            };
            store.apply(&ev);
            // If the SSE channel is full or closed, drop and keep going.
            if tx.try_send(ev).is_err() {
                // No-op; clients come and go.
            }
        }
        tick = tick.wrapping_add(1);
        sleep(DEV_FRAME_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_radiotap_returns_payload_offset() {
        // Radiotap header is 8 bytes: version=0, pad=0, length=8, present=0.
        let mut buf = vec![0u8; 8 + 4];
        buf[2] = 8; // length low
        buf[3] = 0;
        let (len, payload) = strip_radiotap(&buf);
        assert_eq!(len, 8);
        assert_eq!(payload.len(), 4);
    }

    #[test]
    fn strip_radiotap_falls_back_when_header_bogus() {
        let buf = vec![0xffu8; 32];
        let (len, payload) = strip_radiotap(&buf);
        assert_eq!(len, 0);
        assert_eq!(payload.len(), 32);
    }

    #[test]
    fn freq_to_channel_maps_24ghz_and_5ghz() {
        assert_eq!(freq_to_channel(2412), 1);
        assert_eq!(freq_to_channel(2437), 6);
        assert_eq!(freq_to_channel(2462), 11);
        assert_eq!(freq_to_channel(5240), 48);
        assert_eq!(freq_to_channel(0), 0);
    }

    #[test]
    fn parse_radiotap_extracts_rssi_and_channel() {
        // Minimal radiotap header: version=0, pad=0, length=12, present=0x48
        // (bits 3 + 6 set: Channel + DB signal). TLVs follow at offset 8.
        // TLV bit ordering: bit 0=TSFT, bit 1=Flags, bit 2=Rate, bit 3=Channel.
        // Channel TLV is 4 bytes (freq u16 LE + flags u16 LE), aligned to 4.
        // DB signal TLV is 1 byte, aligned to 1.
        //
        // Layout (offset 8 is the TLV start, channel comes first because
        // it's a lower bit number):
        //   8..10: freq = 2437 → channel 6
        //   10..12: channel flags = 0
        //   12:     DB signal = -60
        let mut buf = vec![0u8; 13];
        buf[2] = 13; // length (header 8 + TLVs 5)
        let present: u32 = (1 << 3) | (1 << 6);
        buf[4..8].copy_from_slice(&present.to_le_bytes());
        buf[8] = 0x85;
        buf[9] = 0x09;
        buf[10] = 0x00;
        buf[11] = 0x00;
        buf[12] = 0xc4;
        let (rssi, channel) = parse_radiotap_rssi_channel(&buf);
        assert_eq!(channel, 6);
        assert_eq!(rssi, -60);
    }
}