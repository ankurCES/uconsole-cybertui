# Phase 4 — Polish

Phase 4 is the in-progress polish phase. The headline item — pane-number
badges — is shipped. Per-pane scrollback, shell + cwd persistence, and
layout presets are next.

## Pane-number badges — done

Every pane title bar shows `[N] title` where `N` is the pane number
(1..=9). Pressing `Ctrl-W N` jumps focus to pane N. Closing a pane
renumbers the rest contiguously so the badges stay 1..=N with no gaps.

What this gets you:

- One-keystroke focus jumps, no arrow-key mazes.
- Stable pane identity across resize / repaint.
- A clear visual signal when you've hit the 9-pane cap (the badges
  stop appearing after 9).

## Per-pane scrollback — in progress

The terminal panes currently show only the live output. There's no
scrollback buffer beyond what `vte` parses on each frame.

Target behaviour:

- 10,000 lines of scrollback per pane.
- `Shift-PageUp` / `Shift-PageDown` scroll the focused pane.
- Mouse-wheel scroll works in the focused pane.
- The scrollback is ring-buffered; old lines are dropped when the
  buffer fills.

Implementation will live in `wm/window.rs` (the per-pane state) and
`wm/render.rs` (the renderer will take a scroll offset into account).

## Shell + cwd persistence — planned

When you close a terminal pane and re-open one with `Ctrl-W n`, the new
shell starts in `$HOME`. The plan is to remember:

- The `$SHELL` (default `$SHELL`).
- The cwd (last-seen pwd, updated by `OSC 7` escape sequences).
- The history file (`HISTFILE` is preserved by the shell itself; we
  just don't kill the process prematurely).

These are stored in `~/.config/cyberdeck/sessions.json` and restored on
TUI startup.

## Layout presets — planned

A few preset layouts for the most common splits:

- **Single** — one pane, full screen.
- **Side-by-side** — two vertical panes (50 / 50).
- **Stacked** — two horizontal panes (50 / 50).
- **Triple** — one terminal pane on the left, two status screens on the
  right (vertically split).
- **Quad** — four equal panes (2 × 2 grid).

These are reachable from the command palette:

```
: layout single
: layout side-by-side
: layout stacked
: layout triple
: layout quad
```

The layout is applied by walking the tree and replacing it with the
preset tree, then renumbering.

## Theme + colour — planned

Right now the colour palette is hard-coded in `wm/render.rs`. The plan
is to externalise it into `~/.config/cyberdeck/theme.toml` so the user
can tune it without rebuilding. The default theme is the cyberdeck
one (dark, amber-on-charcoal, with green for success and red for
errors).

## Status-bar cells — done

The status bar in the bottom-right shows a battery indicator (with a
charging glyph), a Wi-Fi glyph (with an off-air fallback), and a
volume glyph (with a muted state). These are populated from the
`Arc<Live>` data and re-rendered every frame.

## Tests

Phase 4 polish items don't add new PTY-touching tests; they add
unit tests on the new data structures (scrollback buffer, layout
presets, theme loader). When a polish item is PTY-touching, it
follows one of the patterns in [Hardening](./Hardening.md).
