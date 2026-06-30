# Implementation Plan: LoRa screen with Meshtastic HTTP node

**Spec:** `docs/superpowers/specs/2026-06-30-lora-screen-design.md`
**Target:** cyberdeck-tui (no other crates touched)

## Slice map

Each slice leaves the crate in a green state. After every slice,
`cargo test -p cyberdeck-tui --lib screens::lora` and
`cargo build -p cyberdeck-tui` pass.

### Slice 1 — Rename only (Mesh → LoRa, no behavior change)

Touch list (6 files):
1. `crates/tui/src/app/screen.rs` — `ScreenId::Mesh` → `ScreenId::LoRa`,
   label `"Mesh"` → `"LoRa"`, glyph unchanged (`≣`), `ALL` reorders the
   same. `has_right_pane` unchanged.
2. `crates/tui/src/app.rs` — rename `mesh_*` fields to `lora_*` on
   `App`. `default_mesh_transport` → `default_lora_transport`.
3. `crates/tui/src/screens/mod.rs` — `pub mod mesh;` → `pub mod lora;`.
4. `crates/tui/src/screens/mesh.rs` → `crates/tui/src/screens/lora.rs`
   (rename + verbatim content updates: type names `MeshNode`→
   `LoraNode`, `MeshChatLine`→`LoraChatLine`, `MeshTransport`→
   `LoraTransport`, `MeshScreen`→`LoraScreen`, `MeshError`→
   `LoraError`, `FakeTransport` stays `FakeTransport`).
5. `crates/tui/src/main.rs` — `ScreenId::Mesh` → `ScreenId::LoRa`,
   `screens::mesh::*` → `screens::lora::*`. Rename `poll` downcast
   target. Rename `boot_toast_sent` welcome string to mention LoRa
   only if it already mentioned Mesh (it doesn't — leave alone).
6. `crates/tui/src/app/screen.rs` tests — update
   `mesh_screen_is_registered` → `lora_screen_is_registered`, the
   `cycle_backward_wraps_around`/`cycle_forward_wraps_around` expected
   values, the `screen_renders_layout_audit` allowlist (delete `mesh`,
   add `lora`).

Acceptance:
- `cargo build -p cyberdeck-tui` clean.
- `cargo test -p cyberdeck-tui --lib` green (122 tests still pass).
- Sidebar shows `LoRa` instead of `Mesh`. Glyph unchanged.

### Slice 2 — `InputKind::LoraNodeIp` + modal open on `i`

Touch list (2 files):
1. `crates/tui/src/app.rs` — add `InputKind::LoraNodeIp` variant.
   Add `pub fn open_lora_ip_modal(&mut self)` helper.
2. `crates/tui/src/screens/lora.rs` — `on_key` consumes `i` and calls
   `app.open_lora_ip_modal()`.

Acceptance:
- `cargo build -p cyberdeck-tui` clean.
- `cargo test -p cyberdeck-tui --lib` green.
- New unit test: `lora_i_key_opens_ip_modal`.

### Slice 3 — IP validation + `HttpLoraTransport` (live backend)

Touch list (3 files):
1. `crates/tui/src/app.rs` — extend `InputKind` submit dispatch in
   `main.rs` with `LoraNodeIp` arm that validates the input and
   swaps `app.lora_transport`.
2. `crates/tui/src/screens/lora/http.rs` (new) — `HttpLoraTransport`
   behind a `reqwest = "0.12"` dep already vendored via
   `cyberdeck-core`. Polls `/api/v1/fromradio` every 3 s, PUTs to
   `/api/v1/toradio`. Tracks `connected` state. Implements
   `LoraTransport`.
3. `crates/tui/Cargo.toml` — add `reqwest = { version = "0.12",
   default-features = false, features = ["rustls-tls", "stream"] }`
   to `[dependencies]`. (Asking first — see Boundaries.)

Acceptance:
- `cargo build -p cyberdeck-tui` clean.
- New test: `http_lora_transport_marks_connected_on_probe_success`
  (uses `mockito`).
- New test: `http_lora_transport_marks_disconnected_on_probe_failure`.

### Slice 4 — `i` modal submit-dispatch wiring

Touch list (1 file):
1. `crates/tui/src/main.rs` — add the `InputKind::LoraNodeIp` arm to
   the `submit_input` dispatcher (around the existing
   `WifiEnterpriseIdentity` arm). Parse with
   `std::net::Ipv4Addr::from_str` + optional `:port`. Valid → swap
   transport + toast. Invalid → warn toast + re-open modal with text
   pre-filled.

Acceptance:
- `cargo test -p cyberdeck-tui --lib` green.
- New test: `lora_ip_modal_invalid_input_reopens_with_text`.
- New test: `lora_ip_modal_valid_input_swaps_transport_to_http`.

### Slice 5 — On-screen online indicator + hops field

Touch list (1 file):
1. `crates/tui/src/screens/lora.rs` — add `is_online` column to
   `LoraNode`, computed in `poll()` using
   `now - last_heard_secs < 900` (15-min threshold per spec). Render
   `●` / `○` glyph + hops in the right pane.

Acceptance:
- `cargo build -p cyberdeck-tui` clean.
- `cargo test -p cyberdeck-tui --lib` green.
- New test: `lora_node_is_online_within_threshold`.

### Slice 6 — Verify + PR

1. `cargo test -p cyberdeck-tui --lib` (full lib suite).
2. `cargo build -p cyberdeck-tui`.
3. `cargo clippy -p cyberdeck-tui --no-deps -- -D warnings`.
4. Commit each slice individually (atomic).
5. Push branch, open PR to `main` via `gh pr create`.

## Risks

- **`reqwest` dependency add.** Out of repo today. Asked-first per
  Boundaries. If user vetoes, fall back to a hand-rolled HTTP client
  using `std::net::TcpStream` (matches the existing PTY infra's
  portable-pty style) — more code, no new transitive deps.
- **Trait object safety.** The existing `MeshTransport` is object-safe;
  the renamed `LoraTransport` must stay that way (test
  `mesh_transport_is_object_safe` → `lora_transport_is_object_safe`
  pins it).
- **`ScreenId::ALL` ordering.** Critical — moving the entry shifts the
  sidebar number-key mapping. The slice-1 edits keep the order
  (LoRa at index 14, where Mesh was).
- **Layout-audit test** (`screen_renders_layout_audit`) is a
  string-level invariant — must be updated to point at `lora.rs`.

## Verification commands (targeted only, per project policy)

```bash
cargo test -p cyberdeck-tui --lib screens::lora
cargo test -p cyberdeck-tui --lib app::screen::tests
cargo test -p cyberdeck-tui --lib
cargo build  -p cyberdeck-tui
cargo clippy -p cyberdeck-tui --no-deps -- -D warnings
```

Never `cargo test --workspace` or `cargo test` (per project policy).