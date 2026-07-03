#!/usr/bin/env bash
# wifi-vitals.sh — one-shot installer for CSI human sensing (breathing /
# heartbeat / presence) on the ClockworkPi uConsole CM4.
#
# Runs the whole chain:
#   1. install dependencies — apt/dnf/pacman packages + the Rust toolchain
#      (via rustup) if they're missing
#   2. (with --setup-nexmon) build + flash nexmon_csi firmware on the on-board
#      BCM43455c0 — picks the seemoo-lab Makefile.rpi path on 6.x kernels, the
#      nexmon_csi_bin precompiled installer on 5.10/5.4/4.19
#   3. build wifi-radar (with --csi-pcap support)
#   4. configure the CSI collection (makecsiparams + nexutil, monitor mode)
#   5. start the capture pipe:  tcpdump 'udp port 5500' | wifi-radar --csi-pcap -
#      (foreground by default, or as a persistent systemd service with --service)
#
# Without --setup-nexmon the firmware is a prerequisite: the script stops with
# instructions if nexutil/makecsiparams aren't present. Flashing patches the
# Wi-Fi firmware and disrupts normal Wi-Fi while active — it is reversible with
# --revert-nexmon. See docs/wiki/WiFi-Vitals-Nexmon-CM4.md.
#
# Usage:
#   sudo ./install/wifi-vitals.sh [OPTIONS]
#
# Options:
#   --iface <dev>        Wi-Fi interface (default: wlan0)
#   --channel <spec>     makecsiparams chanspec, e.g. 6/20, 36/80 (default: 6/20)
#   --core <n>           makecsiparams core mask -C (default: 1)
#   --nss <n>            makecsiparams spatial-stream mask -N (default: 1)
#   --bind <addr:port>   wifi-radar bind address (default: 0.0.0.0:8743)
#   --rate <hz>          --csi-rate; set to your CSI/ping rate (default: 20)
#   --motion <f>         --csi-motion-threshold presence sensitivity (default: 0.15)
#   --service            Install + enable a systemd service instead of running once
#   --setup-nexmon       Build + flash nexmon_csi firmware (patches Wi-Fi firmware)
#   --revert-nexmon      Restore stock Wi-Fi firmware and exit
#   --nexmon-dir <path>  Where to build nexmon (default: /opt/nexmon)
#   --no-build           Skip the cargo build (use an existing binary)
#   --skip-deps          Don't install apt/Rust deps (assume they're present)
#   -y, --yes            Assume yes (skip the nexmon-flash confirmation)
#   --dry-run            Print every step without executing (runs anywhere)
#   -h, --help           This help
set -euo pipefail

IFACE=wlan0
CHANNEL="6/20"
CORE=1
NSS=1
BIND="0.0.0.0:8743"
RATE=20
MOTION=0.15
DO_SERVICE=0
NO_BUILD=0
DRY_RUN=0
SKIP_DEPS=0
SETUP_NEXMON=0
REVERT_NEXMON=0
ASSUME_YES=0
NEXMON_DIR=/opt/nexmon
SERVICE_NAME=wifi-vitals
PREFIX=/usr/local

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log()  { printf '\033[36m[vitals]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[vitals] warning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31m[vitals] error:\033[0m %s\n' "$*" >&2; exit 1; }

# Execute (or, in --dry-run, just print) a command.
run() {
    if [[ $DRY_RUN -eq 1 ]]; then printf '  + %s\n' "$*"; else "$@"; fi
}

# Find cargo, pulling in an existing rustup install from the usual places
# (root's, or the sudo-invoking user's home) so `sudo` doesn't hide it.
have_cargo() {
    command -v cargo >/dev/null 2>&1 && return 0
    local envf candidates=("$HOME/.cargo/env" "/root/.cargo/env")
    if [[ -n "${SUDO_USER:-}" ]]; then
        local uh; uh="$(getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6)"
        [[ -n "$uh" ]] && candidates+=("$uh/.cargo/env")
    fi
    for envf in "${candidates[@]}"; do
        if [[ -f "$envf" ]]; then
            # shellcheck disable=SC1090
            source "$envf"
            command -v cargo >/dev/null 2>&1 && return 0
        fi
    done
    return 1
}

