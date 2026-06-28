# Phase 2 — PTY / ANSI

Before cyberdeck had a window manager it had a PTY/ANSI stack. Phase 2
is the foundation that Phase 3 (WM) and Phase 5 (modals) build on.

## Modules

```
crates/tui/src/wm/
├── pty.rs          # PTY child + reader
├── ansi.rs         # Minimal ANSI parser
├── broadcaster.rs  # Async broadcast of PTY bytes to subscribers
├── input.rs        # Translate raw key events into WM verbs  (Phase 3)
└── tree.rs         # Split tree data structure                (Phase 3)
```

`pty.rs` is the only one that touches the OS. Everything else is pure
Rust on top of `tokio::sync::broadcast`.

## pty.rs — opening a PTY

```rust
pub struct Pty {
    pub reader: Box<dyn Read + Send + Sync>,
    pub writer: Box<dyn Write + Send + Sync>,
    pub parser: vte::Parser,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}
```

The `Pty` owns:
- the **master** end of the PTY (read + write sides, used by the UI),
- the **kill-switch** (a portable-pty abstraction that sends `SIGKILL`
  to the child without owning the `Child` itself),
- a **vte parser** for ANSI escape sequence parsing.

A separate `Child` handle lives in the broadcaster task; the UI never
touches it directly.

`Pty::spawn(command, args, env, rows, cols)` returns `(Pty, Child)`.
The `Child` is moved into the broadcaster task; the `Pty` is moved
into the WM window state.

## ansi.rs — minimal ANSI parser

We don't need a full xterm emulator. We need:

- Cursor positioning (CUP / HVP).
- Erase in line / screen.
- SGR (colour + bold + reverse + underline + reset).
- The 256-colour and truecolour forms of SGR.

`ansi::Parser` is built on `vte::Parser` and produces
`ansi::Action::{Print, Execute, CsiDispatch, EscDispatch}` events that
get translated into grid mutations.

`ansi::Grid` is a `Vec<Vec<Cell>>` where each `Cell` carries a `char`
and a `Style` (foreground + background + modifiers). The grid is
resized by `ansi::Grid::resize(rows, cols)` — the contents are
clipped / padded, not lost.

## broadcaster.rs — async broadcast

```rust
pub struct Broadcaster {
    tx: tokio::sync::broadcast::Bytes,
}

impl Broadcaster {
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Bytes>;
    pub fn push(&self, bytes: Bytes);
}
```

The broadcaster task (spawned in `main()`) loops:

```rust
loop {
    let mut buf = [0u8; 4096];
    let n = pty.reader.read(&mut buf)?;
    broadcaster.push(Bytes::copy_from_slice(&buf[..n]));
    if broadcaster.receiver_count() == 0 {
        // nobody listening — drop the bytes on the floor
        continue;
    }
}
```

Subscribers (the WM panes) get a `Receiver<Bytes>` and parse the bytes
through their own `vte::Parser` into their own `Grid`. Subscribers can
come and go without affecting the broadcaster.

When a subscriber is dropped, the receiver is automatically removed
from the broadcast channel — the broadcaster task keeps running until
the PTY child exits.

## Tests

Phase 2 has PTY-touching tests. Every one of them follows one of the
patterns in [Hardening](./Hardening.md):

| Test | Hardening |
| --- | --- |
| `wm::broadcaster::tests::roundtrip_echo_via_broadcaster` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::broadcaster::tests::echo_emits_into_ansi_grid` | Pattern A — kill-switch clone + `tokio::time::timeout(2s)` |
| `wm::pty::tests::write_and_read_roundtrip` | Pattern B — `kill()` + `try_wait()` + thread-spawned `wait()` + drop-on-scope |
| `wm::pty::tests::spawn_and_read` | already safe — `/bin/sh -c "printf …"` exits on its own |

The invariant: a test never outlives its PTY child. Even if
`portable_pty` wedges inside a `wait()`, the bounded timeout returns,
the child is `kill()`-ed, and the next test gets a fresh PTY
allocation. The full suite finishes in ~1 s.

## What Phase 2 does NOT do

- It does not handle the SIGWINCH protocol — yet. When the WM resizes
  a pane in Phase 3, the PTY is resized with `TIOCSWINSZ` but the
  child shell has to cooperate (it usually does, via `stty rows cols`).
- It does not implement bracketed paste. The WM in Phase 3 just sends
  the bytes as-is.
- It does not support any mouse protocol. The terminal is keyboard-only
  in Phase 2.

These are all on the [Roadmap](./Roadmap.md).
