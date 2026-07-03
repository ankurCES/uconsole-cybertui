```bash
git add Cargo.toml crates/wifi-radar/
git commit -m "wifi-radar: scaffold workspace member with version() smoke test"
```

---

## Task 2: Implement `frames.rs` — pure-data 802.11 frame parser

**Files:**
- Create: `crates/wifi-radar/src/frames.rs`

**Contract:** `parse_frame(bytes: &[u8], rssi_dbm: i8, channel: u8) -> Option<DeviceEvent>`

- [x] `DeviceEvent { mac: String, kind: FrameKind, rssi_dbm: i8, channel: u8 }` — serializable so it can ride the SSE stream.
- [x] `FrameKind { Beacon, Probe, Data }` — the three frame types we surface; control frames and unknown subtypes return `None`.
- [x] Reject frames shorter than 24 bytes (MAC header minimum).
- [x] Reject source addresses with the multicast bit set (LSB of byte 0).
- [x] MAC formatted as lowercase hex with colons.
- [x] `#[cfg(test)] mod tests` covers: beacon, probe req, data, control (reject), too short (reject), multicast source (reject), unknown mgmt subtype (reject).
- [x] `tests/frames_parsing.rs` exercises the public API.

---

## Task 3: Implement `devices.rs` — in-memory `DeviceStore`

**Files:**
- Create: `crates/wifi-radar/src/devices.rs`

- [x] `DeviceStore` wraps `RwLock<HashMap<String, DeviceState>>` (MAC is the key).
- [x] EMA smoothing with `α = 0.3` so signal strength doesn't flicker on every frame.
- [x] MRU eviction at `MAX_DEVICES = 1024`; when full, drop the entry with the smallest `last_seen_unix`.
- [x] `DeviceState` carries `mac`, `rssi_dbm`, `channel`, `last_kind`, `last_seen_unix`, `frames_seen` (debug-only).
- [x] `apply(event)`, `snapshot()`, `len()`, `is_empty()`.
- [x] Tests: insert, EMA smoothing, EMA convergence, channel/kind update, frame counter, MRU eviction cap.
- [x] `tests/devices_store.rs` exercises the public API.

---

## Task 4: Implement `tags.rs` — persistent tag DB

**Files:**
- Create: `crates/wifi-radar/src/tags.rs`
- Create: `crates/wifi-radar/data/tags.example.json`

- [x] `Tag { label, icon, color }`; `TagFile { tags: HashMap<String, Tag> }`.
- [x] `TagDb::load(path)` — reads JSON, starts empty if file absent or corrupt.
- [x] `upsert(mac, tag)` / `delete(mac)` / `get(mac)` / `overlay(macs)` — case-insensitive on MAC.
- [x] Atomic save: write to `tags.json.tmp`, then rename.
- [x] `KNOWN_ICONS` lists the fixed icon enum: `person`, `phone`, `laptop`, `tablet`, `speaker`, `tv`, `watch`, `generic`.
- [x] `data/tags.example.json` ships three example entries (phone, speaker, laptop) matching the plan's format.
- [x] Tests: load missing, upsert + persist, upsert returns previous, delete, delete miss, overlay, corrupt file, MAC normalisation.

---

## Task 5: Implement `scanner.rs` — pcap capture loop + dev-mode fallback

**Files:**
- Create: `crates/wifi-radar/src/scanner.rs`

- [x] `ScannerSource::PcapFile(PathBuf)` — reads via `pcap-file`'s `PcapReader` (pure-Rust, no libpcap C dep).
- [x] `ScannerSource::Dev` — emits a deterministic synthetic stream of 8 MACs sweeping channel 6 at 4 Hz.
- [x] Radiotap header stripped; `parse_radiotap_rssi_channel` pulls channel + RSSI from standard TLVs (3 = Channel, 6 = DB Antenna Signal).
- [x] 802.11 payload passed to `frames::parse_frame`; events pushed into the store and the SSE channel.
- [x] `spawn(...)` returns a `ScannerHandle` with `stop()` for graceful shutdown.
- [x] Tests: `strip_radiotap`, `strip_radiotap` bogus fallback, `freq_to_channel`, `parse_radiotap_extracts_rssi_and_channel`.

---

## Task 6: Implement `api.rs` — axum routes + SSE

**Files:**
- Create: `crates/wifi-radar/src/api.rs`

- [x] `GET /api/health` — `{"ok": true}`.
- [x] `GET /api/devices` — snapshot + tag overlay in one response.
- [x] `GET /api/tags` — JSON snapshot.
- [x] `POST /api/tags` — upsert (returns `{ok, replaced}`).
- [x] `DELETE /api/tags/:mac` — remove (returns `{ok, removed}`).
- [x] `GET /api/events` — SSE stream of `DeviceEvent`s, 15-second keepalive.
- [x] `tests/http_api.rs` drives every route via `tower::ServiceExt::oneshot` — no live socket.

