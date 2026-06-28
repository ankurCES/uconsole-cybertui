# Roadmap

Cyberdeck is built in numbered phases. Each phase is shipped before the
next one starts; partial phases are kept on this page so the next steps
are visible.

## Phase 1 — TUI ✅ shipped

13 screens, sidebar, command palette, toast log, live header.

See [Phase 1 — TUI](./Phase-1-TUI.md).

## Phase 2 — PTY / ANSI ✅ shipped

PTY child + reader, minimal ANSI parser, async broadcast.

See [Phase 2 — PTY / ANSI](./Phase-2-PTY-ANSI.md).

## Phase 3 — Window manager ✅ shipped

Split tree, focus jumps, terminal panes, pane-number badges.

See [Phase 3 — WM](./Phase-3-WM.md).

## Phase 4 — Polish ⏳ in progress

- ✅ Pane-number badges (`[N] title` in each pane).
- ⏳ Per-pane scrollback (10,000 lines, `Shift-PageUp`/`Down`).
- ⏳ Shell + cwd persistence (`~/.config/cyberdeck/sessions.json`).
- ⏳ Layout presets (single / side-by-side / stacked / triple / quad).
- ⏳ External theme (`~/.config/cyberdeck/theme.toml`).

See [Phase 4 — Polish](./Phase-4-Polish.md).

## Phase 5 — Modal upgrade ✅ shipped

Five modal kinds: Secret, Choice, Wizard, Progress, AuthFailure.

See [Phase 5 — Modals](./Phase-5-Modals.md).

## Phase 6 — Hardening ✅ shipped (continuous)

Every PTY-touching test follows the kill-switch + bounded
`tokio::time::timeout(2s)` pattern. New PTY tests must follow Pattern A
or Pattern B.

See [Hardening](./Hardening.md).

## Phase 7 — Wiki ⏳ in progress

The wiki lives under `docs/wiki/` and mirrors the GitHub wiki
structure. Pages so far: Home, Architecture, Phase 1, Phase 2, Phase 3,
Phase 4, Phase 5, Hardening, Keymaps, Hardware/Setup, Photos, Roadmap.
Fleshing out individual screens (per-screen design notes) is next.

## Future (not yet scoped)

These are listed in the order they're most likely to land, but no
phase is reserved for them yet. Open an issue to vote on which should
move up.

- **Mouse support.** Wheel scroll on the focused pane, click to focus
  a pane, click+drag on a split to resize.
- **Bracket paste.** Pasted multi-line input is sent as a single
  chunk to avoid half-pasted commands in the terminal panes.
- **Clipboard sync.** Copy on the TUI → paste on the desktop (via
  OSC 52 or a small HTTP endpoint).
- **Plugin screens.** A small WASM host so users can ship their own
  screens without rebuilding cyberdeck.
- **Mobile / ssh client.** A pure-WebAssembly build of the TUI that
  talks to a remote cyberdeck-web instance over WebSocket.

## Out of scope

- **A graphical (non-TUI) UI.** Cyberdeck is a TUI. The web UI is a
  JSON + WebSocket bridge, not a graphical desktop replacement.
- **Replacing `nmcli` / `pactl` / `systemctl`.** Cyberdeck shells out
  to the canonical tools; it doesn't reimplement them.
- **Replacing the kernel's network stack.** Cyberdeck configures
  NetworkManager; it doesn't manage interfaces directly.

## How to propose a phase

Open an issue titled `phase proposal: <name>` with:

- The user story (who, what, why).
- The screens / modules it would touch.
- A sketch of the data flow.
- A list of PTY-touching tests it would add (if any), each marked
  with **Pattern A** or **Pattern B** per [Hardening](./Hardening.md).

The maintainers will triage and either schedule it for a numbered
phase or fold it into an existing one.
