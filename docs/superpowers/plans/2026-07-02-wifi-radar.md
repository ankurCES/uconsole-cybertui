# wifi-radar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A constantly-accessible browser UI served by a new `crates/wifi-radar/` workspace member. Backend scans nearby Wi-Fi (passive monitor mode via libpcap, with a synthetic-frame fallback so the crate builds + runs on dev machines that can't actually scan), tags known devices to people, and pushes a live device stream to the browser over Server-Sent Events. Frontend is a single-page app with an OpenScope-style radar canvas that renders tagged devices as named people/object icons and untagged devices as generic dots — in a clean, modern UI.

**Architecture:**

```
crates/wifi-radar/
├── Cargo.toml               # binary + lib, depends on cyberdeck-core for shared config/paths
├── src/
│   ├── lib.rs               # pub mod re-exports
│   ├── main.rs              # bin entrypoint: parse args, init tracing, build AppState, call run_with
│   ├── run.rs               # public run_with(bind, dev_mode) entry point + StandaloneLive
│   ├── scanner.rs           # pcap-based monitor-mode capture loop (background tokio task)
│   ├── frames.rs            # 802.11 frame parser: beacon/probe/data → DeviceEvent
│   ├── devices.rs           # in-memory DeviceStore (MAC → DeviceState) + tag overlay
│   ├── tags.rs              # tag DB: load/save data/tags.json, Tag CRUD
│   ├── api.rs               # axum routes: GET /api/devices, POST /api/tags, GET /api/events (SSE)
│   └── shell.rs             # askama-rendered HTML shell + static asset mount
├── web/                     # frontend assets, embedded via include_dir! (or served from disk in dev)
│   ├── index.html           # radar canvas + tag sidebar + status bar
│   ├── app.js               # SSE client + radar render loop + tag editor
│   ├── radar.js             # OpenScope-style canvas: sweep line, fading trail, device dots
│   └── style.css            # clean dark UI: monospace font, single accent color, generous spacing
├── tests/
│   ├── frames_parsing.rs    # unit tests: parser on captured 802.11 frame bytes
│   ├── devices_store.rs     # unit tests: tag CRUD, MRU eviction, RSSI smoothing
│   ├── http_api.rs          # axum integration: /api/devices, /api/tags, /api/events SSE
│   └── web_shell.rs         # integration: GET / returns 200 + radar canvas element
└── data/
    └── tags.example.json    # example tag overlay (committed)
```

**Tech Stack:** Rust 1.80 (workspace MSRV), axum 0.7, tokio 1.40, serde 1, askama 0.12, tower-http 0.6, pcap-file 2 (pure-Rust reader — no libpcap C dep), tracing 0.1. Frontend: vanilla HTML/JS/Canvas 2D — no framework. SSE for live updates (simpler than WebSocket and unidirectional).

**Out of scope (per locked goal):** GPS/geo-location of devices, real angle-of-arrival (we use a heuristic RSSI+channel→angle so the radar *looks* like radar without specialized hardware), multi-user auth beyond an optional bearer token (matches `cyberdeck-web` precedent), persistence beyond local `data/tags.json`, historical replay, BLE/Zigbee scanning, deployment/CI, mobile-native app.

**Workspace integration:** Register `crates/wifi-radar` in the root `Cargo.toml` `[workspace] members` list alongside `crates/core`, `crates/tui`, `crates/web`. Reuse all relevant `[workspace.dependencies]` entries (axum, tokio, serde, tracing, askama, thiserror, anyhow, chrono, futures). Add one new workspace dep: `pcap-file = "2"`.

**Testing strategy:** All tests are scoped per the project's testing rule — never run the full workspace suite, always target `-p wifi-radar` or the specific test file. Tests live in `crates/wifi-radar/tests/` (axum integration) and `#[cfg(test)] mod tests` blocks inside each module (parser, devices, tags). Every task ends with the scoped `cargo test` command and the expected pass count.

**Tag overlay format** (`data/tags.json`, identical schema to `tags.example.json`):

```json
{
  "aa:bb:cc:dd:ee:ff": {
    "label": "Ankur's phone",
    "icon": "person",
    "color": "#7fdcff"
  }
}
```

Icons are a fixed enum: `person`, `phone`, `laptop`, `tablet`, `speaker`, `tv`, `watch`, `generic`. Colors are CSS hex. Unknown devices render as hollow circles; tagged devices render as the icon SVG at the device's polar position with the label underneath.

---## File Structure Summary

