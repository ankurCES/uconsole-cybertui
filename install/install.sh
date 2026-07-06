#!/usr/bin/env bash
# install.sh — curl|bash entry point for cyberdeck, with cyberpunk-style
# ASCII art + brief boot animation. The art runs locally; the install
# behaviour is unchanged.
#
# Usage:
#   curl -fsSL …/install/install.sh | bash                       # default --full
#   curl -fsSL …/install/install.sh | bash -s -- --tui
#   curl -fsSL …/install/install.sh | bash -s -- --web
#   curl -fsSL …/install/install.sh | bash -s -- --full
#   curl -fsSL …/install/install.sh | bash -s -- --radar
#   curl -fsSL …/install/install.sh | bash -s -- --build
#   curl -fsSL …/install/install.sh | bash -s -- --help
#
# Honours the usual suspects:
#   CYBERDECK_NO_BANNER=1   # skip the art (CI / non-interactive logs)
#   CYBERDECK_NO_ANIM=1     # skip the boot animation (keep the banner)
#   NO_COLOR=1              # disable ANSI entirely
#
# Why two scripts:
#   - This one is the *public* surface: short, readable, audited easily,
#     and it puts on a bit of theatre first.
#   - The real installer lives in the repo (./install.sh) and is what gets
#     committed and reviewed. Anything that needs sudo lives there.

set -euo pipefail

REPO="${CYBERDECK_REPO:-https://github.com/ankurCES/uconsole-cybertui.git}"
REF="${CYBERDECK_REF:-main}"

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31mxx\033[0m %s\n' "$*" >&2; exit 1; }

# ============================================================================
#  Cyberdeck banner + boot animation
# ----------------------------------------------------------------------------
#  Rendered in cyan + magenta with a horizontal colour gradient.
#  Animation = three short phases:
#    1. Banner fades in row by row (~40 ms / row, only on the figlet rows).
#    2. A magenta scan line sweeps top → bottom across the banner region.
#    3. A "BOOT ::" status block cycles through six system checks with a
#       unicode braille spinner in front of each label.
#  Everything is gated by:
#    - tty check (piped curl has no tty → fall back to plain banner)
#    - NO_COLOR (any value → skip all ANSI)
#    - CYBERDECK_NO_BANNER / CYBERDECK_NO_ANIM overrides.
# ============================================================================

can_ansi() {
    [[ -z "${NO_COLOR:-}" ]] && [[ -t 1 ]] && command -v tput >/dev/null 2>&1
}

# Linear-interp RGB gradient between cyan (#08FFD2) and magenta (#FF2EC4).
banner_glyph() {
    local pct="$1"
    local r g b
    r=$(awk  -v p="$pct" 'BEGIN{ printf("%d", 8   + (255-8  )*p/100 ) }')
    g=$(awk  -v p="$pct" 'BEGIN{ printf("%d", 255 + (46 -255)*p/100 ) }')
    b=$(awk  -v p="$pct" 'BEGIN{ printf("%d", 210 + (196-210)*p/100) }')
    printf '\033[38;2;%d;%d;%dm' "$r" "$g" "$b"
}

# --- figlet-style banner ---------------------------------------------------
# Small slant-figlet of "CYBERDECK". 7 lines tall, plus 2 subtitle lines.
# Each line is colour-graded left → right (cyan → magenta) in the animated
# path. The static path prints the same lines in plain cyan.
#
# Note on quoting: every `\'` in the figlet is a literal backslash-apostrophe
# so the pipe `|` in the rendered output isn't parsed by bash.
banner_lines=(
"  ____      _              ____            _            _      "
" / ___| ___| |__   ___ _ _|  _ \\ ___  _ __| | ___  _ __| | ___ "
"| |    / _ \\'_ \\ / _ \\'__| |_) / _ \\| \'__| |/ _ \\| \'__| |/ _ \\"
"| |___|  __/ |_) |  __/ |  |  __/ (_) | |  | | (_) | |  | |  __/"
" \\____|\\___|_.__/ \\___|_|  |_|   \\___/|_|  |_|\\___/|_|  |_|\\___|"
"                                                                "
$'    \xe2\x96\xb8 one terminal. one mesh.   tui \xc2\xb7 web \xc2\xb7 wifi-radar         '
$'       v0.1.0 \xc2\xb7 curl | bash                                   '
)

banner_render_plain() {
    printf '\033[1;36m'
    for line in "${banner_lines[@]}"; do
        printf '  %s\n' "$line"
    done
    printf '\033[0m\n'
}

banner_render_animated() {
    # Phase 1 — fade the banner in row by row with a left → right gradient
    # anchored across the *entire* line width (so the colour stays coherent
    # even when the line is short).
    local total="${#banner_lines[@]}"
    for i in "${!banner_lines[@]}"; do
        local line="${banner_lines[$i]}"
        local width=${#line}
        printf '  '
        local j=0
        while (( j < width )); do
            local ch="${line:$j:1}"
            local pct=$(( j * 100 / (width > 1 ? width : 1) ))
            banner_glyph "$pct"
            printf '%s' "$ch"
            j=$(( j + 1 ))
        done
        printf '\033[0m\n'
        # Only delay on rows that visibly change (skip empty / subtitle rows).
        if (( i < 5 )); then
            sleep 0.04 2>/dev/null || true
        fi
    done

    # Phase 2 — sweep a magenta scan line top → bottom.
    printf '\033[38;2;255;46;196m'
    for ((row=0; row<total+2; row++)); do
        printf '\033[s\033[%d;0H\xe2\x96\x8c\033[u' "$row"
        sleep 0.025 2>/dev/null || true
    done
    printf '\033[0m\n'
}

# --- boot checklist -------------------------------------------------------
boot_animation() {
    local checks=(
        "tty mux .......... ONLINE"
        "ansi driver ...... 24-BIT TRUE"
        "io channels ...... sudo . git . curl"
        "power bus ........ mains / battery"
        "mesh link ........ nmcli . bluetooth . pactl"
        "ready ............ CYBERDECK"
    )
    # 16-frame braille spinner — store as an array (one UTF-8 char per slot)
    # so we can index by codepoint, not by byte.
    local frames=(
        $'\xe2\xa0\x80' $'\xe2\xa0\x81' $'\xe2\xa0\x82' $'\xe2\xa0\x83'
        $'\xe2\xa0\x84' $'\xe2\xa0\x85' $'\xe2\xa0\x86' $'\xe2\xa0\x87'
        $'\xe2\xa0\x88' $'\xe2\xa0\x89' $'\xe2\xa0\x8b' $'\xe2\xa0\x8c'
        $'\xe2\xa0\x8d' $'\xe2\xa0\x8e' $'\xe2\xa0\x8f' $'\xe2\xa0\x88'
    )
    local i=0
    printf '\n  \033[1;35mBOOT ::\033[0m  '
    # Emit one check per line so `while read` splits cleanly
    # (default IFS would join array elements with spaces).
    printf '%s\n' "${checks[@]}" | while read -r check; do
        local glyph="${frames[$(( i % ${#frames[@]} ))]}"
        printf '\033[1;36m%s\033[0m %s\n' "$glyph" "$check"
        sleep 0.18 2>/dev/null || true
        i=$(( i + 1 ))
    done
    printf '\n'
}

# --- public entry: show_banner ---------------------------------------------
show_banner() {
    [[ "${CYBERDECK_NO_BANNER:-}" == "1" ]] && return 0
    printf '\n'
    if can_ansi && [[ "${CYBERDECK_NO_ANIM:-0}" != "1" ]] \
            && [[ -t 1 && -t 0 ]] && command -v sleep >/dev/null 2>&1; then
        banner_render_animated
        boot_animation
    elif can_ansi; then
        banner_render_plain
    else
        for line in "${banner_lines[@]}"; do
            printf '  %s\n' "$line"
        done
        printf '\n'
    fi
}

# --- self-help (before we do anything destructive) ------------------------
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "$#" -eq 0 ]]; then
    show_banner
    cat <<'EOF'
