# Contributing

## Iteration loop

We **do not** run `cargo test` as part of the inner save loop on this
repo. The unit tests under `crates/tui/src/wm/{ansi,pty,broadcaster,window}.rs`
spin up real PTYs and shells; on the dev box we share with the
running editor and a few other services, the scheduler occasionally
hitches and the tests hang.

Use these commands while you're iterating:

- `cargo check -p cyberdeck-tui` — fastest, catches most type errors.
- `cargo check -p cyberdeck-tui --all-targets` — also picks up the
  `#[cfg(test)]` modules so an unused import inside a test will fail
  the build.
- `cargo build -p cyberdeck-tui` — when you want to actually run the
  binary.
- `cargo clippy -p cyberdeck-tui --all-targets -- -D warnings` —
  before sending a PR.

## Running the WM tests deliberately

When you do want to run them — e.g. after editing `wm/tree.rs` or
`wm/ansi.rs` — use:

```bash
cargo test -p cyberdeck-tui wm:: -- --test-threads=1
```

`--test-threads=1` is the important bit. The PTY tests in `pty.rs` and
`broadcaster.rs` both spawn a child process; running them in parallel
sometimes exhausts the available PTYs and triggers a hang inside
`portable-pty`. Sequential is slower but reliable.

If a test still hangs, the binary that's stuck is almost always
`/bin/cat` or `/bin/sh` from a previous run that didn't reap cleanly:

```bash
pkill -f 'target/debug/deps/cyberdeck_tui-*'
```

then re-run.

## Running the full test suite (CI parity)

```bash
cargo test --workspace -- --test-threads=1
```

Run this before opening a PR. Expect 20-40 seconds on a quiet box.

## Manual smoke test for Phase 3 (window manager)

After touching any file under `crates/tui/src/wm/`, run the binary
and confirm:

1. The TUI starts on the System screen.
2. `Ctrl-W v` opens a vertical split. Both panes show the System
   screen.
3. `Ctrl-W h` / `Ctrl-W l` move the focus border between the two
   panes.
4. `Ctrl-W n` turns the focused pane into a terminal. The shell's
   prompt appears within ~100 ms.
5. Type `echo hello` and press Enter. The text `hello` appears in
   the pane.
6. `Ctrl-W h` / `Ctrl-W l` move focus back to the other pane; the
   shell is still alive in the other pane.
7. `Ctrl-W q` closes the focused pane. The tree collapses to a
   single pane.
8. `q` quits the TUI cleanly (the shell child is reaped, the
   terminal returns to its normal mode).

If any of these fail, the regression is almost always in
`wm/render.rs` (paint order) or `wm/manager.rs` (tree bookkeeping).

## Manual smoke test for Phase 4 (pane number badges + Ctrl-W N jump)

After touching `wm/manager.rs`, `wm/render.rs`, or the `Ctrl-W`
arms in `main.rs`, run the binary and confirm. Note: built-in pane
titles are rendered by each `Screen::render` and do **not** yet show
the badge — only terminal pane titles do (see follow-up in
ROADMAP.md).

1. Open TUI. Single built-in pane; the title reads ` System ` (no
   badge yet for built-ins). Open a terminal pane with `Ctrl-W n` —
   the new terminal title reads ` [1] terminal ` (terminals do show
   the badge).
2. `Ctrl-W v`. Two panes side by side. New pane is focused.
3. `Ctrl-W h`. Focus border moves left; titles unchanged.
4. `Ctrl-W 1`. Focus jumps to pane 1.
5. Open four more terminal panes (each `Ctrl-W n` on the focused
   pane, then `Ctrl-W v` or `Ctrl-W s` to split as needed). Six panes,
   terminal titles ` [1] terminal `..` [6] terminal `.
6. Open three more (`Ctrl-W n` then split). Nine panes total.
7. `Ctrl-W v` on pane 9. No new pane appears; toast at the bottom
   reads `pane limit reached (9)`.
8. `Ctrl-W 1`. Focus jumps to pane 1.
9. `Ctrl-W q` on pane 9. Eight panes remain; terminal titles
   renumber to ` [1] terminal `..` [8] terminal `.
10. `Ctrl-W 9`. Toast `no pane 9` (only 8 exist).
11. `Ctrl-W 8`. Focus jumps to pane 8.

If any step fails, the regression is almost always in
`Manager::focus_pane_index` / `focus_pane` (Task 1), the `Result`
shape of `split_focused` (Task 2), or the index threading in
`wm::render::render` (Task 3).