---

## Task 7: Implement `shell.rs` — askama HTML shell + static mount

**Files:**
- Create: `crates/wifi-radar/src/shell.rs`
- Create: `crates/wifi-radar/templates/index.html`

- [x] `IndexTemplate` askama struct renders `templates/index.html` (title, version, status pill).
- [x] Static assets mounted at `/static/*` via `tower_http::services::ServeDir` in `run.rs`.
- [x] `tests/web_shell.rs` asserts GET `/` returns 200 and the body contains `<canvas id="radar">`; static assets under `/static/{style.css,app.js,radar.js}` return 200.

---

## Task 8: Implement frontend assets

**Files:**
- Create: `crates/wifi-radar/web/index.html` (optional — the askama template at `/` is the real shell)
- Create: `crates/wifi-radar/web/style.css`
- Create: `crates/wifi-radar/web/app.js`
- Create: `crates/wifi-radar/web/radar.js`

- [x] `style.css` — clean dark UI, monospace, single accent (`#7fdcff`), generous spacing, responsive grid.
- [x] `app.js` — opens `EventSource("/api/events")`, applies events to in-browser store, drives the radar render loop via `requestAnimationFrame`, wires the tag editor (POST/DELETE to `/api/tags`).
- [x] `radar.js` — OpenScope-style canvas: sweep wedge, fading trail buffer (offscreen), range rings, device dots positioned by RSSI→radius + MAC-hash→angle, labels under tagged devices.

---

## Task 9: Wire `run.rs` + `main.rs` entrypoint

**Files:**
- Create: `crates/wifi-radar/src/run.rs`
- Create: `crates/wifi-radar/src/main.rs`

- [x] `RunConfig { bind, dev_mode, tags_path, static_dir, pcap_path }`.
- [x] `run_with(cfg)` — builds `AppState`, spawns scanner, mounts shell + API + SSE + static, serves until ctrl-c.
- [x] `main.rs` parses `--bind`/`--dev`/`--tags`/`--static-dir`/`--pcap`/`--help`, initialises tracing with `tracing-subscriber`, calls `run_with`.

---

## Task 10: Integration tests

**Files:**
- Create: `crates/wifi-radar/tests/skeleton.rs`
- Create: `crates/wifi-radar/tests/frames_parsing.rs`
- Create: `crates/wifi-radar/tests/devices_store.rs`
- Create: `crates/wifi-radar/tests/http_api.rs`
- Create: `crates/wifi-radar/tests/web_shell.rs`
- Create: `crates/wifi-radar/tests/dev_mode_frames.rs`

- [x] `skeleton.rs` — `version()` smoke test.
- [x] `frames_parsing.rs` — public-API parser tests.
- [x] `devices_store.rs` — public-API store tests.
- [x] `http_api.rs` — 6 axum integration tests via `oneshot`.
- [x] `web_shell.rs` — GET `/` returns radar HTML; static assets served.
- [x] `dev_mode_frames.rs` — dev scanner populates store + SSE channel.

---

## Task 11: Final verification + workspace integration

- [x] `cyberdeck-core` added as a dependency (plan requirement for "shared config/paths").
- [x] `pcap-file = "2"` workspace dep added.
- [x] All tests scoped to `-p wifi-radar` (never the full workspace suite).
- [x] **41/41 tests passing** across 9 test binaries (26 lib + 6 http_api + 2 web_shell + 1 skeleton + 2 frames_parsing + 3 devices_store + 1 dev_mode_frames).
- [x] Release build clean.
- [x] 7 commits landed:
      1. scaffold workspace member
      2. frames parser, device store, tag DB
      3. scanner, api, run, shell, frontend, integration tests
      4. wire main.rs bin entrypoint
      5. split tests/frames_parsing.rs and tests/devices_store.rs
      6. depend on cyberdeck-core
      7. mark Task 1 steps complete in the plan

---

## Summary

The wifi-radar crate is fully implemented per the plan:

- **Backend:** `frames.rs` (pure parser), `devices.rs` (EMA + MRU store), `tags.rs` (JSON overlay), `scanner.rs` (pcap + dev fallback), `api.rs` (axum + SSE), `shell.rs` (askama template), `run.rs` (server bootstrap).
- **Frontend:** vanilla HTML/CSS/JS — `templates/index.html` + `web/{style.css,app.js,radar.js}`.
- **Tests:** 41 passing across parser, store, tags, scanner, HTTP API, shell, static assets, and dev-mode streams.
- **Entry:** `main.rs` parses `--bind`/`--dev`/`--tags`/`--static-dir`/`--pcap` and calls `run_with`.

Ready to ship.