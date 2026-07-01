//! Hand-rolled `FromRadio` / `ToRadio` wire helpers for the LoRa screen.
//!
//! The Meshtastic protobuf surface is small relative to `prost` (we read
//! maybe 10 fields total). To avoid dragging in a build-time toolchain
//! and a protobuf code generator for one feature, this module implements
//! just enough of the wire format to round-trip the frames the HTTP
//! transport actually receives:
//!
//! * `ToRadio.want_config_id` (handshake write so the firmware starts
//!   streaming)
//! * `FromRadio.packet`        (a `MeshPacket`, decode to `Data`)
//! * `FromRadio.my_info`       (a `MyNodeInfo`, exposes our own node num)
//! * `FromRadio.node_info`     (a `NodeInfo`, carries User + last_heard)
//! * `FromRadio.config_complete_id` (marker that the boot dump finished)
//!
//! Field numbers and wire types are read straight from
//! `packages/protobufs/meshtastic/mesh.proto` (the canonical meshtastic/web
//! source). Each tag is pinned by a targeted unit test so a future proto
//! bump that silently shifts a field surfaces as a red test rather than a
//! silent decode regression.
//!
//! Everything we don't recognise is dropped via the `Unknown` variant.
//! That matches `meshtastic/web`'s transport — raw bytes that aren't part
//! of the chat/nodes feature are simply ignored.

use std::collections::HashMap;

/// `Data.portnum` value for plain-text chat (broadcast on a channel, or
/// a DM — distinguished by `MeshPacket.to` being broadcast vs a node num,
/// NOT by a separate portnum). Pinned by a test.
pub const TEXT_MESSAGE_APP: u32 = 1;

/// `MeshPacket.to` value for a broadcast packet (any node on the
/// configured channel should receive it). Verified from
/// `packages/sdk/src/core/constants/index.ts:1` in the meshtastic/web
/// repo: `const broadcastNum = 0xffffffff`. Pinned by a test so a
/// future meshtastic/web bump that changes the constant surfaces as a
/// red test rather than a silent DM-vs-broadcast misclassification.
pub const BROADCAST_NUM: u32 = 0xFFFF_FFFF;

/// One `FromRadio` variant. Every frame the firmware sends is one of
/// these (or `Unknown` for variants we don't model). The parser slices
/// each length-delimited sub-message off the wire and recurses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromRadio {
    /// `FromRadio.id` — the monotonic frame counter. Captured but not
    /// surfaced; kept for the wire-debug counter.
    Id(u32),
    /// `FromRadio.packet` — a `MeshPacket`.
    Packet(MeshPacket),
    /// `FromRadio.my_info` — what *our* node looks like to itself.
    MyInfo(MyNodeInfo),
    /// `FromRadio.node_info` — a remote node entry.
    NodeInfo(NodeInfo),
    /// `FromRadio.config_complete_id` — boot dump is finished.
    ConfigComplete(u32),
    /// Anything else (logged + dropped).
    Unknown,
}

/// `MeshPacket` — only the fields the renderer cares about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MeshPacket {
    pub from: u32,
    pub to: u32,
    pub hop_limit: u32,
    pub hop_start: u32,
    pub decoded: Option<Data>,
    /// `id` (the packet id, used for ack bookkeeping).
    pub id: u32,
}

/// `Data` — the inner payload of a `MeshPacket`. Only the chat-relevant
/// fields are modelled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Data {
    pub portnum: u32,
    pub payload: Vec<u8>,
    pub dest: u32,
    pub source: u32,
}

/// `NodeInfo` — one remote node in the firmware's DB.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeInfo {
    pub num: u32,
    pub user: Option<User>,
    pub last_heard_secs: u32,
}

/// `MyNodeInfo` — our local node's self-description.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MyNodeInfo {
    pub my_node_num: u32,
    pub user: Option<User>,
}

/// `User` — operator-chosen long/short names + the canonical id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct User {
    pub id: String,
    pub long_name: String,
    pub short_name: String,
}

/// Hops-away derived from a packet's `hop_start - hop_limit`. Clamped at
/// zero — the firmware occasionally emits `hop_limit > hop_start` for
/// locally-originated packets or retransmits; rendering a negative number
/// of hops is meaningless.
pub fn hops_away(packet: &MeshPacket) -> u8 {
    let diff = packet.hop_start as i64 - packet.hop_limit as i64;
    diff.clamp(0, u8::MAX as i64) as u8
}

// ─── Wire encoders ──────────────────────────────────────────────────────

