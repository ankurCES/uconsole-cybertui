#!/usr/bin/env bash
# install.sh — install cyberdeck. Run via ./install.sh, or via the curl
# entry point at install/install.sh.
#
# Usage (curl):
#   curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh | bash -s -- [PRESET] [OPTIONS]
#
# Usage (local):
#   ./install.sh --tui
#   ./install.sh --web
#   ./install.sh --full
#   ./install.sh --radar
#   ./install.sh --build
#   ./install.sh --uninstall
#
# Presets:
#   --tui    Build + install the cyberdeck-tui binary to $INSTALL_PREFIX/bin.
#            No sudo, no service, no firewall changes.
#   --web    Build + install cyberdeck-web as a systemd service.
#            Creates a dedicated system user, writes the sudoers fragment,
#            installs the unit, opens the firewall port, generates a token.
#            Skips the TUI binary.
#   --radar  Build + install the wifi-radar binary as a systemd service.
#            Passive 802.11 monitor — no privileged sudoers needed; just
#            a unit, a bind address, and a firewall hole. By default runs
#            in --dev mode (synthetic 8-MAC stream) so you can see it work
#            without a monitor-mode adapter; pass --pcap to point at a
#            capture file. Skips the TUI binary.
#   --full   Both (default if no preset is given). Equivalent to legacy
#            behaviour.
#   --build  Build both binaries into target/release and exit.
#            No install, no sudo, no service. Useful in CI.
#   --deps   Install the system packages the TUI shells out to (bluez,
#            NetworkManager, wpa_supplicant, PipeWire/PulseAudio utilities,
#            brightnessctl, smartmontools, lm_sensors, …) without building
#            or installing anything else. Idempotent; safe to re-run.
#
# Options:
#   -y, --yes            Non-interactive; assume yes for any prompt.
#   --prefix <dir>       Install prefix for binaries (default: /usr/local).
#   --bind <addr>        Web server bind address (default: 0.0.0.0:7878).
#   --service-user <u>   System user for the web service (default: cyberdeck).
#   --refuse-sudo        Refuse to call sudo; useful when --tui or --build
#                        is what you wanted but you forgot to pass it.
#   --skip-deps          Don't try to install OS-level packages. Useful on
#                        a system where you've already installed them by
#                        hand, or where the host firewall blocks apt.
#   --uninstall          Remove installed binaries, user, service, token.
#
# What it does (in order):
#   1. Sanity-checks (repo layout, systemctl).
#   2. Installs OS-level dependencies (apt/dnf/pacman). Skipped with
#      --skip-deps or on distros we don't recognise.
#   3. Bootstraps the Rust toolchain via rustup if cargo is missing, then
#      builds the requested binaries (release) — skipped on --install-only.
#   4. Escalates to root for system install steps (skipped for --tui,
#      --build, and when already root).
#   5. Installs binaries to ${INSTALL_PREFIX}/bin.
#   6. Creates the system user, config dir, and bearer token.
#   7. Installs the NOPASSWD sudoers fragment for the service user.
#   8. Installs the systemd unit.
#   9. Opens the firewall port (ufw if active, hint for nft otherwise).
#  10. Enables and starts the service; prints LAN URL + token.
#
# Re-running is safe: nothing is duplicated, the token is preserved,
# `systemctl enable` and `restart` are idempotent.
#
# NOTE: This script handles the "cargo is installed for a non-root user"
# case (the common one on a personal machine) by doing the build before
# switching to root for the install steps.

set -euo pipefail

# ---------- config (override via env) ----------
SERVICE_USER="${SERVICE_USER:-cyberdeck}"
INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"
BIND_ADDR="${BIND_ADDR:-0.0.0.0:7878}"
REPO_DIR="${REPO_DIR:-$(cd "$(dirname "$0")" && pwd)}"
TOKEN_FILE="${TOKEN_FILE:-/etc/cyberdeck/token}"
# Redirect cargo's build artifacts off tmpfs. When invoked via the curl|bash
# entry point, $REPO_DIR lives in $SCRATCH on tmpfs (often size-capped at
# 1-2 GiB); writing ratatui's rmeta there hits ENOSPC long before linking.
# Cargo honours $CARGO_TARGET_DIR, so we set it to a persistent cache on /
# unless the caller overrides it. The directory is created lazily by cargo
# (mkdir -p on first write), so no mkdir call here.
CYBERDECK_TARGET_DIR="${CYBERDECK_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/cyberdeck/target}"
export CARGO_TARGET_DIR="$CYBERDECK_TARGET_DIR"
# Cargo writes the release binaries into $CARGO_TARGET_DIR/release (not
# $REPO_DIR/target/release as it did before the tmpfs redirect). Every
# post-build [[ -x ]] check and `install -m` below must read from
# $BUILD_DIR so we don't die with "Expected ... — build first" when the
# build succeeded but landed in the redirected cache.
BUILD_DIR="$CARGO_TARGET_DIR/release"
SERVICE_NAME="cyberdeck-web"
RADAR_SERVICE_NAME="wifi-radar"
TUI_BIN="cyberdeck-tui"
WEB_BIN="cyberdeck-web"
RADAR_BIN="wifi-radar"
RADAR_BIND_ADDR="${RADAR_BIND_ADDR:-0.0.0.0:8743}"

