# Sub-Screen Esc Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reorder the `handle_key` dispatcher so that sub-screens can claim `Esc` for their own "back" action (e.g. Files = "go up a folder"), and add a dedicated `B` shortcut for "back to launcher". Sub-screens that don't add an `Esc` arm keep the existing behavior.

**Architecture:** One structural change in `main::handle_key` (swap steps 7 and 8 of the dispatch order) and a handful of per-screen `Esc` arms. No new modules, no new types, no new dependencies. The `App::region` model is unchanged; the `B` shortcut reuses `app.set_region(Region::Sidebar)`.

**Tech Stack:** Rust 2021, the existing `cyberdeck-tui` crate, the existing `Screen` trait, the existing `handle_key` dispatcher.

---

## File Structure

**Modified files**
- `crates/tui/src/main.rs` — reorder `handle_key`, add `B` shortcut, update help hint, add 3 new tests in `mod tests`.
- `crates/tui/src/screens/files.rs` — add `Esc` arm to `FilesScreen::on_key`.
- `crates/tui/src/screens/editor.rs` — add `Esc` arm to `EditorScreen::on_key` (close the editor).
- `crates/tui/src/screens/logs.rs` — add `Esc` arm to `LogsScreen::on_key` (dismiss active filter).

**No new files. No new modules.**

---

### Task 1: Reorder `handle_key` so the screen sees Esc first

**Files:**
- Modify: `crates/tui/src/main.rs:1428-1651` (the section after the modals, around the menu-bar and region-nav blocks)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block of `crates/tui/src/main.rs` (find an existing render/handle_key test for the exact style; the existing `esc_from_sidebar_goes_to_content` test around line 3044 is a good model):

```rust
#[tokio::test]
async fn esc_in_content_dispatches_to_screen_on_key_first() {
    use crate::screens::files::FilesScreen;
    use crate::app::ScreenId;
    use crate::app::Region;
    // Wire the Files screen so screen.on_key sees the Esc.
    let (_tx, _rx, mut app) = make_app();
    app.current = ScreenId::Files;
    app.set_region(Region::ContentLeft);
    // Pre-set a non-root cwd so the Files screen has a parent to go to.
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    app.files_cwd = nested.clone();

    let _ = handle_key(&mut [Box::new(FilesScreen)], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await;

    // After this change, the screen should claim Esc and go up a folder.
    // The exact assertion depends on what the screen's Esc arm does; we
    // assert here that the launcher did NOT take it (region unchanged).
    assert_eq!(app.region, Region::ContentLeft,
               "screen should claim Esc; region should stay in content");
}
```

(The test uses the existing `handle_key` signature; if `handle_key` doesn't take a `&mut [Box<dyn Screen>]` parameter, adjust to whatever it actually takes. The point of the test is to assert that the region is *not* changed by a launcher "back" action — i.e. the screen's `on_key` consumed the Esc.)

- [ ] **Step 2: Run the test, confirm it fails**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in_content_dispatches_to_screen_on_key_first
```

Expected: FAIL. Today the launcher eats Esc and the region changes to Sidebar. After the reorder, the screen sees it first and the region stays ContentLeft.

- [ ] **Step 3: Reorder the dispatch in `handle_key`**

In `crates/tui/src/main.rs`, the `handle_key` function currently has the region-nav step (the "B/Esc" block) at lines ~1550-1651, *before* the screen `on_key` call. The screen `on_key` call is at the end of the function (after the region-nav block).

Move the region-nav "Esc → leave to sidebar" handler (the one at line 1645-1651, the `Esc if app.region == Region::Sidebar` arm and the launcher `Char('b')` fallback if any) to run **after** the screen's `on_key` call. The screen `on_key` call stays where it is (it's currently the last step before `false` falls through to "not handled").

Concretely: cut the launcher-Esc arm from its current position and paste it immediately *after* the screen `on_key` call. If the current code uses a single `match` statement for the region nav, **do not** restructure the match — instead, **delete** the `Esc if app.region == Region::Sidebar` arm from its current `match` and re-add a new arm (or a separate `if` block) at the bottom of `handle_key` that handles the *fall-through* case: "if we got here, no screen claimed the Esc, so the launcher takes it."

The result: any screen whose `on_key` returns `true` claims Esc; the rest fall through to the launcher.

- [ ] **Step 4: Run the new test, confirm it passes**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in_content_dispatches_to_screen_on_key_first
```

Expected: PASS.

- [ ] **Step 5: Run the existing sidebar-Esc test, confirm it still passes**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_from_sidebar
```

Expected: PASS. (The sidebar-Esc handler still fires for `region == Sidebar`; it just moved position in the file.)

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "refactor(tui): reorder handle_key so screen.on_key sees Esc first"
```

