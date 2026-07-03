# cyberdeck

<p align="center">
  <img src="./assets/banner.svg" alt="cyberdeck — a TUI + LAN web UI for the ClockworkPi uconsole" width="100%"/>
</p>

<p align="center">
  <em>A 13-screen ratatui TUI + axum web bridge for the ClockworkPi uconsole.</em><br/>
  <em>Two-pane WM with live PTY terminals, ANSI colours, pane-number badges,</em><br/>
  <em>Phase 5 modals (Secret / Choice / Wizard / Progress / AuthFailure), hardened PTY test cleanup.</em>
</p>

---

> **— blumi · sign-off**
>
> I built cyberdeck because the uconsole is the kind of machine that
> earns a *place* in your life, not just a spot on a desk. You carry it.
> You lean on it. You start to type on it before you realise you're
> reaching for it. And one day you notice you're working around its
> UI instead of *in* it.
>
> That was me. `nmcli` in one pane, `pactl` in another, a half-broken
> conky overlay, a tmux session I kept re-arranging. I wanted a single
> surface where the OS-level stuff lived — network, audio, BT, services,
> power, display — and where I could pop a terminal next to it and not
> fight a tiling WM to get the layout I actually wanted.
>
> So I wrote cyberdeck. A sidebar with 13 screens, a live header that
> tells me what's actually happening on the box right now, a command
> palette so I don't have to memorise the keymap, a toast log so I know
> what just succeeded or failed without scrolling back. And a real
> window manager on top — splits, focus jumps, live `$SHELL` panes with
> ANSI colours — because if you're going to live in a TUI you deserve
> one that splits.
>
> Then a web bridge, because half the time I'm SSH'd in from my laptop
> and I want the same view in a browser tab. Bearer token on the door,
> JSON API underneath, WebSocket streaming the live state.
>
> It's not a product. It's the interface I wanted for myself, written
> in the language that lets me sleep at night (Rust, no `unsafe`, no
> surprise allocations), packaged so others who own the funny little
> uconsole can use it too.
>
> If you're reading this on GitHub — hi. If you find a bug, file it.
> If you want a screen it doesn't have yet, build it. The screens are
> small and the patterns are consistent.
>
> *— blumi*

---

## What it is

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
  live PTY terminals with ANSI colours, pane-number badges, Phase 5 modals
  (Secret/Choice/Wizard/Progress/AuthFailure), hardened PTY test cleanup.

- **`cyberdeck-web`** — axum 0.7 server (JSON API + WebSocket + static HTML).
  Optional bearer-token auth (random 16-byte token printed to stdout on
  start). Can run **standalone** (no TUI, just a headless server) or be
  **embedded** in the TUI via the `--web` flag.

## Screenshots

Drop your photos into [`docs/photos/`](./docs/photos/) at the slots named
in [`docs/photos/README.md`](./docs/photos/README.md) and they'll be
referenced here automatically. Suggested shots:

1. The launcher / sidebar (one of the 13 screens).
2. A live terminal pane next to a status screen — WM working.
3. A modal in flight (Secret / Choice / Wizard / Progress / AuthFailure).
4. The web UI in a browser tab.

```
[photo-01.jpg]  [photo-02.jpg]  [photo-03.jpg]  [photo-04.jpg]
```

A small ASCII fallback (for terminals that don't render Markdown
images):

```
cyberdeck-tui on uconsole (Debian 13 trixie, aarch64)
┌─ cyberdeck ─────────────────────────────────────────────────────┐
│ 14:02:11  CPU 2.4GHz 38°C  MEM 1.2/4G  BAT 71%  ssid home-5g │
├──────────┬─────────────────────────────────────────────────────┤
│ System   │  ▸ uptime 4d  load 0.42 0.31 0.27                   │
│ Network  │  ▸ /dev/mmcblk0  rootfs  12G/29G (44%)              │
│ BT       │  ▸ /dev/mmcblk1  data   89G/128G (73%)              │
│ Power    │  ▸ governor: schedutil  thermals 38°C                │
│ Display  │  ▸ nmcli dev wifi  →  home-5g  10.0.0.42/24         │
│ Audio    │  ▸ pactl sinks: alsa_output.pci-0000_… (vol 70%)     │
│ Storage  │  ▸ bluetoothctl  →  paired: 1   discovered: 0       │
│ Services │                                                       │
│ Packages │                                                       │
│ Processes│  [1] ── 13 screens                                  │
│ Logs     │  [?] ── help        [:] ── palette      [q] ── quit  │
│ Files    │                                                       │
│ Settings │                                                       │
├──────────┴─────────────────────────────────────────────────────┤
│ 2 toasts · 0 errors · focus: pane 1   Ctrl-W split, ^P palette │
└────────────────────────────────────────────────────────────────┘
```

