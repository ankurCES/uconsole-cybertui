# TUI polish + new features — design spec

**Date:** 2026-06-30
**Branch:** `fix/tui-mesh-nodes-scroll`
**Author:** blumi

## Scope

Four bug fixes plus five new features, all behind one branch so the four
fixes land together and the new features land in reviewable chunks
(separate commits per module, single PR).

### Bug fixes

1. Sidebar overflow items are selectable but don't render — cursor can
   move past the visible window on terminals shorter than the 8-row
   grid.
2. Logs screen `r` is a no-op; the screen advertises `● live` but is
   actually a one-shot `journalctl -n 50` fetch. Same for System's
   "recent logs" pane — hint says `r refresh`, handler missing.
3. Packages `s` clears the filter and runs an empty-string search.
   Should open an input modal that takes the query.
4. `cargo test` typed directly bypasses `scripts/safe-test`. On a busy
   box a parallel blanket run can hang the suite for ≥30 minutes
   (PTY-pool exhaustion).

### New features

5. **Network sparkline** in the header chip — active-interface rx/tx
   sparkline sampled at 1 Hz.
6. **Process tree toggle** on the Processes screen — `t` toggles flat
   vs. tree view.
7. **Toast log** — past toasts are reviewable, not just visible for 5 s.
8. **Saved-Wi-Fi viewer** — Network's right pane switches between live
   scan and saved connections.
9. **Clipboard paste** into input modals — `Ctrl-Shift-V` / bracketed
   paste appends to the focused `Modal::Input` / `Modal::Secret` buffer.

## Design for isolation

Each module is independent and has one clear purpose. New state lives
on `App` or `Live` only where the existing pattern already puts it.
No new modules outside `cyberdeck-core` (one new file:
`crates/core/src/logs.rs`; everything else edits existing files).
Screens remain stateless functions of `App` + live data.

For each module, the question "can someone understand what it does
without reading its internals?" is yes: every new behaviour has a
narrow entry point (one new key, one new modal variant, or one new
background task) and a single targeted test that pins the contract.

## Module 1 — Sidebar scroll + scrollbar

### Goal

Every `ScreenId` is reachable *and* visible. Cursor never sits on an
unpainted row. A scrollbar on the right edge of the sidebar tells the
user how far down the list they are.

### Changes

- **`crates/tui/src/app.rs`** — `App` gains `pub sidebar_offset: usize`
  (default 0 in `App::new`).
- **`crates/tui/src/ui/mod.rs`**:
  - `draw_sidebar_grid` (lines 205-267) computes
    `visible_rows = inner.height as usize`. The grid renders only
    rows `[sidebar_offset, sidebar_offset + visible_rows)`, clamped so
    the cursor is always in the window. `sidebar_offset` advances when
    `sidebar_idx` would exit the window.
  - `draw_sidebar_narrow` (lines 148-198) uses `ListState::with_offset`
    for the same windowed behaviour. Existing tests stay green because
    the default 15-row case fits a tall terminal.
  - New helper `fn sidebar_scrollbar(area, offset, total, visible,
    theme)` renders a vertical `▴ / ● / ▾` gutter, merged with the
    existing focus gutter (`mod.rs:252-265`) so the sidebar has one
    combined gutter instead of two.
- **`crates/tui/src/main.rs:1030-1041`** — sidebar Up/Down handlers
  call a helper `fn clamp_sidebar_offset(idx, offset, total, visible)`
  before assigning `sidebar_idx`, so the cursor stays in window.

### Tests (in `crates/tui/src/ui/mod.rs` `tests` mod)

- `sidebar_clamps_offset_when_cursor_exits_top_window` — set
  `sidebar_idx=5, sidebar_offset=0, visible=4`, press Down once →
  `sidebar_idx=6, sidebar_offset=3`.
- `sidebar_offset_does_not_advance_when_cursor_still_visible` —
  `sidebar_idx=5, sidebar_offset=0, visible=8`, Down → `sidebar_idx=6,
  sidebar_offset=0`.
- `sidebar_scrollbar_renders_when_total_exceeds_visible` —
  `total=15, visible=4, offset=4`, gutter contains both `▴` and `▾`
  markers.
- Existing `sidebar_down_moves_cursor_wrapping`,
  `sidebar_up_wraps_to_last`, `sidebar_j_k_navigate`,
  `sidebar_enter_commits_cursor`, `sidebar_left_returns_focus_*`,
  `content_left_returns_to_sidebar`,
  `sidebar_keys_do_not_fire_when_content_focused`,
  `number_keys_when_sidebar_focused_move_cursor_to_that_row` continue
  to pass unchanged.

## Module 2 — Live logs

### Goal

