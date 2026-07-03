# Wi-Fi human sensing (breathing / heartbeat) on the uConsole CM4

The `wifi-radar` crate's RSSI radar shows *devices*. It cannot see a person's
chest move — that needs **Channel State Information (CSI)**, per-subcarrier
amplitude+phase. This page wires CSI human sensing (breathing, heart rate,
presence) to the ClockworkPi uConsole's Raspberry Pi **CM4** using its on-board
Broadcom **BCM43455c0** — no extra hardware.

```
nexmon_csi firmware ──UDP:5500──▶ tcpdump ──pipe──▶ wifi-radar --csi-pcap - ──▶ /api/vitals ──▶ UI
```

The DSP lives in [`crates/wifi-radar/src/csi.rs`](../../crates/wifi-radar/src/csi.rs):
it mirrors ruview's `wifi-densepose-vitals` algorithm (static-clutter removal →
band-pass per vital → zero-crossing for breathing, autocorrelation for heart).
ruview has no nexmon ingestion path, so we parse the frames ourselves.

## 1. Flash nexmon_csi on the CM4

The BCM43455c0 is supported by [nexmon_csi](https://github.com/seemoo-lab/nexmon_csi).
The patch is **kernel-version specific** — use the branch of
[nexmonster/nexmon_csi](https://github.com/nexmonster/nexmon_csi) that matches
`uname -r` on your uConsole image (e.g. `pi-5.4.51-plus`). Follow that repo's
build steps; the short version:

```sh
# On the CM4, matching branch for your kernel:
git clone https://github.com/nexmonster/nexmon_csi.git
cd nexmon_csi
# build the toolchain + firmware patch per the repo README, then:
make install-firmware          # installs the patched BCM43455c0 firmware
# build the utilities:
make -C utilities/nexutil && make -C utilities/makecsiparams
```

> ⚠️ Patching Wi-Fi firmware disables normal station Wi-Fi while active. On the
> uConsole, connect over Ethernet/USB or a second adapter, or plan to revert
> (`make restore-firmware`).

## 2. Configure a CSI collection

Pick the channel/bandwidth to monitor and (optionally) a MAC to filter to.
`makecsiparams` emits a base64 blob that `nexutil` loads:

```sh
# channel 6, 20 MHz, first core, first spatial stream:
CSIPARAMS=$(makecsiparams -c 6/20 -C 1 -N 1)
ifconfig wlan0 up
nexutil -Iwlan0 -s500 -b -l34 -v"$CSIPARAMS"
# enable monitor + the UDP framing:
iw dev wlan0 interface add mon0 type monitor 2>/dev/null || true
ifconfig mon0 up
```

CSI frames now stream as **UDP packets to port 5500** on `wlan0`.

To get a steady sample rate, generate traffic on the monitored link (CSI is
produced per matching frame). A fixed-rate ping to the AP is the usual trick:

```sh
ping -i 0.05 <AP_or_target_ip>   # ~20 Hz → pass --csi-rate 20 below
```

## 3. Feed it to wifi-radar

Once nexmon is flashed (step 1), **one script does the rest** — build, load the
CSI params, bring up monitor mode, and start the capture pipe:

```sh
sudo ./install.sh --vitals                 # foreground; Ctrl-C to stop
sudo ./install.sh --vitals --service       # or a persistent systemd service
```

Flags (all optional): `--iface wlan0 --channel 6/20 --rate 20 --bind
0.0.0.0:8743 --motion 0.15`. Add `--dry-run` to print every step without
running it. The script (`install/wifi-vitals.sh`) refuses to run and prints
instructions if `nexutil`/`makecsiparams` or the patched firmware are missing.

Open `http://<uconsole>:8743/` — the **Human sensing** panel and the green
radar contact show presence, breathing bpm, and heart bpm. `GET /api/vitals`
returns the raw JSON.

Under the hood it runs the reliable streaming pipe (`-U` = unbuffered):

```sh
sudo tcpdump -i wlan0 -s 0 -U -w - 'udp port 5500' \
  | wifi-radar --csi-pcap - --csi-rate 20 --bind 0.0.0.0:8743
```

Alternative (if your setup delivers datagrams to a local socket, e.g. via a
`socat` bridge): `wifi-radar --nexmon` binds `0.0.0.0:5500` directly.

## 4. Tuning (real hardware needs it)

A minimal DSP can't know your room, antenna, or distance. Knobs:

| Flag | Default | What it does |
| --- | --- | --- |
| `--csi-rate <HZ>` | estimate from arrivals | Fix the sample rate. Set it to your ping rate — the frequency→bpm conversion depends on it. |
| `--csi-motion-threshold <F>` | `0.15` | Presence/motion sensitivity. Raise if it false-triggers on an empty room; lower if it misses a still person. |

Expect breathing to lock in ~15–20 s (the analysis window). Heart rate is
harder: it needs a mostly-still subject, a strong return, and benefits from a
higher CSI rate (100 Hz+). If heart-rate confidence stays low, the amplitude-only
estimator can be extended with unwrapped-phase fusion — the parser already keeps
`CsiFrame::phases` for exactly that upgrade.

## Verifying without hardware

Dev-mode proves the pipeline end to end with synthetic CSI:

```sh
wifi-radar --nexmon --csi-rate 20 &
python3 - <<'PY'
import socket, struct, math
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
for i in range(400):
    t = i/20.0
    hdr = bytes([0x11,0x11,(256-50)&0xff,0]) + b'\x00'*6 + struct.pack('<H', i&0xffff) + b'\x00'*6
    csi = b''.join(struct.pack('<hh', int(500+200*math.sin(2*math.pi*0.25*t)) if s_==20 else 100, 0) for s_ in range(64))
    s.sendto(hdr+csi, ('127.0.0.1', 5500))
PY
curl -s localhost:8743/api/vitals   # → breathing_rate_bpm ~15, presence:true
```
