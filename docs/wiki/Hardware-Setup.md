# Hardware / Setup — ClockworkPi uconsole on Debian 13 trixie

Cyberdeck is built and tested on the **ClockworkPi uconsole** (aarch64,
Debian 13 trixie, NetworkManager, systemd, thermals via
`/sys/class/thermal`). This page is the canonical setup guide for that
target. For other targets, most of the same steps apply with minor
tweaks.

## Hardware

The ClockworkPi uconsole is a single-board computer with:

- **CPU:** aarch64 (Cortex-A55 × 4 cores, ~2.0 GHz).
- **RAM:** 4 GB LPDDR4.
- **Storage:** microSD + optional eMMC.
- **Display:** 1280 × 720 IPS, 7" diagonal.
- **Keyboard:** 65 % mechanical, Fn layer for navigation.
- **Network:** Wi-Fi 5 (NetworkManager), Bluetooth 5.0 (bluez).
- **Battery:** 3000 mAh (enough for ~5 hours of cyberdeck use).
- **Audio:** 3.5 mm jack + USB-C audio.

The "uconsole" suffix in the repo name and tags refers to this device.

## OS

Debian 13 "trixie" is the recommended base. Earlier Debian releases
work but `nmcli` and `systemctl` integration is tested on trixie.

```sh
# Base install
sudo apt install -y \
  network-manager \
  systemd \
  sudo \
  pulseaudio \
  bluez \
  x11-xserver-utils \
  brightnessctl \
  iproute2 \
  curl \
  ca-certificates
```

Cyberdeck assumes NetworkManager, systemd, PulseAudio, and bluez are
installed and running. If you're using a non-Debian base, you'll need
to provide equivalent `nmcli`, `systemctl`, `pactl`, and `bluetoothctl`
binaries.

## Build from source

Rust 1.80+ is required (tested on 1.96):

```sh
# Install rustup if you haven't
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone
git clone https://github.com/ankurCES/uconsole-cybertui.git
cd uconsole-cybertui

# Build the TUI
cargo build -p cyberdeck-tui --release
sudo cp target/release/cyberdeck-tui /usr/local/bin/

# Build the web server (optional)
cargo build -p cyberdeck-web --release
sudo cp target/release/cyberdeck-web /usr/local/bin/
```

## Install via the one-liner

```sh
# TUI only
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --tui

# TUI + web service
curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
  | bash -s -- --full
```

The installer handles:

- Building from source.
- Installing the binary to `/usr/local/bin/`.
- (For `--web` and `--full`) creating the `cyberdeck` system user.
- Writing a NOPASSWD sudoers fragment.
- Installing the systemd unit.
- Opening the firewall (port 7878 by default).
- Generating a bearer token.

## Privilege model

If you prefer to manage sudo yourself instead of using the installer:

```sh
echo "youruser ALL=(ALL) NOPASSWD: /usr/bin/systemctl, /usr/bin/apt, /usr/bin/reboot, /usr/bin/shutdown, /usr/bin/pm-suspend, /usr/bin/pm-hibernate, /usr/bin/iwctl, /usr/bin/nmcli, /usr/bin/pactl, /usr/bin/bluetoothctl, /usr/bin/xrandr, /usr/bin/brightnessctl, /usr/sbin/iptables" \
  | sudo tee /etc/sudoers.d/cyberdeck
sudo chmod 440 /etc/sudoers.d/cyberdeck
```

The fragment above is the exact one the installer writes. It's narrow
on purpose — only the commands cyberdeck actually invokes.

## Thermal monitoring

The uconsole exposes CPU temperature via
`/sys/class/thermal/thermal_zone0/temp` (in millidegrees Celsius). The
TUI reads this every second and surfaces it in the live header.

If your kernel doesn't expose `/sys/class/thermal`, the temperature
cell shows `—` and the last-seen value is logged as a toast.

## Display brightness

`brightnessctl` is the recommended backlight control. Install:

```sh
sudo apt install -y brightnessctl
```

Cyberdeck reads brightness via `brightnessctl get` (returns 0..100)
and writes via `brightnessctl set <0..100>%`. The Display screen
has a slider for this.

## Audio

PulseAudio (`pactl`) and PipeWire (with the PulseAudio shim) are both
supported. Cyberdeck calls `pactl list sinks`, `pactl set-sink-volume`,
and `pactl set-sink-mute`.

```sh
sudo apt install -y pulseaudio pulseaudio-utils
```

## Bluetooth

`bluetoothctl` is the recommended Bluetooth control. Cyberdeck calls
`bluetoothctl devices`, `bluetoothctl pair`, and `bluetoothctl connect`.

```sh
sudo apt install -y bluez
sudo systemctl enable --now bluetooth
```

## systemd unit

The installer writes a unit at `/etc/systemd/system/cyberdeck-web.service`:

```ini
[Unit]
Description=Cyberdeck web UI
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=cyberdeck
ExecStart=/usr/local/bin/cyberdeck-web 0.0.0.0:7878
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

Enable and start:

```sh
sudo systemctl enable --now cyberdeck-web
sudo systemctl status cyberdeck-web
```

The bearer token is in `/etc/cyberdeck/token`.

## Firewall

The installer opens port 7878 (the web UI) on the `INPUT` chain. If
you prefer to manage the firewall yourself, allow inbound TCP on
7878:

```sh
sudo iptables -A INPUT -p tcp --dport 7878 -j ACCEPT
```

## Tested on

- ClockworkPi uconsole with the stock Debian 13 trixie image.
- Kernel 6.6.x (Debian trixie default).
- NetworkManager 1.46+.
- systemd 256+.

## Troubleshooting

- **`sudo -n` fails.** Cyberdeck will surface this as an `AuthFailure`
  modal. Re-run the installer with `--full` to write the NOPASSWD
  fragment, or set up sudo manually (see above).
- **`nmcli` not found.** Install NetworkManager: `sudo apt install
  network-manager`.
- **`pactl` not found.** Install PulseAudio: `sudo apt install
  pulseaudio pulseaudio-utils`.
- **`brightnessctl` not found.** Install it: `sudo apt install
  brightnessctl`. The Display screen will show `—` until it's
  installed.
- **PTY/ANSI colours wrong in the terminal panes.** Make sure your
  shell's `TERM` is set to something modern (`xterm-256color` or
  `tmux-256color`). Add `export TERM=xterm-256color` to your
  `~/.bashrc` if needed.

See [Architecture](./Architecture.md) for the data flow, [Keymaps](./Keymaps.md)
for navigation, and [Hardening](./Hardening.md) for the no-hang test
guarantees.
