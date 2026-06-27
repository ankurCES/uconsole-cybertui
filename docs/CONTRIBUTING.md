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