# Phase 3 — Window manager

The window manager is the part of cyberdeck that turns a 13-screen
launcher into a real terminal multiplexer. You can split the screen
into up to 9 panes, swap any pane for a live `$SHELL`, and jump focus
with `Ctrl-W 1..9`.

## Layout

```
crates/tui/src/wm/
├── tree.rs       # Split tree data structure
├── window.rs     # Per-pane state (terminal + screen types)
├── manager.rs    # Orchestrator: input → action → state mutation
├── render.rs     # Tree-walk renderer
├── input.rs      # Translate raw key events into WM verbs
├── pty.rs        # PTY child + reader  (Phase 2)
├── ansi.rs       # Minimal ANSI parser  (Phase 2)
└── broadcaster.rs# Async broadcast of PTY bytes  (Phase 2)
```

## Split tree

`wm::tree::Tree` is a binary tree. Internal nodes are `Split { dir,
ratio, a, b }`; leaves are `Leaf { id, kind }`. The tree supports:

- `Tree::new()` — single-leaf tree, no splits.
- `Tree::split_focused(dir, kind) -> Result<SplitId, …>` — split the
  focused leaf in `dir` (vertical / horizontal), insert a new leaf, give
  it focus.
- `Tree::close_focused() -> Result<(), …>` — close the focused leaf,
  collapse its parent. If the closed leaf was the root, the tree
  collapses back to a single leaf.
- `Tree::focus(dir) -> Result<(), …>` — move focus in `dir`
  (left / right / up / down).
- `Tree::resize_focused(delta) -> Result<(), …>` — grow / shrink the
  focused pane by 5 % along its split axis.
- `Tree::jump(n) -> Result<(), …>` — focus pane `n` (1..=9).
- `Tree::renumber()` — renumber leaves contiguously 1..=N after a close.

`ratio` is stored as a `u8` percentage (5..=95) and clamped on every
mutation. The tree renderer (`wm::render.rs`) walks the tree and lays
out `Rect`s accordingly.

## Pane kinds

Each leaf has a `kind`:

- `LeafKind::Screen(ScreenId)` — the legacy single-screen view from
  Phase 1.
- `LeafKind::Terminal(TerminalId)` — a live `$SHELL` from Phase 2.

The WM does not care which kind a leaf is; it just routes input to the
focused leaf and renders whatever the leaf returns. The screen
implementation handles its own keys; the terminal implementation
forwards bytes to the PTY child.

## WM verbs (`Ctrl-W`)

Every `Ctrl-W` verb is a two-key sequence: press `Ctrl-W`, then the
second key. Unknown second keys are no-ops (the prefix is consumed
either way, so `Ctrl-W x` doesn't trigger the `x` key on the focused
leaf).

| Key        | Action                                                                  |
| ---------- | ----------------------------------------------------------------------- |
| `h` / `←`  | focus pane to the left                                                  |
| `j` / `↓`  | focus pane below                                                        |
| `k` / `↑`  | focus pane above                                                        |
| `l` / `→`  | focus pane to the right                                                 |
| `v`        | split focused pane vertically (new pane on the right)                   |
| `s`        | split focused pane horizontally (new pane below)                        |
| `n`        | swap focused pane for a live terminal (`$SHELL`)                        |
| `q` / `x`  | close the focused pane                                                  |
| `=` / `+`  | grow focused pane by 5%                                                 |
| `-`        | shrink focused pane by 5%                                               |
| `1`..`9`   | jump focus to pane N (pane number is shown in the title bar)            |

A hard cap of **9 panes** is enforced; `Ctrl-W v` / `Ctrl-W s` past the
cap toasts `pane limit reached (9)`. `Ctrl-W 1..9` with no pane at that
index toasts `no pane N`. Closing a pane renumbers the rest
contiguously.

## Focus algorithm

`Tree::focus(dir)` walks the tree to find the leaf closest to the
focused leaf in `dir`. The score is:

- `1000 * (axis_alignment)` — preferred-axis motion scores 1000.
- `+ 100 * (cross_axis_proximity)` — closer-in-cross-axis wins.
- `+   1 * (along_axis_proximity)` — closer-along-axis breaks ties.

The leaf with the highest score becomes the new focus. This handles
both trivial cases (one neighbour) and the corner case of three panes
where two are "left" and one is "right but far".

## Resize

`Tree::resize_focused(delta)` walks up to the parent of the focused
leaf, then to the parent's parent, and so on, applying `delta` to each
ancestor's `ratio` along the focused leaf's split axis. The 5..=95
clamp is applied per-ancestor so the tree can never produce a 0-pixel
pane.

`Ctrl-W =` grows the focused pane; `Ctrl-W -` shrinks it.

## Renderer

`wm::render.rs::render(frame, tree, focus, panes)` walks the tree and
lays out `Rect`s. Each `Rect` is filled by the leaf's `kind.render()`,
which is either a `Screen::render()` or a terminal-grid render. A
single-character border between siblings gives the user visual
feedback that the pane is split.

The title bar of each pane shows `[N] title`, where `N` is the
pane-number badge (1..=9). The focused pane's title bar is
reverse-video.

## Tests

Phase 3 has PTY-touching tests:

| Test | Hardening |
| --- | --- |
| `wm::window::tests::terminal_window_holds_grid_and_resizes` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::tree::tests::resize_clamps_to_valid_range` | (pure unit test — no PTY) |
| `wm::tree::tests::focus_neighbor_three_pane` | (pure unit test — no PTY) |

The two `tree` tests are pure unit tests on the data structure, so they
don't need Pattern A or B. The `window::terminal_window_holds_grid_and_resizes`
test is PTY-touching and follows Pattern A.

## What Phase 3 does NOT do

- It does not persist layout across restarts.
- It does not remember the cwd or shell of a closed terminal pane.
- It does not implement a per-pane scrollback buffer beyond what `vte`
  sees (terminal panes show the live output only).

These are all on the [Roadmap](./Roadmap.md).
