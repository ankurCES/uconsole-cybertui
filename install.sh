#!/usr/bin/env bash
# install.sh — install cyberdeck-web as a systemd service, LAN-accessible.
# Also installs the cyberdeck-tui binary so it can be launched from any shell.
#
# Usage:
#   ./install.sh                  # build + install TUI and web
#   ./install.sh --web-only       # install only cyberdeck-web (skip TUI build/install)
#   ./install.sh --build-only     # just build, don't touch /etc or start the service
#   ./install.sh --install-only   # assume binaries already at target/release/...
#
# What it does:
#   1. Builds cyberdeck-web (release) from this repo, unless --install-only.
#   2. Builds cyberdeck-tui (release) too, unless --web-only.
#   3. Installs both binaries to /usr/local/bin (as root).
#   4. Creates a dedicated 'cyberdeck' system user (no password, no shell).
#   5. Installs a NOPASSWD sudoers fragment so the service can drive
#      systemctl / network / power without prompting.
#   6. Installs /etc/systemd/system/cyberdeck-web.service.
#   7. Generates (or reuses) a bearer token, stores it in /etc/cyberdeck/token.
#   8. Opens the firewall port (ufw if active, otherwise hints for nft).
#   9. Enables and starts the service, then prints the LAN URL + token.
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
BUILD_ONLY=0
INSTALL_ONLY=0
WEB_ONLY=0

for arg in "$@"; do
    case "$arg" in
        --build-only)   BUILD_ONLY=1 ;;
        --install-only) INSTALL_ONLY=1 ;;
        --web-only)     WEB_ONLY=1 ;;
        -h|--help)
            sed -n '2,28p' "$0"
            exit 0
            ;;
        *) echo "Unknown option: $arg" >&2; exit 2 ;;
    esac
done

# ---------- helpers ----------
log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31mxx\033[0m %s\n' "$*" >&2; exit 1; }

# ---------- 1. sanity ----------
[[ -d "$REPO_DIR/crates/web" ]] \
    || die "Could not find $REPO_DIR/crates/web — set REPO_DIR to the repo root."
command -v systemctl >/dev/null 2>&1 \
    || die "systemctl not found — this script is for systemd systems."

# ---------- 2. build (unprivileged) ----------
if [[ $INSTALL_ONLY -eq 0 ]]; then
    command -v cargo >/dev/null 2>&1 \
        || die "cargo not found in PATH. Install Rust first: https://rustup.rs"
    log "Building cyberdeck-web (release) as $(id -un)…"
    ( cd "$REPO_DIR" && cargo build --release -p cyberdeck-web )
    if [[ $WEB_ONLY -eq 0 ]]; then
        log "Building cyberdeck-tui (release)…"
        ( cd "$REPO_DIR" && cargo build --release -p cyberdeck-tui )
    fi
fi

[[ -x "$REPO_DIR/target/release/cyberdeck-web" ]] \
    || die "Expected $REPO_DIR/target/release/cyberdeck-web — build first."

if [[ $BUILD_ONLY -eq 1 ]]; then
    log "Build complete."
    [[ -x "$REPO_DIR/target/release/cyberdeck-web" ]] \
        && log "  cyberdeck-web: $REPO_DIR/target/release/cyberdeck-web"
    [[ -x "$REPO_DIR/target/release/cyberdeck-tui" ]] \
        && log "  cyberdeck-tui: $REPO_DIR/target/release/cyberdeck-tui"
    exit 0
fi

# ---------- 3. escalate to root for the rest ----------
if [[ $EUID -ne 0 ]]; then
    log "Re-executing with sudo for system install steps"
    exec sudo -E REPO_DIR="$REPO_DIR" \
               INSTALL_PREFIX="$INSTALL_PREFIX" \
               BIND_ADDR="$BIND_ADDR" \
               SERVICE_USER="$SERVICE_USER" \
               TOKEN_FILE="$TOKEN_FILE" \
               "$0" --install-only "$@"
fi

# === running as root from here on ===

# ---------- 4. install binaries ----------
log "Installing cyberdeck-web to ${INSTALL_PREFIX}/bin/cyberdeck-web"
install -m 0755 \
    "$REPO_DIR/target/release/cyberdeck-web" \
    "${INSTALL_PREFIX}/bin/cyberdeck-web"

if [[ $WEB_ONLY -eq 0 ]] && [[ -x "$REPO_DIR/target/release/cyberdeck-tui" ]]; then
    log "Installing cyberdeck-tui to ${INSTALL_PREFIX}/bin/cyberdeck-tui"
    install -m 0755 \
        "$REPO_DIR/target/release/cyberdeck-tui" \
        "${INSTALL_PREFIX}/bin/cyberdeck-tui"
fi

# ---------- 5. system user ----------
if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
    log "Creating system user '$SERVICE_USER'"
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
else
    log "System user '$SERVICE_USER' already exists"
fi

# ---------- 6. config dir + token ----------
# The service runs as $SERVICE_USER, so it must be able to traverse the
# config dir and read the token file. We make the dir group-traversable by
# the service user's primary group and the file group-readable. Without
# this, `cyberdeck-web --token-file /etc/cyberdeck/token` silently falls
# back to generating a fresh token every start (the URL the installer
# prints goes stale on the first restart).
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

# ---------- 7. sudoers fragment ----------
# cyberdeck-core uses `sudo -n <cmd>` to drive privileged actions. Allow the
# service user the narrow set of commands it actually needs, no password.
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

# ---------- 8. systemd unit ----------
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
ExecStart=${INSTALL_PREFIX}/bin/cyberdeck-web ${BIND_ADDR} --token-file ${TOKEN_FILE}
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

# ---------- 9. firewall ----------
if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
    log "Opening ${BIND_ADDR##*:}/tcp in ufw"
    ufw allow "${BIND_ADDR##*:}/tcp" comment "cyberdeck-web" >/dev/null || true
elif command -v nft >/dev/null 2>&1 && [[ -f /etc/nftables.conf ]]; then
    warn "nftables detected — not editing /etc/nftables.conf automatically."
    warn "If the host firewall is nftables, add a rule to allow ${BIND_ADDR##*:}/tcp."
else
    warn "No active firewall detected (no ufw, no nftables.conf). Skipping."
fi

# ---------- 10. enable + start ----------
log "Reloading systemd, enabling and starting $SERVICE_NAME"
systemctl daemon-reload
systemctl enable "${SERVICE_NAME}.service" >/dev/null
systemctl restart "${SERVICE_NAME}.service"

sleep 1
if ! systemctl is-active --quiet "${SERVICE_NAME}.service"; then
    die "Service failed to start. Last 20 log lines:
$(journalctl -u "${SERVICE_NAME}.service" -n 20 --no-pager 2>&1 || true)"
fi

# ---------- 11. summary ----------
HOST_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
PORT="${BIND_ADDR##*:}"

cat <<EOF

  ✓ cyberdeck-web is installed and running.

    LAN URL  : http://${HOST_IP:-<host>}:${PORT}/?token=${TOKEN}
    Local URL: http://127.0.0.1:${PORT}/?token=${TOKEN}

    Service  : systemctl {status,restart,stop} ${SERVICE_NAME}
    Logs     : journalctl -u ${SERVICE_NAME} -f
    Token    : stored at ${TOKEN_FILE} (delete the file to regenerate)

  Re-running this installer is safe; it will rebuild the binary in place
  and preserve the existing token.
EOF
