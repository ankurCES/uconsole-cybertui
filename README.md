# cyberdeck

A rich TUI + LAN web UI for OS-level control of a single-board computer —
designed for the **ClockworkPi uconsole** (aarch64, Debian 13 trixie,
NetworkManager, systemd, thermals via `/sys/class/thermal`).

```
+------------------+   +-----------------+   +----------------+
|  cyberdeck-core  |<--|  cyberdeck-tui  |   |  cyberdeck-web |
|  (no TUI/web)    |   |  ratatui front  |<->|  axum HTTP/WS  |
+------------------+   +-----------------+   +----------------+
```

- **`cyberdeck-core`** — async, async-trait-free wrapper around `nmcli`,
  `systemctl`, `apt`, `pactl`, `bluetoothctl`, `xrandr`/`brightnessctl`,
  `suspend`/`reboot`/`shutdown`, `journalctl`, etc. All shell-outs go through
  a single `run()` helper that respects a `Privilege::{User, Root}` enum
  using `sudo -n` (non-interactive). Every command has a per-call timeout
  and a uniform `CoreError` type.

- **`cyberdeck-tui`** — ratatui front-end. 13 screens (System, Network,
  Bluetooth, Power, Display, Audio, Storage, Services, Packages,
  Processes, Logs, Files, Settings) + command palette + help modal + toast
  log. Live header shows clock, CPU/mem/disk gauges, active SSID,
  Bluetooth status, battery %. Privilege-aware: most reads work unprivileged,
  writes that need root are gated.

- **`cyberdeck-web`** — axum 0.7 server (JSON API + WebSocket + static HTML).
  Optional bearer-token auth (random 16-byte token printed to stdout on
  start). Can run **standalone** (no TUI, just a headless server) or be
  **embedded** in the TUI via the `--web` flag.

## Build

```sh
# TUI only (smallest)
cargo build -p cyberdeck-tui

# TUI with embedded web server
cargo build -p cyberdeck-tui --features web

# Standalone web server binary
cargo build -p cyberdeck-web

# Whole workspace
cargo build --workspace
```

Rust 1.80+ (tested on 1.96). No system deps beyond what Debian already
provides (`sudo`, `network-manager`, `systemd`, `pulseaudio`/`pipewire`,
`bluez`, `xrandr`/`brightnessctl` if you want display control).

## Run

### TUI

```sh
./target/debug/cyberdeck-tui
```

| Key            | Action                          |
| -------------- | ------------------------------- |
| `q` / `Ctrl-C` | quit                            |
| `?`            | help                            |
| `:`            | command palette                 |
| `1`..`9`/`0`   | jump to a screen                |
| `Tab`          | toggle sidebar ↔ content focus  |
| `↑/↓` or `j/k` | navigate list in current focus  |
| `Enter`        | confirm / open                  |
| `Esc`          | back / cancel modal             |
| `r`            | refresh current screen          |

### TUI + LAN web UI

```sh
./target/debug/cyberdeck-tui --web                # bind 0.0.0.0:7878
./target/debug/cyberdeck-tui --web --web-bind 127.0.0.1:9000
```

On startup the TUI prints a bearer token to stderr. Pass it as
`?token=<tok>` once, or in the `Authorization: Bearer <tok>` header.

You can also start/stop the web server at runtime from **Settings → Web
Server** or from the command palette.

### Standalone web server

```sh
./target/debug/cyberdeck-web 0.0.0.0:7878
```

Same bearer-token model, same JSON API, same WebSocket payload. Useful for
headless deployments or for putting the cyberdeck behind a reverse proxy.

## HTTP API

All routes are under `/api/`. GETs are reads, POSTs are actions. Bodies are
JSON. Errors are `{"error": "<message>"}` with an appropriate status code.

