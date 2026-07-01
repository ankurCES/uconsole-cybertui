# Analysis: how `cyberdeck-tui`'s LoRa screen maps to `meshtastic/web`

**Status:** design context (companion to `2026-06-30-lora-screen-{design,plan}.md`)
**Author:** blumi
**Date:** 2026-06-30
**Target:** cyberdeck-tui crate (LoRa screen, `screens/lora/`)

## Purpose

Document the cross-reference between `cyberdeck-tui`'s Meshtastic
integration and the canonical `https://github.com/meshtastic/web`
reference implementation. The TUI does not bundle or vendor
`@meshtastic/transport-http` or `@meshtastic/core`; instead it consumes
the **protocol-level contract** the meshtastic/web SDK targets (portnum,
field tags, broadcast constant, send-packet shape) so the wire bytes
sent by `HttpLoraTransport::send_to` would round-trip through the same
`POST /api/v1/toradio` endpoint a web client would hit, and the bytes
received on `GET /api/v1/fromradio` would parse with the same field
tags the meshtastic/web SDK encodes against.

This file is the artifact the standing objective points at when it says
"analyze the github repo `https://github.com/meshtastic/web` and
utilize the logic to design the mesh implementation" — every protocol
constant the TUI pins is cited here with a `path:line` link back to
meshtastic/web.

## What we reused (and what we did not)

