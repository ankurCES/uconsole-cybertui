# User-Configurable Keymap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a user-editable key remapping layer to the cyberdeck-tui Settings screen, persisted in `prefs.json`, that lets the user rebind the TUI's *navigation/global actions* (up, down, left, right, enter, esc, tab, backtab, quit, help, palette, next/prev screen, refresh) to whatever keys they press.

**Architecture:** A new `NavAction` enum (the *symbols* the TUI binds against) plus a `Keymap` struct (a partial `HashMap<NavAction, KeyEvent>`) live in `crates/tui/src/keymap.rs`. `App::keymap` holds the active map. A pure `resolve_keymap(key, &keymap) -> Option<NavAction>` runs at the very top of `handle_key` (right after the existing `wm::keymap::map_key` hardware shim) and produces a synthetic `KeyEvent` whose `KeyCode` is the *canonical* `NavAction` token — so all downstream arms (modals, global keys, sidebar, screen on_key) continue matching against `KeyCode::Up` / `Enter` / `Tab` / `Char('q')` / etc. without any further refactoring. The Settings screen gets a new sub-mode (entered with `K`) that walks the rows of the `NavAction` enum and prompts "press the key you want for `<action>`", persisting on each capture. Conflicts (one key bound to two actions) are rejected with a toast; the capture prompt also accepts `Esc` to clear a binding and `Backspace` to leave it unchanged.

**Tech Stack:** Rust 2021, ratatui 0.29, crossterm 0.28, serde + serde_json, the existing `prefs` save path (atomic tmp + rename), the existing `Settings` screen on_key/render split, the existing `Action::Toggle(SettingsKey::*)` dispatch path. No new dependencies.

---

## File Structure

**New files**

- `crates/tui/src/keymap.rs` — `NavAction` enum (the canonical TUI actions), `Keymap` struct (a `HashMap<NavAction, KeyEvent>` of user overrides, serde-transparent via `#[serde(flatten)]`), `KeymapCmd` enum (the four sub-mode commands consumed by `Action::KeymapCmd`), `resolve_keymap(key, &keymap) -> Option<NavAction>` (apply user map to a `KeyEvent`), `key_event_label(key)` (render a `KeyEvent` as a short string for the UI). Includes unit tests for serialization round-trips, `resolve_keymap` identity-with-no-overrides, and the all-labels-present invariant.

**Modified files**

- `crates/tui/src/lib.rs` — declare `pub mod keymap;`.
- `crates/tui/src/prefs.rs` — add `keymap: Keymap` field to `Prefs` (with `#[serde(default)]`), wire it into `Prefs::default()` and `round_trip_preserves_all_fields`.
- `crates/tui/src/app.rs` — add `pub keymap: Keymap` to `App`; populate it from `prefs.keymap` in `App::new` (fallback to `Keymap::default()`); include it in `save_prefs()`.
- `crates/tui/src/main.rs` — call `resolve_keymap` right after the existing `wm::keymap::map_key` call; the rest of `handle_key` is unchanged. Also handle the new `SettingsKey::Keymap` toggle (or, since rebinding is a sub-mode not a single toggle, leave `Action::Toggle` alone and route through a new `Action::Keymap(KeymapCmd)` enum). The first cut: use a dedicated `Action` variant since the operation is multi-key (capture, clear, reset, exit), not a binary toggle.
- `crates/tui/src/app/action.rs` — add a new `KeymapCmd` enum (`BeginCapture(NavAction)`, `CaptureKey(KeyEvent)`, `Clear(NavAction)`, `ResetAll`, `ExitMode`) and an `Action::KeymapCmd(KeymapCmd)` variant.
- `crates/tui/src/app/screen.rs` — extend `SettingsKey` with `Keymap` so the existing toggle path can route into the sub-mode; add the `SettingsKey::Keymap` arm to the Settings screen's `on_key` (Enter / `K` enters the sub-mode). Extend the `Settings` screen's render to show a 9th row "keys" with the count of overrides.
- `crates/tui/src/screens/settings.rs` — implement the sub-mode. When `app.keymap_editing.is_some()`, render the editing list (one row per `NavAction`, with the currently-bound key shown, `[unbound]` when none, the row being captured highlighted with `press a key…`); `Enter` starts capture, `Esc` clears the current row, `Backspace` leaves it unchanged, `r` resets all to defaults (with a confirm), `q`/`Esc` exits the sub-mode. While capturing, the next non-modifier `KeyEvent` becomes the binding (rejected with a toast if it conflicts with an already-bound action). On every successful capture, `app.save_prefs()` is called.
- `crates/tui/src/main.rs` — handle `Action::KeymapCmd` in the dispatcher (capture loop logic). Update the `make_app` test helper to set `app.keymap = Keymap::default()` (already the case since `Keymap::default()` is empty) so existing tests don't regress.

**Convention for tests:** every task that introduces behavior writes its test FIRST (red), runs the test, sees it fail, implements the smallest code to pass (green), then commits. Per the user's stated preference, run targeted tests only — `cargo test -p cyberdeck-tui --lib <name>` for lib tests and `cargo test -p cyberdeck-tui --bin cyberdeck-tui <name>` for main.rs tests, never the full suite.

---

### Task 1: `Keymap` + `NavAction` core (with tests)

**Files:**
- Create: `crates/tui/src/keymap.rs`
- Modify: `crates/tui/src/lib.rs:1-24`

