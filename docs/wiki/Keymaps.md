# Keymaps

Cyberdeck has three layers of keys: **global**, **window manager**,
and **per-screen**. The global layer is always active; the WM layer
activates after `Ctrl-W`; the per-screen layer activates when a
non-WM leaf has focus.

## Global

| Key            | Action                                       |
| -------------- | -------------------------------------------- |
| `q` / `Ctrl-C` | quit                                         |
| `?`            | help modal                                   |
| `:`            | command palette (`web start`, `web stop`, …) |
| `1`..`9`/`0`   | jump to a screen                             |
| `Tab`          | toggle sidebar ↔ content focus               |
| `↑/↓` or `j/k` | navigate list in current focus               |
| `Enter`        | confirm / open                               |
| `Esc`          | back / cancel modal                          |
| `r`            | refresh current screen                       |

`Ctrl-C` is intercepted as a quit signal only when no modal is open.
When a modal is open, `Ctrl-C` cancels the modal first (same as
`Esc`).

## Window manager (`Ctrl-W` prefix)

Every `Ctrl-W` verb is a two-key sequence: press `Ctrl-W`, then the
second key. Unknown second keys are no-ops (the prefix is consumed
either way).

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

The new pane from `v` / `s` gets focus (vim convention). A hard cap
of **9 panes** is enforced; `Ctrl-W v` / `Ctrl-W s` past the cap
toasts `pane limit reached (9)`. `Ctrl-W 1..9` with no pane at that
index toasts `no pane N`. Closing a pane renumbers the rest
contiguously.

## Per-screen (subset)

The 13 screens have their own keymaps. The most-used are:

### Network

| Key     | Action                                |
| ------- | ------------------------------------- |
| `s`     | scan Wi-Fi (opens Choice modal)       |
| `c`     | connect to highlighted SSID (opens Secret modal for password) |
| `d`     | disconnect active Wi-Fi               |
| `r`     | refresh                               |

### Bluetooth

| Key     | Action                                |
| ------- | ------------------------------------- |
| `s`     | scan for devices                      |
| `p`     | pair highlighted device (opens Secret modal for PIN) |
| `c`     | connect to paired device              |
| `r`     | refresh                               |

### Power

| Key     | Action                                |
| ------- | ------------------------------------- |
| `g`     | cycle governor (opens Choice modal)   |
| `S`     | suspend                               |
| `H`     | hibernate                             |
| `R`     | reboot                                |
| `P`     | power off                             |

### Packages

| Key     | Action                                |
| ------- | ------------------------------------- |
| `/`     | focus the search box                  |
| `u`     | `apt update` (opens Progress modal)   |
| `U`     | `apt upgrade -y` (opens Progress modal) |
| `i`     | install highlighted (opens Choice modal) |
| `x`     | remove highlighted (opens Choice modal) |

### Services

| Key     | Action                                |
| ------- | ------------------------------------- |
| `s`     | start highlighted                     |
| `S`     | stop highlighted                      |
| `r`     | restart highlighted                   |
| `e`     | enable on boot                        |
| `d`     | disable on boot                       |

### Files

| Key     | Action                                |
| ------- | ------------------------------------- |
| `Enter` | open highlighted                      |
| `Backspace` | up one directory                   |
| `/`     | focus the path box                    |

## uconsole-specific

The ClockworkPi uconsole has a small keyboard with no dedicated
navigation cluster. Cyberdeck is designed to be usable from this
keyboard:

- **`Tab`** is on the keyboard (next to `q`).
- **`Shift-Tab`** cycles focus in reverse.
- **`Ctrl-W`** is on the keyboard (`Fn-W` on the uconsole's Fn layer,
  or `Ctrl-W` directly with the Fn key held).
- **`Ctrl-C`** is on the keyboard.
- **`Esc`** is on the keyboard.
- **`:`** is `Shift-;` on the uconsole.

The uconsole's hardware keymap (Fn layer + glyphs) is documented in
[`docs/keymap-uconsole.md`](../keymap-uconsole.md) and the rendered
HTML version at [`docs/keymap-uconsole.html`](../keymap-uconsole.html).

## Modal keymap

When a modal is open, the global keymap is replaced with the modal's
keymap:

| Modal          | Enter            | Esc               | Other                        |
| -------------- | ---------------- | ----------------- | ---------------------------- |
| `Secret`       | submit masked value | cancel         | —                            |
| `Choice`       | select highlighted  | cancel         | `j`/`k` navigate             |
| `Wizard`       | next step        | cancel             | `b` = back, `n` = next       |
| `Progress`     | —                | cancel (if cancellable) | —                       |
| `AuthFailure`  | acknowledge      | acknowledge        | —                            |

`Ctrl-W` is **not** routed to the WM while a modal is open; the modal
owns all input until it's closed.

## Discovering keys

- `?` opens the help modal which lists every keymap that's currently
  active (global + WM + per-screen + modal).
- The command palette (`:`) lists every verb across all keymaps; you
  can search for a verb and activate it without memorising the key.

## Conventions

- **Single-letter keys for verbs** (`s` for scan, `r` for refresh).
- **Capital-letter keys for destructive verbs** (`S` for stop, `R`
  for reboot, `U` for upgrade).
- **Enter to confirm, Esc to cancel.** No other "OK / Cancel" pattern.
- **No mouse.** Cyberdeck is keyboard-only by design. Mouse support
  is on the [Roadmap](./Roadmap.md).