---

### Task 2: Add `Esc` arm to `FilesScreen` (go up a folder)

**Files:**
- Modify: `crates/tui/src/screens/files.rs:25-67` (the `on_key` match)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block of `crates/tui/src/main.rs` (so it has access to `make_app`, `handle_key`, etc.):

```rust
#[tokio::test]
async fn esc_in_files_goes_up_a_folder() {
    use crate::screens::files::FilesScreen;
    use crate::app::ScreenId;
    use crate::app::Region;
    let (_tx, _rx, mut app) = make_app();
    app.current = ScreenId::Files;
    app.set_region(Region::ContentLeft);
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    app.files_cwd = nested.clone();

    let _ = handle_key(&mut [Box::new(FilesScreen)], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await;

    assert_eq!(app.files_cwd, nested.parent().unwrap().to_path_buf(),
               "Esc should go up one folder");
    assert_eq!(app.region, Region::ContentLeft,
               "screen claimed Esc; region should stay in content");
}
```

- [ ] **Step 2: Run the test, confirm it fails**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in_files_goes_up_a_folder
```

Expected: FAIL. `app.files_cwd` is unchanged because `FilesScreen::on_key` has no `Esc` arm.

- [ ] **Step 3: Add the `Esc` arm to `FilesScreen::on_key`**

In `crates/tui/src/screens/files.rs`, add a new arm in the `on_key` `match` (after the `Char('h') | KeyCode::Left` arm at line 59):

```rust
KeyCode::Esc => {
    // Same as Char('h') / Left — go up a folder. Returning true
    // claims the key so the launcher doesn't get it. If we're
    // already at the filesystem root (no parent), return false so
    // the launcher takes Esc and moves focus to the sidebar.
    if let Some(parent) = app.files_cwd.parent() {
        app.files_cwd = parent.to_path_buf();
        app.files_selected = 0;
        true
    } else {
        false
    }
}
```

- [ ] **Step 4: Run the test, confirm it passes**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in_files_goes_up_a_folder
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/files.rs crates/tui/src/main.rs
git commit -m "feat(tui): Files screen claims Esc to go up a folder"
```

---

### Task 3: Add `Esc` arm to `FilesScreen` for the root case (fall through to launcher)

**Files:**
- Modify: `crates/tui/src/screens/files.rs` (the `Esc` arm added in Task 2)
- Test: `crates/tui/src/main.rs` (mod tests)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn esc_at_filesystem_root_falls_through_to_launcher() {
    use crate::screens::files::FilesScreen;
    use crate::app::ScreenId;
    use crate::app::Region;
    let (_tx, _rx, mut app) = make_app();
    app.current = ScreenId::Files;
    app.set_region(Region::ContentLeft);
    app.files_cwd = std::path::PathBuf::from("/");

    let _ = handle_key(&mut [Box::new(FilesScreen)], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await;

    // No parent — Files returned false, launcher took Esc.
    assert_eq!(app.region, Region::Sidebar,
               "at filesystem root, Esc should fall through to the launcher");
    assert_eq!(app.files_cwd, std::path::PathBuf::from("/"),
               "cwd should be unchanged when Esc falls through");
}
```

- [ ] **Step 2: Run the test**

The Task 2 code already returns `false` at root, so this test should PASS without any new code change. Run it to confirm:

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_at_filesystem_root_falls_through_to_launcher
```

Expected: PASS. (If it fails, the `Esc` arm in Task 2 needs a tweak — but the code we wrote already returns `false` at root, so it should be green.)

- [ ] **Step 3: Commit (test only)**

```bash
git add crates/tui/src/main.rs
git commit -m "test(tui): Files at root, Esc falls through to launcher"
```

---

### Task 4: Add `B` shortcut for "back to launcher"

**Files:**
- Modify: `crates/tui/src/main.rs` (the `handle_key` region-nav block, around the existing `Char('b')` handler)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn b_in_content_moves_to_launcher() {
    use crate::app::Region;
    let (_tx, _rx, mut app) = make_app();
    app.set_region(Region::ContentLeft);

    let _ = handle_key(&mut [], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)).await;

    assert_eq!(app.region, Region::Sidebar,
               "B from content should move focus to the launcher");
}

#[tokio::test]
async fn b_in_sidebar_is_noop() {
    use crate::app::Region;
    let (_tx, _rx, mut app) = make_app();
    app.set_region(Region::Sidebar);

    let _ = handle_key(&mut [], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)).await;

    assert_eq!(app.region, Region::Sidebar,
               "B in sidebar is a no-op");
}
```

- [ ] **Step 2: Run the test, confirm it fails**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui b_in_content_moves_to_launcher
```

