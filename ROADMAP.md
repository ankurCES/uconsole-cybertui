# cyberdeck-tui roadmap

The references in `app.rs`, `app/action.rs`, `app/screen.rs`, `theme.rs`, and
`wm/{mod,ansi,pty,broadcaster}.rs` to "see ROADMAP.md" point here. Anything
marked **done** in this file is shipped; anything marked **next** is the next
chunk of work.

## Phase 1 — single-pane TUI (done)

13 screens, sidebar, status bar, modals (help / command palette / confirm /
input), command palette, refreshers, web bridge. Build is clean.

## Phase 1b — D-pad navigation redesign (done)

The old TUI's left↔right panel navigation was broken: focus was a single
`sidebar_focused: bool` while the surface actually has three regions
(`Sidebar | ContentLeft | ContentRight`). `Tab` was overloaded for both
"go back to sidebar" and "cycle screen," multi-pane screens didn't
track sub-pane focus, and there was no `←` to *enter* the sidebar from
the right pane.

Rewritten with a `Region` enum and a clean D-pad contract:

- `Region { Sidebar | ContentLeft | ContentRight }` on `App`; the
  legacy `sidebar_focused` is derived via `set_region` so the two never
  drift.
- `←`/`h`: ContentLeft → Sidebar; ContentRight → ContentLeft
  (always-step-left, no screen defer).
- `→`/`l`: Sidebar → ContentLeft; ContentLeft → ContentRight (defers to
  the screen's `on_key` first so screens like Network's `→ = jump to
  first wifi` keep working).
- `Tab`/`Shift-Tab` cycles screens only on the content side and only
  when no modal is open.
