//! Pure-data 802.11 frame parser.
//!
//! Given a slice of bytes from an 802.11 monitor-mode capture, returns a
//! [`DeviceEvent`] describing what we observed. No I/O, no async, no
//! globals. Designed to be cheap and `no_std`-friendly enough that the
//! scanner can call it once per frame in a tight loop.
//!
//! We deliberately *only* pull out the three pieces of information the radar
//! UI cares about:
//!   - the transmitter MAC (so we can dedupe devices across frames)
//!   - the frame type (beacon / probe / data), used to colour the dot
//!   - the signal strength (RSSI, dBm), used to compute the polar radius
//!
//! Any frame we can't parse is silently skipped (we return `None`); the
//! scanner drops it without surfacing an error because monitor-mode traffic
//! is full of control frames we don't care about (ACK, RTS/CTS, etc.).
//!
//! Reference: IEEE Std 802.11-2020 §9.2 (frame format), §9.3.3.3
//! (Beacon), §9.3.3.6 (Probe Request), §9.3.3.7 (Probe Response),
//! §9.3.3.1 (Data).
//!
//! `pcap-file` exposes radiotap headers as a separate field on the
//! `PcapRecord`; we don't parse the radiotap TLVs here — the scanner feeds
//! us the raw 802.11 frame *after* the radiotap header has been stripped,
//! with the RSSI passed in alongside. See [`scanner`] for the full path.
//!
//! [`scanner`]: crate::scanner

/// What we observed for a single device on a single frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEvent {
    /// Transmitter MAC (6 bytes, lowercase hex, colon-separated).
    pub mac: String,
    /// Frame kind — drives the dot color and the radar sweep intensity.
    pub kind: FrameKind,
    /// Signal strength in dBm (negative; e.g. `-45`).
    pub rssi_dbm: i8,
    /// Wi-Fi channel the frame was heard on (used for the angle heuristic).
    pub channel: u8,
}

/// Discriminator for the three frame types we surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FrameKind {
    /// Access point beacon (advertising an SSID).
    Beacon,
    /// Client probe request (looking for a known network).
    Probe,
    /// Data frame (actual traffic — strongest "this device is here" signal).
    Data,
}

/// Minimum size of an 802.11 MAC header in non-QoS/non-HT: 24 bytes.
const MAC_HEADER_MIN: usize = 24;

/// Parse a raw 802.11 frame (radiotap already stripped) plus its RSSI +
/// channel as captured by the monitor-mode driver.
///
/// Returns `None` for any frame we don't recognise. Notably:
///   - control frames (ACK/RTS/CTS/...) are skipped — Frame Control byte 0
///     bits 3..2 == 0b01
///   - frames shorter than `MAC_HEADER_MIN` are skipped (corrupt)
///   - frames with a multicast/broadcast source address (bit 0 of byte 0)
///     are skipped — those aren't "a device near you," that's the AP
///     shouting
pub fn parse_frame(bytes: &[u8], rssi_dbm: i8, channel: u8) -> Option<DeviceEvent> {
    if bytes.len() < MAC_HEADER_MIN {
        return None;
    }

    let frame_type = frame_type(bytes);
    let kind = match frame_type {
        // Management frames (0b00) — sub-type in bits 7..4
        0b00 => {
            let subtype = (bytes[0] >> 4) & 0x0f;
            match subtype {
                0x08 => FrameKind::Beacon,    // §9.3.3.3
                0x04 => FrameKind::Probe,     // §9.3.3.6 (Probe Request)
                0x05 => FrameKind::Probe,     // §9.3.3.7 (Probe Response)
                _ => return None,
            }
        }
        // Data frames (0b10) — §9.3.3.1
        0b10 => FrameKind::Data,
        // Control frames (0b01) and reserved (0b11) — skip silently
        _ => return None,
    };

    // Source address is at byte offset 10..16 (Address 2 in a non-QoS
    // header). For To-DS data frames the source might be in Address 4;
    // we don't bother because we already have Address 2 and that's
    // "good enough" for surveillance — a device that *acts* as an AP
    // would only confuse the radar briefly and the EMA will smooth it.
    let src = &bytes[10..16];
    if src[0] & 0x01 != 0 {
        // Multicast / broadcast source — skip.
        return None;
    }

    let mac = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        src[0], src[1], src[2], src[3], src[4], src[5]
    );

    Some(DeviceEvent {
        mac,
        kind,
        rssi_dbm,
        channel,
    })
}

