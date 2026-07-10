# Development & Architecture

## Crate map

```
+------------------+   +-----------------+   +----------------+
|  cyberdeck-core  |<--|  cyberdeck-tui  |   |  cyberdeck-web |
|  (no TUI/web)    |   |  ratatui front  |<->|  axum HTTP/WS  |
+------------------+   +-----------------+   +----------------+
                        |  cyberdeck-cli  |   |  wifi-radar    |
                        +-----------------+   +----------------+
                        |  cyberdeck-intel|   |  cyberdeck-daemon|
                        +-----------------+   +----------------+
```

- **`cyberdeck-core`** — async wrappers around `nmcli`, `systemctl`, `apt`,
  `pactl`, `bluetoothctl`, `xrandr`, etc. Single `run()` helper with
  `Privilege::{User, Root}` enum, per-call timeout, uniform `CoreError`.
- **`cyberdeck-tui`** — ratatui 0.29 front-end. 13+ screens, window manager,
  live PTY terminals with ANSI colours, modal system.
- **`cyberdeck-web`** — axum 0.7 (JSON API + WebSocket + static HTML).
  Optional bearer-token auth.
- **`wifi-radar`** — passive 802.11 monitor with web UI.
- **`cyberdeck-cli`** — CLI commands (`cyberdeck city locate`, etc).
- **`cyberdeck-intel`** — threat intelligence layer snapshots.

## Architecture

- **Single source of truth.** `Arc<Live>` of `RwLock`s with background
  refreshers. Web reads via `TuiLiveRead` adapter.
- **One action channel.** `tokio::sync::mpsc::Sender<Action>` for UI events
  and async results.
- **Window manager.** Split tree (`wm/tree.rs`), per-pane state, PTY/ANSI
  stack. Real `$SHELL` processes.
- **No unsafe.** Every crate: `#![forbid(unsafe_code)]`.

## PTY test hardening

PTY tests can never outlive their child process:

- **Pattern A** (broadcaster/window): `ChildKiller` kill-switch +
  `tokio::time::timeout(2s)`.
- **Pattern B** (raw pty): `kill()` + `try_wait()` + thread-spawned `wait()`
  dropped on scope exit.

## Wiki

Detailed docs at [`docs/wiki/`](wiki/):
[Architecture](wiki/Architecture.md) |
[Phase 1-5](wiki/) |
[Hardening](wiki/Hardening.md) |
[Keymaps](wiki/Keymaps.md) |
[Hardware](wiki/Hardware-Setup.md)