cyberdeck installer (curl|bash entry point)

USAGE
  curl -fsSL https://raw.githubusercontent.com/ankurCES/uconsole-cybertui/main/install/install.sh \
    | bash -s -- [PRESET] [OPTIONS]

PRESETS
  --tui      Build + install the cyberdeck-tui binary only.
             No sudo, no service, no firewall changes.
             Useful on shared dev machines or when you just want to try it.
  --web      Build + install cyberdeck-web as a systemd service.
             Creates the cyberdeck system user, opens the firewall port,
             generates a bearer token. Skips the TUI binary.
  --radar    Build + install wifi-radar as a systemd service.
             Passive 802.11 monitor with synthetic 8-MAC fallback
             (works without a monitor-mode adapter). Binds on
             0.0.0.0:8743. Skips the TUI binary.
  --full     Both TUI and web (default if no preset is given).
  --build    Build both binaries into ./target/release and exit.
             No install, no sudo, no service.
  -h/--help  Show this message.

OPTIONS (passed through to the repo's install.sh)
  -y/--yes            Non-interactive; assume yes for any prompt.
  --prefix <dir>      Install prefix for binaries (default: /usr/local).
  --bind <addr>       Web server bind address (default: 0.0.0.0:7878).
  --radar-bind <addr> Wi-Fi radar bind address (default: 0.0.0.0:8743).
  --radar-pcap <path> Use a pcap file instead of dev mode.
  --service-user <u>  System user for the web service (default: cyberdeck).
  --uninstall         Remove installed binaries, user, service, token.

ENVIRONMENT
  CYBERDECK_REPO       Git URL to clone from (default: $REPO).
  CYBERDECK_REF        Git ref to pin to (default: $REF).
  CYBERDECK_NO_BANNER  Skip the ASCII banner.
  CYBERDECK_NO_ANIM    Skip the boot animation, keep the static banner.
  NO_COLOR             Disable ANSI colour entirely.

EXAMPLES
  # Try the TUI without touching system state:
  curl -fsSL …/install/install.sh | bash -s -- --tui

  # Production web service on a fresh uconsole:
  curl -fsSL …/install/install.sh | bash -s -- --web

  # Everything:
  curl -fsSL …/install/install.sh | bash -s -- --full

  # Wi-Fi radar as a systemd service (passive monitor, synth fallback):
  curl -fsSL …/install/install.sh | bash -s -- --radar

  # Pin to a specific tag:
  curl -fsSL …/install/install.sh | CYBERDECK_REF=v0.1.0 bash -s -- --tui

  # CI / non-interactive logs — skip the animation:
  curl -fsSL …/install/install.sh | CYBERDECK_NO_ANIM=1 bash -s -- --tui
EOF
    exit 0
fi

# --- preflight -------------------------------------------------------------
command -v git >/dev/null 2>&1 \
    || die "git not found in PATH. Install git first."

# `tui` and `build` presets need cargo but never sudo. `web` and `full`
# need sudo for the install steps; we'll re-exec with sudo only when
# the chosen preset actually needs it.
NEED_SUDO=0
for arg in "$@"; do
    case "$arg" in
        --web|--full|--radar|--uninstall) NEED_SUDO=1 ;;
    esac
done

# --- banner + workspace ----------------------------------------------------
show_banner

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