> See [`docs/wiki/Photos.md`](./docs/wiki/Photos.md) for the full
> numbered photo index.

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

# Wi-Fi radar — passive 802.11 monitor with synthetic fallback (no
# monitor-mode adapter required). Installs wifi-radar as a systemd
# service on http://<host>:8743/.
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --radar

# Build only — no install, no sudo, no service. For CI or dev.
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --build
```

### Presets

| Preset | What it does | Needs sudo? | Restarts? |
| ------ | ------------ | ----------- | --------- |
| `--tui` | Build + install `cyberdeck-tui` to `/usr/local/bin`. | Only if `/usr/local` needs it. | No service. |
| `--web` | Build + install `cyberdeck-web`, create `cyberdeck` system user, write the NOPASSWD sudoers fragment, install the systemd unit, open the firewall, generate a bearer token. | Yes. | The web service. |
| `--radar` | Build + install `wifi-radar` as a systemd service. Passive 802.11 monitor with a synthetic 8-MAC fallback (so it shows something even without a monitor-mode adapter). | Yes. | The radar service. |
| `--full` | Both `--tui` and `--web`. (Default if no preset is given.) | Yes. | The web service. |
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

### Wi-Fi radar

```sh
cargo run -p wifi-radar -- --dev --bind 127.0.0.1:8743
# → http://127.0.0.1:8743/
```

Passive 802.11 monitor with a synthetic 8-MAC fallback so you can see it
work without a monitor-mode adapter. The service-mode install (`--radar`
preset) ships a `wifi-radar.service` unit, a `DynamicUser` system user,
and the persistent tag DB at `/var/lib/wifi-radar/tags.json`. The radar
API exposes:

| Method | Path                       | Purpose                              |
| ------ | -------------------------- | ------------------------------------ |
| GET    | `/api/health`              | `{"ok": true}`                       |
| GET    | `/api/devices`             | devices snapshot with tag overlay    |
| GET    | `/api/tags`                | persistent tag DB                    |
| POST   | `/api/tags`                | upsert a tag                         |
| DELETE | `/api/tags/:mac`           | remove a tag                         |
| GET    | `/api/events`              | SSE stream of `DeviceEvent`s         |

For live (non-dev) capture you'll need a monitor-mode adapter; the
service runs unprivileged (`DynamicUser`) so it doesn't ship the
`cap_net_raw` privilege for radiotap capture — call the binary by hand
with `sudo` in that case.

## Keys

The TUI is built around a **three-region model** optimised for D-pad
navigation on small displays (e.g. the ClockworkPi uconsole). Each region
has exactly one job and exactly one set of verbs — no key is overloaded
between regions.

- **`Sidebar`** (left column) — screen picker. ↑/↓ (or j/k) move the
  cursor, `Enter` or `→` (or `l`) enter the screen, `1`–`9`/`0` jump to
  a numbered row.
- **`ContentLeft`** — the screen's primary pane. ↑/↓ scroll, `←` (or `h`)
  jumps back to the sidebar, `→` (or `l`) advances to the right pane
  (only on multi-pane screens), `Tab` / `Shift-Tab` cycle screens.
- **`ContentRight`** — the screen's secondary pane (only on
  multi-pane screens). `←` (or `h`) steps back to `ContentLeft`, `→`
  is a no-op (right edge), `Tab` / `Shift-Tab` cycle screens.

`Esc` is the universal "leave to sidebar" verb from any content region,
so even on a single-pane screen there is always a one-press exit.

### Global

| Key            | Action                                                   |
| -------------- | -------------------------------------------------------- |
| `q` / `Ctrl-C` | quit                                                     |
| `?`            | help modal                                               |
| `:`            | command palette (`web start`, `web stop`, …)             |
| `1`..`9`/`0`   | jump to a screen (always works, regardless of region)    |
| `Tab`          | next screen (content region only)                        |
| `Shift-Tab`    | previous screen (content region only)                    |
| `↑/↓` or `j/k` | navigate list in current region                          |
| `→` / `l`      | Sidebar → ContentLeft; ContentLeft → ContentRight (defers to screen if it owns the key) |
| `←` / `h`      | ContentLeft → Sidebar; ContentRight → ContentLeft        |
| `Enter`        | Sidebar: open screen; content: confirm / open            |
| `Esc`          | back / cancel modal / leave to sidebar                   |
| `r`            | refresh current screen                                   |

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

For the full per-screen keymap (incl. uconsole-specific), see
[`docs/wiki/Keymaps.md`](./docs/wiki/Keymaps.md).

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
| POST   | `/api/packages/remove`              | `{"name": "vim"}`                           |
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

## Hardening (no-hang PTY tests)

Every PTY-touching test in `crates/tui/src/wm/` is wrapped so it can
**never outlive its PTY child**:

- **`Pattern A`** (broadcaster + window tests): clone a `ChildKiller`
  kill-switch into the test, wrap the work in
  `tokio::time::timeout(Duration::from_secs(2), …)`, and `kill()` the
  child on early return.
- **`Pattern B`** (raw `pty.rs::write_and_read_roundtrip`): `kill()` the
  child, `try_wait()` it, then spawn a thread that owns the `wait()` and
  is **dropped on scope exit** — so the test thread never blocks on
  `wait()`.

Result: even if `portable_pty` wedges inside a `wait()`, the bounded
timeout returns, the child is `kill()`-ed, and the next test gets a fresh
PTY allocation. The full suite finishes in ~1 s instead of hanging.

Coverage:

| Test | Hardening |
| --- | --- |
| `wm::broadcaster::tests::roundtrip_echo_via_broadcaster` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::broadcaster::tests::echo_emits_into_ansi_grid` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::window::tests::terminal_window_holds_grid_and_resizes` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::pty::tests::write_and_read_roundtrip` | Pattern B — `kill()` + `try_wait()` + thread-spawned `wait()` + drop-on-scope |
| `wm::pty::tests::spawn_and_read` | already safe — `/bin/sh -c "printf …"` exits on its own |