Logs screen shows a live `journalctl` tail. System's "recent log" pane
reflects the same buffer so the user sees the last few lines without
switching screens. `r` does an immediate longer-window fetch on top
of the periodic feed.

### Approach

Periodic 1 Hz poll, no `journalctl -f` subprocess. Matches the
existing `Live::spawn_refreshers` pattern (`app.rs:254-317`).

### Changes

- **`crates/core/src/logs.rs`** (new) —
  `pub async fn recent_since(secs: u64) -> Result<Vec<String>, CoreError>`,
  thin wrapper around `journalctl -n 200 --no-pager -q --since="-<secs>s"`.
  Module-level `#[cfg(test)]` mod asserts the binary path is
  discoverable via `which::which("journalctl")` on test setup; the
  test only runs when the binary is present (no hard failure on CI).
- **`crates/tui/src/app/action.rs`** — no new variant; reuse
  `Action::LogPushed` (line 34).
- **`crates/tui/src/app.rs`** — `Live::spawn_refreshers` (lines
  254-317) gains a third background task that ticks at 1 Hz, calls
  `logs::recent_since(2)`, and sends each line as
  `Action::LogPushed { ts: Local::now(), line }`. Dedupe by line
  text: drop a line if it's byte-identical to the last entry in
  `app.logs`. Buffer cap stays as today (already documented).
- **`crates/tui/src/screens/logs.rs`**:
  - Replace the one-shot `f` handler (lines 31-66) with a hint-row
    update: `● live (G to scroll up) · r fetch · c clear`. Bind
    `r` (and keep `f` as an alias) to a one-shot immediate fetch
    using `recent_since(60)` so the user can pull a longer window
    on demand.
  - On screen entry (first render after switch), set
    `app.logs_offset = 0` so a returning user sees the live tail
    immediately.
- **`crates/tui/src/screens/system.rs`**:
  - Bind `r` to the same one-shot fetch. Hint at line 259 already
    says `r refresh` — handler now exists.
  - No process change: System's right pane already reads `app.logs`,
    so the new periodic refiller feeds it for free.

### Tests

- `logs::recent_since_returns_recent_lines` — `journalctl --since="-1s"`
  is invocable, returns a `Vec<String>`. Skipped if `journalctl`
  isn't on the test box.
- `live_log_refresher_dedupes_consecutive_identical_lines` — push two
  identical `LogPushed` lines, `app.logs.len() == 1`.
- `logs_r_fetches_last_60s` — set `app.logs = vec![]`, dispatch `r`
  in `LogsScreen::on_key`, assert a one-shot fetch task spawns.
- `system_r_also_fetches` — same pattern against System's `on_key`.

## Module 3 — Packages search modal

### Goal

Pressing `s` opens an input modal; submit searches the typed string.

### Changes

- **`crates/tui/src/app.rs:133-145`** — add `InputKind::PackageSearch`
  to the enum.
- **`crates/tui/src/main.rs:1286`** — new arm in `run_input`:
  ```rust
  InputKind::PackageSearch => {
      let tx = app.tx.clone();
      let q = value.clone();
      tokio::spawn(async move {
          match cyberdeck_core::packages::search(&q).await {
              Ok(v) => { /* Action::PkgSearchResult(v) */ }
              Err(e) => { /* Action::Toast(Error, e) */ }
          }
      });
  }
  ```
- **`crates/tui/src/app/action.rs`** — new `Action::PkgSearchResult(Vec<Package>)`
  and `Action::PkgSearchError(String)`. Handler in the main loop
  writes into `app.pkg_search_results` or pushes a toast.
- **`crates/tui/src/screens/packages.rs:107-112`** — replace the
  current `s`/`/` arms:
  - `s` → `app.open_input("search packages", InputKind::PackageSearch);
    app.pkgs_filter = String::new();`
  - `/` → clears filter, no-op otherwise (existing semantics).
- Render: existing right-pane "search: <buf>_" line stays; on submit,
  results land in `app.pkg_search_results` and the right pane's
  `matches: N (offset/total)` line renders the new count.

### Tests

- `packages_s_opens_package_search_modal` — press `s`, assert
  `matches!(app.modal, Modal::Input { kind: InputKind::PackageSearch, .. })`.
- `packages_submit_searches_typed_query` — open modal, submit "vim",
  assert `search("vim")` was invoked. Mockable via a trait indirection
  in `cyberdeck_core::packages::search` (gated behind a `cfg(test)`
  seam).

## Module 4 — Cargo test waits

### Goal

`cargo test` typed by reflex always runs through `scripts/safe-test`
on this developer's machine. The wrapper itself is unchanged; we add
a shell-level reminder that nudges the developer toward the wrapper
without breaking `cargo test` on other repos.

### Changes

