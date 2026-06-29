# Phase 1 — TUI

The first thing cyberdeck shipped: a 13-screen ratatui front-end with a
sidebar, a live header, a command palette, and a toast log. No PTYs, no
ANSI parsing, no window manager — just screens.

## Screens

13 screens, all with the same shape (`Screen` trait):

| # | Screen | Backed by | Typical verbs |
| - | ------ | --------- | ------------- |
| 1 | System | `/proc`, `uname`, `uptime`, `free`, `df` | refresh |
| 2 | Network | `nmcli dev`, `nmcli con` | wifi scan / connect / disconnect |
| 3 | Bluetooth | `bluetoothctl` | scan / pair / connect / disconnect |
| 4 | Power | `/sys/class/power_supply`, `/sys/class/thermal` | governor, suspend, hibernate, reboot, shutdown |
| 5 | Display | `xrandr`, `brightnessctl` | brightness ±, output on/off |
| 6 | Audio | `pactl list sinks`, `pactl set-sink-volume` | volume ±, mute, default sink |
| 7 | Storage | `df -h`, `lsblk` | mount / unmount (root only) |
| 8 | Services | `systemctl list-units` | start / stop / restart / enable / disable |
| 9 | Packages | `apt list --upgradable`, `apt-cache search` | search / install / remove / update / upgrade |
| 10 | Processes | `ps -eo pid,pcpu,pmem,comm --sort=-pcpu \| head` | SIGTERM, refresh |
| 11 | Logs | `journalctl -n 200 -f` | filter, follow |
| 12 | Files | `ls`, `stat` | (read-only viewer in Phase 1) |
| 13 | Settings | (UI prefs, theme, token rotate) | — |

Each screen implements the `Screen` trait:

```rust
pub trait Screen {
    fn id(&self) -> ScreenId;
    fn title(&self) -> &'static str;
    fn on_enter(&mut self, live: &Live) -> Result<(), CoreError>;
    fn render(&mut self, area: Rect, buf: &mut Buffer, live: &Live);
    fn on_key(&mut self, key: KeyEvent, live: &Live) -> Result<Action, CoreError>;
    fn on_action(&mut self, a: Action, live: &Live) -> Result<(), CoreError>;
}
```

The TUI holds a `Vec<Box<dyn Screen>>` indexed by `ScreenId`. The
sidebar renders the list; selecting one pushes a `Screen::Switch`
action. The focused screen receives all subsequent key events until
either the user picks another screen or the WM takes over.

## Sidebar + content layout

```
+-----------+--------------------------------------------+
| System    |                                            |
| Network   |                                            |
| Bluetooth |        Screen content goes here.           |
| Power     |                                            |
| Display   |                                            |
| Audio     |                                            |
| Storage   |                                            |
| Services  |                                            |
| Packages  |                                            |
| Processes |                                            |
| Logs      |                                            |
| Files     |                                            |
| Settings  |                                            |
+-----------+--------------------------------------------+
| 14:02:11  CPU 2.4GHz 38°C  MEM 1.2/4G  BAT 71%  ssid |
+-------------------------------------------------------+
```

The sidebar is 18 columns wide and the content area is the rest.
`Tab` toggles focus between the sidebar and the content. `j`/`k` (or
`↑`/`↓`) navigate; `Enter` activates.

## Live header

The top-right header is rebuilt every frame from the `Arc<Live>` data.
It shows:

- Wall clock (HH:MM:SS).
- CPU temperature from `/sys/class/thermal/thermal_zone0/temp`.
- CPU frequency from `/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq`.
- Memory used / total from `/proc/meminfo`.
- Battery percentage from `/sys/class/power_supply/BAT0/capacity`.
- Active Wi-Fi SSID + IPv4 from `nmcli`.

If any of those calls fails, the corresponding cell shows `—` and the
last-seen value is logged as a toast.

## Command palette

`:` opens a fuzzy-search palette over the union of:
- All screens (`system`, `network`, `bluetooth`, …).
- All WM verbs (`split vertical`, `split horizontal`, `close pane`, …).
- Web bridge actions (`web start`, `web stop`, `web status`).
- A few platform ones (`reboot`, `shutdown`, `suspend`).

`Esc` closes the palette; `Enter` activates the highlighted action.

## Toast log

Every `Action` result that the user should know about is logged as a
toast with a level (`Info` / `Success` / `Warning` / `Error`) and an
expiry timestamp. Toasts are rendered in the top-right corner of the
status bar with a 5-second TTL.

If the toast queue overflows 64 entries, the oldest are dropped and a
`toast queue overflow (n dropped)` warning is pushed.

## Where the WM plugs in (Phase 3+)

In Phase 1, the WM is a no-op: the sidebar + content + status bar is
the only layout. Phase 3 replaces the `content` cell with a tree-walk
of WM panes; each leaf is either a `Screen` (the legacy single-pane
view) or a `TerminalPane` (a real `$SHELL`).

`Screen::on_key` still works the same way for `Screen` leaves; the WM
just routes the input to the focused leaf. See
[Phase 3 — WM](./Phase-3-WM.md).

## Adding a new screen

1. Add a new variant to `ScreenId` in `crates/tui/src/screens/id.rs`.
2. Implement the `Screen` trait for your screen in
   `crates/tui/src/screens/<name>.rs`.
3. Register it in `App::screens()` (the `Vec<Box<dyn Screen>>`).
4. Add a sidebar entry in `crates/tui/src/screens/sidebar.rs`.
5. Add an entry to the command palette by implementing
   `Screen::palette_aliases`.
6. Add tests under `#[cfg(test)] mod tests` in the screen file.
   Use `cargo check -p cyberdeck-tui --all-targets` to verify; only run
   `make test ARGS='-p cyberdeck-tui --bin cyberdeck-tui'` once you've checked
   the build is clean.

## Conventions

- **Read-only by default.** If a screen does writes, the verb has to go
  through `core` with `Privilege::Root`. No direct `Command::new("sudo")`
  in screen code.
- **Per-call timeouts.** Every `core` call has a per-call timeout. If
  `nmcli` hangs, the toast says `nmcli timed out (5s)` and the screen
  shows the last-seen data.
- **No background threads.** Background refreshers run on the tokio
  runtime as `tokio::spawn` tasks. They send their results through the
  action channel, not through shared state with locks.

## Tests

Phase 1 has no PTY-touching tests, so the
[Hardening](./Hardening.md) patterns aren't needed. The tests under
`crates/tui/src/screens/` are pure unit tests on the screen trait
implementations (rendered into a `Buffer` with a fixed `Rect`).