# Install the Rust toolchain via rustup if cargo isn't already available.
ensure_rust() {
    if have_cargo; then
        log "Rust toolchain: $(command -v cargo 2>/dev/null || echo cargo)"
        return
    fi
    log "Installing Rust toolchain via rustup…"
    run bash -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path"
    # rustup installs to \$HOME/.cargo (root's here, since we run under sudo).
    if [[ $DRY_RUN -eq 0 ]]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
        have_cargo || die "cargo still not found after rustup install"
    fi
}

# Install system packages (a C toolchain + tcpdump + iw) then Rust.
ensure_deps() {
    if [[ $SKIP_DEPS -eq 1 ]]; then
        log "Skipping dependency install (--skip-deps)"
        ensure_rust
        return
    fi
    log "Installing system packages…"
    if command -v apt-get >/dev/null 2>&1; then
        run apt-get update -y
        run apt-get install -y tcpdump iw curl ca-certificates build-essential pkg-config libssl-dev
    elif command -v dnf >/dev/null 2>&1; then
        run dnf install -y tcpdump iw curl ca-certificates gcc gcc-c++ make pkgconf-pkg-config openssl-devel
    elif command -v pacman >/dev/null 2>&1; then
        run pacman -Sy --noconfirm tcpdump iw curl ca-certificates base-devel pkgconf openssl
    else
        warn "unknown package manager — ensure tcpdump, iw and a C toolchain are installed"
    fi
    ensure_rust
}

# Yes/no prompt. Auto-yes under -y or --dry-run; defaults to No.
confirm() {
    [[ $ASSUME_YES -eq 1 || $DRY_RUN -eq 1 ]] && return 0
    local reply
    read -r -p "$1 [y/N] " reply || true
    [[ "$reply" =~ ^[Yy]$ ]]
}

# Print (dry-run) or execute a multi-line shell block under strict mode.
run_block() {
    if [[ $DRY_RUN -eq 1 ]]; then
        printf '  --- would run: ---\n%s\n  ------------------\n' "$1"
    else
        bash -c "set -euo pipefail
$1"
    fi
}

# Build + flash nexmon_csi. Picks the path by kernel: seemoo-lab Makefile.rpi
# for modern (6.x) kernels, the nexmon_csi_bin precompiled installer for the
# legacy 5.10/5.4/4.19 kernels. This PATCHES the Wi-Fi firmware — it's gated
# behind --setup-nexmon and a confirmation.
setup_nexmon() {
    cat >&2 <<'EOF'

  ==================================================================
   nexmon_csi setup PATCHES the Wi-Fi firmware. Normal Wi-Fi will be
   disrupted while CSI is active. Have Ethernet/USB or console access
   to the deck first. It is reversible: sudo ./install.sh --vitals
   --revert-nexmon. Based on seemoo-lab/nexmon_csi (discussions/395).
  ==================================================================
EOF
    confirm "Build & flash nexmon_csi now?" || die "aborted nexmon setup (nothing changed)"

    local kernel; kernel="$(uname -r)"
    log "Kernel: $kernel  →  build dir: $NEXMON_DIR"
    case "$kernel" in
        6.*)                 setup_nexmon_modern ;;
        5.10.*|5.4.*|4.19.*) setup_nexmon_legacy ;;
        *) die "kernel $kernel has no automated path — see docs/wiki/WiFi-Vitals-Nexmon-CM4.md" ;;
    esac
    hash -r 2>/dev/null || true
    if [[ $DRY_RUN -eq 0 ]] && { ! command -v nexutil >/dev/null 2>&1 || ! command -v makecsiparams >/dev/null 2>&1; }; then
        die "nexmon setup ran but nexutil/makecsiparams still aren't on PATH — check the log above."
    fi
    log "nexmon_csi ready. Revert any time: sudo ./install.sh --vitals --revert-nexmon"
}

