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
#   --full   Both (default if no preset is given). Equivalent to legacy
#            behaviour.
#   --build  Build both binaries into target/release and exit.
#            No install, no sudo, no service. Useful in CI.
#
# Options:
#   -y, --yes            Non-interactive; assume yes for any prompt.
#   --prefix <dir>       Install prefix for binaries (default: /usr/local).
#   --bind <addr>        Web server bind address (default: 0.0.0.0:7878).
#   --service-user <u>   System user for the web service (default: cyberdeck).
#   --refuse-sudo        Refuse to call sudo; useful when --tui or --build
#                        is what you wanted but you forgot to pass it.
#   --uninstall          Remove installed binaries, user, service, token.
#
# What it does (in order):
#   1. Sanity-checks (repo layout, cargo, systemctl).
#   2. Builds the requested binaries (release) — skipped on --install-only.
#   3. Escalates to root for system install steps (skipped for --tui,
#      --build, and when already root).
#   4. Installs binaries to ${INSTALL_PREFIX}/bin.
#   5. Creates the system user, config dir, and bearer token.
#   6. Installs the NOPASSWD sudoers fragment for the service user.
#   7. Installs the systemd unit.
#   8. Opens the firewall port (ufw if active, hint for nft otherwise).
#   9. Enables and starts the service; prints LAN URL + token.
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
SERVICE_NAME="cyberdeck-web"
TUI_BIN="cyberdeck-tui"
WEB_BIN="cyberdeck-web"

# ---------- preset / flag state ----------
PRESET_FULL=0
PRESET_TUI=0
PRESET_WEB=0
PRESET_BUILD=0
ASSUME_YES=0
REFUSE_SUDO=0
DO_UNINSTALL=0

usage() {
    sed -n '2,40p' "$0"
    cat <<'EOF'

PRESETS
  --tui      TUI binary only. No sudo, no service.
  --web      Web service only. Sudo for install steps.
  --full     TUI + web (default).
  --build    Build only; no install, no sudo.

OPTIONS
  -y, --yes            Non-interactive.
  --prefix <dir>       Bin install prefix (default: /usr/local).
  --bind <addr>        Web bind address (default: 0.0.0.0:7878).
  --service-user <u>   Service user (default: cyberdeck).
  --refuse-sudo        Refuse to escalate; require preset to be non-sudo.
  --uninstall          Reverse the install.

ENV
  INSTALL_PREFIX, BIND_ADDR, SERVICE_USER, TOKEN_FILE, REPO_DIR
  override the corresponding flag/default.
EOF
}

# Map legacy flags to the new ones so older docs / scripts still work.
declare -a PASSTHROUGH=()
i=1
while [[ $# -gt 0 ]]; do
    arg="$1"
    case "$arg" in
        --tui)        PRESET_TUI=1 ;;
        --web)        PRESET_WEB=1 ;;
        --full)       PRESET_FULL=1 ;;
        --build)      PRESET_BUILD=1 ;;
        -y|--yes)     ASSUME_YES=1 ;;
        --refuse-sudo) REFUSE_SUDO=1 ;;
        --uninstall)  DO_UNINSTALL=1 ;;
        --prefix)     INSTALL_PREFIX="$2"; shift ;;
        --bind)       BIND_ADDR="$2"; shift ;;
        --service-user) SERVICE_USER="$2"; shift ;;
        # legacy aliases
        --web-only)   PRESET_WEB=1 ;;
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
if [[ $((PRESET_TUI + PRESET_WEB + PRESET_FULL + PRESET_BUILD)) -eq 0 ]]; then
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

# ---------- 0. uninstall ----------
if [[ $DO_UNINSTALL -eq 1 ]]; then
    log "Uninstalling cyberdeck (preset-agnostic)"
    if [[ $EUID -ne 0 ]]; then
        log "Re-executing with sudo for uninstall"
        exec sudo -E REPO_DIR="$REPO_DIR" \
                   INSTALL_PREFIX="$INSTALL_PREFIX" \
                   SERVICE_USER="$SERVICE_USER" \
                   TOKEN_FILE="$TOKEN_FILE" \
                   SERVICE_NAME="$SERVICE_NAME" \
                   "$0" --uninstall
    fi
    systemctl disable --now "${SERVICE_NAME}.service" 2>/dev/null || true
    rm -f "/etc/systemd/system/${SERVICE_NAME}.service"
    rm -f "/etc/sudoers.d/cyberdeck"
    rm -f "${INSTALL_PREFIX}/bin/${WEB_BIN}"
    rm -f "${INSTALL_PREFIX}/bin/${TUI_BIN}"
    rm -rf /etc/cyberdeck
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
if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    command -v systemctl >/dev/null 2>&1 \
        || die "systemctl not found — --web/--full need a systemd system."