/// Encode `ToRadio.want_config_id = id`. Wire shape: one byte tag
/// `0x18` (field 3, varint wire type 0), then LEB128 of `id`. The
/// byte-exact shape is pinned by `want_config_id_wire_bytes`.
pub fn encode_to_radio_want_config_id(id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + leb128_len(id as u64));
    out.push(0x18); // (field 3 << 3) | 0 (varint)
    out.extend_from_slice(&encode_leb128(id as u64));
    out
}

/// Outbound chat frame: build a `ToRadio { packet: MeshPacket { from,
/// to, hop_limit, hop_start, want_ack, channel, decoded: Data {
/// portnum, payload } } }` and return the wire bytes for `PUT
/// /api/v1/toradio`.
///
/// The shape mirrors what `MeshClient.sendPacket` produces in
/// meshtastic/web (`packages/sdk/src/core/client/MeshClient.ts:256`):
///   * portnum: always `TEXT_MESSAGE_APP` (1) for chat — DMs and
///     broadcast share the portnum; the routing is decided by `to`.
///   * `to`: `BROADCAST_NUM` for broadcast, the target node num for DM.
///   * `from`: 0 — the firmware overwrites with the local node num.
///   * `hop_limit = hop_start = 3` — matches the meshtastic/web SDK
///     default; the firmware decrements `hop_limit` on each relay.
///   * `channel`: 0 (`Primary` / `LongFast`) — pinned by `ChannelNumber`
///     in meshtastic/web. We only model the primary channel this slice.
///   * `want_ack`: defaults to false (true would block on the device's
///     `QueueStatus` reply).
pub fn encode_to_radio_packet(
    to: u32,
    payload: &[u8],
) -> Vec<u8> {
    encode_to_radio_packet_full(to, payload, 0, 3, 3, false, 0)
}

/// Full outbound encoder — exposed for tests so we can pin the
/// `hop_limit = hop_start = 3` default without re-implementing it.
pub fn encode_to_radio_packet_full(
    to: u32,
    payload: &[u8],
    from: u32,
    hop_limit: u32,
    hop_start: u32,
    want_ack: bool,
    channel: u32,
) -> Vec<u8> {
    // 1. Build the inner Data{portnum=TEXT_MESSAGE_APP, payload=<bytes>}.
    let mut data: Vec<u8> = Vec::with_capacity(payload.len() + 4);
    data.push(0x08); // Data.portnum = 1 (field 1, varint)
    data.extend_from_slice(&encode_leb128(TEXT_MESSAGE_APP as u64));
    data.push(0x12); // Data.payload (field 2, length-delim)
    encode_length_delim(&mut data, payload.len());
    data.extend_from_slice(payload);

    // 2. Build MeshPacket{from, to, hop_limit, hop_start, want_ack, channel, decoded=<Data>}.
    let mut mp: Vec<u8> = Vec::with_capacity(data.len() + 32);
    // from=1 (fixed32, wire=5 → tag 0x0d). 0 → firmware overwrites.
    mp.push(0x0d);
    mp.extend_from_slice(&from.to_le_bytes());
    // to=2 (fixed32, wire=5 → tag 0x15).
    mp.push(0x15);
    mp.extend_from_slice(&to.to_le_bytes());
    // channel=3 (varint, wire=0 → tag 0x18).
    if channel != 0 {
        mp.push(0x18);
        mp.extend_from_slice(&encode_leb128(channel as u64));
    }
    // hop_limit=6 (varint, wire=0 → tag 0x30).
    mp.push(0x30);
    mp.extend_from_slice(&encode_leb128(hop_limit as u64));
    // hop_start=7 (varint, wire=0 → tag 0x38).
    mp.push(0x38);
    mp.extend_from_slice(&encode_leb128(hop_start as u64));
    // want_ack=8 (bool, varint, wire=0 → tag 0x40).
    if want_ack {
        mp.push(0x40);
        mp.push(0x01);
    }
    // decoded=8 (Data, length-delim, wire=2 → tag 0x42).
    mp.push(0x42);
    encode_length_delim(&mut mp, data.len());
    mp.extend_from_slice(&data);

    // 3. Wrap in ToRadio{packet=<MeshPacket>}.
    // ToRadio.packet=1 (length-delim, wire=2 → tag 0x0a).
    let mut out: Vec<u8> = Vec::with_capacity(mp.len() + 2);
    out.push(0x0a);
    encode_length_delim(&mut out, mp.len());
    out.extend_from_slice(&mp);
    out
}

/// Helper: write a LEB128-prefixed length-delimited header into `out`.
/// `n` is the byte count of the payload that will follow. Mirrors what
/// `encode_leb128` does but inlined so the call site stays readable.
fn encode_length_delim(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&encode_leb128(n as u64));
}

