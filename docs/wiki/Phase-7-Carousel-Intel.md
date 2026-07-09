# Phase 7 — Carousel menu + Intel + Recon

Phase 7 overhauls the front of the TUI (Overworld), adds a 9-layer
OSINT aggregator screen (Intel), and ships a 7-tab action console for
on-demand OSINT lookups (Recon). M1–M7 of the internal plan landed
together; M8 polished + documented.

## Overworld (front door)

`ScreenId::Overworld` is now at index `0` of `ScreenId::ALL` so the
Tab cycle always opens here (the "front door" metaphor from
Bruce firmware). The screen is a single-pane carousel: every visible
sidebar entry (Editor excluded) is rendered as a tile in a
width-bucketed grid:

| Width    | Cols |
|----------|-----:|
| ≤  80    |    2 |
| ≤ 120    |    3 |
| ≤ 160    |    4 |
| >  160   |    5 |

Keys:

* `Tab` / arrow keys — move the cursor between tiles
* `Enter` — switch to the focused screen (mirrors `switch_screen`
  with `app.current` + `wm::WindowKind`)
* `Esc` — show an info toast (never quits)
* `Esc`-on-Overworld — do not propagate

## Tab-strip preview indicator (M3)

`app.tab_cursor` already owned `cycle_tab_cursor` / `commit_tab_cursor` /
`clear_tab_cursor`. M3 wired those into the tab-strip area:
`ui::chunks()` returns `Option<Rect>` for the strip (collapses to
`None` when `area.height < 10`); `main::draw()` paints the strip
when `Some` and writes `app.tab_strip_rect`. Key handling:

* `Tab` / `BackTab` on the content side → `cycle_tab_cursor(forward)`
* `Enter` → `commit_tab_cursor()` (jumps to the highlighted tab)
* `Esc` on the content side → `clear_tab_cursor()` (drops the
  highlight, returns to current)

## Intel screen (M4 + M5)

`ScreenId::Intel` (18th entry). Two-pane layout
(`Horizontal`: `Percentage(28)` left grid | `Min(40)` right detail).
Left = 9 OSINT layer rows (Flights / Earthquakes / Fires / Weather /
Satellites / News / CCTV / Maritime / Conflicts) plus a sentinel
chip in the title bar. Right = selected layer's summary line,
sentinel chip, entity count, "last ok" timestamp, and the head of
the upstream JSON.

### Data path (M5)

`cyberdeck_intel::refiller::spawn_all()` runs **one `tokio::spawn`
task per `LayerId`**, each at its layer's staggered poll interval
(30 s – 21 600 s so consecutive layers never share an interval).
Every fetch produces a `Snapshot { layer, status, sentinel, summary,
entity_count, raw }` which is pushed through an `mpsc` → wrapped as
`Action::IntelSnapshot` → handled in `main.rs` →
`App::intel_snapshots.insert(layer, snap)`. The render path reads
from `intel_snapshots` first and falls back to the M4 fixture for
any layer not yet populated.

The footer line reads `intel: N/M layers live · K <SENTINEL>`
where `N = layers with Ok status` and the worst sentinel is rolled
up via `cyberdeck_intel::worst_sentinel`.

## Recon screen (M7)

`ScreenId::Recon` (19th entry). Single-pane — the output IS the
screen, so `has_right_pane()` returns `false`. Seven tabs in stable
order:

| Tab      | Glyph | Primitive                       | Endpoint             |
|----------|:-----:|---------------------------------|----------------------|
| DNS      |   D   | `dig +short`                    | system `dig`         |
| WHOIS    |   W   | `whois`                         | system `whois`       |
| IP       |   I   | `ureq::get` ip-api.com          | `ip-api.com/json`    |
| SSL      |   S   | `openssl s_client -brief`       | system `openssl`     |
| CVE      |   C   | bundled NVD fixture             | local CSV            |
| CRYPTO   |   ₿   | bundled risk table              | local CSV            |
| SANCTIONS|   ⚖   | bundled OFAC SDN mirror         | local CSV            |

