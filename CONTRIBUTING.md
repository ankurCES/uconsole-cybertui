# Contributing

Thanks for working on `uconsole-cybertui`. Before you do anything else,
read the rule below — it governs every commit on this repo.

## Test discipline (hard rule)

> **Tests in this project must always be targeted — by crate, by
> module, or by test name. Never run `cargo test` or
> `cargo test --workspace`.**

`cargo test` with no filter will build *every* test target in the
workspace, including the PTY-spinners in
`crates/tui/src/wm/{ansi,pty,broadcaster}.rs`. On the dev box we share
with a running editor and a few background services, those tests
sometimes exhaust the PTY pool and hang the suite indefinitely — a
hang is much worse than a slow build. The targeted commands below
sidestep the problem and still exercise everything that matters for a
given change.

## Quick reference — pick the narrowest command that covers your change

All commands below go through `scripts/safe-test` (via `make test`),
which mechanically refuses blanket form and auto-injects
`--test-threads=1` for `cyberdeck-tui`. The bare `cargo test …` form
is shown here for readability — `scripts/safe-test` is a transparent
wrapper.

| You changed | Run |
|---|---|
| Anything in `crates/tui/src/wm/` | `make test ARGS='-p cyberdeck-tui wm::'` |
| A specific screen module | `make test ARGS='-p cyberdeck-tui screens::<module>::tests'` |
| Anything under `crates/web/` | `make test ARGS='-p cyberdeck-web --test lan_smoke'` |
| Anything else | `make test ARGS='-p <crate> <module_or_test_path>'` |
| The full workspace, before opening a PR (CI parity) | `make test-ci` |

The last row is the only sanctioned blanket run, and it's gated to
pre-PR / CI. If you find yourself reaching for it during the inner
save loop, narrow the scope instead.

## Iteration loop

- `cargo check -p cyberdeck-tui` — fastest; catches most type errors.
- `cargo check -p cyberdeck-tui --all-targets` — also picks up the
  `#[cfg(test)]` modules, so an unused import inside a test fails the
  build.
- `cargo build -p cyberdeck-tui` — when you actually want to run the
  binary.
- `cargo clippy -p cyberdeck-tui --all-targets -- -D warnings` — before
  sending a PR.

## Stuck PTY test?

If a `wm::pty` or `wm::broadcaster` test hangs, the binary that's
stuck is almost always `/bin/cat` or `/bin/sh` from a previous run
that didn't reap cleanly:

```bash
pkill -f 'target/debug/deps/cyberdeck_tui-*'
```

then re-run with `--test-threads=1`.

## More

The full per-suite recipes (WM tests, web tests, manual smoke tests
for the Phase-3 + Phase-4 window manager) live in
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md). The repo-root file
you're reading now is the short version that links there.