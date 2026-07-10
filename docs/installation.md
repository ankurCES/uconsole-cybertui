# Installation

## One-liner install

```sh
# TUI only
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --tui

# Web service (systemd unit + firewall)
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --web

# Both TUI + web
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --full

# Wi-Fi radar (passive 802.11 monitor)
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --radar

# Build only (no install, no sudo)
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --build
```

## Presets

| Preset    | What it does | Needs sudo? |
|-----------|-------------|-------------|
| `--tui`   | Build + install `cyberdeck-tui` to `/usr/local/bin`. | Only if `/usr/local` needs it. |
| `--web`   | Build + install `cyberdeck-web`, systemd unit, firewall, bearer token. | Yes. |
| `--radar` | Build + install `wifi-radar` as a systemd service. | Yes. |
| `--full`  | Both `--tui` and `--web`. Default if no preset given. | Yes. |
| `--build` | Build into `./target/release` and exit. | No. |

## Options

```sh
-y, --yes            # non-interactive
--prefix <dir>       # install prefix (default: /usr/local)
--bind <addr>        # web bind address (default: 0.0.0.0:7878)
--service-user <u>   # system user for web (default: cyberdeck)
--uninstall          # remove binaries, user, service, token
```

## Pin to a version

```sh
curl -fsSL .../install.sh | CYBERDECK_REF=v0.4.0 bash -s -- --tui
```

Re-running is safe. To remove: `cyberdeck --uninstall`.

## Suppress banner animation

| Variable                | Effect                              |
|-------------------------|-------------------------------------|
| `CYBERDECK_NO_BANNER=1` | Skip art entirely.                  |
| `CYBERDECK_NO_ANIM=1`   | Static banner, no animation.        |
| `NO_COLOR=1`            | Disable all ANSI.                   |

## Build from source

Rust 1.80+ (tested on 1.96). No system deps beyond what Debian provides
(`sudo`, `network-manager`, `systemd`, `pulseaudio`/`pipewire`, `bluez`,
`xrandr`/`brightnessctl`).

```sh
cargo build -p cyberdeck-tui              # TUI only
cargo build -p cyberdeck-tui --features web  # TUI + embedded web
cargo build -p cyberdeck-web              # Standalone web server
cargo build --workspace                   # Everything
```