### SSRF gate

Every primitive that resolves a user-supplied target to a network
endpoint runs through `cyberdeck_intel::recon::ssrf::check_ip`
**before** any process spawn or HTTP call. Reject table:

| Range                          | Rule                           |
|--------------------------------|--------------------------------|
| `127.0.0.0/8`                  | loopback                       |
| `10.0.0.0/8`                   | RFC1918 private                |
| `172.16.0.0/12`                | RFC1918 private                |
| `192.168.0.0/16`               | RFC1918 private                |
| `169.254.0.0/16`               | link-local                     |
| `0.0.0.0/8`                    | "this network"                 |
| `224.0.0.0/4`                  | multicast                      |
| `240.0.0.0/4`                  | reserved                       |
| `::1`                          | IPv6 loopback                  |
| `::`                           | IPv6 unspecified               |
| `fe80::/10`                    | IPv6 link-local                |
| `fc00::/7`                     | IPv6 ULA                       |
| `ff00::/8`                     | IPv6 multicast                 |

The handler emits a `SsrfError::Blocked { addr, rule }`. Six
property tests (`proptest`, 256 cases each) pin the reject bands
in CI.

### Keymap

* `Tab` / `BackTab` — cycle tabs
* printable chars — append to query buffer (cap 256)
* `Enter` — runs the active arm
* `Esc` — clears the query + output
* `j` / `k` (and arrow keys) — scroll the output area

Output buffer caps at 4 KiB so a 1 MiB WHOIS response can't pin the
renderer; truncations are signalled with `(… output truncated)` at
the head of the buffer.

### CLI parity

`cyberdeck recon <arm> <query>` mirrors the screen's seven tabs:
`dns`, `whois`, `ip`, `ssl`, `cve`, `crypto`, `sanctions`. Errors
surface as `{"ok":false, "error":{"message":"…"}}` with the SSRF
rule tag intact (e.g. `"refused to target 127.0.0.1: loopback
(127.0.0.0/8)"`).

## Known issues

* Six pre-existing bin tests
  (`bluetooth_passkey_rejects_letters`, `esc_in_editor_closes_editor`,
  `esc_in_files_goes_up_a_folder`, `esc_in_logs_clears_active_filter`,
  `keymap_capture_rejects_conflict`, `choice_modal_cursor_wraps_and_enter_dispatches`)
  predate Phase 7; they are not regressions from this work.
* The refiller starts immediately on `App::new`. If you launch the
  TUI without internet, layers will surface as `Error { reason:
  "fetch: …" }` rows after the first poll. This is intentional — the
  Intel screen falls back to the M4 fixture for any layer not yet
  populated, so first-paint still looks correct.
* `cyberdeck recon ip <hostname>` goes through ip-api.com, which
  rate-limits anonymous calls. Production deployments behind a
  proxy should set `HTTPS_PROXY` and tune polling accordingly.

## Files added or substantively touched

* `crates/intel/{lib,refiller}.rs` + 11 layer/recon modules
* `crates/intel/testdata/{sanctions_sample.csv,crypto_risk.csv}`
* `crates/tui/src/screens/{overworld,intel,recon}/` (new)
* `crates/tui/src/screens/mod.rs` + `crates/tui/src/main.rs`
* `crates/tui/src/app/{screen,action}.rs` + `crates/tui/src/app.rs`
* `crates/tui/src/ui/{mod,tab_strip}.rs`
* `crates/cli/src/commands/{intel,recon}.rs` + `commands/mod.rs`
  + `lib.rs`
* `crates/cli/tests/cli_dispatch.rs`
* `crates/daemon/src/{rpc,handlers}.rs`
* `crates/daemon/Cargo.toml` (added `cyberdeck-intel` dep)
* `crates/cli/Cargo.toml` (added `cyberdeck-intel` dep)
* `crates/intel/Cargo.toml` (added `ureq`, `csv`, `proptest`)