# Legacy kernels (5.10/5.4/4.19): reuse the maintained precompiled installer.
setup_nexmon_legacy() {
    log "Legacy kernel → nexmonster/nexmon_csi_bin precompiled installer"
    run_block 'curl -fsSL https://raw.githubusercontent.com/nexmonster/nexmon_csi_bin/main/install.sh | bash'
}

# Modern kernels (6.x, Bookworm/Trixie): seemoo-lab Makefile.rpi source build.
# Kernel-agnostic, upgrade-safe (update-alternatives on the Cypress firmware).
setup_nexmon_modern() {
    log "Modern kernel → seemoo-lab Makefile.rpi build (this takes a while)…"
    export NEXMON_DIR
    run_block '
: "${NEXMON_DIR:?}"
export DEBIAN_FRONTEND=noninteractive

# 1. build deps
apt-get update -y
apt-get install -y git libgmp3-dev gawk qpdf bison flex make autoconf libtool \
  texinfo xxd libnl-3-dev libnl-genl-3-dev bc libssl-dev tcpdump
apt-get install -y raspberrypi-kernel-headers || apt-get install -y "linux-headers-$(uname -r)" || true

# 2. armhf cross-libs for the 32-bit nexmon toolchain on a 64-bit userland
dpkg --add-architecture armhf
apt-get update -y
apt-get install -y libc6:armhf libisl23:armhf libmpfr6:armhf libmpc3:armhf libstdc++6:armhf
[ -e /usr/lib/arm-linux-gnueabihf/libisl.so.10 ] || ln -s /usr/lib/arm-linux-gnueabihf/libisl.so.23 /usr/lib/arm-linux-gnueabihf/libisl.so.10
[ -e /usr/lib/arm-linux-gnueabihf/libmpfr.so.4 ] || ln -s /usr/lib/arm-linux-gnueabihf/libmpfr.so.6 /usr/lib/arm-linux-gnueabihf/libmpfr.so.4

# 3. python2.7 (needed by b43-beautifier) from the Debian archive
if ! command -v python2.7 >/dev/null 2>&1; then
  cp /etc/apt/sources.list /tmp/nexmon-sources.bak
  echo "deb http://archive.debian.org/debian/ stretch contrib main non-free" >> /etc/apt/sources.list
  apt-get update -y -o Acquire::Check-Valid-Until=false || true
  apt-get install -y --allow-unauthenticated python2.7 || true
  mv /tmp/nexmon-sources.bak /etc/apt/sources.list
  apt-get update -y || true
fi

# 4. toolchain
rm -rf "$NEXMON_DIR"
git clone --depth=1 https://github.com/seemoo-lab/nexmon.git "$NEXMON_DIR"
cd "$NEXMON_DIR"
source ./setup_env.sh
sed -i "1 s/\$/2.7/" "$NEXMON_ROOT/buildtools/b43-v3/debug/b43-beautifier" || true
make

# 5. nexutil (vendor-command build is required for the Makefile.rpi path)
cd "$NEXMON_ROOT/utilities/nexutil"
make install USE_VENDOR_CMD=1
setcap cap_net_admin+ep "$(command -v nexutil)" || true

# 6. nexmon_csi patch + firmware (update-alternatives on cyfmac43455-sdio.bin)
cd "$NEXMON_ROOT/patches/bcm43455c0/7_45_189"
rm -rf nexmon_csi
git clone --depth=1 https://github.com/seemoo-lab/nexmon_csi.git
cd nexmon_csi
make -f Makefile.rpi install-firmware
make -f Makefile.rpi unmanage
make -f Makefile.rpi reload-full

# 7. makecsiparams has no install target — build and symlink it
cd utils/makecsiparams
make
ln -sf "$PWD/makecsiparams" /usr/local/bin/makecsiparams
ln -sf "$PWD/makecsiparams" /usr/local/bin/mcp
'
}