/// Extract the 802.11 frame type field from byte 0 of the MAC header.
/// Returns the raw 2-bit value so the caller can match on it.
fn frame_type(bytes: &[u8]) -> u8 {
    (bytes[0] >> 2) & 0x03
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake 802.11 frame for testing. `byte0` is the Frame Control
    /// byte 0; `src` is the source MAC.
    fn fake_frame(byte0: u8, src: [u8; 6]) -> Vec<u8> {
        let mut v = vec![0u8; MAC_HEADER_MIN];
        v[0] = byte0;
        v[1] = 0; // duration
        v[2..8].copy_from_slice(&[0xff; 6]); // Address 1 = broadcast
        v[8..10].copy_from_slice(&[0; 2]); // seq
        v[10..16].copy_from_slice(&src);
        v
    }

    #[test]
    fn parses_beacon_frame() {
        let src = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        // Type 0b00 (mgmt), subtype 0x08 (beacon) → byte0 = (0x08 << 4) | 0x00 = 0x80
        let frame = fake_frame(0x80, src);
        let ev = parse_frame(&frame, -45, 6).expect("beacon should parse");
        assert_eq!(ev.mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(ev.kind, FrameKind::Beacon);
        assert_eq!(ev.rssi_dbm, -45);
        assert_eq!(ev.channel, 6);
    }

    #[test]
    fn parses_probe_request_frame() {
        // Source MAC must have the multicast bit (LSB of byte 0) cleared.
        let src = [0x12, 0x22, 0x33, 0x44, 0x55, 0x66];
        // Mgmt, subtype 0x04 (probe req) → byte0 = 0x40
        let frame = fake_frame(0x40, src);
        let ev = parse_frame(&frame, -67, 11).expect("probe should parse");
        assert_eq!(ev.mac, "12:22:33:44:55:66");
        assert_eq!(ev.kind, FrameKind::Probe);
        assert_eq!(ev.rssi_dbm, -67);
    }

    #[test]
    fn parses_data_frame() {
        let src = [0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe];
        // Type 0b10 (data) → byte0 = (0x10 << 2) | 0x08 (from DS) = 0x48
        let frame = fake_frame(0x48, src);
        let ev = parse_frame(&frame, -80, 1).expect("data should parse");
        assert_eq!(ev.mac, "de:ad:be:ef:ca:fe");
        assert_eq!(ev.kind, FrameKind::Data);
    }

    #[test]
    fn rejects_control_frame() {
        // ACK = type 0b01, subtype 0x1c → byte0 = (0x1c << 4) | (0x01 << 2) = 0xd4
        let frame = fake_frame(0xd4, [0xaa; 6]);
        assert!(parse_frame(&frame, -50, 6).is_none());
    }

    #[test]
    fn rejects_too_short_frame() {
        let too_short = vec![0x80u8; 10];
        assert!(parse_frame(&too_short, -50, 6).is_none());
    }

    #[test]
    fn rejects_multicast_source() {
        // Source with the multicast bit set (LSB of byte 0).
        let src = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
        let frame = fake_frame(0x80, src);
        assert!(parse_frame(&frame, -50, 6).is_none());
    }

    #[test]
    fn rejects_unknown_mgmt_subtype() {
        // Mgmt, subtype 0x03 (reserved) → byte0 = 0x30
        let frame = fake_frame(0x30, [0xaa; 6]);
        assert!(parse_frame(&frame, -50, 6).is_none());
    }
}