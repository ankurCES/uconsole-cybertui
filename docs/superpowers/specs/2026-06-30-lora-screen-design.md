# Spec: LoRa screen with Meshtastic HTTP node

**Status:** design (not yet implemented)
**Author:** blumi
**Date:** 2026-06-30
**Target:** cyberdeck-tui crate

## Objective

Replace the existing sidebar `Mesh` item with `LoRa` and give it a real
on-LAN Meshtastic node connection. The user types an IP address into a
modal, the TUI connects to the Meshtastic device's HTTP API at
`http://<ip>`, and the screen renders the device's live longfast chat
(left pane) and node DB with online status + hops (right pane).

The change borrows the structure of `meshtastic/web`'s
`@meshtastic/transport-http` (HTTP poll loop, framed write, status
emission) and `NodesClient` (node field set) but uses the project's
existing `MeshTransport` trait and `FakeTransport` so unit tests stay
in-process and the renderer is never blocked on a real socket.

No existing feature regresses: the existing `MeshScreen` polling path,
`MeshTransport` trait, and `FakeTransport` shape are preserved 1:1 and
simply renamed to the LoRa vocabulary. The sidebar item label/glyph,
`ScreenId` variant, and state fields all rename in lock-step.

## Tech Stack

- Rust 2021, edition `1.80` (workspace default)
- ratatui `0.29` (existing)
- crossterm `0.28` (existing)
- tokio `1.40` with `full` (existing)
- reqwest `0.12` (new, **dev-only** `reqwest` feature on `cyberdeck-core`)
  — used by the live `LoraTransport` HTTP backend only; the in-process
  `FakeTransport` keeps the test surface free of any HTTP I/O
- prost / protobuf decoding: **out of scope** for this slice. The
  Meshtastic HTTP `/api/v1/fromradio` payload is a binary protobuf
  stream. For the first slice we expose the raw `Vec<u8>` response as a
  wire-debug pane so the UI works end-to-end against a real node
  without committing to a full proto decode. Text chat and node parsing
  decode is a follow-up slice (see *Open questions*).

## Commands

This slice adds no top-level workspace commands — only internal
`cargo test -p cyberdeck-tui` runs. Verification commands used during
implementation:

```bash
# Targeted test runs (per project policy: never full workspace suite).
cargo test -p cyberdeck-tui --lib screens::lora
cargo test -p cyberdeck-tui --lib screens::lora::tests
cargo test -p cyberdeck-tui --lib app::screen::tests::screen_renders_layout_audit
cargo test -p cyberdeck-tui --lib
cargo build  -p cyberdeck-tui
cargo clippy -p cyberdeck-tui --no-deps -- -D warnings
```

## Project Structure

The change is contained to `crates/tui/src/screens/`:

```
crates/tui/src/screens/
├── lora.rs        # new — replaces mesh.rs, same shape, LoRa vocabulary
├── lora/
│   ├── mod.rs     # re-exports
│   ├── http.rs    # HttpLoraTransport (reqwest, behind feature)
│   ├── fake.rs    # FakeTransport (was inside mesh.rs)
│   └── tests.rs   # tests (was inside mesh.rs)
```

`crates/tui/src/screens/mesh.rs` is deleted.

The sidebar item rename + `ScreenId::Mesh → ScreenId::LoRa` touches:
- `crates/tui/src/app/screen.rs` — enum variant, label, glyph, ALL list, layout audit
- `crates/tui/src/app.rs` — state fields rename (`mesh_*` → `lora_*`) and the `default_lora_transport` constructor
- `crates/tui/src/main.rs` — screen dispatch, `poll` call site, modal open on `i`

The modal reuse pattern follows the existing `Modal::Input` /
`InputKind::ConnectSSID` flow: a new `InputKind::LoraNodeIp` carries
the prompt semantics, and the existing submit-dispatch in `main.rs`
opens / spawns the connect action.

## Code Style

Existing project conventions (per ROADMAP + the 122 tests already in
the binary):

- snake_case fields, PascalCase types
- snake_case module paths in `mod foo { ... }`
- `pub use` re-exports of the per-feature units so callers don't
  reach into submodules