- **`scripts/sh/cargo-test-with-safe-test.bash`** (new) — a 15-line
  bash snippet the user pastes into `~/.bashrc`/`~/.zshrc` (or
  sources from `direnv`):
  ```bash
  cargo() {
      if [[ "$1" == "test" ]]; then
          shift
          command cargo test "$@"
          local rc=$?
          if [[ $rc -ne 0 && "$1" != *"--ci"* ]]; then
              echo "↑ blanket 'cargo test' risks hanging (PTY pool). Use: scripts/safe-test ..." >&2
          fi
          return $rc
      fi
      command cargo "$@"
  }
  ```
  This runs cargo first and emits a *nudge* (not a refusal) on failure.
  The hook must NOT refuse blanket runs — that would block `cargo test`
  on every other repo the user has open. It's a reminder, not a gate.
- **`CONTRIBUTING.md`** — add a one-paragraph "preventing endless
  test waits" section pointing at the snippet and the existing
  `scripts/safe-test` docs.
- **Lint test** — `bash -n scripts/sh/cargo-test-with-safe-test.bash`
  parses (runs as a Makefile-level check, not a unit test).

### Tests

- No Rust-level test. The verification is the bash parse check +
  the existing `scripts/safe-test` blanket-detection tests (which
  this change leaves untouched).

## Module 5 — Network sparkline in header chip

### Goal

Active interface shows a tx/rx sparkline so the user can see "did
that download just finish?" without leaving the sidebar.

### Changes

- **`crates/core/src/net.rs`** — new
  `pub fn interface_byte_counts() -> Result<Vec<(String, u64, u64)>, CoreError>`,
  reads `/sys/class/net/*/statistics/{rx,tx}_bytes`. Pure read, no
  privileges.
- **`crates/tui/src/app.rs`** — `Live` gains
  `pub net_history: Arc<RwLock<VecDeque<(String, u64, u64)>>>` — 60-sample
  rolling buffer of (iface, rx_delta, tx_delta).
- **`crates/tui/src/app.rs`** — `Live::spawn_refreshers` extends the
  existing 1 Hz task with a delta computation against the previous
  sample. New `Action::NetSampled` so the UI redraws; no toast, just
  a redraw.
- **`crates/tui/src/ui/mod.rs`** — header widget gets a new chip
  slot: when `app.live.active_ssid` is `Some`, render
  `rx: ▆▆▅▃▁  tx: ▁▃▅▆▇` (5 samples each, normalized to the 8-glyph
  `▁▂▃▄▅▆▇█` ramp).

### Tests

- `sparkline_normalizes_to_eight_buckets` — feed 60 samples ranging
  0–1000, assert output contains exactly 5 visible glyphs (5-sample
  window) bucketed into the 8-ramp height.
- `sparkline_renders_empty_when_no_active_interface` —
  `active_ssid = None`, header chip renders without sparkline glyphs.
- `net_history_is_bounded_at_60` — feed 100 samples, queue holds
  exactly 60.

## Module 6 — Process tree toggle

### Goal

Processes screen supports both flat (default) and tree views.

### Changes

- **`crates/core/src/process.rs`** — new
  `pub async fn list_with_ppid() -> Result<Vec<(Process, i32)>, CoreError>`,
  reads `/proc/<pid>/stat` field 4 (ppid). Same cadence as the
  existing flat list.
- **`crates/tui/src/app.rs`** — `Live` gains
  `pub proc_ppid: Arc<RwLock<HashMap<i32, i32>>>` refreshed at 15 s
  (already the long-cadence loop).
- **`crates/tui/src/screens/processes.rs`** — add
  `pub proc_tree: bool` to `App`; `t` toggles. In tree mode, group
  by ppid, render with `├─` / `└─` connectors. The flat list is
  unchanged.
- App defaults `proc_tree: false` in `App::new`.

### Tests

- `proc_tree_toggle_flips_state` — press `t`, `app.proc_tree = true`;
  press `t` again, false.
- `proc_tree_renders_children_under_parent` — feed `[(1, 0), (2, 1),
  (3, 1)]`, render, assert rows for pid 2 and 3 carry the connector
  prefix.

## Module 7 — Toast log

### Goal

Past toasts are reviewable, not just visible for 5 s.

### Changes

- **`crates/tui/src/app.rs`** — `App` gains
  `pub toast_history: VecDeque<Toast>` (cap 200, ring).
  `push_toast` pushes to both `toasts` and `toast_history`.
- **`crates/tui/src/app.rs:42-94`** — new `Modal::ToastLog` — read-only
  scrolling list of `toast_history`. Open via `T` (chord `Shift-T`)
  from anywhere; close via `Esc` or `T`.

### Tests

- `toast_history_is_bounded` — push 250 toasts, assert
  `toast_history.len() == 200`.
- `toast_log_modal_lists_in_reverse_chronological_order` — push 3
  toasts, open modal, newest is row 0.