# ---------- preset / flag state ----------
PRESET_FULL=0
PRESET_TUI=0
PRESET_WEB=0
PRESET_RADAR=0
PRESET_BUILD=0
PRESET_DEPS=0
ASSUME_YES=0
REFUSE_SUDO=0
DO_UNINSTALL=0
SKIP_DEPS=0
DO_DEPS=0
FORCE_DEV=0
RADAR_TAGS_PATH="${RADAR_TAGS_PATH:-/var/lib/wifi-radar/tags.json}"
RADAR_PCAP="${RADAR_PCAP:-}"
SKIP_MODEL=0
AI_MODEL_DIR="${AI_MODEL_DIR:-${HOME}/.cyberdeck/models}"
AI_MODEL_FILE="MiniCPM5-1B-Q4_K_M.gguf"
AI_MODEL_URL="https://huggingface.co/openbmb/MiniCPM5-1B-GGUF/resolve/main/Q4_K_M.gguf"

usage() {
    sed -n '2,40p' "$0"
    cat <<'EOF'

PRESETS
  --tui      TUI binary only. No sudo, no service.
  --web      Web service only. Sudo for install steps.
  --radar    Wi-Fi radar service. Sudo for install steps.
  --full     TUI + web (default).
  --build    Build only; no install, no sudo.
  --deps     Install OS-level packages the TUI shells out to (bluez,
             NetworkManager, wpa_supplicant, brightnessctl, smartctl,
             PipeWire/PulseAudio utils, journalctl). No build, no sudo
             for cargo.

OPTIONS
  -y, --yes            Non-interactive.
  --prefix <dir>       Bin install prefix (default: /usr/local).
  --bind <addr>        Web bind address (default: 0.0.0.0:7878).
  --radar-bind <addr>  Wi-Fi radar bind address (default: 0.0.0.0:8743).
  --service-user <u>   Service user (default: cyberdeck).
  --radar-dev          Force the wifi-radar into dev mode (synthetic 8-MAC
                       stream). Default in --radar install.
  --radar-pcap <path>  Use a pcap file instead of dev mode.
  --refuse-sudo        Refuse to escalate; require preset to be non-sudo.
  --skip-deps          Don't install OS-level packages.
  --skip-model         Don't download the AI model (MiniCPM5-1B GGUF).
  --uninstall          Reverse the install.

ENV
  INSTALL_PREFIX, BIND_ADDR, RADAR_BIND_ADDR, SERVICE_USER, TOKEN_FILE,
  REPO_DIR override the corresponding flag/default.
EOF
}

# --vitals: CSI human sensing (breathing/heart/presence) has its own installer.
# It needs a different privilege model than the DynamicUser services here (root
# raw capture + nexmon config), so delegate the whole invocation to it.
if [[ "${1:-}" == "--vitals" ]]; then
    shift
    _here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
    _vit="${_here}/install/wifi-vitals.sh"
    [[ -f "$_vit" ]] || _vit="${_here}/wifi-vitals.sh"
    [[ -f "$_vit" ]] || { echo "Run --vitals from a cloned repo (needs cargo build + nexmon). See docs/wiki/WiFi-Vitals-Nexmon-CM4.md" >&2; exit 1; }
    exec bash "$_vit" "$@"
fi

# Map legacy flags to the new ones so older docs / scripts still work.
declare -a PASSTHROUGH=()
i=1
while [[ $# -gt 0 ]]; do
    arg="$1"
    case "$arg" in
        --tui)        PRESET_TUI=1 ;;
        --web)        PRESET_WEB=1 ;;
        --radar)      PRESET_RADAR=1 ;;
        --full)       PRESET_FULL=1 ;;
        --build)      PRESET_BUILD=1 ;;
        --deps)       PRESET_DEPS=1 ;;
        -y|--yes)     ASSUME_YES=1 ;;
        --refuse-sudo) REFUSE_SUDO=1 ;;
        --skip-deps)  SKIP_DEPS=1 ;;
        --skip-model) SKIP_MODEL=1 ;;
        --uninstall)  DO_UNINSTALL=1 ;;
        --prefix)     INSTALL_PREFIX="$2"; shift ;;
        --bind)       BIND_ADDR="$2"; shift ;;
        --radar-bind) RADAR_BIND_ADDR="$2"; shift ;;
        --radar-dev)  FORCE_DEV=1 ;;
        --radar-pcap) RADAR_PCAP="$2"; shift ;;
        --service-user) SERVICE_USER="$2"; shift ;;
        # legacy aliases
        --web-only)   PRESET_WEB=1 ;;
        --radar-only) PRESET_RADAR=1 ;;
        --build-only) PRESET_BUILD=1 ;;
        --install-only) PASSTHROUGH+=("--install-only") ;;  # internal, used by sudo re-exec
        -h|--help)    usage; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