- `tracing::{debug,info,error}` not `println!` for diagnostics
- `thiserror` enums for transport errors; `anyhow::Result` only at
  the `main` boundary
- Tests inline in `mod tests` at the bottom of the module; each test
  uses `FakeTransport` (no network); targeted assertions on the
  shared `App` state fields

Example of the modal open (matches existing `app.open_input(...,
InputKind::WifiPassword)` style):

```rust
// On `i` keypress from the LoRa screen:
app.open_input("Meshtastic node IP", InputKind::LoraNodeIp);
```

## Data flow

```
┌──────────────────────────────────────────────────────────────┐
│ LoRa screen (TUI)                                            │
│   ┌────────────────────────┐   ┌──────────────────────────┐  │
│   │ longfast chat (left)   │   │ nodes + hops + status    │  │
│   └────────────────────────┘   └──────────────────────────┘  │
│   input strip → Enter → transport.send_longfast(text)         │
└──────────────────────────────────────────────────────────────┘
              │
              ▼ poll() on every Action::Tick
┌──────────────────────────────────────────────────────────────┐
│ LoraTransport trait (object-safe, Send)                      │
│   nodes() -> Vec<LoraNode>                                   │
│   messages() -> Vec<LoraChatLine>                            │
│   connected() -> bool                                        │
│   send_longfast(text) -> Result<(), LoraError>               │
└──────────────────────────────────────────────────────────────┘
              │                       │
              ▼                       ▼
   ┌───────────────────┐     ┌────────────────────────────┐
   │ FakeTransport     │     │ HttpLoraTransport          │
   │ (tests, no IO)    │     │ reqwest GET /fromradio     │
   │                   │     │ reqwest PUT /toradio       │
   └───────────────────┘     └────────────────────────────┘
                                          │
                                          ▼
                              http://<ip>:80 / 443
                              Meshtastic device
```

## IP modal flow

1. User presses `i` while the LoRa screen is focused.
2. Screen calls `app.open_input("Meshtastic node IP", InputKind::LoraNodeIp)`.
3. Modal collects an IPv4 string (`d.d.d.d[:p]`).
4. On Enter: submit handler in `main.rs` validates the address (parses
   with `std::net::IpAddr::from_str`; optional `:port` suffix). On
   invalid input, push a `ToastKind::Warn` and re-open the modal with
   the offending text pre-filled.
5. Valid → `app.lora_transport = Box::new(HttpLoraTransport::new(addr))`
   + `app.push_toast(ToastKind::Info, "lora: connecting to <ip>")`.
6. The next `poll()` flips `lora_connected = true` once the first
   successful `/api/v1/fromradio` round-trip lands; failure flips it
   back to `false` and pushes an error toast on transition.

## Error handling

| Failure                    | UX                                              |
|----------------------------|-------------------------------------------------|
| Invalid IP in modal        | Warn toast, re-open modal with text preserved   |
| HTTP non-2xx on probe      | Error toast, modal closes, screen shows connect prompt |
| HTTP timeout (read)        | Silent retry; `lora_connected=false` until next 2xx |
| HTTP timeout (write)       | Error toast on send, transport kept             |
| Empty / overlong message   | Warn toast, no write (existing behavior preserved) |

## Testing Strategy

Targeted `cargo test -p cyberdeck-tui --lib screens::lora` runs only.
The Mesh-screen test suite moves 1:1 into `lora::tests` with the rename
applied (`FakeTransport`, `MeshNode → LoraNode`, etc.). New tests:

1. `http_lora_transport_marks_connected_on_first_fromradio` — using a
   `mockito` server, asserts `connected()` flips after the first
   successful GET (so the user sees the connect dot before any
   message has arrived).
2. `http_lora_transport_marks_disconnected_on_probe_failure` — uses a
   closed port, asserts `connected() == false` after the failure path.
3. `modal_lora_node_ip_dispatches_to_http_transport` — wires a
   `Modal::Input { kind: InputKind::LoraNodeIp, ... }` through the
   submit-dispatch in `main.rs` and asserts the transport pointer
   was swapped to a `HttpLoraTransport` (or the fake equivalent).
