//! Public re-export tests for the 802.11 frame parser.
//!
//! Exists as a separate `tests/` file (instead of only the `#[cfg(test)] mod`
//! inside `frames.rs`) so external code can verify the parser contract by
//! importing through the public API.

use wifi_radar::frames::{parse_frame, FrameKind};

#[test]
fn public_parse_beacon_returns_expected_event() {
    let src = [0x02, 0x42, 0x63, 0x84, 0xa5, 0xc6];
    let mut frame = vec![0u8; 24];
    frame[0] = 0x80; // beacon
    frame[10..16].copy_from_slice(&src);
    let ev = parse_frame(&frame, -55, 6).expect("beacon parses");
    assert_eq!(ev.mac, "02:42:63:84:a5:c6");
    assert_eq!(ev.kind, FrameKind::Beacon);
}

#[test]
fn public_parse_returns_none_for_short_frame() {
    let bytes = vec![0u8; 4];
    assert!(parse_frame(&bytes, -55, 6).is_none());
}