- **Crate root:** `crates/wifi-radar/Cargo.toml` (binary + library), `crates/wifi-radar/src/{lib,main,run}.rs` — entrypoint pattern mirrors `cyberdeck-web` exactly (lib exposes `run_with(bind, dev_mode)`, bin calls it).
- **Backend modules:**
  - `frames.rs` — pure data: byte slice → `DeviceEvent` enum. No I/O, no async.
  - `devices.rs` — `DeviceStore` (Arc<RwLock<HashMap<MacAddr, DeviceState>>>) with RSSI smoothing (EMA, α=0.3), last-seen timestamp, MRU eviction at 1024 entries.
  - `tags.rs` — `TagDb` that loads/saves `data/tags.json` and applies the overlay on read.
  - `scanner.rs` — async task that reads from a `pcap_file::pcap::PcapReader`, parses each frame via `frames.rs`, and pushes `DeviceEvent`s into a tokio `mpsc::Sender<DeviceEvent>`.
  - `api.rs` — axum router: `GET /api/devices` (snapshot), `POST /api/tags` (add/update), `DELETE /api/tags/{mac}` (remove), `GET /api/events` (SSE stream of `DeviceEvent`s).
  - `shell.rs` — askama template `index.html.askama` that embeds references to the static assets, plus a `tower_http::services::ServeDir` mount on `/static/*`.
- **Frontend:** `crates/wifi-radar/web/{index.html,app.js,radar.js,style.css}` — vanilla, no build step. `app.js` opens an `EventSource("/api/events")`, applies each event to `DeviceStore`-equivalent state in the browser, and on `requestAnimationFrame` calls `radar.js`'s `draw(store)`. Tag editor is a sidebar that lists devices, lets you set label/icon/color, and POSTs to `/api/tags`.
- **Tests:**
  - `src/frames.rs` — `#[cfg(test)] mod tests` with hex-encoded frame fixtures.
  - `src/devices.rs` — `#[cfg(test)] mod tests` for EMA + MRU eviction.
  - `src/tags.rs` — `#[cfg(test)] mod tests` for load/save/overlay.
  - `tests/frames_fixtures.rs` — public re-export tests with named fixtures (so external code can verify the parser contract).
  - `tests/http_api.rs` — axum integration: build router with `axum::Router::new()`, drive via `tower::ServiceExt::oneshot` (no live socket).
  - `tests/web_shell.rs` — GET `/` returns 200 and the body contains `<canvas id="radar">`.
  - `tests/dev_mode_frames.rs` — when `--dev` is passed (or no monitor-mode iface exists), the scanner emits a deterministic synthetic stream so the UI is always visible.

---## Task 1: Create the `wifi-radar` crate skeleton

**Files:**
- Create: `crates/wifi-radar/Cargo.toml`
- Create: `crates/wifi-radar/src/lib.rs`
- Create: `crates/wifi-radar/src/main.rs`
- Modify: `Cargo.toml` (root) — add `"crates/wifi-radar"` to `[workspace] members`

- [x] **Step 1.1: Write the failing test**

Add `crates/wifi-radar/tests/skeleton.rs`:

```rust
use wifi_radar::version;

#[test]
fn exposes_version_string() {
    let v = version();
    assert!(v.starts_with("wifi-radar "), "got {v:?}");
}
```

- [x] **Step 1.2: Run test to verify it fails**

Run: `cargo test -p wifi-radar --test skeleton`
Expected: FAIL with `error[E0432]: unresolved import 'wifi_radar'` because the crate doesn't exist yet.

- [x] **Step 1.3: Register the new workspace member**

Edit the root `Cargo.toml` `[workspace] members` list to add `crates/wifi-radar`:

```toml
[workspace]
resolver = "2"
members = ["crates/core", "crates/tui", "crates/web", "crates/wifi-radar"]
```

Also add `pcap-file = "2"` to `[workspace.dependencies]`:

```toml
# wifi-radar: 802.11 frame capture (pure-Rust pcap reader, no libpcap C dep)
pcap-file = "2"
```

- [x] **Step 1.4: Create the crate manifest**

Create `crates/wifi-radar/Cargo.toml`:

```toml
[package]
name = "wifi-radar"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "Browser-accessible Wi-Fi radar: ruview-style surveillance + OpenScope-style radar canvas."

[lib]
name = "wifi_radar"
path = "src/lib.rs"

[[bin]]
name = "wifi-radar"
path = "src/main.rs"

[dependencies]
axum.workspace = true
tower.workspace = true
tower-http.workspace = true
askama.workspace = true
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
thiserror.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
chrono.workspace = true
futures.workspace = true
pcap-file.workspace = true

[dev-dependencies]
tower = { workspace = true, features = ["util"] }
```

- [x] **Step 1.5: Create `src/lib.rs`**

Create `crates/wifi-radar/src/lib.rs`:

```rust
//! wifi-radar: a browser-accessible Wi-Fi radar. See README + design doc.

pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the package name + version, e.g. `"wifi-radar 0.1.0"`.
pub fn version() -> String {
    format!("wifi-radar {PKG_VERSION}")
}
```

- [x] **Step 1.6: Create `src/main.rs` (minimal)**

Create `crates/wifi-radar/src/main.rs`:

```rust
fn main() {
    println!("{}", wifi_radar::version());
}
```

- [x] **Step 1.7: Run test to verify it passes**

Run: `cargo test -p wifi-radar --test skeleton`
Expected: PASS, 1 test green.

- [x] **Step 1.8: Commit**

```bash
git add Cargo.toml crates/wifi-radar/
git commit -m "wifi-radar: scaffold workspace member with version() smoke test"
```

---