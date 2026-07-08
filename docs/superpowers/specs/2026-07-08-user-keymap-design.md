# User-Configurable Keymap — Design

**Date:** 2026-07-08. Plan: [`2026-07-08-user-keymap`](../plans/2026-07-08-user-keymap.md) (Task 11).

> A second keymap layer, user-editable from Settings → Keys, that
> remaps 14 navigation/global actions to any physical key. Persisted
> to `prefs.json`, layered above the existing hardware-profile shim,
> invisible to every other TUI handler.

---

## 1. Two-layer keymap model

`main::handle_key` applies two distinct keymap layers, in order:

1. **Hardware-profile shim** (`wm::keymap`) — normalises non-keyboard
   hardware (UConsole buttons X/Y/A/B) and honours the
   `CYBERDECK_KEYMAP` env-var profile override. Runs first.
2. **User rebinding** (`keymap`, the new module) — applies the user's
   preferences. Runs second; rewrites rebound keys back to their
   *canonical* `KeyCode` so downstream handlers match what they
   always have (§4).

The two layers don't interact: the shim turns weird hardware into
ordinary keys; the user map personalises the resulting flow. Code
lives in `crates/tui/src/wm/keymap.rs` and
`crates/tui/src/keymap.rs` respectively.

## 2. The 14 `NavAction`s

`NavAction` is a `#[serde(rename_all = "kebab-case")]` enum
referenced by string in `prefs.json` (renaming a variant breaks
existing user files). The list is **append-only**; new variants go
at the end of both the enum and `NavAction::ALL` (the Settings
screen order).

| `NavAction`     | Canonical `KeyCode`  | What it triggers      |
| --------------- | -------------------- | --------------------- |
| `Up`            | `KeyCode::Up`        | arrow / pane movement |
| `Down`          | `KeyCode::Down`      | arrow / pane movement |
| `Left`          | `KeyCode::Left`      | arrow / pane movement |
| `Right`         | `KeyCode::Right`     | arrow / pane movement |
| `Enter`         | `KeyCode::Enter`     | confirm / open row    |
| `Esc`           | `KeyCode::Esc`       | close modal / cancel  |
| `Tab`           | `KeyCode::Tab`       | next pane             |
| `BackTab`       | `KeyCode::BackTab`   | prev pane             |
| `NextScreen`    | `KeyCode::Tab`       | next screen           |
| `PrevScreen`    | `KeyCode::BackTab`   | prev screen           |
| `Refresh`       | `KeyCode::Char('r')` | re-fetch current view |
| `Help`          | `KeyCode::Char('?')` | toggle help overlay   |
| `Palette`       | `KeyCode::Char(':')` | command palette       |
| `Quit`          | `KeyCode::Char('q')` | quit the TUI          |

`NextScreen`/`Tab` both alias to `KeyCode::Tab`;
`PrevScreen`/`BackTab` to `KeyCode::BackTab`. Either row's binding
delivers the same canonical key — handlers can't tell them apart,
which is the point (§4).

## 3. Capture loop contract

Settings → Keys: pressing Enter on a row sends
`Action::KeymapCmd(BeginCapture(action))`; the dispatch arm sets
`app.keymap_capture = Some(action)`, arming the capture gate at
the top of `handle_key`. The next non-modifier event:

1. **Conflict** (`Keymap::is_key_taken` for a *different* action):
   warn toast naming the action that already owns the key;
   capture stays armed.
2. **No conflict**: `Keymap::bind`, persist to `prefs.json`,
   info toast `<action> → <key>`, capture cleared.
3. **`Esc`**: cancelled, info toast, capture cleared.
4. **Modifier-only key** (`Shift`/`Ctrl`/`Alt` alone): silently
   consumed, capture stays armed.

After capture completes (success or cancel) the event is **not**
propagated: capture short-circuits ahead of modal dispatch, the
user-map rewrite, global keys, and the screen `on_key`. This is
what lets the user bind `Enter` (Settings' natural "open row" key)
without the screen re-opening itself mid-capture.

## 4. Alias-to-canonical trick

The architectural decision that keeps every other TUI handler
untouched. A user-bound key is rewritten back to its canonical
`KeyCode` **before** any other dispatcher sees it: every modal,
global-key arm, and screen `on_key` continues to match the same
`KeyCode`s it always has. No handler learns about user rebinding.

```
key: KeyEvent
  │
  ▼
wm::keymap::map_key        ◄── layer 1 (hardware shim)
  │
  ▼
resolve_keymap(key, &app.keymap) ◄── layer 2 (user)
  │  None → fall through, original key
  │  Some(action) → match → rewrite to canonical KeyCode
  ▼
rest of handle_key — unchanged, still matches canonical codes
```

The 14-arm match in `main::handle_key` is the only place the
`NavAction` type appears outside `keymap.rs` and `screens/settings.rs`.
Adding a new action is exactly one new arm there.

## 5. Persistence shape

`Keymap` wraps a `HashMap<NavAction, KeyEvent>` serialised via
`#[serde(flatten)]` as a flat object of kebab-case keys:

```json
{
  "down":    { "code": "Down",          "modifiers": "",         "kind": "Press", "state": "" },
  "refresh": { "code": { "Char": "r" }, "modifiers": "CONTROL",  "kind": "Press", "state": "" }
}
```

The verbose shape follows crossterm's externally-tagged enums (bare
strings for unit variants, `{"Char": "r"}` for tuple variants) and
`KeyModifiers` as a bitflags string. Round-trip is lossless. Missing
fields (pre-keymap `prefs.json`s) load as empty = identity, every
action uses its built-in binding.

## 6. Adding new `NavAction`s

Append-only. To add a rebindable action:

1. Append the variant to the `NavAction` enum.
2. Append the same variant to the end of `NavAction::ALL`.
3. Add a label to `NavAction::label()`.
4. Add an arm to the resolve match in `main::handle_key`.

That's it. The Settings screen iterates `NavAction::ALL`, so the
new row appears automatically; the all-variants-have-labels test
fails until step 3 lands.

## 7. Non-goals

Per-context bindings, modifier-only captures, and machine-portable
maps are all out of scope for v0 — the user map is one global,
key+modifier-only map, hand-editable in `prefs.json`.

## 8. Rollout

One commit, ships alongside Tasks 1-10: a new `crates/tui/src/keymap.rs`
module, the capture gate and resolve-arm match in `main::handle_key`,
the Settings → Keys sub-mode in `screens/settings.rs`, and the
`Action::KeymapCmd` glue. No migration; pre-keymap `prefs.json`s
load with an empty `keymap` (identity).