- Sidebar redesigned as a numbered two-column grid (D-pad friendly on a
  5" uconsole) with a narrow-list fallback.
- Region-aware status bar: shows the focused region's label and the
  region-conditional hint strip.
- Sub-focus borders on every multi-pane screen (System, Network, Files,
  Power, Display, Packages) — the focused half of each screen now
  lights up while the unfocused half dims.
- Header chip — a three-segment `sidebar / left / right` pill under the
  title bar that always reflects the focused region. (Came in after
  the initial D-pad pass; the original Phase 1b copy only mentions the
  status-bar label, which still exists as the spoken label.)
- Focus gutter — a left-edge highlight rail on the sidebar and on the
  focused content half, so D-pad users have a single visible focus
  signal even when borders are off (narrow uconsole mode, dim themes).
  Narrow mode replaces the right pane with a focus gutter and an honest
  "single-pane" hint instead of pretending both halves exist.
- Region-aware help modal — `?` now describes the keys for the focused
  region, not a single static keymap.
- Status-bar vocabulary aligned with the chip: `▶ sidebar`, `▶ left`,
  `▶ right` (was `← content · left` / `content · right →`). The
  `←`/`→` characters still appear in the hint strip itself
  (`←/h sidebar`, `→/l other pane`) — only the region label changed.

Regression tests pinned: `content_left_returns_to_sidebar`,
`sidebar_left_returns_focus`, `router_walk_three_regions`,
`number_keys_when_sidebar_focused_move_cursor_to_that_row`,
`number_keys_when_content_focused_still_swap_pane_kind`, and the
new `ui::status_region_vocabulary::{sidebar,content_left,content_right}`
asserts that lock the `▶` label so a future revert of the chip-vs-label
alignment trips the test instead of silently regressing. All 122
tests in the binary pass.

## Phase 2 — PTY / ANSI / broadcaster (done, not wired)

`wm/ansi.rs`, `wm/pty.rs`, `wm/broadcaster.rs` are implemented and tested but
nothing in the rest of the crate imports them yet. They exist so the
window-manager work in Phase 3 can stand on them without re-deriving the PTY
glue.

## Phase 3 — window manager (done)

Split tree (`wm/tree.rs`), window state (`wm/window.rs`), manager
(`wm/manager.rs`), tree-walk renderer (`wm/render.rs`), and the `Ctrl-W`
keymap in `main.rs`. Terminal panes work end-to-end: the focused pane
can be swapped from a built-in screen to a live PTY (`Ctrl-W n`); bytes
typed into the focused pane are translated by `wm/input.rs` and forwarded
to the child; output is parsed by `wm/ansi.rs` and painted into the
pane's `Grid`.

Milestones:

- [x] **`wm/tree.rs`** — `Node` enum, `compute_layout`, mutators
      (`split`, `close`, `resize`).
- [x] **`wm/window.rs`** — `Window` + `WindowKind` (`Builtin(ScreenId)` /
      `Terminal`), owns its `Grid` + `AnsiParser` + `PaneOutput` +
      `PtyWriter` when terminal.
- [x] **`Focus::Pane(PaneId)`** — replaced via `wm::Manager`; the
      binary `Focus` enum is gone. Sidebar focus is now a `bool` on
      `App` (`App::sidebar_focused`) because the sidebar lives outside
      the WM tree.
- [x] **`Ctrl-W` keymap** — `h/j/k/l` move focus, `v`/`s` split,
      `n` new term, `q`/`x` close, `=`/`+`/`-` resize. (`r` rotate
      deferred — see known issues below.)
- [x] **Render path** — `wm::render::render` walks the tree, paints
      each leaf into its computed rect; the focused leaf gets the focus
      border style.
- [x] **Terminal pane** — `WindowKind::Terminal` spawns `$SHELL` via
      the existing PTY infra, subscribes to its broadcaster, paints
      the grid.
- [x] **Tests** — tree layout, focus traversal, close/resize
      invariants, terminal-pane resize, keymap translation
      (uconsole + bytes-for-key).

Known issues (filed for a follow-up plan):

- **`Ctrl-W n` spawns `$SHELL` twice.** `broadcaster::spawn` consumes
  the `Pty` handle, so we re-spawn a fresh one for the `Window`. The
  workaround is to thread the `Pty` *out* of the broadcaster, which is
  a bigger refactor. Documented at the call site in
  `crates/tui/src/main.rs`.
- **`Ctrl-W =`/`-` resize silently no-ops inside vertical splits.**
  `Manager::resize_focused` only tries `SplitDir::Horizontal`. The fix
  is to discover the parent split's direction from the tree, but that
  is again a bigger refactor. Documented at the call site in
  `crates/tui/src/main.rs`.
- **`Ctrl-W r` (rotate) not wired.** `Node::rotate` was deleted as
  dead code in commit `47441e1`; the keymap entry was not added back.
  Reintroduce `Node::rotate` plus a `Manager::rotate_focused` wrapper
  to restore the verb.

## Phase 4 — polish

- [x] Pane number badges in titles (`1`/`2`/…) so `Ctrl-W N` jump is discoverable.
- Last-used shell + cwd persistence (config file or env).
- Optional: per-pane scrollback for terminals.
- Optional: pane presets (`:layout 2v`, `:layout 1+2`).

## Phase 6 — City screen (done)

New `screens::city` module + 14th sidebar entry. Two-pane layout
(`[Percentage(60), Percentage(40)] Horizontal`, audit-pinned): braille
road map (Unicode `U+2800` + 8-dot bit offset via `BrailleGrid` +
Bresenham) on the left, weather + traffic legend on the right; `w`
toggles the weather panel off so the map fills the content width.

Data sources: IP geolocation via `ip-api.com` HTTP (free tier, no key),
Open-Meteo current + 12h precipitation (HTTPS, no key), synthetic
traffic overlay as a pure function of `(road, hour, weekday)` —
motorway/trunk can `Gridlock` in commute peaks, `footway` never
exceeds `Light`, weekend ≠ weekday. Roads come from a bundled
`<slug>.json` per city (`crates/tui/data/cities/seattle.json` first);
other slugs fall back to seattle until their fixtures land.

Keymap (9 keys, all consumed in `CityScreen::on_key`): `h/j/k/l` pan,
`+/-` zoom, `r` refresh, `c` cycle cities, `t` traffic overlay, `w`
weather panel. City picker + overlay + weather panel toggles persist
via `Prefs` so the user's choices survive relaunch.

CLI parity via `cyberdeck city <subcommand>` (`Locate` / `Weather` /
`Roads` / `Bundled`), sharing the same data path as the TUI. The
async arms (`locate`, `weather`) lazy-build a current-thread tokio
runtime; the data arms (`roads`, `bundled`) stay synchronous.

Background refiller runs on a 600s cadence via `App::spawn_refreshers`
and writes through `App::live.{city_loc, city_weather}`, so the render
path is a pure read of in-memory snapshots. Detailed design + tests +
known issues live at [`docs/wiki/Phase-6-City.md`](./docs/wiki/Phase-6-City.md).

Test counts: 45 unit tests under `screens::city` + 4 dispatcher
integration tests under `app::tests::city_*` + 6 CLI dispatch tests
under `cli_dispatch::city_*`. All green.