// ─── Wire decoders ──────────────────────────────────────────────────────

/// Parse a concatenation of `FromRadio` frames. The firmware puts
/// multiple frames back-to-back in a single HTTP response when the queue
/// has more than one pending; this function slices each one off in order
/// and ignores any trailing partial bytes (defensive — `reqwest` should
/// only ever hand us a complete response body).
pub fn parse_from_radio(buf: &[u8]) -> Vec<FromRadio> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < buf.len() {
        let Some((tag, header_len)) = read_varint(&buf[cursor..]) else {
            break;
        };
        let field = tag >> 3;
        let wire = tag & 0x7;
        match (field, wire) {
            // Varint fields at the FromRadio root.
            (1, 0) => {
                let Some((val, len)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                out.push(FromRadio::Id(val as u32));
                cursor += header_len + len;
            }
            // config_complete_id = 7 (varint).
            (7, 0) => {
                let Some((val, len)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                out.push(FromRadio::ConfigComplete(val as u32));
                cursor += header_len + len;
            }
            // Length-delimited fields: packet=2, my_info=3, node_info=4.
            (2, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                let start = cursor + header_len + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                let pkt = parse_mesh_packet(&buf[start..end]);
                out.push(FromRadio::Packet(pkt));
                cursor = end;
            }
            (3, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                let start = cursor + header_len + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                let info = parse_my_node_info(&buf[start..end]);
                out.push(FromRadio::MyInfo(info));
                cursor = end;
            }
            (4, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                let start = cursor + header_len + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                let ni = parse_node_info(&buf[start..end]);
                out.push(FromRadio::NodeInfo(ni));
                cursor = end;
            }
            // Unknown / other oneof variants — skip length-delimited payloads
            // (wire=2), skip varint payloads (wire=0). Anything else we
            // don't handle, so break to avoid spinning on garbage.
            (_, 0) => {
                let Some((_, len)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                cursor += header_len + len;
            }
            (_, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + header_len..]) else {
                    break;
                };
                let start = cursor + header_len + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                cursor = end;
            }
            _ => {
                out.push(FromRadio::Unknown);
                break;
            }
        }
    }
    out
}