4. `lora_node_label_prefers_long_then_short_then_id` — verbatim port
   of the existing test, locks the rename.
5. Existing layout-audit test in `app/screen.rs` updates its
   `MULTI` allowlist to include `lora.rs` and remove `mesh.rs`.

The sidebar vocabulary / screen-id / glyph regression tests in
`app/screen.rs` are updated so the `▶ LoRa` label and the `≣` glyph
swap are locked in.

## Boundaries

- **Always:** run targeted `cargo test -p cyberdeck-tui` after each
  slice; never run the full workspace suite; never break the 122-test
  baseline; keep the `MeshTransport` trait object-safe; never introduce
  `unsafe`.
- **Ask first:** adding new dependencies to `Cargo.toml`
  (`reqwest = "0.12"` is the only one proposed and is needed for the
  live HTTP backend); bumping any dep past what the workspace already
  pins; touching the WM tree; touching the embedded web-server in
  `crates/web`.
- **Never:** commit secrets or real node IPs to tests; edit vendor
  directories; remove a failing test without explicit user approval;
  change `ScreenId::ALL` ordering (which would break
  `number_keys_when_sidebar_focused_move_cursor_to_that_row`).

## Success Criteria

1. Sidebar shows `LoRa` (not `Mesh`) at the position the Mesh item
   previously held. `ScreenId::LoRa` variant exists; the old `Mesh`
   variant is gone.
2. `cargo build -p cyberdeck-tui` is clean with no new warnings.
3. `cargo clippy -p cyberdeck-tui --no-deps -- -D warnings` is clean.
4. `cargo test -p cyberdeck-tui --lib` passes — the existing 122 tests
   stay green after the rename, and the new LoRa tests pass.
5. From the LoRa screen, pressing `i` opens a modal titled
   "Meshtastic node IP". Entering `10.10.0.57` (or any
   `<ipv4>[:port]`) swaps the transport to the live HTTP backend;
   entering garbage shows a warn toast and re-opens the modal.
6. While connected, the longfast chat left pane updates from
   `/api/v1/fromradio` polls; the right pane shows nodes with their
   `hopsAway` and `isOnline` (online = `lastHeard` within the
   configured online threshold, default 15 minutes).
7. The 14 existing screen-id/layout/dispatch regression tests
   (`screen_renders_layout_audit`, `cycle_*`, `mesh_screen_is_registered`
   → `lora_screen_is_registered`, …) still pass after the rename.
8. A PR is opened to `main` with a clear title + summary, and CI
   green before merge.

## Out of scope (this slice)

- Decoding the protobuf `/api/v1/fromradio` payload into chat lines
  and node records. The HTTP backend exposes the raw frame count and
  a "wire-debug" pane (raw hex of the last received frame) so the user
  can confirm the link is up. Full proto parsing lands in a follow-up
  slice — it needs a prost submodule + the meshtastic protobufs pulled
  in as a path dep, which is a separate decision.
- Persisting the IP across restarts. TUI is process-local; the
  connection dies on exit. Persistence goes behind the existing
  `--config` flag in a follow-up.
- TLS / `https://`. The Meshtastic HTTP API is plain HTTP on the LAN
  by default. Toggle for TLS is a one-line change in
  `HttpLoraTransport::new` later.
- BLE / USB transports. Web repo's `transport-web-ble`,
  `transport-web-serial`, `transport-node-serial` are not in scope.

## Open Questions

- Confirm the user is OK with **raw wire-debug** in this slice vs.
  blocking on protobuf decode first. My recommendation is ship
  wire-debug now + decode next, because (a) the rename + modal +
  transport trait swap is independently valuable, (b) the decode
  needs a proto dependency decision the user should make separately,
  (c) wire-debug lets the user verify the link is up before we commit
  to a wire format. If the user prefers decode-first, this spec
  becomes a single larger slice.
- Online threshold: 15 minutes matches Meshtastic's own UI convention.
  Is that the right default here, or do we want a shorter window
  (e.g. 2 minutes) since LoRa activity is bursty?