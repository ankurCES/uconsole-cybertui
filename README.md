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
  writes that need root are gated. **Window manager** for splitting panes,
  live PTY terminals with ANSI colours, and pane-number badges for
  one-keystroke focus jumps.

- **`cyberdeck-web`** — axum 0.7 server (JSON API + WebSocket + static HTML).
  Optional bearer-token auth (random 16-byte token printed to stdout on
  start). Can run **standalone** (no TUI, just a headless server) or be
  **embedded** in the TUI via the `--web` flag.

## Install (one-liner)

```sh
# TUI only — no sudo, no service, no firewall changes.
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --tui

# Web service — installs cyberdeck-web as a systemd unit, opens the firewall.
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --web

# Both — TUI binary + web service.
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --full

# Build only — no install, no sudo, no service. For CI or dev.
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --build
```

### Presets

| Preset | What it does | Needs sudo? | Restarts? |
| ------ | ------------ | ----------- | --------- |
| `--tui` | Build + install `cyberdeck-tui` to `/usr/local/bin`. | Only if `/usr/local` needs it. | No service. |
| `--web` | Build + install `cyberdeck-web`, create `cyberdeck` system user, write the NOPASSWD sudoers fragment, install the systemd unit, open the firewall, generate a bearer token. | Yes. | The web service. |
| `--full` | Both of the above. (Default if no preset is given.) | Yes. | The web service. |
| `--build` | Build both binaries into `./target/release` and exit. | No. | Nothing. |

### Options

```sh
-y, --yes            # non-interactive; assume yes for prompts
--prefix <dir>       # install prefix for binaries (default: /usr/local)
--bind <addr>        # web server bind address (default: 0.0.0.0:7878)
--service-user <u>   # system user for the web service (default: cyberdeck)
--uninstall          # remove binaries, user, service, token
```

### Pin to a version

```sh
curl -fsSL …/install.sh | CYBERDECK_REF=v0.1.0 bash -s -- --tui
```

Re-running is safe — the token is preserved, `systemctl enable` /
`restart` are idempotent. To remove: `cyberdeck --uninstall` (or
`curl -fsSL …/install/install.sh | bash -s -- --uninstall`).

## Build from source

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
cyberdeck-tui
```

### TUI + embedded web server

```sh
cyberdeck-tui --web                # bind 0.0.0.0:7878
cyberdeck-tui --web --web-bind 127.0.0.1:9000
```

On startup the TUI prints a bearer token to stderr. Pass it as
`?token=<tok>` once, or in the `Authorization: Bearer <tok>` header.

### Standalone web server

```sh
cyberdeck-web 0.0.0.0:7878
```

Same bearer-token model, same JSON API, same WebSocket payload. Useful for
headless deployments or for putting the cyberdeck behind a reverse proxy.

## Keys

### Global

| Key            | Action                                       |
| -------------- | -------------------------------------------- |
| `q` / `Ctrl-C` | quit                                         |
| `?`            | help modal                                   |
| `:`            | command palette (`web start`, `web stop`, …) |
| `1`..`9`/`0`   | jump to a screen                             |
| `Tab`          | toggle sidebar ↔ content focus               |
| `↑/↓` or `j/k` | navigate list in current focus               |
| `Enter`        | confirm / open                               |
| `Esc`          | back / cancel modal                          |
| `r`            | refresh current screen                       |

### Window manager (`Ctrl-W` prefix)

Every `Ctrl-W` verb is a two-key sequence: press `Ctrl-W`, then the second
key. Unknown second keys are no-ops (the prefix is consumed either way).

| Key        | Action                                                                  |
| ---------- | ----------------------------------------------------------------------- |
| `h` / `←`  | focus pane to the left                                                  |
| `j` / `↓`  | focus pane below                                                        |
| `k` / `↑`  | focus pane above                                                        |
| `l` / `→`  | focus pane to the right                                                 |
| `v`        | split focused pane vertically (new pane on the right)                   |
| `s`        | split focused pane horizontally (new pane below)                        |
| `n`        | swap focused pane for a live terminal (`$SHELL`)                        |
| `q` / `x`  | close the focused pane                                                  |
| `=` / `+`  | grow focused pane by 5%                                                 |
| `-`        | shrink focused pane by 5%                                               |
| `1`..`9`   | jump focus to pane N (pane number is shown in the title bar)            |

The new pane from `v` / `s` gets focus (vim convention). A hard cap of
**9 panes** is enforced; `Ctrl-W v` / `Ctrl-W s` past the cap toasts
`pane limit reached (9)`. `Ctrl-W 1..9` with no pane at that index
toasts `no pane N`. Closing a pane renumbers the rest contiguously.

Terminal pane titles show `[N] terminal` (the badge); built-in pane
titles are still rendered by each `Screen::render` and don't yet show
the badge — that's filed as a follow-up in `ROADMAP.md`.

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

The `--web` and `--full` installers write a narrow NOPASSWD sudoers
fragment for the `cyberdeck` system user, so the web UI can drive the
same commands without a TTY. The fragment lists exactly the commands
the service needs — `systemctl`, `nmcli` — and nothing else.

If you prefer to manage sudo yourself instead of using the installer:

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
- **Window manager.** `crates/tui/src/wm/` owns a split tree
  (`wm/tree.rs`), per-pane state (`wm/window.rs`), the orchestrator
  (`wm/manager.rs`), the tree-walk renderer (`wm/render.rs`), and a
  PTY/ANSI/broadcaster stack from Phase 2 (`wm/pty.rs`, `wm/ansi.rs`,
  `wm/broadcaster.rs`). Terminal panes are real `$SHELL` processes — bytes
  typed into the focused pane are translated by `wm/input.rs` and forwarded
  to the child; output is parsed by `wm/ansi.rs` and painted into the
  pane's grid.
- **No unsafe.** Every crate compiles with `#![forbid(unsafe_code)]`.
- **Privilege isolation.** `cyberdeck-core` has no tokio runtime of its
  own and never touches the network — it just shells out. The web crate
  has no system-level authority; everything it does goes through `core`.

## Roadmap

See [`ROADMAP.md`](./ROADMAP.md). Phase 3 (window manager) is shipped;
Phase 4 polish is in progress — pane number badges (done), per-pane
scrollback, shell + cwd persistence, and layout presets are next.

## License

MIT.
