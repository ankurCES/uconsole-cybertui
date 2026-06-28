# TUI UX improvements — design

> Spec for the standing goal: make Network and Bluetooth screens actually
> usable (scanned networks / paired devices show up in a single navigable
> left list, with TUI-based password / passkey modals, hidden-SSID
> connect, an in-TUI file editor, and a clean single-panel sidebar) and
> keep the test loop tight (no blanket `cargo test`).

## Scope

In scope, scoped to the existing `cyberdeck-tui` crate (no new crates,
no protocol changes):

1. **Network screen** — scanned Wi-Fi networks appear in a single left list
   (interfaced + Wi-Fi in one navigable list with section headers); `Enter`
   on a network opens the connect flow; `h` opens a hidden-SSID connect.
2. **TUI password/passkey modal** — `Modal::Secret` and `Modal::Input`
   already exist; render an explicit `OK` / `Cancel` button row inside
   the modal so the affordance is visible. Behaviour (Enter submits, Esc
   cancels) is unchanged. Add `InputKind::BluetoothPasskey` for numeric
   BT pairing.
3. **Bluetooth screen** — devices appear in a single left list with
   status (connected / paired / unpaired); `p` opens the passkey modal
   for pairing.
4. **Files: in-TUI editor** — add a `ScreenId::Editor` reachable from the
   Files screen via `e` on a selected file. Tiny embedded text editor:
   read file → buffer → `Ctrl-S` saves, `Esc` exits. Read-only fallback
   for binaries / files larger than a cap.
5. **Sidebar / content layout** — already a single 13-entry sidebar list;
   per-screen content area gets a single left pane (the list / form) plus
   a right status / preview pane. No nested sub-panels.
6. **Test discipline** — `CONTRIBUTING.md` documents "always scope to a
   specific crate / module / test name; never `cargo test` / `--workspace`".

Out of scope (explicitly):

- Splitting panes (the WM is locked to single-pane).
- Web UI changes (the browser side already has the same data via the
  `LiveRead` bridge; not part of this goal).
- New screens (Editor reuses the existing `Screen` trait; it's a new
  `ScreenId` variant, not a new module).
- Refactoring `wm::*` or `app.rs` plumbing beyond what the modules above
  need.

## Architecture

The work is six independent modules. Each lands as its own commit and is
pushed immediately so the remote history tracks progress.

### Module 1 — Network screen refactor

Files:

- `crates/tui/src/screens/network.rs` (rewrite the render + on_key).
- `crates/tui/src/app.rs` (no new fields needed; `net_selected`,
  `wifi_scan_results`, `live.interfaces` already cover the model).

Behaviour:

- Single left list with section headers: `── interfaces ──` then rows of
  `<iface> <state> <ipv4>`; then `── wifi ──` then rows of `<ssid>
  <signal>% <security>`; the active connection is starred.
- `j/k`/`Up/Down` walk the unified cursor (clamped to bounds).
- `Enter` on a Wi-Fi row → if security is empty, dispatch
  `RunAction::WifiConnect { ssid, password: None }` immediately; if
  secured, open `Modal::Secret` with the SSID stashed in `pending_ssid`.
- `Enter` on an interface row → no-op (interfaces are info-only here;
  `Space` still toggles up/down).
- `r` triggers an immediate `RunAction::WifiScan`.
- `c` opens a hidden-SSID `Modal::Input` (`InputKind::HiddenSSID`),
  followed by `Modal::Secret` for the password on submit.
- `d` opens the existing `Confirm` modal for `WifiDisconnect`.
- The right pane is a status pane: active SSID, IP, gateway, last scan
  time, and the result count from the most recent scan.

Tests (targeted, no `--workspace`):

- `cargo test -p cyberdeck-tui --lib screens::network::tests`
- `arrows_walk_unified_list_with_section_headers`
- `enter_on_open_network_dispatches_wifi_connect`
- `enter_on_secured_network_opens_secret_modal_with_pending_ssid`
- `h_opens_hidden_ssid_input`
- `c_submits_hidden_ssid_and_password_chain`

### Module 2 — Modal OK/Cancel polish + BluetoothPasskey

Files:

- `crates/tui/src/main.rs` (`draw_modal` arms for `Modal::Input`,
  `Modal::Secret`: append a ` [ OK ]   Cancel ` line, theme-button
  styles).
- `crates/tui/src/app.rs` (add `InputKind::BluetoothPasskey`; numeric
  filter on the modal buffer when this kind is active).

Behaviour:

- `Modal::Input` and `Modal::Secret` render an extra line:
  `   [ OK ]      [ Cancel ]` with the focused button styled
  `theme.selection_bg` (Enter → OK, Esc → Cancel).
- `Tab` while the modal is open toggles focus between OK and Cancel.
- `InputKind::BluetoothPasskey` accepts only `0-9`; other chars are
  ignored at the buffer-insert step.

Tests (targeted):

- `cargo test -p cyberdeck-tui --lib modal_secret_ok_cancel_button_renders`
- `cargo test -p cyberdeck-tui --lib modal_input_ok_cancel_button_renders`
- `cargo test -p cyberdeck-tui --lib bluetooth_passkey_rejects_letters`