Expected: FAIL. There's no production handler for `Char('b')` from content today.

- [ ] **Step 3: Add the `B` shortcut to `handle_key`**

In `crates/tui/src/main.rs`, find the region-nav block (after the menu-bar block, before the screen `on_key` call). Add a new arm for `Char('b') | Char('B')` from content regions:

```rust
Char('b') | Char('B') if matches!(app.region, Region::ContentLeft | Region::ContentRight) => {
    app.set_region(Region::Sidebar);
    return false;
}
Char('b') | Char('B') if app.region == Region::Sidebar => {
    // No-op — you're already at the launcher.
    return false;
}
```

(Place this in the same region-nav block where the other region-change arms live; it should run *before* the screen `on_key` call so a screen can't accidentally claim `B` if it didn't mean to.)

- [ ] **Step 4: Run the tests, confirm they pass**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui b_in_content_moves_to_launcher
cargo test -p cyberdeck-tui --bin cyberdeck-tui b_in_sidebar_is_noop
```

Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "feat(tui): B shortcut moves focus to launcher from any content region"
```

---

### Task 5: Add `Esc` arm to `EditorScreen` (close the editor)

**Files:**
- Modify: `crates/tui/src/screens/editor.rs:72-...` (the `on_key` method)

- [ ] **Step 1: Read the existing `EditorScreen::on_key` to see the current close-Editor affordance**

Look at `crates/tui/src/screens/editor.rs:72-...` and find how the editor is currently closed (likely a `Char('q')` arm or similar). The `Esc` arm should do the same thing but with `KeyCode::Esc` and the comment that it's the dedicated "back" key.

- [ ] **Step 2: Write the failing test**

Add to `mod tests` of `main.rs`:

```rust
#[tokio::test]
async fn esc_in_editor_closes_editor() {
    use crate::screens::editor::EditorScreen;
    use crate::app::ScreenId;
    use crate::app::Region;
    let (_tx, _rx, mut app) = make_app();
    app.current = ScreenId::Editor;
    app.set_region(Region::ContentLeft);
    // Assume the editor has an `editor_open: bool` or similar field
    // (or whatever the actual state field is — read editor.rs to find it).
    // For the test, set up the minimal state that "the editor is open".

    let _ = handle_key(&mut [Box::new(EditorScreen)], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await;

    // The assertion depends on the actual editor state field. After
    // reading editor.rs, replace this with the right invariant:
    // e.g. `assert!(!app.editor_open, "Esc should close the editor")`.
}
```

(Adapt the test to the actual field name and behavior of the editor. The point is to pin that Esc closes the editor.)

- [ ] **Step 3: Add the `Esc` arm to `EditorScreen::on_key`**

In `crates/tui/src/screens/editor.rs`, add a new arm at the top of the `on_key` `match`:

```rust
KeyCode::Esc => {
    // Dedicated "back" key — close the editor and return focus to
    // the launcher. (Same as the existing Char('q') close path,
    // if there is one — DRY: refactor the close into a helper if
    // the implementation is non-trivial.)
    // ... (call the same close-Editor logic) ...
    true
}
```

(If the existing close path is just `app.current = previous_screen; app.editor_open = false;`, mirror that. The important thing is that Esc does the same thing as the existing close affordance, and that the screen returns `true` to claim the key.)