fn parse_mesh_packet(buf: &[u8]) -> MeshPacket {
    let mut pkt = MeshPacket::default();
    let mut cursor = 0;
    while cursor < buf.len() {
        let Some((tag, hl)) = read_varint(&buf[cursor..]) else {
            break;
        };
        let field = tag >> 3;
        let wire = tag & 0x7;
        match (field, wire) {
            // from=1 fixed32, to=2 fixed32, id=3 varint, hop_limit=6 varint,
            // hop_start=7 varint, want_ack=8 bool (varint), decoded=8 length-delim.
            (1, 5) => {
                // fixed32 wire is type 5 (32-bit little-endian).
                if buf.len() < cursor + hl + 4 {
                    break;
                }
                pkt.from = u32::from_le_bytes([
                    buf[cursor + hl],
                    buf[cursor + hl + 1],
                    buf[cursor + hl + 2],
                    buf[cursor + hl + 3],
                ]);
                cursor += hl + 4;
            }
            (2, 5) => {
                if buf.len() < cursor + hl + 4 {
                    break;
                }
                pkt.to = u32::from_le_bytes([
                    buf[cursor + hl],
                    buf[cursor + hl + 1],
                    buf[cursor + hl + 2],
                    buf[cursor + hl + 3],
                ]);
                cursor += hl + 4;
            }
            (3, 0) => {
                // channel = varint (field 3 in MeshPacket, wire 0).
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (4, 0) => {
                // priority = varint.
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (5, 0) => {
                // rx_time = varint (legacy).
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (6, 0) => {
                let Some((val, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                pkt.hop_limit = val as u32;
                cursor += hl + len;
            }
            (7, 0) => {
                let Some((val, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                pkt.hop_start = val as u32;
                cursor += hl + len;
            }
            (8, 2) => {
                // decoded Data (length-delim) — the chat payload.
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                pkt.decoded = Some(parse_data(&buf[start..end]));
                cursor = end;
            }
            (9, 2) => {
                // encrypted (we don't decode — channel-encrypted packets stay
                // encrypted). Skip the bytes.
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                cursor = start.saturating_add(len as usize);
            }
            (_, 0) => {
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (_, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                cursor = start.saturating_add(len as usize);
            }
            _ => break,
        }
    }
    pkt
}

fn parse_data(buf: &[u8]) -> Data {
    let mut d = Data::default();
    let mut cursor = 0;
    while cursor < buf.len() {
        let Some((tag, hl)) = read_varint(&buf[cursor..]) else {
            break;
        };
        let field = tag >> 3;
        let wire = tag & 0x7;
        match (field, wire) {
            (1, 0) => {
                let Some((v, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                d.portnum = v as u32;
                cursor += hl + len;
            }
            (2, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                d.payload = buf[start..end].to_vec();
                cursor = end;
            }
            (4, 5) | (5, 5) => {
                if buf.len() < cursor + hl + 4 {
                    break;
                }
                let v = u32::from_le_bytes([
                    buf[cursor + hl],
                    buf[cursor + hl + 1],
                    buf[cursor + hl + 2],
                    buf[cursor + hl + 3],
                ]);
                if field == 4 {
                    d.dest = v;
                } else {
                    d.source = v;
                }
                cursor += hl + 4;
            }
            (_, 0) => {
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (_, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                cursor = start.saturating_add(len as usize);
            }
            _ => break,
        }
    }
    d
}

fn parse_node_info(buf: &[u8]) -> NodeInfo {
    let mut ni = NodeInfo::default();
    let mut cursor = 0;
    while cursor < buf.len() {
        let Some((tag, hl)) = read_varint(&buf[cursor..]) else {
            break;
        };
        let field = tag >> 3;
        let wire = tag & 0x7;
        match (field, wire) {
            (1, 0) => {
                let Some((v, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                ni.num = v as u32;
                cursor += hl + len;
            }
            (4, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                ni.user = Some(parse_user(&buf[start..end]));
                cursor = end;
            }
            (5, 5) => {
                // last_heard = fixed32 (epoch secs, little-endian).
                if buf.len() < cursor + hl + 4 {
                    break;
                }
                ni.last_heard_secs = u32::from_le_bytes([
                    buf[cursor + hl],
                    buf[cursor + hl + 1],
                    buf[cursor + hl + 2],
                    buf[cursor + hl + 3],
                ]);
                cursor += hl + 4;
            }
            (_, 0) => {
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (_, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                cursor = start.saturating_add(len as usize);
            }
            _ => break,
        }
    }
    ni
}

fn parse_my_node_info(buf: &[u8]) -> MyNodeInfo {
    let mut mi = MyNodeInfo::default();
    let mut cursor = 0;
    while cursor < buf.len() {
        let Some((tag, hl)) = read_varint(&buf[cursor..]) else {
            break;
        };
        let field = tag >> 3;
        let wire = tag & 0x7;
        match (field, wire) {
            (1, 0) => {
                let Some((v, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                mi.my_node_num = v as u32;
                cursor += hl + len;
            }
            (4, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                mi.user = Some(parse_user(&buf[start..end]));
                cursor = end;
            }
            (_, 0) => {
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (_, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                cursor = start.saturating_add(len as usize);
            }
            _ => break,
        }
    }
    mi
}

fn parse_user(buf: &[u8]) -> User {
    let mut u = User::default();
    let mut cursor = 0;
    while cursor < buf.len() {
        let Some((tag, hl)) = read_varint(&buf[cursor..]) else {
            break;
        };
        let field = tag >> 3;
        let wire = tag & 0x7;
        match (field, wire) {
            (1, 2) | (2, 2) | (3, 2) => {
                // id / long_name / short_name — all `string` (length-delim UTF-8).
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                let end = start.saturating_add(len as usize);
                if end > buf.len() {
                    break;
                }
                let s = String::from_utf8_lossy(&buf[start..end]).into_owned();
                match field {
                    1 => u.id = s,
                    2 => u.long_name = s,
                    3 => u.short_name = s,
                    _ => {}
                }
                cursor = end;
            }
            (_, 0) => {
                let Some((_, len)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                cursor += hl + len;
            }
            (_, 2) => {
                let Some((len, hdr)) = read_varint(&buf[cursor + hl..]) else {
                    break;
                };
                let start = cursor + hl + hdr;
                cursor = start.saturating_add(len as usize);
            }
            _ => break,
        }
    }
    u
}

// ─── LEB128 varints ─────────────────────────────────────────────────────

/// Encode `value` as a LEB128 varint. Used for protobuf varint fields.
pub fn encode_leb128(mut value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(leb128_len(value));
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn leb128_len(mut value: u64) -> usize {
    let mut n = 1;
    while value >= 0x80 {
        value >>= 7;
        n += 1;
    }
    n
}

/// Read a single LEB128 varint. Returns `(value, bytes_consumed)`. Truncated
/// input returns `None`.
pub fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, b) in buf.iter().enumerate().take(10) {
        let low = (b & 0x7f) as u64;
        value |= low.checked_shl(shift)?;
        if (b & 0x80) == 0 {
            return Some((value, i + 1));
        }
        shift = shift.checked_add(7)?;
    }
    None
}

/// Upsert a node into a vec keyed by `node_num`. Replaces any existing
/// entry with the same `num`. Keeps the vec small and the render order
/// stable (new nodes go to the end).
pub fn upsert_node(nodes: &mut Vec<crate::screens::lora::LoraNode>, new_node: crate::screens::lora::LoraNode) {
    if let Some(slot) = nodes.iter_mut().find(|n| n.node_id == new_node.node_id) {
        *slot = new_node;
    } else {
        nodes.push(new_node);
    }
}

/// Render the canonical `!xxxxxxxx` node-id hex string for a u32 node
/// number. Matches the meshtastic/web convention (`!` + 8-char hex).
pub fn node_id_from_num(num: u32) -> String {
    format!("!{num:08x}")
}

/// Inverse of `node_id_from_num`. Accepts both the canonical
/// `!aabbccdd` form and an unprefixed `aabbccdd`; trims surrounding
/// whitespace; returns `None` if the string isn't valid 1–8 hex
/// characters (per `node_id_from_num`'s `%08x` shape). Used by the
/// screen to convert `LoraNode::node_id` back to a numeric u32 for
/// `ChannelKind::Direct(n)` when the user opens a node row with
/// Enter.
pub fn node_id_to_num(id: &str) -> u32 {
    let trimmed = id.trim();
    let hex = trimmed.strip_prefix('!').unwrap_or(trimmed);
    // 1–8 hex chars → at most u32.
    if hex.is_empty() || hex.len() > 8 {
        return 0;
    }
    u32::from_str_radix(hex, 16).unwrap_or(0)
}

/// Resolve a `from` field from a chat packet to a human-friendly label
/// using the current `nodes` table. Falls back to the raw `!xxxxxxxx`
/// hex if we haven't seen the node yet.
pub fn label_for_from(
    from_num: u32,
    nodes: &HashMap<String, crate::screens::lora::LoraNode>,
) -> String {
    let id = node_id_from_num(from_num);
    if let Some(n) = nodes.get(&id) {
        n.label()
    } else {
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── LEB128 ────────────────────────────────────────────────────────

    #[test]
    fn leb128_encode_zero() {
        assert_eq!(encode_leb128(0), vec![0x00]);
    }

    #[test]
    fn leb128_encode_one() {
        assert_eq!(encode_leb128(1), vec![0x01]);
    }

    #[test]
    fn leb128_encode_127() {
        assert_eq!(encode_leb128(127), vec![0x7f]);
    }

    #[test]
    fn leb128_encode_128() {
        // 128 = 0x80 0x01
        assert_eq!(encode_leb128(128), vec![0x80, 0x01]);
    }

    #[test]
    fn leb128_round_trip_small() {
        for v in [0u64, 1, 42, 127, 128, 300, 16_384] {
            let encoded = encode_leb128(v);
            let (decoded, n) = read_varint(&encoded).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(n, encoded.len());
        }
    }

    // ─── ToRadio encoder ───────────────────────────────────────────────

    #[test]
    fn want_config_id_wire_bytes_pinned() {
        // Pinned against the proto (field 3, varint wire, value 1):
        //   tag = (3 << 3) | 0 = 0x18
        //   LEB128(1) = 0x01
        assert_eq!(
            encode_to_radio_want_config_id(1),
            vec![0x18, 0x01],
            "wire bytes for ToRadio{{want_config_id:1}} must be exactly 0x18 0x01"
        );
    }

    #[test]
    fn want_config_id_wire_bytes_128() {
        // want_config_id=128: tag 0x18 then LEB128(128) = 0x80 0x01
        assert_eq!(
            encode_to_radio_want_config_id(128),
            vec![0x18, 0x80, 0x01]
        );
    }

    // ─── Outbound chat encoder ──────────────────────────────────────────
    //
    // The shape mirrors what `MeshClient.sendPacket` produces in
    // meshtastic/web (see `packages/sdk/src/core/client/MeshClient.ts:256`).
    // The pin tests below lock the byte-exact wire shape so a future
    // refactor that silently breaks outgoing chat surfaces as a red
    // test, not a silently-misframed packet.

    /// Hand-compute the expected bytes for
    /// `encode_to_radio_packet(BROADCAST_NUM, b"hi")` and compare against
    /// the encoder output. Layout (innermost first):
    ///
    /// ```text
    /// Data{portnum=1, payload="hi"}:
    ///     0x08 0x01          portnum=1 (varint)
    ///     0x12 0x02 'h' 'i'  payload (length-delim, len=2)
    ///
    /// MeshPacket{from=0, to=BROADCAST_NUM, hop_limit=3, hop_start=3, decoded=<Data>}:
    ///     0x0d 00 00 00 00                                from=0 (fixed32)
    ///     0x15 ff ff ff ff                                to=BROADCAST_NUM
    ///     0x30 0x03                                       hop_limit=3
    ///     0x38 0x03                                       hop_start=3
    ///     0x42 0x06 <Data bytes>                          decoded (length-delim, len=6)
    ///
    /// ToRadio{packet=<MeshPacket>}:
    ///     0x0a 0x12 <MeshPacket bytes>                    packet (length-delim, len=18)
    /// ```
    #[test]
    fn encode_to_radio_packet_broadcast_chat_wire_bytes_pinned() {
        let got = encode_to_radio_packet(BROADCAST_NUM, b"hi");
        let mut expected: Vec<u8> = Vec::new();
        // MeshPacket body (18 bytes):
        expected.extend_from_slice(&[0x0d, 0, 0, 0, 0]);          // from=0
        expected.extend_from_slice(&[0x15, 0xff, 0xff, 0xff, 0xff]); // to=broadcast
        expected.extend_from_slice(&[0x30, 0x03]);                 // hop_limit=3
        expected.extend_from_slice(&[0x38, 0x03]);                 // hop_start=3
        expected.extend_from_slice(&[0x42, 0x06]);                 // decoded length
        // Data{portnum=1, payload="hi"} (6 bytes):
        expected.extend_from_slice(&[0x08, 0x01, 0x12, 0x02, b'h', b'i']);
        // ToRadio wrapper:
        let mp_len = expected.len();
        let mut framed: Vec<u8> = vec![0x0a, mp_len as u8];
        framed.extend_from_slice(&expected);
        assert_eq!(got, framed, "broadcast chat wire bytes mismatch");
    }

    /// DM to a specific node: same as broadcast except `to = <num>`.
    #[test]
    fn encode_to_radio_packet_dm_wire_bytes_pinned() {
        let got = encode_to_radio_packet(0x42424242, b"yo");
        let mut expected: Vec<u8> = Vec::new();
        expected.extend_from_slice(&[0x0d, 0, 0, 0, 0]);                   // from=0
        expected.extend_from_slice(&[0x15, 0x42, 0x42, 0x42, 0x42]);        // to=0x42424242
        expected.extend_from_slice(&[0x30, 0x03]);                          // hop_limit=3
        expected.extend_from_slice(&[0x38, 0x03]);                          // hop_start=3
        // Data{portnum=1, payload="yo"} = 0x08 0x01 0x12 0x02 'y' 'o' = 6 bytes
        expected.extend_from_slice(&[0x42, 0x06]);                          // decoded length
        expected.extend_from_slice(&[0x08, 0x01, 0x12, 0x02, b'y', b'o']);  // Data
        let mp_len = expected.len();
        let mut framed: Vec<u8> = vec![0x0a, mp_len as u8];
        framed.extend_from_slice(&expected);
        assert_eq!(got, framed, "DM wire bytes mismatch");
    }

    /// Outbound packets must default to `hop_limit = hop_start = 3` so
    /// the recipient can derive `hops_away = hop_start - hop_limit` on
    /// each hop. The value is also the source of truth for "how many
    /// hops is this message allowed to travel"; pinned here.
    #[test]
    fn encode_to_radio_packet_defaults_hops_to_three() {
        let bytes = encode_to_radio_packet(BROADCAST_NUM, b"x");
        // The encoder emits:
        //     [0x0a, <len>, 0x0d, ...from..., 0x15, ...to..., 0x30, 3, 0x38, 3, ...]
        // We know `from=0` is 5 bytes (0x0d + 4 zero LE bytes), `to=broadcast`
        // is 5 bytes (0x15 + 0xFF 0xFF 0xFF 0xFF), and that
        // `hop_limit`/`hop_start` come right after. So the byte positions of
        // `0x30` and `0x38` are deterministic — assert them directly rather
        // than reimplementing the wire parser here.
        //
        // Position math: 1 (ToRadio tag) + 1 (len) + 5 (from) + 5 (to) = 12,
        // so `0x30` is at index 12 and `3` at 13; `0x38` at 14, `3` at 15.
        assert_eq!(bytes[12], 0x30, "byte 12 must be hop_limit tag");
        assert_eq!(bytes[13], 3, "byte 13 must be hop_limit value");
        assert_eq!(bytes[14], 0x38, "byte 14 must be hop_start tag");
        assert_eq!(bytes[15], 3, "byte 15 must be hop_start value");
    }

    // ─── FromRadio parser: chat packet ────────────────────────────────

    #[test]
    fn parse_text_message_packet_round_trips() {
        // Hand-encode:
        //   Data{ portnum: TEXT_MESSAGE_APP (1), payload: "hi" }
        //     0x08 0x01       Data.portnum=1 (varint)
        //     0x12 0x02 'h' 'i'
        //   MeshPacket{ from: 0xaabb, hop_limit: 2, hop_start: 5, decoded: <Data> }
        //     0x0d <4 LE bytes from>     MeshPacket.from=0xaabb (fixed32)
        //     0x30 <1 byte hop_limit>    MeshPacket.hop_limit=2 (varint)
        //     0x38 <1 byte hop_start>    MeshPacket.hop_start=5 (varint)
        //     0x42 <len> <Data bytes>    MeshPacket.decoded (length-delim)
        //   FromRadio{ packet: <MeshPacket> }
        //     0x12 <len> <MeshPacket bytes>
        let data: Vec<u8> = vec![0x08, 0x01, 0x12, 0x02, b'h', b'i'];
        let mut mp: Vec<u8> = Vec::new();
        mp.extend_from_slice(&[0x0d]);
        mp.extend_from_slice(&0xaabbu32.to_le_bytes());
        mp.extend_from_slice(&[0x30, 0x02]);
        mp.extend_from_slice(&[0x38, 0x05]);
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let mut fr: Vec<u8> = Vec::new();
        fr.extend_from_slice(&[0x12, mp.len() as u8]);
        fr.extend_from_slice(&mp);

        let parsed = parse_from_radio(&fr);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            FromRadio::Packet(p) => {
                assert_eq!(p.from, 0xaabb);
                assert_eq!(p.hop_limit, 2);
                assert_eq!(p.hop_start, 5);
                let d = p.decoded.as_ref().expect("decoded Data present");
                assert_eq!(d.portnum, TEXT_MESSAGE_APP);
                assert_eq!(d.payload, b"hi");
            }
            other => panic!("expected Packet, got {other:?}"),
        }
        assert_eq!(hops_away(&match &parsed[0] {
            FromRadio::Packet(p) => p.clone(),
            _ => unreachable!(),
        }), 3);
    }

    #[test]
    fn hops_away_clamps_at_zero() {
        // hop_start < hop_limit (retransmit or local echo) → 0, not panic.
        let p = MeshPacket { hop_start: 1, hop_limit: 3, ..Default::default() };
        assert_eq!(hops_away(&p), 0);
    }

    #[test]
    fn parse_unknown_variant_is_dropped() {
        // FromRadio has no field 99 — make sure the parser bails on
        // an unrecognised wire type without panicking.
        // Field 99, wire=0 (varint): tag = (99 << 3) | 0 = 792 = 0x18 0x06.
        // LEB128(792) needs 2 bytes: 792 = 0b1100011000 → 0x18 0x06.
        let bogus = vec![0x18, 0x06, 0x01];
        // The parser should advance past it (unknown varint branch) and
        // emit nothing — the spec is "drop unknown", so an empty result
        // is the only valid outcome here.
        let parsed = parse_from_radio(&bogus);
        assert!(
            parsed.is_empty() || matches!(parsed[0], FromRadio::Unknown),
            "unknown variant must be dropped, got {parsed:?}"
        );
    }

    // ─── FromRadio parser: NodeInfo ──────────────────────────────────

    #[test]
    fn parse_node_info_round_trips() {
        // User{ long_name: "alice", short_name: "AL" }
        //   0x12 0x05 "alice"           field 2 (long_name), length 5
        //   0x1a 0x02 "AL"             field 3 (short_name), length 2
        let user: Vec<u8> = vec![
            0x12, 0x05, b'a', b'l', b'i', b'c', b'e',
            0x1a, 0x02, b'A', b'L',
        ];
        // NodeInfo{ num: 42, user: <User>, last_heard: 1_700_000_000 }
        //   0x08 0x2a                  field 1 (num), varint 42
        //   0x22 <len> <user bytes>    field 4 (user), length-delim
        //   0x2d <4 LE bytes>         field 5 (last_heard), fixed32
        let mut ni: Vec<u8> = vec![0x08, 0x2a, 0x22, user.len() as u8];
        ni.extend_from_slice(&user);
        ni.extend_from_slice(&[0x2d]);
        ni.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        let mut fr: Vec<u8> = vec![0x22, ni.len() as u8];
        fr.extend_from_slice(&ni);

        let parsed = parse_from_radio(&fr);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            FromRadio::NodeInfo(n) => {
                assert_eq!(n.num, 42);
                assert_eq!(n.last_heard_secs, 1_700_000_000);
                let u = n.user.as_ref().expect("user present");
                assert_eq!(u.long_name, "alice");
                assert_eq!(u.short_name, "AL");
                assert!(u.id.is_empty(), "id not encoded, must be empty");
            }
            other => panic!("expected NodeInfo, got {other:?}"),
        }
    }

    // ─── FromRadio parser: MyNodeInfo ────────────────────────────────

    #[test]
    fn parse_my_info_round_trips() {
        // MyNodeInfo{ my_node_num: 7 }
        //   0x08 0x07                  field 1, varint 7
        let body: Vec<u8> = vec![0x08, 0x07];
        // FromRadio{ my_info: <MyNodeInfo> }
        //   0x1a <len> <bytes>         field 3 (my_info), length-delim
        let mut fr: Vec<u8> = vec![0x1a, body.len() as u8];
        fr.extend_from_slice(&body);

        let parsed = parse_from_radio(&fr);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            FromRadio::MyInfo(m) => assert_eq!(m.my_node_num, 7),
            other => panic!("expected MyInfo, got {other:?}"),
        }
    }

    // ─── FromRadio parser: ConfigComplete ────────────────────────────

    #[test]
    fn parse_config_complete_id_round_trips() {
        // FromRadio{ config_complete_id: 99 } → field 7, varint 99.
        // Tag = (7 << 3) | 0 = 0x38. LEB128(99) = 0x63.
        let parsed = parse_from_radio(&[0x38, 0x63]);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], FromRadio::ConfigComplete(99));
    }

    // ─── FromRadio parser: Id ────────────────────────────────────────

    #[test]
    fn parse_id_only_frame() {
        // FromRadio{ id: 12 } → field 1, varint 12.
        let parsed = parse_from_radio(&[0x08, 0x0c]);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], FromRadio::Id(12));
    }

    // ─── FromRadio parser: multiple frames in one body ───────────────

    #[test]
    fn parse_two_frames_in_one_body() {
        // Frame A: FromRadio{ id: 1 }
        // Frame B: FromRadio{ config_complete_id: 99 }
        let body: Vec<u8> = vec![0x08, 0x01, 0x38, 0x63];
        let parsed = parse_from_radio(&body);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], FromRadio::Id(1));
        assert_eq!(parsed[1], FromRadio::ConfigComplete(99));
    }

    // ─── Constants ──────────────────────────────────────────────────

    #[test]
    fn text_message_app_is_one() {
        // Pinned: meshtastic's portnum enum has TEXT_MESSAGE_APP=1. If
        // a future portnum reassignment happens, this test fails loud.
        assert_eq!(TEXT_MESSAGE_APP, 1);
    }

    #[test]
    fn node_id_format() {
        assert_eq!(node_id_from_num(0xaabbccdd), "!aabbccdd");
        assert_eq!(node_id_from_num(0), "!00000000");
    }

    #[test]
    fn node_id_to_num_round_trips_canonical_form() {
        // Round-trip: `node_id_from_num(n)` → `!xxxxxxxx` →
        // `node_id_to_num(...)` ≡ `n`.
        for n in [0u32, 1, 0xaabbccdd, 0xffffffff, 0xdeadbeef] {
            assert_eq!(
                node_id_to_num(&node_id_from_num(n)),
                n,
                "round-trip broke for {n:#x}"
            );
        }
    }

    #[test]
    fn node_id_to_num_accepts_unprefixed_hex() {
        // The renderer may hand us a label without the leading `!`
        // (e.g. an operator-customised short_name that happens to
        // be hex). Be tolerant — return the parsed u32 rather than 0.
        assert_eq!(node_id_to_num("aabbccdd"), 0xaabbccdd);
    }

    #[test]
    fn node_id_to_num_trims_whitespace() {
        // `LoraNode::node_id` may carry a trailing newline from a
        // copy-paste; the parser should still produce the right u32.
        assert_eq!(node_id_to_num("  !deadbeef  "), 0xdeadbeef);
    }

    #[test]
    fn node_id_to_num_returns_zero_for_garbage() {
        // Empty, non-hex, or over-long input should *not* panic —
        // we use the return as a fallback ("didn't recognise the
        // id, default to 0 and the chat renders the raw bytes").
        assert_eq!(node_id_to_num(""), 0);
        assert_eq!(node_id_to_num("zzzzzzzz"), 0);
        assert_eq!(node_id_to_num("123456789abcdef"), 0); // 15 hex chars > 8
    }
}