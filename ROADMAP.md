# cyberdeck-tui roadmap

The references in `app.rs`, `app/action.rs`, `app/screen.rs`, `theme.rs`, and
`wm/{mod,ansi,pty,broadcaster}.rs` to "see ROADMAP.md" point here. Anything
marked **done** in this file is shipped; anything marked **next** is the next
chunk of work.

## Phase 1 — single-pane TUI (done)

13 screens, sidebar, status bar, modals (help / command palette / confirm /
input), command palette, refreshers, web bridge. Build is clean.

## Phase 2 — PTY / ANSI / broadcaster (done, not wired)

`wm/ansi.rs`, `wm/pty.rs`, `wm/broadcaster.rs` are implemented and tested but
nothing in the rest of the crate imports them yet. They exist so the
window-manager work in Phase 3 can stand on them without re-deriving the PTY
glue.

## Phase 3 — window manager (in progress)

The TUI renders a single content rectangle today. The WM replaces this with a
binary split tree of panes, each of which can host a built-in screen (existing
`Screen` impl) or a live PTY (Phase-2 infra).

Milestones:

- [ ] **`wm/tree.rs`** — `Node` enum (`Split { dir, ratio, a, b }` / `Leaf { id }`),
      `compute_layout(&Node, Rect) -> Vec<(PaneId, Rect)>`, mutators
      (`split`, `close`, `resize`, `rotate`, `find_leaf`).
- [ ] **`wm/window.rs`** — `Window` struct + `WindowKind` (`Builtin(ScreenId)` /
      `Terminal`), owns its `Grid` + `AnsiParser` + `PaneOutput` + `PtyWriter`
      when terminal.
- [ ] **`Focus::Pane(PaneId)`** — replace the binary `Focus` with something
      addressable per pane.
- [ ] **`Ctrl-W` keymap** — `h/j/k/l` move focus, `v`/`s` split, `n` new term,
      `q`/`x` close/kill, `=`/`+`/`-` resize, `r` rotate.
- [ ] **Render path** — `draw()` walks the tree, renders each leaf into its
      computed rect, marks the focused leaf with the focus border style.
- [ ] **Terminal pane** — `WindowKind::Terminal` spawns `$SHELL` via the
      existing PTY infra, subscribes to its broadcaster, paints the grid.
- [ ] **Tests** — tree layout, focus traversal, close/rotate invariants,
      terminal pane echo roundtrip.

## Phase 4 — polish

- Pane number badges in titles (`1`/`2`/…) so `Ctrl-W N` jump is discoverable.
- Last-used shell + cwd persistence (config file or env).
- Optional: per-pane scrollback for terminals.
- Optional: pane presets (`:layout 2v`, `:layout 1+2`).