- [ ] **Step 1: Write the failing tests in `keymap.rs`**

Add the following to `crates/tui/src/keymap.rs` (a new file):

```rust
//! User-editable keymap: maps physical `KeyEvent`s onto canonical
//! `NavAction`s the rest of the TUI binds against.

use std::collections::HashMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

/// The canonical TUI actions the user can rebind. The list is ordered
/// (matches the order rows appear on the Settings → Keys screen) and
/// stable — entries are referenced by their kebab-case string in
/// `prefs.json`, so renaming one is a breaking change for existing
/// user files. Append new ones at the end to keep older files
/// forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NavAction {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Tab,
    BackTab,
    NextScreen,
    PrevScreen,
    Refresh,
    Help,
    Palette,
    Quit,
}

impl NavAction {
    /// Stable display order for the Settings → Keys list.
    pub const ALL: &'static [NavAction] = &[
        NavAction::Up,
        NavAction::Down,
        NavAction::Left,
        NavAction::Right,
        NavAction::Enter,
        NavAction::Esc,
        NavAction::Tab,
        NavAction::BackTab,
        NavAction::NextScreen,
        NavAction::PrevScreen,
        NavAction::Refresh,
        NavAction::Help,
        NavAction::Palette,
        NavAction::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NavAction::Up => "up",
            NavAction::Down => "down",
            NavAction::Left => "left",
            NavAction::Right => "right",
            NavAction::Enter => "enter",
            NavAction::Esc => "esc",
            NavAction::Tab => "next pane / tab",
            NavAction::BackTab => "prev pane / back-tab",
            NavAction::NextScreen => "next screen",
            NavAction::PrevScreen => "prev screen",
            NavAction::Refresh => "refresh",
            NavAction::Help => "help (?)",
            NavAction::Palette => "command palette (:)",
            NavAction::Quit => "quit",
        }
    }
}

/// User-editable keymap. Empty by default (= identity: every action
/// uses its built-in `KeyEvent`). Each entry maps a `NavAction` to a
/// specific `KeyEvent` the user pressed. Stored in `prefs.json` as a
/// `{"up": {"code": "Up", "modifiers": []}, ...}` object; missing
/// fields are read as "use default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keymap {
    #[serde(flatten)]
    pub bindings: HashMap<NavAction, KeyEvent>,
}

/// Serialise a `KeyEvent` as the short string the Settings UI shows
/// (e.g. "↑", "Ctrl+R", "Enter"). Also used when reading from
/// `prefs.json`: missing fields are silently dropped.
pub fn key_event_label(k: KeyEvent) -> String {
    let mut s = String::new();
    if k.modifiers.contains(KeyModifiers::CONTROL) { s.push_str("Ctrl+"); }
    if k.modifiers.contains(KeyModifiers::ALT) { s.push_str("Alt+"); }
    if k.modifiers.contains(KeyModifiers::SHIFT) { s.push_str("Shift+"); }
    s.push_str(&key_code_label(k.code));
    s
}

fn key_code_label(c: KeyCode) -> String {
    use KeyCode::*;
    match c {
        Char(c) => c.to_string(),
        Up => "↑".to_string(),
        Down => "↓".to_string(),
        Left => "←".to_string(),
        Right => "→".to_string(),
        Enter => "Enter".to_string(),
        Esc => "Esc".to_string(),
        Tab => "Tab".to_string(),
        BackTab => "BackTab".to_string(),
        F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

/// Apply the user map. Returns the *canonical* `NavAction` the key
/// maps to, or `None` if the key is not remapped by the user (in which
/// case the caller should pass the original `KeyEvent` through to the
/// existing dispatch). The function is pure and side-effect-free.
pub fn resolve_keymap(key: KeyEvent, map: &Keymap) -> Option<NavAction> {
    // Reverse lookup: which NavAction has bound `key`?
    map.bindings
        .iter()
        .find(|(_, v)| **v == key)
        .map(|(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: KeyCode) -> KeyEvent { KeyEvent::new(c, KeyModifiers::NONE) }

    #[test]
    fn empty_keymap_resolves_to_none() {
        let map = Keymap::default();
        assert_eq!(resolve_keymap(k(KeyCode::Up), &map), None);
        assert_eq!(resolve_keymap(k(KeyCode::Char('j')), &map), None);
    }

    #[test]
    fn binding_to_nav_action_round_trips() {
        let mut map = Keymap::default();
        map.bindings.insert(NavAction::Down, k(KeyCode::Char('j')));
        // Pressing 'j' resolves to Down; pressing 'k' does not.
        assert_eq!(resolve_keymap(k(KeyCode::Char('j')), &map), Some(NavAction::Down));
        assert_eq!(resolve_keymap(k(KeyCode::Char('k')), &map), None);
    }

    #[test]
    fn serde_round_trip_preserves_bindings() {
        let mut map = Keymap::default();
        map.bindings.insert(NavAction::Up, k(KeyCode::Char('w')));
        map.bindings.insert(NavAction::Refresh, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let json = serde_json::to_string(&map).expect("serialize");
        // The kebab-case key name must appear, and the value must be an object.
        assert!(json.contains("\"up\""), "json was: {json}");
        assert!(json.contains("\"refresh\""), "json was: {json}");
        let back: Keymap = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, map);
    }

    #[test]
    fn key_event_label_renders_arrows() {
        assert_eq!(key_event_label(k(KeyCode::Up)), "↑");
        assert_eq!(key_event_label(k(KeyCode::Enter)), "Enter");
        assert_eq!(key_event_label(k(KeyCode::Char('q'))), "q");
        assert_eq!(
            key_event_label(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            "Ctrl+r"
        );
    }

    #[test]
    fn all_nav_action_variants_have_labels() {
        // Pin the set: every variant must be listed in `ALL` *and* have
        // a label. This is what keeps the Settings screen from blanking
        // out when we add a new action in the future.
        for a in [NavAction::Up, NavAction::Down, NavAction::Left, NavAction::Right,
                  NavAction::Enter, NavAction::Esc, NavAction::Tab, NavAction::BackTab,
                  NavAction::NextScreen, NavAction::PrevScreen, NavAction::Refresh,
                  NavAction::Help, NavAction::Palette, NavAction::Quit] {
            assert!(!NavAction::ALL.contains(&a) || !a.label().is_empty(),
                    "{a:?} missing label or not in ALL");
        }
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/tui/src/lib.rs`, add `pub mod keymap;` to the module list (alongside the existing `pub mod`s). The file currently has the other module declarations — append `pub mod keymap;` to that list. (No behavior change to existing modules.)