- [ ] **Step 4: Run the test, confirm it passes**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in_editor_closes_editor
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/editor.rs crates/tui/src/main.rs
git commit -m "feat(tui): Editor screen claims Esc to close the editor"
```

---

### Task 6: Add `Esc` arm to `LogsScreen` (dismiss active filter)

**Files:**
- Modify: `crates/tui/src/screens/logs.rs:24-...` (the `on_key` method)

- [ ] **Step 1: Read `LogsScreen::on_key` to find the current filter field**

Look at `crates/tui/src/screens/logs.rs:24-...` and find how filters are currently set/cleared. The `Esc` arm should clear the filter if one is active, and return `false` if no filter is set (so the launcher takes Esc).

- [ ] **Step 2: Write the failing test**

```rust
#[tokio::test]
async fn esc_in_logs_clears_filter() {
    use crate::screens::logs::LogsScreen;
    use crate::app::ScreenId;
    use crate::app::Region;
    let (_tx, _rx, mut app) = make_app();
    app.current = ScreenId::Logs;
    app.set_region(Region::ContentLeft);
    // Set a filter (the actual field name depends on the implementation):
    app.logs_filter = Some("error".to_string());

    let _ = handle_key(&mut [Box::new(LogsScreen)], &mut app, &tokio::sync::mpsc::channel::<Action>(1).0,
                       KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await;

    assert!(app.logs_filter.is_none(), "Esc should clear the active filter");
}
```

(Adapt the field name to whatever `logs.rs` actually uses.)

- [ ] **Step 3: Add the `Esc` arm to `LogsScreen::on_key`**

In `crates/tui/src/screens/logs.rs`, add a new arm at the top of the `on_key` `match`:

```rust
KeyCode::Esc => {
    if app.logs_filter.is_some() {
        app.logs_filter = None;
        true            // claimed — launcher doesn't get it
    } else {
        false           // no filter active — let the launcher take Esc
    }
}
```

(Adapt the field name to whatever the actual logs filter field is.)

- [ ] **Step 4: Run the test, confirm it passes**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in_logs_clears_filter
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/screens/logs.rs crates/tui/src/main.rs
git commit -m "feat(tui): Logs screen claims Esc to clear the active filter"
```

---

### Task 7: Update the help hint

**Files:**
- Modify: `crates/tui/src/main.rs:560-580` (the help hint block)

- [ ] **Step 1: Update the help block**

In `crates/tui/src/main.rs`, find the help hint block (around line 560-580). Currently it has:

```rust
("esc", "leave to sidebar"),
```

Change it to two lines:

```rust
("esc", "back (sub-screen · or leave to launcher)"),
("b",   "back to launcher"),
```

- [ ] **Step 2: Run a render test to confirm the hint is shown**

Run:
```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui render
```

Expected: existing render tests pass; the help hint change is text-only and shouldn't break anything.

- [ ] **Step 3: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "docs(tui): help hint explains Esc/B roles"
```

---

### Task 8: Final smoke test

- [ ] **Step 1: Run all the new tests**

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_in
cargo test -p cyberdeck-tui --bin cyberdeck-tui b_in
cargo test -p cyberdeck-tui --bin cyberdeck-tui esc_at_filesystem
```

Expected: all PASS.

- [ ] **Step 2: Run the full lib + bin test suite for the affected crate**

```bash
cargo test -p cyberdeck-tui --lib
cargo test -p cyberdeck-tui --bin cyberdeck-tui
```

Expected: 100% green. No regressions.

- [ ] **Step 3: Commit (no-op — nothing to commit if all green)**

If there are no uncommitted changes, skip. If there are leftover comments or doc tweaks, commit them with `chore(tui): post-esc-reorder cleanup`.

---

### Task 9: Documentation

**Files:**
- Modify: `docs/superpowers/specs/2026-07-15-sub-screen-esc-design.md` (the spec from the brainstorming phase)

- [ ] **Step 1: Verify the spec is up to date**

The spec was written before the implementation. Re-read it and confirm:
- The dispatch order is the same as what the code does.
- The per-screen decisions are all implemented (Files, Editor, Logs).
- The "B shortcut" is documented.
- The "What does NOT change" section is still accurate.

- [ ] **Step 2: Commit the design doc if not already committed**

```bash
git add docs/superpowers/specs/2026-07-15-sub-screen-esc-design.md
git commit -m "docs: design spec for sub-screen Esc ownership"
```

(If the spec was already committed as part of the brainstorming phase, skip this step.)

---

## Self-Review

**1. Spec coverage:**
- ✅ "Innermost consumer wins" — Task 1
- ✅ Files Esc arm — Task 2
- ✅ Files at root falls through — Task 3
- ✅ B shortcut — Task 4
- ✅ Editor Esc arm — Task 5
- ✅ Logs Esc arm — Task 6
- ✅ Help hint update — Task 7
- ✅ Smoke test — Task 8
- ✅ Documentation — Task 9

**2. Placeholder scan:** No TBDs/TODOs. The Editor and Logs tasks read the existing code first (Step 1 of each) before writing the test, so the exact field names are discovered rather than assumed. The "Adapt the field name" lines in Tasks 5 and 6 are explicit pointers to the engineer, not placeholders.

**3. Type consistency:** All tests use the same `make_app` helper, the same `handle_key` signature, the same `KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)` pattern, and the same `KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)` for the B tests. `app.set_region(Region::Sidebar)` and `app.set_region(Region::ContentLeft)` use the existing accessor.

**4. Test discipline:** Every task that adds behavior writes a failing test first (TDD red), runs the test to confirm it fails, implements the change, runs the test to confirm it passes, then commits. Per the user's stated preference, every test invocation in this plan is targeted (`-p cyberdeck-tui --bin cyberdeck-tui <name>`), never the full workspace suite.

**5. Forward compat:** The reorder in Task 1 is the *only* structural change; it makes the system strictly more flexible. Sub-screens that don't add an Esc arm keep the current behavior (Esc → launcher). The B shortcut is additive; the existing launcher's B/Esc test still passes.
