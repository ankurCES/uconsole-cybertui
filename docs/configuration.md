# Configuration & Running

## TUI

```sh
cyberdeck-tui
```

## TUI + embedded web server

```sh
cyberdeck-tui --web                       # bind 0.0.0.0:7878
cyberdeck-tui --web --web-bind 127.0.0.1:9000
```

Bearer token printed to stderr on startup. Pass as `?token=<tok>` or
`Authorization: Bearer <tok>` header.

## Standalone web server

```sh
cyberdeck-web 0.0.0.0:7878
```

Same bearer-token model, same JSON API, same WebSocket payload.

## Wi-Fi radar

```sh
cargo run -p wifi-radar -- --dev --bind 127.0.0.1:8743
```

Passive 802.11 monitor with synthetic 8-MAC fallback. Service-mode install
(`--radar` preset) ships a `wifi-radar.service` unit with `DynamicUser`.

## Privilege model

Reads are unprivileged. Writes that mutate the system use `sudo -n`
(non-interactive). If `sudo -n` fails, the TUI shows a red toast.

The `--web` / `--full` installers write a narrow NOPASSWD sudoers fragment
for the `cyberdeck` system user listing only the commands the service needs.

Manual sudoers setup:

```sh
echo "youruser ALL=(ALL) NOPASSWD: /usr/bin/systemctl, /usr/bin/apt, \
/usr/bin/reboot, /usr/bin/shutdown, /usr/bin/nmcli, /usr/bin/pactl, \
/usr/bin/bluetoothctl, /usr/bin/xrandr, /usr/bin/brightnessctl" \
  | sudo tee /etc/sudoers.d/cyberdeck
sudo chmod 440 /etc/sudoers.d/cyberdeck
```

## Human sensing (CSI / vitals)

RSSI shows devices; Channel State Information shows people. On the uConsole
CM4, the BCM43455c0 can produce CSI via
[nexmon_csi](https://github.com/seemoo-lab/nexmon_csi).

```sh
sudo ./install.sh --vitals --setup-nexmon --service
```

See [WiFi-Vitals-Nexmon-CM4.md](wiki/WiFi-Vitals-Nexmon-CM4.md) for full
setup and tuning.