fi

# ---------- 2. refuse-sudo guard ----------
if [[ $REFUSE_SUDO -eq 1 ]]; then
    if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
        die "--refuse-sudo given but preset needs sudo. Use --tui or --build instead."
    fi
fi

# ---------- 3. build (unprivileged) ----------
NEED_BUILD=0
[[ $PRESET_TUI  -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]] && NEED_BUILD=1
NEED_WEB_BUILD=0
[[ $PRESET_WEB  -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]] && NEED_WEB_BUILD=1

if [[ $INSTALL_ONLY -eq 0 ]] && [[ $NEED_BUILD -eq 1 || $NEED_WEB_BUILD -eq 1 ]]; then
    command -v cargo >/dev/null 2>&1 \
        || die "cargo not found in PATH. Install Rust first: https://rustup.rs"
    if [[ $NEED_WEB_BUILD -eq 1 ]]; then
        log "Building ${WEB_BIN} (release) as $(id -un)…"
        ( cd "$REPO_DIR" && cargo build --release -p cyberdeck-web )
    fi
    if [[ $NEED_BUILD -eq 1 ]]; then
        log "Building ${TUI_BIN} (release)…"
        ( cd "$REPO_DIR" && cargo build --release -p cyberdeck-tui )
    fi
fi

if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 || $PRESET_BUILD -eq 1 ]]; then
    [[ -x "$REPO_DIR/target/release/${WEB_BIN}" ]] \
        || die "Expected $REPO_DIR/target/release/${WEB_BIN} — build first."
fi

if [[ $PRESET_BUILD -eq 1 ]]; then
    log "Build complete."
    log "  ${WEB_BIN}: $REPO_DIR/target/release/${WEB_BIN}"
    log "  ${TUI_BIN}: $REPO_DIR/target/release/${TUI_BIN}"
    exit 0
fi

# ---------- 4. escalate to root for the rest (only when needed) ----------
NEED_SUDO_STEPS=0
[[ $PRESET_TUI -eq 1 ]] && [[ "$INSTALL_PREFIX" == "/usr/local" || "$INSTALL_PREFIX" == "/usr" ]] && NEED_SUDO_STEPS=1
[[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]] && NEED_SUDO_STEPS=1

if [[ $NEED_SUDO_STEPS -eq 1 ]] && [[ $EUID -ne 0 ]]; then
    log "Re-executing with sudo for system install steps"
    exec sudo -E REPO_DIR="$REPO_DIR" \
               INSTALL_PREFIX="$INSTALL_PREFIX" \
               BIND_ADDR="$BIND_ADDR" \
               SERVICE_USER="$SERVICE_USER" \
               TOKEN_FILE="$TOKEN_FILE" \
               PRESET_TUI="$PRESET_TUI" \
               PRESET_WEB="$PRESET_WEB" \
               PRESET_FULL="$PRESET_FULL" \
               ASSUME_YES="$ASSUME_YES" \
               "$0" --install-only
fi

# === running as root from here on ===

# ---------- 5. install binaries ----------
if [[ $PRESET_TUI -eq 1 || $PRESET_FULL -eq 1 ]]; then
    [[ -x "$REPO_DIR/target/release/${TUI_BIN}" ]] \
        || die "Expected $REPO_DIR/target/release/${TUI_BIN} — build first."
    log "Installing ${TUI_BIN} to ${INSTALL_PREFIX}/bin/${TUI_BIN}"
    install -m 0755 \
        "$REPO_DIR/target/release/${TUI_BIN}" \
        "${INSTALL_PREFIX}/bin/${TUI_BIN}"
fi

if [[ $PRESET_WEB -eq 1 || $PRESET_FULL -eq 1 ]]; then
    log "Installing ${WEB_BIN} to ${INSTALL_PREFIX}/bin/${WEB_BIN}"
    install -m 0755 \
        "$REPO_DIR/target/release/${WEB_BIN}" \
        "${INSTALL_PREFIX}/bin/${WEB_BIN}"
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

# ---------- 12. summary ----------
HOST_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
PORT="${BIND_ADDR##*:}"

cat <<EOF

  ✓ cyberdeck installed.

EOF

if [[ $PRESET_TUI -eq 1 || $PRESET_FULL -eq 1 ]]; then
    cat <<EOF
    TUI       : ${INSTALL_PREFIX}/bin/${TUI_BIN}
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

cat <<EOF
  Re-running this installer is safe; it will rebuild in place and
  preserve the existing token.
  To uninstall:    $(basename "$0") --uninstall
EOF
