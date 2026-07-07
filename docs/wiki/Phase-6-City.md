# Phase 6 — City screen

The City screen is the 14th sidebar entry (after System, Network,
Bluetooth, Power, Display, Audio, Storage, Services, Packages,
Processes, Files, Logs, Settings, LoRa — Phase 6 adds it at the
end of the existing list). It renders an IP-geolocated road map as
braille in the left pane and live weather + a traffic legend in the
right pane. The map and weather data are independent of every other
screen — they hit ip-api.com + Open-Meteo over plain HTTP/HTTPS, with
no daemon round-trip and no local state to babysit.

## What it looks like

```
+------------------------------------------------+
| ◍ City · Seattle                       ▢ refresh |
+-----------------------+------------------------+
|                       | Weather                |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠂⠀⠀⠈⠐⠠⢀⠠⠐⠈⠀⠀ | conditions  Overcast   |
|  ⠀⠀⠀⠀⠀⠈⠉⠑⠒⠂⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ | temp        9.2°C    |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠐⠂⠈⠐⠄⠂⠀⠀⠈⠐⠠⢀⠀ | feels like  7.4°C    |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠂⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ | humidity    78%      |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠐⠄⠂⠀⠀⠀⠀⠀⠀⠀⠀ | wind        315° NW  |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠂⠀⠀⠀⠀⠀⠀⠀⠀ |             @12 kph  |
|  ⠀⠀⠀⠁⠉⠑⠒⠂⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀ | next 12h    ▁▂▃▄▅▆▇█ |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠐⠠⢀⠠⠐⠈⠀⠀⠀⠀⠀ | fetched     14:02:11 |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠂⠀⠀⠀⠀⠀⠀ |                        |
|  ⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠐⠄⠂⠀⠀ | traffic · synthetic    |
|                       |   fluid    ·           |
|  seattle span 0.123°  |   light    +           |
+-----------------------+------------------------+
|  h/j/k/l pan  +/- zoom  c city  t traffic       |
+------------------------------------------------+
```

The left pane is a Unicode braille grid (`U+2800` + 8-dot bit offset)
lit by Bresenham line-draw over the bundled road polylines, with a
traffic overlay (denser strokes = heavier traffic) and a 5-dot
location marker at the resolved (lat, lon). The right pane is a
textual weather block (WMO code label, temp + feels-like in °C or °F,
humidity, wind direction + speed, 12h precipitation sparkline) plus a
traffic-level legend.

## Data sources