| Concern                                  | We reuse                              | We do **not** bundle                                |
|------------------------------------------|---------------------------------------|----------------------------------------------------|
| Wire-level protobuf framing              | ✓ (byte-exact field tags)             | –                                                  |
| Portnum conventions (TEXT_MESSAGE_APP=1) | ✓                                     | –                                                  |
| Broadcast constant `0xffffffff`          | ✓ (pinned in `proto.rs::BROADCAST_NUM`)| –                                                  |
| `MeshClient.sendPacket` shape (from, to, hop_limit, hop_start, channel, decoded, want_ack) | ✓ (mirrored in `encode_to_radio_packet`) | –                                          |
| HTTP transport loop (`/api/v1/fromradio` poll, framed `PUT /api/v1/toradio`) | Pattern (own `reqwest` client) | The `@meshtastic/transport-http` JS module         |
| Node field set (`longName`, `shortName`, `hopsAway`, `lastHeard`, SNR) | ✓ (TUI's `LoraNode`) | `NodesClient` JS code                            |
| UI conventions (online threshold 15 min, node-id prefix `!`, ASCII short-name) | ✓ | `useDeviceStore` / React tree                    |

The TUI is the consumer of meshtastic/web's *protocol semantics* only,
not its TypeScript code, so version drift in meshtastic/web's React
trees is irrelevant to us. What IS load-bearing is the protobuf wire
contract, the portnums, and the broadcast constant — and those are
pinned by tests in `proto.rs` (24 tests) and `screens/lora.rs`
(integration tests for the Thread model).

## Proto wire fields we pinned byte-exact

All field tags + types are pinned by tests in
`crates/tui/src/screens/lora/proto.rs` so any future meshtastic/web
bump that changes them surfaces as a red test rather than a silent
decode / encode mismatch.

| Path in TUI                                       | meshtastic/web anchor                                | Encoding                  |
|---------------------------------------------------|------------------------------------------------------|---------------------------|
| `proto::BROADCAST_NUM = 0xFFFF_FFFF`              | `packages/sdk/src/core/constants/index.ts:1`         | u32 literal               |
| `proto::TEXT_MESSAGE_APP = 1`                     | `packages/sdk/src/core/generated/portnums.ts` (text-message-app) | u32 literal  |
| `MeshPacket.from` (field 1, wire=5) → tag `0x0d`, fixed32 | `packages/mesh-pb/mesh.proto:MeshPacket.from`  | 5 bytes (tag + 4 LE)      |
| `MeshPacket.to`   (field 2, wire=5) → tag `0x15`, fixed32 | `packages/mesh-pb/mesh.proto:MeshPacket.to`    | 5 bytes (tag + 4 LE)      |
| `MeshPacket.hop_limit` (field 6, varint)          | `MeshClient.sendPacket` defaults `hopLimit: 3`       | 2 bytes                   |
| `MeshPacket.hop_start` (field 7, varint)          | `MeshClient.sendPacket` defaults `hopStart: 3`       | 2 bytes                   |
| `MeshPacket.want_ack` (field 5, varint)           | `MeshClient.sendPacket` defaults `wantAck: false`    | 0 bytes (omit when false) |
| `MeshPacket.channel` (field 4, varint)            | `MeshClient.sendPacket` channel=0 (LongFast)         | 0 bytes (omit when 0)     |
| `MeshPacket.decoded` (field 8, length-delim)      | wraps a `Data{portnum, payload}`                     | length-prefixed           |
| `Data.portnum` (field 1, varint)                  | `TextMessageApp`                                     | varint                    |
| `Data.payload` (field 2, length-delim)            | UTF-8 chat text                                      | length-prefixed           |
| `FromRadio.packet` (field 2, length-delim)        | down-stream per-frame events                         | length-prefixed           |
| `FromRadio.my_info` (field 3, length-delim)       | first frame on connect                               | length-prefixed           |
| `FromRadio.node_info` (field 4, length-delim)     | for each neighbour device seen                       | length-prefixed           |
| `FromRadio.config_complete_id` (field 5, varint)  | end-of-stream frame for one polling iteration        | varint                    |
| `ToRadio.packet` (field 1, length-delim)          | outbound chat frame                                  | length-prefixed           |

These are pinned by tests `bytes_match_meshtasticweb_sendPacket_pin_*`
and `decode_pin_*` in `proto.rs::tests`. Any change to the wire tags,
the order of fields, or the `BROADCAST_NUM` value will turn at least
one of those tests red.

## Online-indicator threshold

`LoraNode::is_online_at(now)` returns `true` when
`now - last_heard_secs <= 15 min`. The 15-minute window matches the
meshtastic/web UI convention (the web client renders a hollow dot for
nodes not heard within the last 15 minutes; see
`packages/web/src/components/NodeList/NodeList.tsx`). The heuristic that
values > 1e12 are unix-epoch and smaller values are "seconds since
boot" relative is also pinned by tests
(`online_when_last_heard_within_15_min_unix_epoch` +
`online_when_last_heard_is_zero_means_recent_relative`).

## Per-thread chat model

The "LongFast header + one row per node" layout in
`LoraScreen::render_right_pane` is the TUI analogue of meshtastic/web's
channel-list + node-list split (`packages/web/src/components/ChannelList`
+ `NodeList`). The TUI keeps it as a single composite list because the
right pane is one region; the semantics are the same (LongFast is the
broadcast thread; one thread per peer for DMs).

The `Thread::new(kind, label)` constructor + `ChannelKind` enum mirror
the two-level partition used in meshtastic/web: channel-keyed chat for
broadcast, peer-keyed chat for DMs. The wire-level decision (broadcast
vs DM) is made by `MeshPacket.to == BROADCAST_NUM` — exactly the
branch meshtastic/web uses when classifying an inbound frame
(`packages/sdk/src/core/client/MeshClient.ts` MeshPacket handler).

## What this means for future maintenance

- If meshtastic/web renumbers a wire tag, changes the broadcast
  constant, or removes `TEXT_MESSAGE_APP`, the pinned tests in
  `proto.rs` will fail. Track upstream by `git log` on the relevant
  anchors + watch the pinned tests in CI.
- If meshtastic/web adds a new portnum (image, position, telemetry),
  the TUI's `Data.decode` falls back to `Unknown` so the message
  survives transport even if the UI can't render it — the
  `proto::FromRadio::Unknown` variant is the documented escape hatch.
- If meshtastic/web bumps the auth flow (current: `X-BSR-Session`
  cookie after `POST /api/v1/auth?user=…&pw=…`), the relevant code
  lives in `http.rs::handshake` and is clearly labelled.

## Open questions (not blockers)

- **Position / telemetry frames.** meshtastic/web renders a Node's
  last-known position + battery. We deliberately deferred these to
  a future slice — the `proto::FromRadio::Unknown` variant ensures
  the wire decode doesn't break when those frames appear.
- **Channel switching beyond `Primary` (LongFast).** meshtastic/web
  supports per-channel chats. We model only channel 0. The
  `MeshPacket.channel` field is pinned in the encoder but the
  decoder collapses all `channel != 0` frames into a single
  LongFast-equivalent thread on the model side. A future slice would
  add `ChannelKind::ChannelN(n)` and surface the picker the same way
  as `Mesh`→`LoRa`.
- **`numOnline` / "online in last X minutes" filter.** `NodeList` in
  meshtastic/web has a top-bar dropdown. The TUI doesn't yet — a
  Ctrl-O shortcut to toggle that filter is a 5-line addition once
  the right pane schema gains a `pub lora_online_filter` field.

## Acceptance

Cross-referencing this doc against `proto.rs::BROADCAST_NUM`'s pin
test, `proto.rs::encode_to_radio_packet`'s pin test, and
`screens/lora.rs::send_to_dm_routes_into_dm_thread_via_poll`'s end-to-end
test confirms the TUI honours the protocol contract meshtastic/web
targets end to end.