# If --install-only was passed (internal: we're already root), keep it.
INSTALL_ONLY=0
for p in "${PASSTHROUGH[@]}"; do
    [[ "$p" == "--install-only" ]] && INSTALL_ONLY=1
done

# Pick a default preset if none was given.
if [[ $((PRESET_TUI + PRESET_WEB + PRESET_RADAR + PRESET_FULL + PRESET_BUILD + PRESET_DEPS)) -eq 0 ]]; then
    PRESET_FULL=1
fi

# ---------- helpers ----------
log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31mxx\033[0m %s\n' "$*" >&2; exit 1; }
confirm() {
    # No-op under -y / --yes. Otherwise prompt on the tty.
    [[ $ASSUME_YES -eq 1 ]] && return 0
    local prompt="$1"
    local reply
    read -r -p "$(printf '\033[1;33m??\033[0m %s [y/N] ' "$prompt")" reply
    [[ "$reply" =~ ^[Yy]$ ]]
}

# ---------- 0a. distro + package install ----------
# The TUI shells out to a number of system binaries (nmcli, bluetoothctl,
# pactl/wpctl, brightnessctl, smartctl, journalctl, sensors, …). Without
# them, individual screens degrade to "unavailable" messages; with them,
# everything works on a fresh install. We detect the distro from
# /etc/os-release and pick the right package list. Re-running is safe —
# every package manager here is idempotent.

APT_PKGS=(
    # Network: Wi-Fi scan + connect via NetworkManager
    network-manager
    wpasupplicant
    # Bluetooth: device list, pair, connect, trust, adapter power
    bluez
    # Audio: PipeWire first (modern), PulseAudio fallback
    pipewire-audio
    pulseaudio-utils
    alsa-utils
    # Display: brightness slider + output enumeration
    brightnessctl
    wlr-randr
    x11-xserver-utils   # provides xrandr for X11 fallback
    # Storage + sensors: df, lsblk, smart, thermals
    smartmontools
    lm-sensors
    util-linux          # lsblk
    # System: ps, log tail
    procps
    systemd             # journalctl
    # Build deps for the Rust toolchain (+ curl/ca-certificates for rustup)
    build-essential
    pkg-config
    libssl-dev
    libdbus-1-dev
    libudev-dev
    curl
    ca-certificates
)

DNF_PKGS=(
    NetworkManager
    wpa_supplicant
    bluez
    bluez-tools
    pipewire-pulseaudio
    alsa-utils
    brightnessctl
    wlr-randr
    xrandr
    smartmontools
    lm_sensors
    util-linux
    procps-ng
    systemd
    gcc
    gcc-c++
    make
    pkgconf-pkg-config
    openssl-devel
    dbus-devel
    systemd-devel
    curl
    ca-certificates
)

PACMAN_PKGS=(
    networkmanager
    wpa_supplicant
    bluez
    bluez-utils
    pipewire-pulseaudio
    alsa-utils
    brightnessctl
    wlr-randr
    xorg-xrandr
    smartmontools
    lm_sensors
    util-linux
    procps-ng
    systemd
    base-devel
    pkgconf
    openssl
    dbus
    curl
    ca-certificates
)

detect_distro() {
    if [[ -f /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        echo "${ID:-unknown}"
        return 0
    fi
    if command -v apt-get >/dev/null 2>&1; then echo "debian"; return 0; fi
    if command -v dnf >/dev/null 2>&1;     then echo "fedora"; return 0; fi
    if command -v pacman >/dev/null 2>&1;  then echo "arch";   return 0; fi
    echo "unknown"
}

install_deps() {
    local distro
    distro="$(detect_distro)"
    log "Detected distro: $distro"

    case "$distro" in
        debian|ubuntu|pop|elementary|linuxmint|zorin|kali|raspbian)
            if ! command -v apt-get >/dev/null 2>&1; then
                die "apt-get not found on a Debian-family distro; install packages by hand."
            fi
            log "Installing packages via apt-get: ${APT_PKGS[*]}"
            if [[ $EUID -ne 0 ]]; then
                sudo apt-get update
                sudo apt-get install -y "${APT_PKGS[@]}"
            else
                apt-get update
                apt-get install -y "${APT_PKGS[@]}"
            fi
            ;;
        fedora|rhel|centos|rocky|alma|nobara)
            if ! command -v dnf >/dev/null 2>&1; then
                die "dnf not found on a Fedora-family distro; install packages by hand."
            fi
            log "Installing packages via dnf: ${DNF_PKGS[*]}"
            local cmd=(dnf install -y)
            [[ $EUID -ne 0 ]] && cmd=(sudo dnf install -y)
            "${cmd[@]}" "${DNF_PKGS[@]}"
            ;;
        arch|manjaro|endeavouros|garuda|archarm)
            if ! command -v pacman >/dev/null 2>&1; then
                die "pacman not found on an Arch-family distro; install packages by hand."
            fi
            log "Installing packages via pacman: ${PACMAN_PKGS[*]}"
            local cmd=(pacman -Syu --noconfirm --needed)
            [[ $EUID -ne 0 ]] && cmd=(sudo pacman -Syu --noconfirm --needed)
            "${cmd[@]}" "${PACMAN_PKGS[@]}"
            ;;
        *)
            warn "Unknown distro '$distro'. The TUI shells out to:"
            warn "  nmcli bluetoothctl pactl wpctl brightnessctl"
            warn "  smartctl sensors journalctl lsblk xrandr wlr-randr"
            warn "Install those for your distro and re-run with --skip-deps."
            ;;
    esac

    # Enable + start the core services the TUI relies on. These are no-ops
    # if they're already enabled and running (systemctl is idempotent).
    if command -v systemctl >/dev/null 2>&1; then
        log "Enabling NetworkManager + bluetooth daemons"
        local sys_cmd=(systemctl)
        [[ $EUID -ne 0 ]] && sys_cmd=(sudo systemctl)
        for unit in NetworkManager bluetooth; do
            "${sys_cmd[@]}" enable --now "$unit.service" >/dev/null 2>&1 || true
        done
    fi
}

