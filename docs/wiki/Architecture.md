# Architecture

Cyberdeck is three crates:

```
+------------------+   +-----------------+   +----------------+
|  cyberdeck-core  |<--|  cyberdeck-tui  |   |  cyberdeck-web |
|  (no TUI/web)    |   |  ratatui front  |<->|  axum HTTP/WS  |
+------------------+   +-----------------+   +----------------+
```

- **`cyberdeck-core`** — async, async-trait-free wrapper around `nmcli`,
  `systemctl`, `apt`, `pactl`, `bluetoothctl`, `xrandr`/`brightnessctl`,
  `suspend`/`reboot`/`shutdown`, `journalctl`, etc. No tokio runtime of
  its own. No network. Just shell-outs through a single `run()` helper
  that respects `Privilege::{User, Root}` via `sudo -n`.
- **`cyberdeck-tui`** — ratatui front-end. Owns the `Arc<Live>` of
  `RwLock`s, drives the action loop, runs background refreshers.
  Embeds the web server when `--web` is passed.
- **`cyberdeck-web`** — axum 0.7 server. Reads from the same `Arc<Live>`
  through a `TuiLiveRead` adapter (`crates/tui/src/web_bridge.rs`).

## Single source of truth

```
                +----------------------+
                | Arc<Live> (RwLock)   |
                |   system / network   |
                |   audio / bt / …     |
                +----^------------^----+
                     |            |
   cyberdeck-tui     |            |     cyberdeck-web
   (background       |            |     (TuiLiveRead)
    refreshers)------+            +----- (axum handlers)
                     |            |
                     +-- reads --+
```

The TUI owns the data; the web reads from it through a thin adapter.
There is no second source of truth and no syncing logic.

## One action channel

UI events (key presses, mouse, palette) and async results (background
refresher completions, web bridge replies) both go through a single
`tokio::sync::mpsc::Sender<Action>`. The main loop is a `select!` over
the terminal input stream and the action receiver.

A *separate* control channel carries `WebStart` / `WebStop` so the
embedded web server can be toggled without racing the main event loop.
The tap task in `main()` listens on this control channel and forwards
into the action channel.

## Window manager (Phase 3)

`crates/tui/src/wm/` is the largest module and gets its own page:
[Phase 3 — WM](./Phase-3-WM.md).

Quick map:

- `wm/tree.rs` — split tree data structure (no UI).
- `wm/window.rs` — per-pane state (terminal + screen types).
- `wm/manager.rs` — orchestrator: input → action → state mutation.
- `wm/render.rs` — tree-walk renderer.
- `wm/pty.rs` — PTY child + reader.
- `wm/ansi.rs` — minimal ANSI parser.
- `wm/broadcaster.rs` — async broadcast of PTY bytes to subscribers.
- `wm/input.rs` — translate raw key events into WM verbs.

## Privilege isolation

| Layer | Authority |
| --- | --- |
| `cyberdeck-web` | Bearer-token auth at the HTTP boundary. No system authority on its own. |
| `cyberdeck-tui` | Owns the terminal. Drives the action loop. Calls `core` with `Privilege::Root` for writes that need it. |
| `cyberdeck-core` | No tokio runtime, no network. Just shells out. |

The web crate can't reboot the box unless the `core` layer says yes,
and `core` can't reach the network unless something outside it does.

## Privilege escalation path

Reads: unprivileged. Writes that mutate the system (mount, install,
reboot, set Wi-Fi credentials, change PulseAudio defaults, set
brightness, power actions) call `run()` with `Privilege::Root`, which
prepends `sudo -n` (non-interactive).

If `sudo -n` fails (NOPASSWD not set), the call returns
`CoreError::Permission("...")` and the TUI shows a red toast pointing
the user at the install instructions.

The `--web` and `--full` installers write a narrow NOPASSWD sudoers
fragment for the `cyberdeck` system user, listing only the commands
the service needs.

## Compile-time guarantees

- `#![forbid(unsafe_code)]` in every crate.
- `#![deny(missing_docs)]` in `cyberdeck-core` (the most stable surface).
- Per-call timeouts on every shell-out (so the UI never blocks on a
  wedged `nmcli`).
- Tests use **kill-switch + bounded `tokio::time::timeout(2s)`** on every
  PTY-touching path (see [Hardening](./Hardening.md)).

## Where the WM fits

```
                   main event loop (select!)
                                |
              +-----------------+-----------------+
              |                                   |
       terminal input                       Action receiver
              |                                   |
              v                                   v
        wm/manager.rs  ←—————  wm/render.rs
              |                      ↑
              v                      |
        wm/window.rs  ——— wm/pty.rs (per pane)
              |             |
              v             v
         ANSI parser    PTY child ($SHELL)
```

The terminal input is split at `Ctrl-W`: most keys are routed to the
focused pane (or to the screen UI when the WM has a single screen
pane); `Ctrl-W` plus a second key is routed to `wm::manager` as a WM
verb.

See [Phase 3 — WM](./Phase-3-WM.md) for the verb table and the focus
algorithm.