| Method | Path                                | Purpose                                    |
| ------ | ----------------------------------- | ------------------------------------------ |
| GET    | `/api/system`                       | hostname, kernel, uptime, load, memory     |
| GET    | `/api/network/interfaces`           | list of interfaces + state + IPv4          |
| POST   | `/api/network/wifi/scan`            | Wi-Fi scan (returns list of networks)      |
| POST   | `/api/network/wifi/connect`         | `{"ssid": "...", "password": "..."}`       |
| POST   | `/api/network/wifi/disconnect`      | drop the active Wi-Fi                      |
| GET    | `/api/services`                     | all systemd units (active + inactive)      |
| POST   | `/api/services/:unit/:op`           | `op` ∈ start/stop/restart/enable/disable   |
| GET    | `/api/power/battery`                | battery state                              |
| GET    | `/api/power/thermals`               | CPU temps                                  |
| GET    | `/api/power/governor`               | current CPU governor                       |
| POST   | `/api/power/governor`               | `{"governor": "performance"}`              |
| POST   | `/api/power/suspend`                | suspend                                    |
| POST   | `/api/power/hibernate`              | hibernate                                  |
| POST   | `/api/power/reboot`                 | reboot                                     |
| POST   | `/api/power/shutdown`               | power off                                  |
| GET    | `/api/storage/df`                   | mounted filesystems                        |
| GET    | `/api/packages/upgradable`          | list of upgradable apt packages            |
| POST   | `/api/packages/search`              | `{"query": "vim"}`                         |
| POST   | `/api/packages/install`             | `{"name": "vim"}`                          |
| POST   | `/api/packages/remove`              | `{"name": "vim"}`                          |
| POST   | `/api/packages/update`              | `apt update`                               |
| POST   | `/api/packages/upgrade`             | `apt upgrade -y`                           |
| GET    | `/api/processes`                    | top by CPU                                 |
| POST   | `/api/processes/:pid/kill`          | `SIGTERM`                                  |
| GET    | `/api/display/outputs`              | `xrandr` outputs                           |
| GET    | `/api/display/brightness`           | 0..100                                     |
| POST   | `/api/display/brightness`           | `{"value": 60}`                            |
| GET    | `/api/audio/sinks`                  | PulseAudio sinks + volume + mute           |
| POST   | `/api/audio/volume`                 | `{"target": "alsa_output.pci-0000_...", "percent": 70}` |
| GET    | `/api/bluetooth/devices`            | paired + discovered                        |
| POST   | `/api/bluetooth/pair`               | `{"mac": "AA:BB:..."}`                     |
| POST   | `/api/bluetooth/connect`            | `{"mac": "AA:BB:..."}`                     |
| GET    | `/api/ws`                           | WebSocket — JSON snapshot every second     |

The `Authorization: Bearer <tok>` header (or `?token=<tok>` for the
WebSocket, which can't set headers) is required when a token is set.

## Privilege model

Reads are unprivileged. Writes that mutate the system (mount, install,
reboot, set Wi-Fi credentials, change PulseAudio defaults, set brightness,
power actions) call `run()` with `Privilege::Root`, which prepends `sudo
-n` (non-interactive) so a TTY prompt never blocks the UI.

If `sudo -n` fails (NOPASSWD not set), the call returns
`CoreError::Permission("...")` and the TUI shows a red toast pointing the
user at the install instructions in the README.

NOPASSWD setup (one line, replace `youruser`):

```sh
echo "youruser ALL=(ALL) NOPASSWD: /usr/bin/systemctl, /usr/bin/apt, /usr/bin/reboot, /usr/bin/shutdown, /usr/bin/pm-suspend, /usr/bin/pm-hibernate, /usr/bin/iwctl, /usr/bin/nmcli, /usr/bin/pactl, /usr/bin/bluetoothctl, /usr/bin/xrandr, /usr/bin/brightnessctl, /usr/sbin/iptables" \
  | sudo tee /etc/sudoers.d/cyberdeck
sudo chmod 440 /etc/sudoers.d/cyberdeck
```

## Architecture notes

- **Single source of truth.** The TUI owns an `Arc<Live>` of `RwLock`s and
  runs background refreshers that fill them. The web reads from the same
  `Arc<Live>` through a `TuiLiveRead` adapter (in `crates/tui/src/web_bridge.rs`)
  that implements the web's `LiveRead` trait.
- **One action channel.** UI events and async results both go through a
  `tokio::sync::mpsc::Sender<Action>`. The tap task in `main()` listens
  on a *separate* control channel for `WebStart`/`WebStop` so the embedded
  web server can be toggled without racing the main event loop.
- **No unsafe.** Every crate compiles with `#![forbid(unsafe_code)]`.
- **Privilege isolation.** `cyberdeck-core` has no tokio runtime of its
  own and never touches the network — it just shells out. The web crate
  has no system-level authority; everything it does goes through `core`.

## License

MIT.
