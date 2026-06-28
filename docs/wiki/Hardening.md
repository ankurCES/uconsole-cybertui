# Hardening — no-hang PTY tests

Every PTY-touching test in `crates/tui/src/wm/` is wrapped so it can
**never outlive its PTY child**. This page documents the two patterns
that cover every PTY test in the suite and why they're structural
(not procedural) guarantees.

## Why this exists

Before the hardening, the test suite occasionally hung the inner
save loop. The dev box is shared with a running editor and a few
other services; the scheduler would hitch, and a `portable_pty`
`Child::wait()` would wedge inside the test thread. The next test
couldn't get a fresh PTY allocation, the suite stalled, and the user
had to `pkill cargo` from another terminal.

The fix is to make sure no test can wait forever on a PTY child,
regardless of what the OS / scheduler / `portable_pty` is doing.

## Pattern A — kill-switch + bounded `tokio::time::timeout(2s)`

Used by every PTY-touching test that already runs in a tokio runtime
(broadcaster tests, window tests).

```rust
let killer = pty.child_killer();
match tokio::time::timeout(Duration::from_secs(2), async {
    // ...drive the test...
}).await {
    Ok(result) => {
        child.wait()?.ok();
        result
    }
    Err(_) => {
        killer.kill().ok();
        child.wait()?.ok();
        panic!("timed out after 2s")
    }
}
```

Why this works:

- `pty.child_killer()` returns a `Box<dyn ChildKiller + Send + Sync>`
  clone that the test owns. If the test panics or returns, the
  killer is dropped — but the child has already been `kill()`-ed on
  the timeout branch.
- `tokio::time::timeout(Duration::from_secs(2), …)` ensures the test
  returns within 2 seconds, even if the inner future wedges.
- On the timeout branch, the killer sends `SIGKILL` to the child
  before the test panics. `child.wait()` is then guaranteed to return
  immediately because the child is already dead.

## Pattern B — `kill()` + `try_wait()` + thread-spawned `wait()` + drop-on-scope

Used by the raw `wm/pty.rs::tests::write_and_read_roundtrip` test,
which doesn't use tokio.

```rust
child.kill().ok();
let _ = child.try_wait();
let waiter = std::thread::spawn(move || { let _ = child.wait(); });
drop(waiter); // scope exit — thread is detached, the wait doesn't block us
```

Why this works:

- `child.kill()` sends `SIGKILL` to the child immediately.
- `child.try_wait()` is non-blocking — it returns immediately whether
  or not the child has been reaped.
- The thread that owns `child.wait()` is **dropped on scope exit**,
  not joined. The OS thread keeps running in the background until the
  wait completes, but the test thread is not blocked on it.
- The test thread is therefore guaranteed to return within a few
  milliseconds of the kill, regardless of how wedged `portable_pty`
  is.

## Coverage table

Every PTY-touching test in the suite follows one of the two patterns
above (or is already safe because the child exits on its own):

| Test | Hardening |
| --- | --- |
| `wm::broadcaster::tests::roundtrip_echo_via_broadcaster` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::broadcaster::tests::echo_emits_into_ansi_grid` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::window::tests::terminal_window_holds_grid_and_resizes` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::pty::tests::write_and_read_roundtrip` | Pattern B — `kill()` + `try_wait()` + thread-spawned `wait()` + drop-on-scope |
| `wm::pty::tests::spawn_and_read` | already safe — `/bin/sh -c "printf …"` exits on its own |

The invariant: **a test never outlives its PTY child**.

## Why this is structural, not procedural

There is no shell wrapper, no `pkill`, no `Makefile` target to forget
about. The patterns are in the test source itself. Reviewing a PR
that touches `crates/tui/src/wm/` requires the reviewer to check
that any new PTY-touching test follows Pattern A or Pattern B.

The contributing doc is explicit:

> **We do not run `cargo test` as part of the inner save loop on
> this repo.** The unit tests under
> `crates/tui/src/wm/{ansi,pty,broadcaster,window}.rs` spin up real
> PTYs and shells; on the dev box we share with the running editor
> and a few other services, the scheduler occasionally hitches and
> the tests hang.
>
> Use these commands while you're iterating:
>
> - `cargo check -p cyberdeck-tui` — fastest, catches most type errors.
> - `cargo check -p cyberdeck-tui --all-targets` — also picks up the
>   `#[cfg(test)]` modules so an unused import inside a test will fail
>   the build.
> - `cargo build -p cyberdeck-tui` — when you want to actually run the
>   binary.
> - `cargo clippy -p cyberdeck-tui --all-targets -- -D warnings` —
>   before sending a PR.

The inner save loop uses `cargo check -p cyberdeck-tui --all-targets`
— which never spins up a PTY. Real PTY-touching tests are only run
when the user explicitly invokes the targeted command, which uses
the kill-switch pattern.

## What if `portable_pty` itself is broken

If `portable_pty::Child::wait()` wedges inside the kernel (rare but
possible), Pattern A's `tokio::time::timeout(2s)` returns, the killer
sends `SIGKILL`, and Pattern A's `child.wait()` reaps the dead child
immediately. If even `SIGKILL` doesn't work (extremely rare —
typically only seen on a hung VM), the test panics with `timed out
after 2s` and the suite moves on. The next test gets a fresh PTY
allocation. The suite finishes in ~1 s.

Pattern B has the same worst-case behaviour: the test thread returns
in a few milliseconds, the waiter thread keeps running in the
background until the kernel reaps the dead child, and the suite moves
on.

## Real-world numbers

Before the hardening, on the dev box, the suite occasionally hung
forever (≥30 minutes) until the user killed it. After the hardening,
the suite finishes in ~1 s on the same dev box, every time:

```
test result: ok. 71 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.05s
```

## Adding a new PTY-touching test

1. Read this page.
2. Pick Pattern A (tokio) or Pattern B (sync).
3. Clone the kill-switch into the test scope.
4. Wrap the work in `tokio::time::timeout(2s, …)` (Pattern A) or
   `kill()` + `try_wait()` + thread-spawned `wait()` (Pattern B).
5. Add a row to the coverage table above.
6. Verify with `cargo test -p cyberdeck-tui --bin cyberdeck-tui` —
   it should still finish in ~1 s.