- `toast_log_T_key_opens_modal` — press `T`, assert
  `matches!(app.modal, Modal::ToastLog)`.

## Module 8 — Saved-Wi-Fi viewer

### Goal

Right pane of Network shows saved connections, not just live scans.

### Changes

- **`crates/core/src/net.rs`** — new
  `pub async fn saved_connections() -> Result<Vec<SavedConnection>, CoreError>`
  where `SavedConnection { name: String, ssid: String, security:
  String, last_connected: Option<String> }`. Wraps
  `nmcli -t -f NAME,SSID,SECURITY,TIMESTAMP connection show`.
- **`crates/tui/src/app.rs`** — `Live` gains
  `pub saved_connections: Arc<RwLock<Vec<SavedConnection>>>` refreshed
  at the existing 15 s cadence.
- **`crates/tui/src/screens/network.rs`** — add
  `pub net_show_saved: bool` to `App`. `s` (in Network screen, when
  no modal is open and no input is focused) toggles between live-scan
  and saved view in the right pane. Disambiguate from any existing
  `s` binding by grep on this branch.
- App defaults `net_show_saved: false` in `App::new`.

### Tests

- `saved_connections_parses_nmcli_tsv` — feed a fixed `nmcli` output
  string, assert `SavedConnection` rows.
- `network_s_toggles_between_scan_and_saved` — assert state flip.

## Module 9 — Clipboard paste

### Goal

`Ctrl-Shift-V` or a paste event pastes the clipboard into the focused
`Modal::Input` / `Modal::Secret` buffer.

### Changes

- **`crates/tui/src/main.rs`** — extend the key router to handle
  `crossterm::event::Event::Paste(String)` and
  `KeyEvent { modifiers: SHIFT|CONTROL, code: Char('V') }`. On paste:
  append `payload` to the focused `Modal::Input`/`Modal::Secret` buf,
  or to `app.palette_buf` if the palette is open. Char-by-char paste
  events from terminals without bracketed-paste support (e.g. urxvt)
  are already handled by the existing `Char(c)` arm; the new path is
  for terminals that deliver a single `Event::Paste`.

### Tests

- `paste_event_appends_to_input_modal_buffer` — open `Modal::Input`,
  fire `Event::Paste("hello")`, assert `buf == "hello"`.
- `paste_event_into_secret_modal_keeps_buf_real` — open
  `Modal::Secret`, fire paste, assert `buf == "hello"` even though
  rendered mask is `•••••`.
- `paste_event_appends_to_palette_buf_when_palette_open` — open
  palette, fire paste, assert `palette_buf == "hello"`.

## Cross-cutting concerns

- **Targeted tests only** — every new test is scoped by crate and
  module, no `cargo test --workspace` additions. `CONTRIBUTING.md`
  rule stays in force.
- **No `unsafe`** — confirmed pattern across all nine modules.
- **No new top-level dependencies** — `journalctl` and
  `/sys/class/net` are already the canonical sources; the paste path
  uses `crossterm::event::Event` which is already in the dependency
  tree.
- **ROADMAP bump** — add a new entry to `ROADMAP.md` for each module
  so the changelog captures them.

## Sequencing

Modules land as separate commits on `fix/tui-mesh-nodes-scroll`,
reviewable independently:

1. Module 1 — Sidebar scroll + scrollbar
2. Module 2 — Live logs (Logs + System)
3. Module 3 — Packages search modal
4. Module 4 — Cargo test hook
5. Module 5 — Network sparkline
6. Module 6 — Process tree toggle
7. Module 7 — Toast log
8. Module 8 — Saved-Wi-Fi viewer
9. Module 9 — Clipboard paste

Each commit's verification command is a `scripts/safe-test` scoped
to the touched module(s). See CONTRIBUTING.md §"Test discipline".

## Open questions resolved

- Q1 (sidebar overflow): **C — windowed list + scrollbar**.
- Q2 (live logs): **B — 1 Hz periodic poll, no subprocess**.
- Q3 (packages search): **A — new `InputKind::PackageSearch`**.
- Q4 (cargo waits): **B — shell hook reminder**.
- Q5 (new features): **all five**: sparkline, process tree, toast log,
  saved-Wi-Fi viewer, clipboard paste.
- Sparkline placement: header chip.
- Toast history cap: 200.
- `s`/`t` collisions: grep on this branch during implementation; if
  a collision exists, the new binding wins and the old binding is
  aliased.

## Self-review

- No `TBD`/`TODO` placeholders.
- Internal consistency: yes — all nine modules reference existing
  patterns (`Live`, `Modal`, `Action`).
- Scope: focused enough for one PR series; not decomposed.
- Ambiguity: each module's "changes" section names exact files,
  functions, and line ranges where applicable.