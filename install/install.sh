#!/usr/bin/env bash
# install.sh — curl|bash entry point for cyberdeck.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh | bash
#   curl -fsSL …/install/install.sh | bash -s -- --tui
#   curl -fsSL …/install/install.sh | bash -s -- --web
#   curl -fsSL …/install/install.sh | bash -s -- --full
#   curl -fsSL …/install/install.sh | bash -s -- --build
#   curl -fsSL …/install/install.sh | bash -s -- --help
#
# What this entry point does:
#   1. Picks a fresh scratch dir under $TMPDIR (or /tmp).
#   2. Shallow-clones the repo at REF (default: main).
#   3. Execs the repo's own ./install.sh with the args you passed through.
#
# Why two scripts:
#   - This one is the *public* surface: short, readable, audited easily.
#   - The real installer lives in the repo (./install.sh) and is what gets
#     committed and reviewed. Anything that needs sudo lives there.

set -euo pipefail

REPO="${CYBERDECK_REPO:-https://github.com/ankurCES/uconsole-cybertui.git}"
REF="${CYBERDECK_REF:-main}"

# Pretty, terse logging.
log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31mxx\033[0m %s\n' "$*" >&2; exit 1; }

# --- self-help (before we do anything destructive) ---
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "$#" -eq 0 ]]; then
    cat <<EOF
cyberdeck installer (curl|bash entry point)

USAGE
  curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \\
    | bash -s -- [PRESET] [OPTIONS]

PRESETS
  --tui      Build + install the cyberdeck-tui binary only.
             No sudo, no service, no firewall changes.
             Useful on shared dev machines or when you just want to try it.
  --web      Build + install cyberdeck-web as a systemd service.
             Creates the cyberdeck system user, opens the firewall port,
             generates a bearer token. Skips the TUI binary.
  --full     Both TUI and web (default if no preset is given).
  --build    Build both binaries into ./target/release and exit.
             No install, no sudo, no service.
  -h/--help  Show this message.

OPTIONS (passed through to the repo's install.sh)
  -y/--yes            Non-interactive; assume yes for any prompt.
  --prefix <dir>      Install prefix for binaries (default: /usr/local).
  --bind <addr>       Web server bind address (default: 0.0.0.0:7878).
  --service-user <u>  System user for the web service (default: cyberdeck).
  --uninstall         Remove installed binaries, user, service, token.

ENVIRONMENT
  CYBERDECK_REPO   Git URL to clone from (default: $REPO).
  CYBERDECK_REF    Git ref to pin to (default: $REF).

EXAMPLES
  # Try the TUI without touching system state:
  curl -fsSL …/install/install.sh | bash -s -- --tui

  # Production web service on a fresh uconsole:
  curl -fsSL …/install/install.sh | bash -s -- --web

  # Everything:
  curl -fsSL …/install/install.sh | bash -s -- --full

  # Pin to a specific tag:
  curl -fsSL …/install/install.sh | CYBERDECK_REF=v0.1.0 bash -s -- --tui
EOF
    exit 0
fi

# --- preflight ---
command -v git >/dev/null 2>&1 \
    || die "git not found in PATH. Install git first."

# `tui` and `build` presets need cargo but never sudo. `web` and `full`
# need sudo for the install steps; we'll re-exec with sudo only when
# the chosen preset actually needs it.
NEED_SUDO=0
for arg in "$@"; do
    case "$arg" in
        --web|--full|--uninstall) NEED_SUDO=1 ;;
    esac
done

# --- workspace ---
SCRATCH="$(mktemp -d -t cyberdeck-install.XXXXXXXX)"
trap 'rm -rf "$SCRATCH"' EXIT

log "Cloning $REPO @ $REF into $SCRATCH"
git clone --depth 1 --branch "$REF" --quiet "$REPO" "$SCRATCH/repo" \
    || die "Clone failed. Check CYBERDECK_REPO / CYBERDECK_REF and network."

# `web` / `full` need sudo for the install. Re-exec through sudo so the
# heavy script doesn't have to split itself into pre-/post-sudo phases
# just to support the `tui`/`build` presets that don't.
if [[ $NEED_SUDO -eq 1 ]] && [[ $EUID -ne 0 ]]; then
    log "Preset requires sudo; re-executing under sudo"
    exec sudo -E bash "$SCRATCH/repo/install.sh" "$@"
fi

log "Running installer"
exec bash "$SCRATCH/repo/install.sh" "$@"
