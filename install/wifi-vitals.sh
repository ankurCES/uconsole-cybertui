#!/usr/bin/env bash
# wifi-vitals.sh — one-shot installer for CSI human sensing (breathing /
# heartbeat / presence) on the ClockworkPi uConsole CM4.
#
# Runs the whole chain:
#   1. install dependencies — apt/dnf/pacman packages + the Rust toolchain
#      (via rustup) if they're missing
#   2. build wifi-radar (with --csi-pcap support)
#   3. configure nexmon_csi on the on-board BCM43455c0 (makecsiparams+nexutil)
#   4. start the capture pipe:  tcpdump 'udp port 5500' | wifi-radar --csi-pcap -
#      (foreground by default, or as a persistent systemd service with --service)
#
# It does NOT flash nexmon firmware — that step is kernel-version-specific and
# can knock out Wi-Fi on a headless deck, so it must be done by hand once. See
# docs/wiki/WiFi-Vitals-Nexmon-CM4.md. This script checks the firmware/tools are
# in place and stops with clear instructions if they aren't.
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
#   --no-build           Skip the cargo build (use an existing binary)
#   --skip-deps          Don't install apt/Rust deps (assume they're present)
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
        --no-build) NO_BUILD=1; shift ;;
        --skip-deps) SKIP_DEPS=1; shift ;;
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

# ---------- 0b. dependencies (apt packages + Rust toolchain) ----------
ensure_deps

# Verify the essentials are now present (unless previewing).
need() { command -v "$1" >/dev/null 2>&1 || [[ $DRY_RUN -eq 1 ]] || die "'$1' still missing after install — see errors above"; }
need cargo
need tcpdump
need iw

# nexmon tools are the hard prerequisite — we do not auto-flash firmware.
if ! command -v nexutil >/dev/null 2>&1 || ! command -v makecsiparams >/dev/null 2>&1; then
    if [[ $DRY_RUN -eq 0 ]]; then
        cat >&2 <<EOF

  nexmon_csi is not installed. CSI capture needs the patched BCM43455c0
  firmware plus 'nexutil' and 'makecsiparams'. This is a one-time, kernel-
  specific step this script deliberately does not automate (a bad flash can
  disable Wi-Fi on the deck).

  Install it once, matching your kernel ($(uname -r)):
    https://github.com/nexmonster/nexmon_csi   (pick the pi-<kernel> branch)
  Full walkthrough: docs/wiki/WiFi-Vitals-Nexmon-CM4.md

  Then re-run this script.
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