# Ensure a Rust toolchain (cargo) is available, installing it via rustup for
# the current user if missing. The build step runs unprivileged, so this
# installs to the invoking user's ~/.cargo — no root, no system pollution.
# `--no-modify-path` keeps it out of shell profiles; we source the env
# ourselves so the build below sees cargo.
ensure_rust() {
    if command -v cargo >/dev/null 2>&1; then return 0; fi
    # Already installed but not on PATH (fresh rustup, un-sourced shell)?
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
        command -v cargo >/dev/null 2>&1 && return 0
    fi
    log "Rust toolchain not found — installing via rustup (user-local)…"
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --no-modify-path
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --no-modify-path
    else
        die "need curl or wget to install Rust. Install one and re-run, or install Rust manually: https://rustup.rs"
    fi
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env" 2>/dev/null || true
    command -v cargo >/dev/null 2>&1 \
        || die "cargo still not found after rustup install — check the output above."
    log "Installed Rust: $(cargo --version 2>/dev/null || echo cargo)"
}

# ---------- 0a. system dependencies ----------
# Runs early so a failed apt-get doesn't waste a 5-minute cargo build.
# `--deps` short-circuits after this step (no build, no install).
if [[ $DO_DEPS -eq 1 || $PRESET_DEPS -eq 1 || $SKIP_DEPS -eq 0 ]]; then
    if [[ $SKIP_DEPS -eq 1 ]]; then
        log "Skipping OS-level package install (--skip-deps)"
    else
        if [[ $DO_DEPS -eq 1 || $PRESET_DEPS -eq 1 || $ASSUME_YES -eq 1 ]]; then
            install_deps
        else
            confirm "Install OS-level packages (apt/dnf/pacman)?" && install_deps \
                || warn "Skipping OS deps; the TUI may show 'unavailable' on some screens."
        fi
    fi
fi

if [[ $PRESET_DEPS -eq 1 ]]; then
    log "--deps: dependencies installed. Nothing more to do."
    exit 0
fi

# ---------- 0b. AI model download ----------
# Downloads MiniCPM5-1B Q4_K_M GGUF (~688 MB) to ~/.cyberdeck/models/.
# Runs before the build so the model is ready when the TUI binary lands.
# Skipped with --skip-model, --web (no TUI), --radar (no TUI), or --build.
# Idempotent: skips if the file already exists at the expected path.
NEED_MODEL=0
[[ $PRESET_TUI -eq 1 || $PRESET_FULL -eq 1 ]] && NEED_MODEL=1
if [[ $NEED_MODEL -eq 1 && $SKIP_MODEL -eq 0 && $INSTALL_ONLY -eq 0 ]]; then
    MODEL_DEST="${AI_MODEL_DIR}/${AI_MODEL_FILE}"
    if [[ -f "$MODEL_DEST" ]]; then
        log "AI model already exists: $MODEL_DEST"
    else
        log "Downloading AI model: MiniCPM5-1B Q4_K_M (~688 MB)"
        log "  → $MODEL_DEST"
        mkdir -p "$AI_MODEL_DIR"
        if command -v curl >/dev/null 2>&1; then
            curl -L --progress-bar -o "${MODEL_DEST}.part" "$AI_MODEL_URL" \
                && mv "${MODEL_DEST}.part" "$MODEL_DEST"
        elif command -v wget >/dev/null 2>&1; then
            wget --show-progress -O "${MODEL_DEST}.part" "$AI_MODEL_URL" \
                && mv "${MODEL_DEST}.part" "$MODEL_DEST"
        else
            warn "Neither curl nor wget found — skipping AI model download."
            warn "Download manually: $AI_MODEL_URL → $MODEL_DEST"
        fi
        if [[ -f "$MODEL_DEST" ]]; then
            log "AI model downloaded: $(du -h "$MODEL_DEST" | cut -f1) at $MODEL_DEST"
        fi
    fi