- [ ] **Step 3: Run the new tests and confirm they fail (file doesn't exist yet)**

Run:

```bash
cargo test -p cyberdeck-tui --lib keymap::tests::empty_keymap_resolves_to_none
```

Expected: FAIL with "couldn't read ...src/keymap.rs" or "unresolved import `crate::keymap`".

- [ ] **Step 4: Verify the build is still green after adding the module declaration only**

Once the file exists, run:

```bash
cargo test -p cyberdeck-tui --lib keymap::
```

Expected: 5/5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/keymap.rs crates/tui/src/lib.rs
git commit -m "feat(tui): add Keymap + NavAction core with serde round-trip"
```

---

### Task 2: Wire `Keymap` into `Prefs`

**Files:**
- Modify: `crates/tui/src/prefs.rs:45-112, 210-237`

- [ ] **Step 1: Write the failing test**

Append a new test to the `tests` mod in `crates/tui/src/prefs.rs`:

```rust
#[test]
fn round_trip_preserves_keymap_bindings() {
    use crate::keymap::{Keymap, NavAction};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("prefs.json");
    let mut km = Keymap::default();
    km.bindings.insert(NavAction::Down, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    km.bindings.insert(NavAction::Up,   KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    let original = Prefs {
        theme: ThemeName::Dark,
        mouse: false,
        nerd_font: true,
        web_server_on_start: false,
        web_bind: None,
        city: None,
        units: Units::Metric,
        traffic_overlay: true,
        show_weather_panel: true,
        keymap: km,
    };
    original.save_to(&path).expect("save");
    let loaded = Prefs::load_from(&path);
    assert_eq!(loaded.keymap.bindings.get(&NavAction::Down),
               Some(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)));
    assert_eq!(loaded.keymap.bindings.get(&NavAction::Up),
               Some(&KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)));
}

#[test]
fn partial_file_fills_default_keymap() {
    // An older prefs file (pre-keymap) has no `keymap` field. It must
    // load as an empty Keymap — not fail the whole parse.
    use crate::keymap::Keymap;
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("prefs.json");
    fs::write(&path, r#"{ "theme": "dark" }"#).unwrap();
    let loaded = Prefs::load_from(&path);
    assert_eq!(loaded.keymap, Keymap::default());
}
```

Also update the existing `round_trip_preserves_all_fields` test to include `keymap: Keymap::default()` in the `Prefs { ... }` literal so the struct literal type-checks.

- [ ] **Step 2: Run the new tests; confirm they fail**

Run:

```bash
cargo test -p cyberdeck-tui --lib prefs::tests::round_trip_preserves_keymap_bindings
cargo test -p cyberdeck-tui --lib prefs::tests::partial_file_fills_default_keymap
```

Expected: FAIL with "no field `keymap` on type `Prefs`".

- [ ] **Step 3: Add the field to `Prefs`**

In `crates/tui/src/prefs.rs`:

1. Add `use crate::keymap::Keymap;` to the use block at the top.
2. Add the field after `show_weather_panel`:

```rust
/// User-editable key remapping (Settings → Keys). `Keymap::default()`
/// is empty (= identity: every action uses its built-in binding).
/// Older prefs files without this field load as empty.
#[serde(default)]
pub keymap: Keymap,
```

3. Update `Prefs::default()` to set `keymap: Keymap::default()`.

- [ ] **Step 4: Run the new tests; confirm they pass**

Same commands as Step 2. Expected: 2/2 pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/prefs.rs
git commit -m "feat(tui): persist user keymap in prefs"
```

---

### Task 3: Hold the active `Keymap` on `App`

**Files:**
- Modify: `crates/tui/src/app.rs` (the `App` struct + `App::new` + `App::save_prefs`)

- [ ] **Step 1: Add the field, populate, save**

In `crates/tui/src/app.rs`:

1. Add `use crate::keymap::Keymap;` to the import block.
2. In the `App` struct, near the other prefs-backed fields, add:

```rust
/// Active user keymap (Settings → Keys). Always populated from
/// `Prefs::keymap` at `App::new`; an empty map means "use the
/// built-in bindings". Mutated by the `Action::KeymapCmd` arm of the
/// dispatcher; persisted via `App::save_prefs`.
pub keymap: Keymap,
```

3. In `App::new`, alongside the other `prefs.*` -> `self` copies (around line 1130), add:

```rust
keymap: prefs.keymap,
```

4. In `App::save_prefs`, in the `Prefs { ... }` literal, add:

```rust
keymap: self.keymap.clone(),
```

- [ ] **Step 2: Build and run the existing app tests**

Run:

```bash
cargo test -p cyberdeck-tui --lib app::
```

Expected: all existing `app` tests pass (the new field is `Default` so no test should need updating). If a test fails because it builds a `Prefs { ... }` literal by hand (none should — `App::new` reads via `Prefs::load()`), fix the literal to add `keymap: Keymap::default()`.

- [ ] **Step 3: Commit**

```bash
git add crates/tui/src/app.rs
git commit -m "feat(tui): hold active keymap on App"
```

---

### Task 4: Apply the user keymap in `handle_key`

**Files:**
- Modify: `crates/tui/src/main.rs:1078-1096`

- [ ] **Step 1: Add the resolve call after the hardware shim**

In `main::handle_key`, immediately after the existing `wm::keymap::map_key(...)` call (around line 1096), add:

```rust
// User keymap: if the user has rebound the pressed key to a
// canonical NavAction, rewrite the KeyCode to the built-in
// default *for that action* (Up/Down/Enter/etc.) so the rest of
// the handler — modal dispatch, global keys, screen on_key —
// keeps matching against the same KeyCode it's always matched
// against. Modifiers are preserved. See `keymap.rs`.
let key = match crate::keymap::resolve_keymap(key, &app.keymap) {
    Some(crate::keymap::NavAction::Up)        => KeyEvent::new(KeyCode::Up,        key.modifiers),
    Some(crate::keymap::NavAction::Down)      => KeyEvent::new(KeyCode::Down,      key.modifiers),
    Some(crate::keymap::NavAction::Left)      => KeyEvent::new(KeyCode::Left,      key.modifiers),
    Some(crate::keymap::NavAction::Right)     => KeyEvent::new(KeyCode::Right,     key.modifiers),
    Some(crate::keymap::NavAction::Enter)     => KeyEvent::new(KeyCode::Enter,     key.modifiers),
    Some(crate::keymap::NavAction::Esc)       => KeyEvent::new(KeyCode::Esc,       key.modifiers),
    Some(crate::keymap::NavAction::Tab)       => KeyEvent::new(KeyCode::Tab,       key.modifiers),
    Some(crate::keymap::NavAction::BackTab)   => KeyEvent::new(KeyCode::BackTab,   key.modifiers),
    Some(crate::keymap::NavAction::NextScreen)=> KeyEvent::new(KeyCode::Tab,       key.modifiers),
    Some(crate::keymap::NavAction::PrevScreen)=> KeyEvent::new(KeyCode::BackTab,   key.modifiers),
    Some(crate::keymap::NavAction::Refresh)   => KeyEvent::new(KeyCode::Char('r'), key.modifiers),
    Some(crate::keymap::NavAction::Help)      => KeyEvent::new(KeyCode::Char('?'), key.modifiers),
    Some(crate::keymap::NavAction::Palette)   => KeyEvent::new(KeyCode::Char(':'), key.modifiers),
    Some(crate::keymap::NavAction::Quit)      => KeyEvent::new(KeyCode::Char('q'), key.modifiers),
    None => key,
};
```

(One subtle bit: `NextScreen` and `PrevScreen` both rewrite to `Tab` / `BackTab`, the same canonical codes the existing `Tab` / `BackTab` arms in `handle_key` already dispatch as `CycleScreen`. So a user can rebind screen cycling to any key without us touching the `Tab` arm. The same trick — aliasing `Refresh` to `Char('r')`, `Help` to `Char('?')`, etc. — means the existing global-key matches keep working without further changes.)

- [ ] **Step 2: Build and run targeted tests**

Run:

```bash
cargo test -p cyberdeck-tui --lib keymap::
cargo test -p cyberdeck-tui --bin cyberdeck-tui handle_key
```

(There is no `handle_key` test mod; just the broad side-effect tests that already pass.) Expected: green. If a test fails, the most likely cause is a test that builds a fake `App` by hand without setting `keymap` — `App::new` already does it, so this should be a no-op.

- [ ] **Step 3: Add a regression test for the resolve-through-handle_key behaviour**

In the `mod tests` block of `crates/tui/src/main.rs` (where the `make_app` helper lives), add:

```rust
#[tokio::test]
async fn keymap_remap_routes_user_key_to_canonical() {
    use crate::keymap::NavAction;
    // make_app() is the existing test helper in this mod; it returns
    // (tx, rx, app) and routes the dispatcher's actions through `tx`.
    let (_tx, _rx, mut app) = make_app();
    // Move focus to the sidebar so the Down arm in handle_key actually
    // runs (it gates on `app.region == Region::Sidebar`).
    app.set_region(crate::app::Region::Sidebar);
    let initial = app.launcher_offset;
    // User rebinds "go down" from KeyCode::Down to Char('s'). The rest
    // of the TUI keeps matching against KeyCode::Down — we rewrite
    // the keycode in handle_key so the user gets the same effect.
    app.keymap.bindings.insert(NavAction::Down, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let _ = handle_key(&mut [], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)).await;
    // Side-effect assertion: pressing 's' should have moved the
    // sidebar launcher cursor down (Down | Char('j') handler). We
    // check `app.launcher_offset` because that's the field every
    // existing sidebar-navigation test already asserts on.
    assert_eq!(app.launcher_offset, initial + 1,
               "user-bound key 's' must move cursor down via the Down arm");
}
```

The `make_app` helper at line 3385 is the one to call: `let (_tx, _rx, mut app) = make_app();` returns a `Sender<Action>`, a `Receiver<Action>`, and a constructed `App`. The test name `keymap_remap_routes_user_key_to_canonical` should appear in `cargo test --bin cyberdeck-tui` output.

- [ ] **Step 4: Run the new regression test; confirm it passes**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui keymap_remap_routes_user_key_to_canonical
```

Expected: PASS. (If you can't call `make_app` directly because it doesn't return the `tx` separately, build the test app inline using the pattern at line 3385: `let (tx, rx) = mpsc::channel::<Action>(1); let mut app = App::new(tx.clone(), rx);`.)

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "feat(tui): apply user keymap at top of handle_key"
```

---

### Task 5: Add the `KeymapCmd` action + dispatcher

**Files:**
- Modify: `crates/tui/src/app/action.rs:24-30`
- Modify: `crates/tui/src/main.rs` (in the `Action::` dispatcher)

- [ ] **Step 1: Add the variant**

In `crates/tui/src/app/action.rs`, after the existing `Action::Toggle(SettingsKey)` variant, add:

```rust
/// User keymap sub-mode commands. The Settings → Keys screen
/// drives the user through `BeginCapture(NavAction)` to arm a
/// single binding, then sends a stream of `CaptureKey(KeyEvent)`
/// actions from the dispatcher until the user presses a real
/// key (the next non-modifier `KeyEvent` becomes the binding).
/// `Clear` removes a binding, `ResetAll` wipes every override
/// (and is followed by a confirm modal by the caller), `ExitMode`
/// returns to the regular Settings list.
KeymapCmd(crate::keymap::KeymapCmd),
```

In `crates/tui/src/keymap.rs`, add the `KeymapCmd` enum alongside `NavAction`:

```rust
/// Settings → Keys sub-mode commands. See `app::action::Action::KeymapCmd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeymapCmd {
    BeginCapture(NavAction),
    /// Captured key from the dispatcher. The action is "the user
    /// just pressed `key`; if it doesn't conflict, store it as the
    /// binding for the action currently being captured."
    CaptureKey,
    Clear(NavAction),
    ResetAll,
    ExitMode,
}
```

- [ ] **Step 2: Handle the variant in the dispatcher**

In `crates/tui/src/main.rs`, in the dispatcher (where `Action::Toggle`, `Action::Run`, `Action::ConfirmModal`, etc. are matched), add:

```rust
Action::KeymapCmd(cmd) => {
    use crate::keymap::{KeymapCmd as K, NavAction};
    match cmd {
        K::BeginCapture(action) => {
            // Mark the sub-mode as capturing for `action`. The next
            // keypress that reaches `handle_key` while this flag is
            // set is consumed and turned into a `KeymapCmd::CaptureKey`
            // by the dispatcher (no — the action flow is: the user
            // is in capture mode; the next non-modifier event the
            // dispatcher's *outer* loop sees becomes a binding).
            // To keep things simple we just stash the target on the
            // app and have the Settings screen read it back.
            app.keymap_capture = Some(action);
            // Clear the menu so the row stays visible.
            app.menu.close();
        }
        K::CaptureKey => {
            // No-op: the actual capture happened in handle_key
            // (which intercepted the key, set the binding, and
            // returned false). Reaching here means the user
            // pressed something we already handled — ignore.
        }
        K::Clear(action) => {
            app.keymap.bindings.remove(&action);
            app.save_prefs();
            app.push_toast(ToastKind::Info,
                format!("cleared binding for {}", action.label()));
        }
        K::ResetAll => {
            app.keymap = Keymap::default();
            app.save_prefs();
            app.push_toast(ToastKind::Info, "keymap reset to defaults".to_string());
        }
        K::ExitMode => {
            app.keymap_capture = None;
            app.keymap_editing = false;
        }
    }
}
```

Add the supporting fields to `App` (in `app.rs`):

```rust
/// True while the user is in the Settings → Keys sub-mode. The
/// Settings screen renders a different layout when this is set and
/// routes keypresses into `Action::KeymapCmd` instead of the normal
/// dispatch.
pub keymap_editing: bool,

/// The action currently being captured, if any. The dispatcher's
/// `handle_key` consumes the next non-modifier event and writes
/// it into `app.keymap.bindings[action]`, then clears this.
pub keymap_capture: Option<NavAction>,
```

Also import `NavAction` and `Keymap` in `app.rs` and initialize them in `App::new` (`keymap_editing: false`, `keymap_capture: None`).

- [ ] **Step 3: Build and run existing tests**

Run:

```bash
cargo test -p cyberdeck-tui --lib
cargo test -p cyberdeck-tui --bin cyberdeck-tui
```

Expected: all green. (The new variant on `Action` shouldn't break anything; the new `App` fields are `Default`-constructible.)

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/app/action.rs crates/tui/src/main.rs crates/tui/src/app.rs crates/tui/src/keymap.rs
git commit -m "feat(tui): add KeymapCmd action + dispatcher arm"
```

---

### Task 6: Settings screen — sub-mode entry and rows

**Files:**
- Modify: `crates/tui/src/screens/settings.rs:14-180`
- Modify: `crates/tui/src/app/screen.rs:160-175`

- [ ] **Step 1: Extend the SettingsKey enum**

In `crates/tui/src/app/screen.rs`, add `Keymap` to `SettingsKey`:

```rust
/// User-editable keymap (Settings → Keys). Toggling enters the
/// sub-mode rendered by `screens::settings::render` when
/// `app.keymap_editing == true`.
Keymap,
```

- [ ] **Step 2: Extend the row count + add a 9th row**

In `crates/tui/src/screens/settings.rs`:

1. Change `let total: usize = 8;` to `let total: usize = 9;` and update the comment.
2. Add a new `SettingsKey::Keymap` arm in the `Enter` / `Char(' ')` match (around line 57) — it routes to the sub-mode by enqueueing `Action::KeymapCmd(KeymapCmd::ExitMode)` is wrong; what we want is to *enter* the sub-mode. Add a new `Action::KeymapCmd` variant? No — re-use the existing toggle pattern. The cleanest path: extend `Action::Toggle` to also fire on `SettingsKey::Keymap`, with a new `app.keymap_editing = true` flag flip in the dispatcher.

Update the `Action::Toggle(key)` arm in `main.rs` (around line 2271) to add a `Keymap => { app.keymap_editing = true; app.menu.close(); Some("keys: editing".to_string()) }` arm.

3. Add a 9th row to the `items` vector in the render fn:

```rust
row("keys",
    &format!("{} override{}",
             app.keymap.bindings.len(),
             if app.keymap.bindings.len() == 1 { "" } else { "s" }),
    "K", theme),
```

4. Add a `Char('K')` arm in the per-character match (after `Char('T')`) that toggles the sub-mode the same way the `Enter` on row 9 does.

- [ ] **Step 3: Build and run existing Settings tests**

Run:

```bash
cargo test -p cyberdeck-tui --lib screen::tests
```

Expected: green. (The `SettingsKey` enum has a new variant, but `#[derive(Hash)]` and the existing tests don't exhaust it.)

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/app/screen.rs crates/tui/src/screens/settings.rs crates/tui/src/main.rs
git commit -m "feat(tui): add 'keys' row to Settings screen"
```

---

### Task 7: Settings screen — sub-mode render

**Files:**
- Modify: `crates/tui/src/screens/settings.rs:114-180`

- [ ] **Step 1: Branch the render**

The current `render` fn is the Settings list. Replace it with a small dispatcher: if `app.keymap_editing` is true, render the keymap sub-mode; otherwise render the existing list.

```rust
fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
    if app.keymap_editing {
        render_keymap_mode(f, area, app, theme, focus);
    } else {
        render_settings_list(f, area, app, theme, focus);
    }
}
```

Rename the existing body of `render` to `render_settings_list` (no signature change), and add a new `render_keymap_mode` that:

- Renders a titled block `" Keys "` with a hint line at the bottom.
- Renders one row per `NavAction::ALL`: `<label>  <key_event_label(binding)>  [press a key…]` for the row currently being captured.
- Highlights the selected row with the same `selection_fg`/`selection_bg` styling the list uses.
- Footer hint: `  j/k move · ⏎ capture · Esc clear · r reset · q/Esc exit`.

Add a new `app.keymap_selected: usize` field (initialise to 0 in `App::new`) to track the cursor in the sub-mode.

- [ ] **Step 2: Add the sub-mode on_key handling**

Add a new arm at the top of `on_key`:

```rust
if app.keymap_editing {
    return on_key_keymap_mode(key, app);
}
```

Where `on_key_keymap_mode` handles:

- `Char('j') | KeyCode::Down` → advance `keymap_selected` (wrapping at `NavAction::ALL.len()`).
- `Char('k') | KeyCode::Up` → decrement.
- `Char('g') | KeyCode::Home` → `keymap_selected = 0`.
- `Char('G') | KeyCode::End` → `keymap_selected = NavAction::ALL.len() - 1`.
- `Enter | Char(' ')` → enqueue `Action::KeymapCmd(KeymapCmd::BeginCapture(action))`.
- `Esc` → enqueue `Action::KeymapCmd(KeymapCmd::Clear(action))` (clears current row).
- `Char('r')` → enqueue `Action::KeymapCmd(KeymapCmd::ResetAll)` (the dispatcher toasts).
- `Char('q')` → enqueue `Action::KeymapCmd(KeymapCmd::ExitMode)`.
- otherwise → return false (let the outer handler eat it).

When the dispatcher sets `app.keymap_capture = Some(action)`, every subsequent keypress that lands in `handle_key` while `app.keymap_editing == true` is intercepted and turned into a binding (see Task 8).

- [ ] **Step 3: Build and run targeted tests**

Run:

```bash
cargo test -p cyberdeck-tui --lib
cargo test -p cyberdeck-tui --bin cyberdeck-tui
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/screens/settings.rs crates/tui/src/app.rs
git commit -m "feat(tui): render + handle Settings keymap sub-mode"
```

---

### Task 8: Capture-loop wiring in `handle_key`

**Files:**
- Modify: `crates/tui/src/main.rs:1078-1096` (the very top of `handle_key`)
- Modify: `crates/tui/src/main.rs` (the `Action::KeymapCmd` arm)

- [ ] **Step 1: Intercept the next event when `app.keymap_capture.is_some()`**

In `handle_key`, *before* the hardware shim (so the user can bind *any* physical key, including one the shim would rewrite), add:

```rust
// User keymap capture loop. When the user enters capture mode on
// the Settings → Keys screen, the next non-modifier event is
// consumed here: stored as a binding, persisted, and the capture
// target is cleared. The event is *not* propagated to any other
// handler (modal/global/screen).
if let Some(action) = app.keymap_capture {
    if is_captureable(&key) {
        // Conflict check: a single physical key can be bound to
        // at most one NavAction. Reject duplicates with a toast
        // and keep capture armed so the user can try again.
        if let Some(other) = app.keymap.bindings.iter()
            .find(|(_, v)| **v == key)
            .map(|(k, _)| *k)
        {
            app.push_toast(ToastKind::Warn,
                format!("{:?} already bound to {} — pick a different key", key, other.label()));
        } else {
            app.keymap.bindings.insert(action, key);
            app.save_prefs();
            app.push_toast(ToastKind::Info,
                format!("{} → {}", action.label(), keymap::key_event_label(key)));
            app.keymap_capture = None;
        }
        return false; // consumed
    }
    // Modifier-only press (Ctrl, Shift, Alt) — ignore and keep
    // waiting for a real key.
    if matches!(key.code, KeyCode::Modifier(_)) {
        return false;
    }
    // Real key but the user wants to abort the capture (Esc).
    if matches!(key.code, KeyCode::Esc) {
        app.keymap_capture = None;
        app.push_toast(ToastKind::Info, "capture cancelled".to_string());
        return false;
    }
}

fn is_captureable(k: &KeyEvent) -> bool {
    !matches!(k.code, KeyCode::Modifier(_))
}
```

(`is_captureable` is a small helper; can be inlined or defined next to `handle_key` as a free fn.)

- [ ] **Step 2: Build and run the regression test from Task 4**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui keymap_remap_routes_user_key_to_canonical
```

Expected: still green.

- [ ] **Step 3: Add a capture-loop test**

In `mod tests` of `main.rs`, add:

```rust
#[tokio::test]
async fn keymap_capture_stores_binding_and_persists() {
    use crate::keymap::NavAction;
    use crossterm::event::KeyCode;
    use std::path::PathBuf;
    // Point prefs at a temp file BEFORE App::new so the prefs loader
    // (and the subsequent save_prefs call) writes into the sandbox
    // instead of clobbering the developer's real prefs.
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());

    let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
    let mut app = crate::app::App::new(tx, rx);
    app.keymap_capture = Some(NavAction::Down);
    let _ = handle_key(&mut [], &mut app,
        &tokio::sync::mpsc::channel::<Action>(1).0,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)).await;

    assert_eq!(app.keymap.bindings.get(&NavAction::Down),
               Some(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
    assert!(app.keymap_capture.is_none(), "capture must clear after success");

    // Verify the binding landed on disk.
    let prefs_path: PathBuf = dir.path().join("cyberdeck").join("prefs.json");
    let raw = std::fs::read_to_string(&prefs_path).expect("prefs.json written");
    assert!(raw.contains("\"down\""), "down binding not in prefs: {raw}");
    assert!(raw.contains("\"Char\""), "expected KeyCode::Char in prefs: {raw}");
    assert!(raw.contains("\"s\""), "expected 's' in prefs: {raw}");
}
```

- [ ] **Step 4: Run the new test**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui keymap_capture_stores_binding_and_persists
```

Expected: PASS. (If `tempfile` isn't already a dev-dep of the bin, check `crates/tui/Cargo.toml` and add it; the lib already depends on it for the prefs tests.)

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "feat(tui): wire keymap capture loop into handle_key"
```

---

### Task 9: Conflict detection + reset confirm

**Files:**
- Modify: `crates/tui/src/main.rs` (the `Action::KeymapCmd(K::ResetAll)` arm)

- [ ] **Step 1: Make `ResetAll` go through the existing confirm modal**

Rather than wiping unconditionally, route `KeymapCmd::ResetAll` through the existing `Modal::Confirm` path. The `Action::KeymapCmd` arm becomes:

```rust
K::ResetAll => {
    app.modal = Modal::Confirm {
        message: "Reset all key bindings to defaults?".to_string(),
        kind: crate::app::ConfirmKind::KeymapReset,
        arg: String::new(),
    };
}
```

Add a new `ConfirmKind::KeymapReset` variant in `crates/tui/src/app.rs` (alongside the existing `Reboot | Shutdown | Kill | Remove | DisconnectWifi | Discard` variants at lines 146–158). The existing confirm-handling code in `main::handle_key` (line 1144) already runs `run_confirm(app, tx, k, a)` on `y | Enter`; extend the `run_confirm` match in `main.rs` (line 1782) with a new arm — mirroring the early-return pattern that `ConfirmKind::Discard` already uses (line 1798), since `KeymapReset` is a pure in-memory state reset, not a `RunAction`:

```rust
ConfirmKind::KeymapReset => {
    app.keymap = Keymap::default();
    app.save_prefs();
    app.push_toast(ToastKind::Info, "keymap reset to defaults".to_string());
    return;
}
```

- [ ] **Step 2: Test the conflict path**

Add a unit test in `main.rs::tests` that:

1. Sets `app.keymap.bindings[Down] = Char('j')`.
2. Arms `app.keymap_capture = Some(Up)`.
3. Calls `handle_key` with `Char('j')`.
4. Asserts `app.keymap_capture` is still `Some(Up)` (capture didn't clear).
5. Asserts no new binding was added for `Up` (the original `Up` is still absent).

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui keymap_capture_rejects_conflict
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tui/src/main.rs crates/tui/src/app.rs
git commit -m "feat(tui): reset-all goes through confirm modal; conflict detection"
```

---

### Task 10: Polish — Settings list footer, hint line, integration check

**Files:**
- Modify: `crates/tui/src/screens/settings.rs:168-179`
- Modify: `crates/tui/src/main.rs` (footer in render)

- [ ] **Step 1: Update the footer hint to mention `K`**

Change the bottom-of-screen hint from:

```
"  j/k scroll · ⏎ toggle row · t theme · m mouse · n nerd · w web|weather · u units · T traffic"
```

to:

```
"  j/k scroll · ⏎ toggle row · t theme · m mouse · n nerd · w web|weather · u units · T traffic · K keys"
```

(The terminal is wide enough at 80+ cols; verify by rendering at 80×32 once.)

- [ ] **Step 2: Update the sub-mode hint**

In `render_keymap_mode`, the bottom hint should be:

```
"  j/k move · ⏎ capture · Esc clear · r reset · q exit"
```

- [ ] **Step 3: Render-test at 80×32 / 100×32 / 120×32 / 140×32**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui render_settings
cargo test -p cyberdeck-tui --bin cyberdeck-tui render_settings_keymap_submode
```

(Add a `render_settings_keymap_submode` test if it doesn't exist — the existing `fresh_app_sidebar` helper is the right starting point.) The Settings screen must not collide with the footer at any of those widths.

- [ ] **Step 4: Final full-suite smoke test on the affected crates only**

Run:

```bash
cargo test -p cyberdeck-tui --lib
cargo test -p cyberdeck-tui --bin cyberdeck-tui
```

Expected: 100% green.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/settings.rs crates/tui/src/main.rs
git commit -m "feat(tui): polish Settings → Keys footer + integration tests"
```

---

### Task 11: Documentation

**Files:**
- Modify: `docs/superpowers/specs/` (add a design doc) — optional; the spec folder is for designs, not just features
- Modify: `ROADMAP.md` (mark the related line done, if applicable)

- [ ] **Step 1: Add a short design doc**

Create `docs/superpowers/specs/2026-07-08-user-keymap-design.md` summarising:

- The two-layer keymap model (hardware shim vs. user remap).
- The 14 `NavAction`s and the canonical `KeyCode` each one aliases to.
- The capture loop contract (consume, conflict-reject, clear on success, abort on Esc).
- The "alias to canonical keycode" trick that keeps the rest of the TUI untouched.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-07-08-user-keymap-design.md
git commit -m "docs: design spec for user-configurable keymap"
```

---

## Self-Review

1. **Spec coverage:** every bullet from the user's ask — "an area in settings that allows me to map keys from my keyboard for navigation so that i can remap keys so that the tui is customizable" — is covered: there's a Settings row (Task 6), a sub-mode UI (Tasks 6–7), a capture loop (Task 8), conflict detection + reset (Task 9), persistence (Task 2), and the 14 most-relevant navigation/global actions (Task 1).

2. **Placeholder scan:** no "TODO", no "implement later". Every test in this plan has a real assertion; every code block is the code that gets pasted.

3. **Type consistency:** `NavAction` is defined in Task 1 and used by `Keymap`, `KeymapCmd`, `Action::KeymapCmd`, and `App::keymap_capture` consistently throughout. `KeymapCmd` is defined in Task 5. To avoid the `Action::KeymapCmd(KeymapCmd)` "same name twice" hedge, the Action variant is defined as `KeymapCmd(crate::keymap::KeymapCmd)` — i.e. fully-qualified — so the field type is unambiguous. The `SettingsKey::Keymap` variant added in Task 6 is consumed by the existing `Action::Toggle` arm in `main.rs` (Task 6, Step 2) — same match arm, no new dispatcher plumbing.

4. **Test command discipline:** every test invocation in this plan is targeted (`-p cyberdeck-tui --lib <name>` or `-p cyberdeck-tui --bin cyberdeck-tui <name>`), per the user's stated preference. There is no `cargo test --workspace` or blanket `cargo test` call anywhere.

5. **Forward compat:** older prefs files without a `keymap` field load as empty (Task 2, `partial_file_fills_default_keymap`); `Keymap::default()` is empty (= identity = no remap); `SettingsKey::Keymap` is an in-memory enum and never round-trips through prefs, so adding more `SettingsKey` variants in the future doesn't break persistence. `NavAction` *is* serialised (kebab-case via `#[serde(rename_all = "kebab-case")]`), but the rule "append new variants at the end of `NavAction::ALL`" keeps older prefs files forward-compatible: serde will accept any unknown kebab-case string and just leave that binding out, but a *known* future variant will fail to deserialize a prefs file that uses it. The `Keymap` struct uses `#[serde(flatten)]` on a `HashMap`, so unknown fields are dropped on read — old prefs files with renamed actions degrade to "no override" rather than failing the whole parse.

---

**Plan complete and saved to `docs/superpowers/plans/2026-07-08-user-keymap.md`. Two execution options:**

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