# Restore stock Wi-Fi firmware (undo --setup-nexmon).
revert_nexmon() {
    log "Reverting nexmon_csi → stock Wi-Fi firmware…"
    run_block '
# Modern (update-alternatives) path
if update-alternatives --list cyfmac43455-sdio.bin >/dev/null 2>&1; then
  update-alternatives --auto cyfmac43455-sdio.bin
fi
# Reload whichever brcmfmac module variant is in use
if modinfo brcmfmac_wcc >/dev/null 2>&1; then
  modprobe -r brcmfmac_wcc 2>/dev/null || true; modprobe brcmfmac_wcc 2>/dev/null || true
else
  modprobe -r brcmfmac 2>/dev/null || true; modprobe brcmfmac 2>/dev/null || true
fi
'
    log "Reverted. If Wi-Fi is still off: sudo apt-get install --reinstall firmware-brcm80211 && sudo reboot"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iface)   IFACE="$2"; shift 2 ;;
        --channel) CHANNEL="$2"; shift 2 ;;
        --core)    CORE="$2"; shift 2 ;;
        --nss)     NSS="$2"; shift 2 ;;
        --bind)    BIND="$2"; shift 2 ;;
        --rate)    RATE="$2"; shift 2 ;;
        --motion)  MOTION="$2"; shift 2 ;;
        --service) DO_SERVICE=1; shift ;;
        --setup-nexmon) SETUP_NEXMON=1; shift ;;
        --revert-nexmon) REVERT_NEXMON=1; shift ;;
        --nexmon-dir) NEXMON_DIR="$2"; shift 2 ;;
        --no-build) NO_BUILD=1; shift ;;
        --skip-deps) SKIP_DEPS=1; shift ;;
        -y|--yes) ASSUME_YES=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) awk 'NR==1{next} /^#/{sub(/^# ?/,"");print;next} {exit}' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

# ---------- 0. preflight ----------
[[ "$(uname -s)" == "Linux" ]] || [[ $DRY_RUN -eq 1 ]] \
    || die "this installs on the Pi (Linux); you're on $(uname -s). Use --dry-run to preview."

if [[ $DRY_RUN -eq 0 && $EUID -ne 0 ]]; then
    die "needs root (tcpdump raw capture + nexutil). Re-run: sudo $0 $*"
fi

# --revert-nexmon short-circuits: undo the firmware patch and stop.
if [[ $REVERT_NEXMON -eq 1 ]]; then
    revert_nexmon
    exit 0
fi

# ---------- 0b. dependencies (apt packages + Rust toolchain) ----------
ensure_deps

# Verify the essentials are now present (unless previewing).
need() { command -v "$1" >/dev/null 2>&1 || [[ $DRY_RUN -eq 1 ]] || die "'$1' still missing after install — see errors above"; }
need cargo
need tcpdump
need iw

# ---------- 0c. nexmon_csi firmware ----------
# --setup-nexmon builds + flashes it; otherwise it's a prerequisite we won't
# auto-flash (a bad flash can disable Wi-Fi on the deck).
if [[ $SETUP_NEXMON -eq 1 ]]; then
    setup_nexmon
fi

if ! command -v nexutil >/dev/null 2>&1 || ! command -v makecsiparams >/dev/null 2>&1; then
    if [[ $DRY_RUN -eq 0 ]]; then
        cat >&2 <<EOF

  nexmon_csi is not installed. CSI capture needs the patched BCM43455c0
  firmware plus 'nexutil' and 'makecsiparams'.

  Let this script build + flash it for you (patches Wi-Fi firmware; reversible):
    sudo ./install.sh --vitals --setup-nexmon
  Or do it by hand: docs/wiki/WiFi-Vitals-Nexmon-CM4.md
EOF
        exit 1
    fi
    warn "nexutil/makecsiparams not found — continuing because --dry-run"
fi

