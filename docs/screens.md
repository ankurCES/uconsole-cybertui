# Screens & Keymaps

## TUI screens

13 screens: System, Network, Bluetooth, Power, Display, Audio, Storage,
Services, Packages, Processes, Logs, Files, Settings.

Plus: City map (braille road/POI/area rendering, weather with ASCII icons,
GPS marker), AI assistant (local LLM via llama-server), command palette,
help modal, toast log.

Live header: clock, CPU/mem/disk gauges, active SSID, BT status, battery %.

## Navigation model

Three-region model optimised for D-pad on small displays:

- **Sidebar** — screen picker. `j/k`/arrows move, `Enter`/`l` enter, `1`-`0` jump.
- **ContentLeft** — primary pane. `h` back to sidebar, `l` to right pane.
- **ContentRight** — secondary pane. `h` back to left pane.

`Esc` always returns to sidebar. `Tab`/`Shift-Tab` cycle screens.

## Global keys

| Key            | Action                        |
|----------------|-------------------------------|
| `q` / `Ctrl-C` | quit                          |
| `?`            | help modal                    |
| `:`            | command palette               |
| `1`..`0`       | jump to screen                |
| `r`            | refresh current screen        |

## Window manager (`Ctrl-W` prefix)

| Key       | Action                                    |
|-----------|-------------------------------------------|
| `h/j/k/l` | focus pane left/down/up/right             |
| `v`       | vertical split                             |
| `s`       | horizontal split                           |
| `n`       | new terminal pane (`$SHELL`)               |
| `q` / `x` | close pane                                |
| `+` / `-` | grow/shrink pane 5%                       |
| `1`..`9`  | jump to pane N                             |

Max 9 panes. See [Keymaps.md](wiki/Keymaps.md) for full per-screen keymap.

## HTTP API

All routes under `/api/`. GETs are reads, POSTs are actions. JSON bodies.
`Authorization: Bearer <tok>` required when a token is set.

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/system` | hostname, kernel, uptime, load, memory |
| GET | `/api/network/interfaces` | interfaces + state + IPv4 |
| POST | `/api/network/wifi/scan` | Wi-Fi scan |
| POST | `/api/network/wifi/connect` | connect to SSID |
| GET | `/api/services` | systemd units |
| POST | `/api/services/:unit/:op` | start/stop/restart/enable/disable |
| GET | `/api/power/battery` | battery state |
| GET | `/api/power/thermals` | CPU temps |
| POST | `/api/power/suspend` | suspend |
| POST | `/api/power/reboot` | reboot |
| GET | `/api/storage/df` | filesystems |
| GET | `/api/processes` | top by CPU |
| GET | `/api/audio/sinks` | PulseAudio sinks |
| GET | `/api/bluetooth/devices` | paired + discovered |
| GET | `/api/ws` | WebSocket live updates |

## Wi-Fi radar API

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/health` | `{"ok": true}` |
| GET | `/api/vitals` | CSI human sensing |
| GET | `/api/devices` | device snapshot |
| GET/POST/DELETE | `/api/tags` | persistent tag DB |
| GET | `/api/events` | SSE stream |