fi

# ---------- 0. uninstall ----------
if [[ $DO_UNINSTALL -eq 1 ]]; then
    log "Uninstalling cyberdeck (preset-agnostic)"
    if [[ $EUID -ne 0 ]]; then
        log "Re-executing with sudo for uninstall"
        exec sudo -E REPO_DIR="$REPO_DIR" \
               CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
               INSTALL_PREFIX="$INSTALL_PREFIX" \
               RADAR_TAGS_PATH="$RADAR_TAGS_PATH" \
               SERVICE_USER="$SERVICE_USER" \
               TOKEN_FILE="$TOKEN_FILE" \
               SERVICE_NAME="$SERVICE_NAME" \
               RADAR_SERVICE_NAME="$RADAR_SERVICE_NAME" \
               "$0" --uninstall
    fi
    systemctl disable --now "${SERVICE_NAME}.service" 2>/dev/null || true
    rm -f "/etc/systemd/system/${SERVICE_NAME}.service"
    systemctl disable --now "${RADAR_SERVICE_NAME}.service" 2>/dev/null || true
    rm -f "/etc/systemd/system/${RADAR_SERVICE_NAME}.service"
    rm -f "/etc/sudoers.d/cyberdeck"
    rm -f "${INSTALL_PREFIX}/bin/${WEB_BIN}"
    rm -f "${INSTALL_PREFIX}/bin/${TUI_BIN}"
    rm -f "${INSTALL_PREFIX}/bin/${RADAR_BIN}"
    rm -rf /etc/cyberdeck
    rm -rf "$(dirname -- "${RADAR_TAGS_PATH}")"
    if id -u "$SERVICE_USER" >/dev/null 2>&1; then
        userdel "$SERVICE_USER" 2>/dev/null || warn "Could not delete user $SERVICE_USER"
    fi
    systemctl daemon-reload
    log "Uninstall complete."
    exit 0
fi

# ---------- 1. sanity ----------
[[ -d "$REPO_DIR/crates" ]] \
    || die "Could not find $REPO_DIR/crates — set REPO_DIR to the repo root."