| Layer | Source | Cadence | Failure mode |
| ----- | ------ | ------- | ------------ |
| IP → city | [ip-api.com](http://ip-api.com) (free tier, HTTP) | 10 min refiller + on `r` | falls back to bundled seattle; logs a debug toast |
| Weather | [Open-Meteo](https://open-meteo.com) (no key, HTTPS) | 10 min refiller + on `r` | shows "(no data yet — press r)" until it lands |
| Roads | Bundled JSON in `crates/tui/data/cities/<slug>.json` | compile-time `include_str!` | seattle.json is the offline default; other slugs fall back to it |
| Traffic | Synthetic function of `(road importance, hour, weekday, road hash)` | recomputed every render | always succeeds (pure function) |

The refiller is `tokio::spawn`'d inside `App::spawn_refreshers` — one
600s loop that runs `geo::locate()` then `weather::fetch(&loc)` and
pushes the results back via `Action::CityResolved` /
`Action::CityWeatherRefreshed`. The dispatcher arms in
`main.rs::handle_action` write through to `App::live.{city_loc,
city_weather}` (both `Arc<RwLock<Option<…>>>`), and `CityScreen::render`
reads them with a non-blocking `try_read()` each frame.

The synthetic traffic model is documented in detail at the top of
`crates/tui/src/screens/city/traffic.rs`. Key behaviour:

- Weekday 07:30–09:30 and 16:30–18:30 = commute peaks. Motorways +
  trunks can reach `Gridlock`.
- Weekday 11:00–14:00 = weekend leisure peak. Arterials can reach
  `Heavy` but never `Gridlock`.
- `footway` never reaches above `Light` regardless of time. A
  regression test (`footway_never_reaches_gridlock`) pins this.

The map footer always says `traffic · synthetic` (or `off` when the
overlay is toggled off) so the data provenance is honest.

## Keymap

9 keys, all consumed in `CityScreen::on_key`:

| Key | Action | Persisted? |
| --- | ------ | ---------- |
| `h` / `←` | pan left (10% of bbox span) | no (viewport is in-memory) |
| `j` / `↓` | pan down | no |
| `k` / `↑` | pan up | no |
| `l` / `→` | pan right | no |
| `+` / `=` | zoom in (shrink bbox to 80% of current span) | no |
| `-` / `_` | zoom out (grow bbox to 125% of current span) | no |
| `r` | refresh — re-fire geo + weather via `Action::CityCtrlRefresh` | no (refetch is one-shot) |
| `c` | cycle city picker through `CityRoads::BUNDLED` | yes (`prefs::city`) |
| `t` | toggle synthetic traffic overlay | yes (`prefs::traffic_overlay`) |
| `w` | toggle right-hand weather panel | yes (`prefs::show_weather_panel`) |

Pan and zoom are pure viewport mutations — they don't touch `App`,
don't fire Actions, and don't persist. City picker + overlay +
weather-panel toggles all call `App::save_prefs()` so quit-and-relaunch
picks them up.

## CLI surface

`cyberdeck city <subcommand>` — same data path as the TUI, reachable
from the shell:

```sh
# Resolve the user's public IP to a CityLocation.
cyberdeck --json city locate

# Fetch weather for an explicit lat/lon (no IP lookup needed).
cyberdeck --json city weather --lat 35.6762 --lon 139.6503

# Print bundled road polylines for a slug (falls back to seattle).
cyberdeck --json city roads seattle
cyberdeck --json city roads atlantis   # → falls back, slug_used=seattle

# List every bundled city slug the binary knows about.
cyberdeck --json city bundled
```

The CLI lazily builds a `tokio::runtime::Builder::new_current_thread`
for the async arms (`locate`, `weather`); the data-only arms (`roads`,
`bundled`) stay synchronous.

## Where the data lives

| Path | Purpose |
| ---- | ------- |
| `crates/tui/src/screens/city/mod.rs` | `CityScreen` + dispatcher + render helpers |
| `crates/tui/src/screens/city/geo.rs` | ip-api HTTP client + `GeoError` mapping |
| `crates/tui/src/screens/city/weather.rs` | Open-Meteo HTTP client + WMO label mapping |
| `crates/tui/src/screens/city/traffic.rs` | synthetic traffic model (pure function) |
| `crates/tui/src/screens/city/roads.rs` | bundled city JSON loader |
| `crates/tui/src/screens/city/render.rs` | `BrailleGrid`, Bresenham, `Viewport` |
| `crates/tui/data/cities/seattle.json` | bundled Seattle polyline fixture (6 roads) |
| `crates/cli/src/commands/city.rs` | CLI verb (4 subcommands) |

## Adding a new bundled city

1. Generate the polyline JSON via `scripts/fetch_city_roads.py <slug>`
   (writes to `crates/tui/data/cities/<slug>.json`).
2. Add `<slug>` to the `match slug { ... }` arm in
   `CityRoads::load_bundled` (`crates/tui/src/screens/city/roads.rs`)
   with the new `include_str!("../../../data/cities/<slug>.json")`.
3. Bump the assertion in `bundled_roads_reside_inside_their_bbox` if
   the new fixture doesn't pass the existing invariants — typically a
   tighter bbox.

## Tests

45 unit tests under `screens::city` (covering geo, weather, traffic,
roads, render, pan/zoom helpers, format_temp, compass_point,
precip_sparkline, cycle_city) + 4 City integration tests under
`app::tests` (write-through of `CityResolved` / `CityWeatherRefreshed`,
overwrite semantics, `CityCtrlRefresh` variant pin) + 6 CLI dispatch
tests under `tests/cli_dispatch.rs` (help, bundled, roads, weather
flag validation).

Run with:

```sh
cargo test -p cyberdeck-tui --lib -- screens::city
cargo test -p cyberdeck-tui --lib -- 'app::tests::city_'
cargo test --manifest-path crates/cli/Cargo.toml --test cli_dispatch -- city_ city_help city_bundled city_roads city_weather
```

## Known issues

- **Only Seattle has bundled road geometry.** The other slugs in
  `BUNDLED` (london, tokyo, berlin, nyc) fall back to seattle until
  their JSON files land. The picker still cycles through them so the
  UI is testable, but the map stays on Seattle until the fixture is
  populated.
- **Synthetic traffic is a stand-in.** Real traffic requires a paid
  key (HERE, TomTom, Google). The footer always says `synthetic` so
  the provenance is honest; a future pluggable `TrafficSource::Live`
  path is the planned replacement.
- **`+` and `-` keymap aliases.** `+` requires Shift on US layouts;
  `=` is the unshifted alias. Both work — the dispatcher accepts
  either.