## Roadmap

See [`ROADMAP.md`](./ROADMAP.md). Phase 3 (window manager) is shipped;
Phase 4 polish is in progress — pane number badges (done), per-pane
scrollback, shell + cwd persistence, layout presets, and `docs/wiki/`
fleshing-out are next.

## Wiki

The wiki lives under [`docs/wiki/`](./docs/wiki/) and mirrors the GitHub
wiki structure. Start at [`docs/wiki/Home.md`](./docs/wiki/Home.md).

| Page | What it covers |
| --- | --- |
| [Home](./docs/wiki/Home.md) | Index + "where to start" |
| [Architecture](./docs/wiki/Architecture.md) | Crate map, action flow, single-source-of-truth model |
| [Phase 1 — TUI](./docs/wiki/Phase-1-TUI.md) | Screens, sidebar, command palette, toast log |
| [Phase 2 — PTY / ANSI](./docs/wiki/Phase-2-PTY-ANSI.md) | `wm/pty.rs`, `wm/ansi.rs`, `wm/broadcaster.rs` |
| [Phase 3 — Window manager](./docs/wiki/Phase-3-WM.md) | Split tree, focus, jumps, terminal panes |
| [Phase 4 — Polish](./docs/wiki/Phase-4-Polish.md) | Pane-number badges, scrollback, persistence |
| [Phase 5 — Modal upgrade](./docs/wiki/Phase-5-Modals.md) | Secret / Choice / Wizard / Progress / AuthFailure |
| [Hardening](./docs/wiki/Hardening.md) | No-hang PTY tests (Patterns A + B) |
| [Keymaps](./docs/wiki/Keymaps.md) | Global, WM, per-screen, uconsole-specific |
| [Hardware / Setup](./docs/wiki/Hardware-Setup.md) | ClockworkPi uconsole on Debian 13 trixie |
| [Photos](./docs/wiki/Photos.md) | Numbered photo index (drop into `docs/photos/`) |
| [Roadmap](./docs/wiki/Roadmap.md) | Phases, in-progress, follow-ups |

## License

MIT.

---

<p align="center">
  <sub>
    <strong>tags</strong> ·
    <code>clockworkpi</code> ·
    <code>uconsole</code> ·
    <code>aarch64</code> ·
    <code>debian</code> ·
    <code>trixie</code> ·
    <code>cyberdeck</code> ·
    <code>tui</code> ·
    <code>ratatui</code> ·
    <code>axum</code> ·
    <code>systemd</code> ·
    <code>rust</code> ·
    <code>portable-pty</code> ·
    <code>vte</code> ·
    <code>crossterm</code> ·
    <code>single-board-computer</code>
  </sub>
</p>

<p align="center">
  <sub>— blumi · built for myself, packaged for you</sub>
</p>