if [[ $PRESET_WEB -eq 1 || $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
    command -v systemctl >/dev/null 2>&1 \
        || die "systemctl not found — --web/--radar/--full need a systemd system."
fi

# ---------- 2. refuse-sudo guard ----------
if [[ $REFUSE_SUDO -eq 1 ]]; then
    if [[ $PRESET_WEB -eq 1 || $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
        die "--refuse-sudo given but preset needs sudo. Use --tui or --build instead."
    fi
fi

# ---------- 3. build (unprivileged) ----------
NEED_BUILD=0
[[ $PRESET_TUI  -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]] && NEED_BUILD=1
NEED_WEB_BUILD=0
[[ $PRESET_WEB  -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]] && NEED_WEB_BUILD=1
NEED_RADAR_BUILD=0
[[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]] && NEED_RADAR_BUILD=1

if [[ $INSTALL_ONLY -eq 0 ]] && [[ $NEED_BUILD -eq 1 || $NEED_WEB_BUILD -eq 1 || $NEED_RADAR_BUILD -eq 1 ]]; then
    ensure_rust
    if [[ $NEED_WEB_BUILD -eq 1 ]]; then
        log "Building ${WEB_BIN} (release) as $(id -un)…"
        ( cd "$REPO_DIR" && cargo build --release -p cyberdeck-web )
    fi
    if [[ $NEED_RADAR_BUILD -eq 1 ]]; then
        log "Building ${RADAR_BIN} (release) as $(id -un)…"
        ( cd "$REPO_DIR" && cargo build --release -p wifi-radar )
    fi
    if [[ $NEED_BUILD -eq 1 ]]; then
        log "Building ${TUI_BIN} (release)…"
        # The `http` feature compiles in `reqwest` + the
        # `HttpLoraTransport` so the LoRa screen can talk to a
        # Meshtastic node over LAN HTTP. Without it the screen falls
        # back to the in-process `FakeTransport` (always disconnected)
        # and shows "not connected" even when the user has typed a
        # valid node IP. The feature is enabled by default in
        # `crates/tui/Cargo.toml`; we pass `--features http`
        # explicitly so an opt-out via `default-features = false`
        # still gets the transport.
        ( cd "$REPO_DIR" && cargo build --release -p cyberdeck-tui --features http )
    fi
fi

if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]]; then
    [[ -x "$BUILD_DIR/${WEB_BIN}" ]] \
        || die "Expected $BUILD_DIR/${WEB_BIN} — build first."
fi

if [[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]]; then
    [[ -x "$BUILD_DIR/${RADAR_BIN}" ]] \
        || die "Expected $BUILD_DIR/${RADAR_BIN} — build first."
fi

if [[ $PRESET_BUILD -eq 1 ]]; then
    log "Build complete."
    log "  ${WEB_BIN}: $BUILD_DIR/${WEB_BIN}"
    log "  ${TUI_BIN}: $BUILD_DIR/${TUI_BIN}"
    log "  ${RADAR_BIN}: $BUILD_DIR/${RADAR_BIN}"
    exit 0
fi

# ---------- 4. escalate to root for the rest (only when needed) ----------
NEED_SUDO_STEPS=0
[[ $PRESET_TUI -eq 1 ]] && [[ "$INSTALL_PREFIX" == "/usr/local" || "$INSTALL_PREFIX" == "/usr" ]] && NEED_SUDO_STEPS=1
[[ $PRESET_WEB -eq 1 || $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]] && NEED_SUDO_STEPS=1

if [[ $NEED_SUDO_STEPS -eq 1 ]] && [[ $EUID -ne 0 ]]; then
    log "Re-executing with sudo for system install steps"
    exec sudo -E REPO_DIR="$REPO_DIR" \
               CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
               INSTALL_PREFIX="$INSTALL_PREFIX" \
               BIND_ADDR="$BIND_ADDR" \
               RADAR_BIND_ADDR="$RADAR_BIND_ADDR" \
               FORCE_DEV="$FORCE_DEV" \
               RADAR_PCAP="$RADAR_PCAP" \
               RADAR_TAGS_PATH="$RADAR_TAGS_PATH" \
               SERVICE_USER="$SERVICE_USER" \
               TOKEN_FILE="$TOKEN_FILE" \
               PRESET_TUI="$PRESET_TUI" \
               PRESET_WEB="$PRESET_WEB" \
               PRESET_RADAR="$PRESET_RADAR" \
               PRESET_FULL="$PRESET_FULL" \
               ASSUME_YES="$ASSUME_YES" \
               "$0" --install-only
fi

# === running as root from here on ===

# ---------- 5. install binaries ----------
if [[ $PRESET_TUI -eq 1 || $PRESET_FULL -eq 1 ]]; then
    [[ -x "$BUILD_DIR/${TUI_BIN}" ]] \
        || die "Expected $BUILD_DIR/${TUI_BIN} — build first."
    log "Installing ${TUI_BIN} to ${INSTALL_PREFIX}/bin/${TUI_BIN}"
    install -m 0755 \
        "$BUILD_DIR/${TUI_BIN}" \
        "${INSTALL_PREFIX}/bin/${TUI_BIN}"
fi

if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    [[ -x "$BUILD_DIR/${WEB_BIN}" ]] \
        || die "Expected $BUILD_DIR/${WEB_BIN} — build first."
    log "Installing ${WEB_BIN} to ${INSTALL_PREFIX}/bin/${WEB_BIN}"
    install -m 0755 \
        "$BUILD_DIR/${WEB_BIN}" \
        "${INSTALL_PREFIX}/bin/${WEB_BIN}"
fi

if [[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
    [[ -x "$BUILD_DIR/${RADAR_BIN}" ]] \
        || die "Expected $BUILD_DIR/${RADAR_BIN} — build first."
    log "Installing ${RADAR_BIN} to ${INSTALL_PREFIX}/bin/${RADAR_BIN}"
    install -m 0755 \
        "$BUILD_DIR/${RADAR_BIN}" \
        "${INSTALL_PREFIX}/bin/${RADAR_BIN}"
fi

# ---------- 6. (web only) system user ----------
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
        log "Creating system user '$SERVICE_USER'"
        useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
    else
        log "System user '$SERVICE_USER' already exists"
    fi
fi

# ---------- 7. (web only) config dir + token ----------
# The service runs as $SERVICE_USER, so it must be able to traverse the
# config dir and read the token file. We make the dir group-traversable by
# the service user's primary group and the file group-readable. Without
# this, `cyberdeck-web --token-file /etc/cyberdeck/token` silently falls
# back to generating a fresh token every start (the URL the installer
# prints goes stale on the first restart).
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    mkdir -p /etc/cyberdeck
    chown "root:${SERVICE_USER}" /etc/cyberdeck
    chmod 0750 /etc/cyberdeck

    if [[ -s "$TOKEN_FILE" ]]; then
        TOKEN="$(tr -d '[:space:]' < "$TOKEN_FILE")"
        log "Reusing existing token from $TOKEN_FILE"
    else
        # 32 url-safe chars, plenty of entropy for a LAN token. We deliberately
        # avoid `tr … | head -c N` here: under `set -o pipefail` the SIGPIPE that
        # `tr` receives when `head` closes the pipe after N bytes propagates as
        # exit 141 and aborts the whole installer. Read a larger chunk once and
        # filter in-process.
        TOKEN=""
        while [[ ${#TOKEN} -lt 32 ]]; do
            chunk="$(head -c 192 </dev/urandom | LC_ALL=C tr -dc 'A-Za-z0-9')"
            TOKEN="${TOKEN}${chunk}"
        done
        TOKEN="${TOKEN:0:32}"
        umask 0037
        printf '%s\n' "$TOKEN" > "$TOKEN_FILE"
        umask 0022
        chown "root:${SERVICE_USER}" "$TOKEN_FILE"
        chmod 0640 "$TOKEN_FILE"
        log "Generated new bearer token, saved to $TOKEN_FILE"
    fi
fi

# ---------- 8. (web only) sudoers fragment ----------
# cyberdeck-core uses `sudo -n <cmd>` to drive privileged actions. Allow the
# service user the narrow set of commands it actually needs, no password.
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    SUDOERS_FILE="/etc/sudoers.d/cyberdeck"
    log "Writing $SUDOERS_FILE"
    cat > "$SUDOERS_FILE" <<EOF
# Cyberdeck web UI: allow the service user to run privileged commands
# without a password. Keep this list narrow on purpose.
${SERVICE_USER} ALL=(root) NOPASSWD: \\
    /usr/bin/systemctl start *, \\
    /usr/bin/systemctl stop *, \\
    /usr/bin/systemctl restart *, \\
    /usr/bin/systemctl enable *, \\
    /usr/bin/systemctl disable *, \\
    /usr/bin/systemctl daemon-reload, \\
    /usr/bin/systemctl suspend, \\
    /usr/bin/systemctl hibernate, \\
    /usr/bin/systemctl reboot, \\
    /usr/bin/systemctl poweroff, \\
    /usr/bin/nmcli connection up *, \\
    /usr/bin/nmcli connection down *, \\
    /usr/bin/nmcli device wifi *, \\
    /usr/bin/nmcli radio *
EOF
    chmod 0440 "$SUDOERS_FILE"
    visudo -c -f "$SUDOERS_FILE" >/dev/null
fi

# ---------- 9. (web only) systemd unit ----------
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    UNIT_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
    log "Writing $UNIT_FILE"
    cat > "$UNIT_FILE" <<EOF
[Unit]
Description=Cyberdeck Web UI (LAN access for the uconsole)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_USER}
ExecStart=${INSTALL_PREFIX}/bin/${WEB_BIN} ${BIND_ADDR} --token-file ${TOKEN_FILE}
Restart=on-failure
RestartSec=2

# Hardening — adjust if a future module needs something more.
NoNewPrivileges=false     # sudo needs setuid, leave it
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/etc/cyberdeck

[Install]
WantedBy=multi-user.target
EOF
fi

# ---------- 9b. (radar only) data dir + systemd unit ----------
# wifi-radar only writes the persistent tag DB at --tags PATH; the
# scanner in --dev mode doesn't touch the radio at all, so we run the
# service unprivileged (DynamicUser), avoiding the cap_net_raw requirement
# of monitor-mode live capture. For real radiotap capture, run the binary
# by hand with sudo.
if [[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
    RADAR_TAGS_DIR="$(dirname -- "${RADAR_TAGS_PATH}")"
    mkdir -p "${RADAR_TAGS_DIR}"
    if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
        log "Creating system user '$SERVICE_USER'"
        useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
    fi
    chown "${SERVICE_USER}:${SERVICE_USER}" "${RADAR_TAGS_DIR}"

    RADAR_UNIT_FILE="/etc/systemd/system/${RADAR_SERVICE_NAME}.service"
    log "Writing $RADAR_UNIT_FILE"
    if [[ -n "${RADAR_PCAP:-}" ]]; then
        RADAR_ARGS="--bind ${RADAR_BIND_ADDR} --pcap ${RADAR_PCAP} --tags ${RADAR_TAGS_PATH}"
    else
        RADAR_DEV_FLAG=""
        [[ $FORCE_DEV -eq 1 ]] && RADAR_DEV_FLAG="--dev "
        RADAR_ARGS="--bind ${RADAR_BIND_ADDR} ${RADAR_DEV_FLAG}--tags ${RADAR_TAGS_PATH}"
    fi
    cat > "$RADAR_UNIT_FILE" <<EOF
[Unit]
Description=Wi-Fi Radar (passive 802.11 monitor with synthetic fallback)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
DynamicUser=yes
StateDirectory=wifi-radar
ExecStart=${INSTALL_PREFIX}/bin/${RADAR_BIN} ${RADAR_ARGS}
Restart=on-failure
RestartSec=2

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/wifi-radar

[Install]
WantedBy=multi-user.target
EOF
fi

# ---------- 10. (web only) firewall ----------
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
        log "Opening ${BIND_ADDR##*:}/tcp in ufw"
        ufw allow "${BIND_ADDR##*:}/tcp" comment "cyberdeck-web" >/dev/null || true
    elif command -v nft >/dev/null 2>&1 && [[ -f /etc/nftables.conf ]]; then
        warn "nftables detected — not editing /etc/nftables.conf automatically."
        warn "If the host firewall is nftables, add a rule to allow ${BIND_ADDR##*:}/tcp."
    else
        warn "No active firewall detected (no ufw, no nftables.conf). Skipping."
    fi
fi

# ---------- 10b. (radar only) firewall ----------
if [[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
    RADAR_PORT="${RADAR_BIND_ADDR##*:}"
    if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
        log "Opening ${RADAR_PORT}/tcp in ufw"
        ufw allow "${RADAR_PORT}/tcp" comment "wifi-radar" >/dev/null || true
    elif command -v nft >/dev/null 2>&1 && [[ -f /etc/nftables.conf ]]; then
        warn "nftables detected — not editing /etc/nftables.conf automatically."
        warn "If the host firewall is nftables, add a rule to allow ${RADAR_PORT}/tcp."
    else
        warn "No active firewall detected (no ufw, no nftables.conf). Skipping."
    fi
fi

# ---------- 11. (web only) enable + start ----------
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    log "Reloading systemd, enabling and starting $SERVICE_NAME"
    systemctl daemon-reload
    systemctl enable "${SERVICE_NAME}.service" >/dev/null
    systemctl restart "${SERVICE_NAME}.service"

    sleep 1
    if ! systemctl is-active --quiet "${SERVICE_NAME}.service"; then
        die "Service failed to start. Last 20 log lines:
$(journalctl -u "${SERVICE_NAME}.service" -n 20 --no-pager 2>&1 || true)"
    fi
fi

# ---------- 11b. (radar only) enable + start ----------
if [[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
    log "Reloading systemd, enabling and starting $RADAR_SERVICE_NAME"
    systemctl daemon-reload
    systemctl enable "${RADAR_SERVICE_NAME}.service" >/dev/null
    systemctl restart "${RADAR_SERVICE_NAME}.service"

    sleep 1
    if ! systemctl is-active --quiet "${RADAR_SERVICE_NAME}.service"; then
        die "Service failed to start. Last 20 log lines:
$(journalctl -u "${RADAR_SERVICE_NAME}.service" -n 20 --no-pager 2>&1 || true)"
    fi
fi

# ---------- 12. summary ----------
HOST_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
PORT="${BIND_ADDR##*:}"

cat <<EOF

  ✓ cyberdeck installed.

EOF

if [[ $PRESET_TUI -eq 1 || $PRESET_FULL -eq 1 ]]; then
    cat <<EOF
    TUI       : ${INSTALL_PREFIX}/bin/${TUI_BIN}
    AI model  : ${AI_MODEL_DIR}/${AI_MODEL_FILE}
    Launch    : ${TUI_BIN}
    Keys      : ? for help, q to quit, Ctrl-W is the window-manager prefix

EOF
fi

if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    cat <<EOF
    Service   : systemctl {status,restart,stop} ${SERVICE_NAME}
    Logs      : journalctl -u ${SERVICE_NAME} -f
    Token     : stored at ${TOKEN_FILE} (delete the file to regenerate)
    LAN URL   : http://${HOST_IP:-<host>}:${PORT}/?token=${TOKEN}
    Local URL : http://127.0.0.1:${PORT}/?token=${TOKEN}

EOF
fi

if [[ $PRESET_RADAR -eq 1 || $PRESET_FULL -eq 1 ]]; then
    RADAR_PORT="${RADAR_BIND_ADDR##*:}"
    cat <<EOF
    Radar     : systemctl {status,restart,stop} ${RADAR_SERVICE_NAME}
    Logs      : journalctl -u ${RADAR_SERVICE_NAME} -f
    Tags      : ${RADAR_TAGS_PATH}
    LAN URL   : http://${HOST_IP:-<host>}:${RADAR_PORT}/
    Local URL : http://127.0.0.1:${RADAR_PORT}/

EOF
fi

cat <<EOF
  Re-running this installer is safe; it will rebuild in place and
  preserve the existing token.
  To uninstall:    $(basename "$0") --uninstall
EOF