# ---------- 1. build ----------
BIN="$REPO_DIR/target/release/wifi-radar"
if [[ $NO_BUILD -eq 0 ]]; then
    log "Building wifi-radar (release)…"
    run bash -c "cd '$REPO_DIR' && cargo build --release -p wifi-radar"
fi
if [[ $DO_SERVICE -eq 1 ]]; then
    # A service needs a stable binary path.
    log "Installing binary to $PREFIX/bin/wifi-radar"
    run install -Dm755 "$BIN" "$PREFIX/bin/wifi-radar"
    BIN="$PREFIX/bin/wifi-radar"
fi

# ---------- 2. nexmon CSI parameters ----------
# makecsiparams emits the base64 CSI config; nexutil loads it into the firmware.
log "Computing CSI parameters for channel $CHANNEL (core $CORE, nss $NSS)…"
if [[ $DRY_RUN -eq 1 ]]; then
    CSIPARAMS='<makecsiparams output>'
    printf '  + makecsiparams -c %s -C %s -N %s\n' "$CHANNEL" "$CORE" "$NSS"
else
    CSIPARAMS="$(makecsiparams -c "$CHANNEL" -C "$CORE" -N "$NSS")"
fi

# The configure steps, reused by both the foreground path and the service's
# ExecStartPre. Printed as a block so the service unit stays readable.
configure_csi() {
    run ifconfig "$IFACE" up
    run nexutil "-I$IFACE" -s500 -b -l34 -v"$CSIPARAMS"
    # A monitor interface must exist for the firmware to emit CSI frames.
    run bash -c "iw dev '$IFACE' interface add mon0 type monitor 2>/dev/null || true"
    run ifconfig mon0 up
}

# ---------- 3. run or install service ----------
PIPE="tcpdump -i $IFACE -s0 -U -w - 'udp port 5500' | $BIN --csi-pcap - --csi-rate $RATE --csi-motion-threshold $MOTION --bind $BIND"

if [[ $DO_SERVICE -eq 1 ]]; then
    UNIT="/etc/systemd/system/${SERVICE_NAME}.service"
    log "Installing systemd service → $UNIT"
    # ExecStartPre reconfigures CSI on every start (firmware state resets on
    # reboot); ExecStart runs the capture pipe. Runs as root — raw capture +
    # nexutil both require it.
    unit_body="$(cat <<EOF
[Unit]
Description=wifi-radar CSI human sensing (nexmon_csi)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStartPre=/sbin/ifconfig ${IFACE} up
ExecStartPre=/usr/bin/env nexutil -I${IFACE} -s500 -b -l34 -v${CSIPARAMS}
ExecStartPre=/bin/sh -c 'iw dev ${IFACE} interface add mon0 type monitor 2>/dev/null || true'
ExecStartPre=/sbin/ifconfig mon0 up
ExecStart=/bin/sh -c 'tcpdump -i ${IFACE} -s0 -U -w - "udp port 5500" | ${BIN} --csi-pcap - --csi-rate ${RATE} --csi-motion-threshold ${MOTION} --bind ${BIND}'
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
)"
    if [[ $DRY_RUN -eq 1 ]]; then
        printf '  + write %s:\n%s\n' "$UNIT" "$unit_body"
    else
        printf '%s\n' "$unit_body" > "$UNIT"
    fi
    run systemctl daemon-reload
    run systemctl enable "${SERVICE_NAME}.service"
    run systemctl restart "${SERVICE_NAME}.service"
    log "Service '${SERVICE_NAME}' installed. Open: http://<uconsole-ip>:${BIND##*:}/"
    log "Logs: journalctl -u ${SERVICE_NAME} -f"
else
    log "Configuring CSI…"
    configure_csi
    log "Starting capture pipe (Ctrl-C to stop). Open: http://127.0.0.1:${BIND##*:}/"
    log "  $PIPE"
    if [[ $DRY_RUN -eq 1 ]]; then
        printf '  + %s\n' "$PIPE"
    else
        # exec the pipe as the foreground process so Ctrl-C stops both halves.
        exec bash -c "$PIPE"
    fi
fi