### Module 3 — Bluetooth screen refactor

Files:

- `crates/tui/src/screens/bluetooth.rs` (rewrite the render + on_key).
- `crates/tui/src/app/action.rs` (add
  `RunAction::BluetoothPairWithPasskey(String, String)`).
- `crates/tui/src/main.rs` `spawn_action` arm for that variant — feeds
  `bluetoothctl pair <mac>` then on prompt pipes the passkey via stdin
  (for the spec, the core helper handles the prompt interaction; the
  TUI just dispatches the action).

Behaviour:

- Single left list of devices (sorted: connected → paired → unpaired,
  then by name). Each row: `<status-badge> <name> <mac> <rssi> [t]`.
- `j/k`/`Up/Down` move cursor.
- `p` on an unpaired device opens `Modal::Secret` with
  `InputKind::BluetoothPasskey` and the device MAC stashed in
  `pending_bt_mac`.
- `c` connect, `C` disconnect, `t` trust, `P` toggle adapter power (all
  unchanged).
- The right pane is a status pane: adapter state, scan progress,
  connected-device count, last scan time.

Tests:

- `cargo test -p cyberdeck-tui --lib screens::bluetooth::tests`
- `p_on_unpaired_device_opens_passkey_modal`
- `passkey_submit_dispatches_pair_with_passkey`
- `pair_with_passkey_run_action_carries_mac_and_pin`

### Module 4 — Files: in-TUI editor

Files:

- `crates/tui/src/screens/editor.rs` (new `EditorScreen`).
- `crates/tui/src/screens/files.rs` (add `e` key: read selected file →
  load into editor buffer → switch `current` to `ScreenId::Editor` and
  set `app.manager.set_pane_kind(WindowKind::Builtin(ScreenId::Editor))`).
- `crates/tui/src/app.rs` (add `ScreenId::Editor` variant + ALL entry;
  add `editor_path: PathBuf`, `editor_buffer: Vec<String>`,
  `editor_cursor: (usize, usize)`, `editor_dirty: bool`,
  `editor_read_only: bool`).
- `crates/tui/src/app/screen.rs` (`ScreenId::Editor` case in `label()`,
  `glyph()`, `ALL`).
- `crates/tui/src/main.rs` (register `EditorScreen` in the `screens`
  vector; switch the `1..9` / `0` digit map if the new entry shifts
  numbering — `Editor` is not on the sidebar; it's reached only from
  Files).

Behaviour:

- `EditorScreen::on_key`:
  - `Esc` → if dirty, open `Modal::Confirm { kind: ConfirmKind::Discard,
    arg: path }`; otherwise close (back to Files).
  - `Ctrl-S` → save (write `editor_buffer.join('\n')` to `editor_path`);
    if read-only, no-op and toast "read-only".
  - Typing / Backspace / Enter / arrow keys edit the buffer.
  - Read-only mode: ignores everything except `Esc` and read-only toast
    on any edit attempt.
- File read at entry:
  - Reject if file > 1 MiB → toast "file too large" + read-only.
  - Reject if binary (heuristic: > 5% non-printable bytes in first
    8 KiB) → toast "binary file" + read-only.
  - Otherwise load lines into `editor_buffer`.

Tests:

- `cargo test -p cyberdeck-tui --lib screens::editor::tests`
- `enter_into_editor_loads_text_file`
- `ctrl_s_writes_buffer_to_disk`
- `esc_on_dirty_opens_discard_confirm`
- `read_only_when_file_too_large`
- `read_only_when_binary`

### Module 5 — Sidebar / content layout cleanup

Files:

- `crates/tui/src/screens/network.rs`, `bluetooth.rs`, `editor.rs`
  (each uses the single-list-left + status-pane-right pattern).
- `crates/tui/src/screens/{system,power,display,audio,storage,services,
  packages,processes,files,logs,settings}.rs` (minor: drop any nested
  Layout splits > 2; ensure the right pane is a status block).
- `CONTRIBUTING.md` (new): "tests run in this project must always be
  targeted; never `cargo test` or `cargo test --workspace`".

Behaviour:

- Every screen renders `Layout::Horizontal([Percentage(60),
  Percentage(40)])` — left = list/form, right = status block.
- No more than one `Layout` split per screen.

Tests:

- `cargo test -p cyberdeck-tui --lib` (targeted module per module).

## Execution order

1. Network refactor → commit → push.
2. Modal OK/Cancel + BluetoothPasskey → commit → push.
3. Bluetooth refactor → commit → push.
4. Files editor → commit → push.
5. Sidebar cleanup + CONTRIBUTING → commit → push.
6. Final: `git push -u origin feature/tui-ux-improvements`, then
   `gh pr create --base main --head feature/tui-ux-improvements --fill`.

## Risks

- The new `ScreenId::Editor` is not in the sidebar (it's reached only
  from Files via `e`); this preserves the "single clean sidebar" goal
  while adding the editor.
- The WM is locked to a single pane. The editor is a regular screen, so
  no pane-state changes are required.
- Modal render change (Module 2) is purely visual; no keymap change.
- `cargo test` discipline: this design and the new CONTRIBUTING rule
  ban it. Per-module commits include targeted test runs only.