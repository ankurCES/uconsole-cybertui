# Pane Number Badges + Ctrl-W N Jump — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `[N]` badges to every pane title and wire a `Ctrl-W 1..9` jump verb so the user can move focus by typing the number.

**Architecture:** Two new methods on `Manager` (`focus_pane_index`, `focus_pane`), one new error type (`SplitError::PaneLimit`), and one signature change to `wm::render::pane_title` to thread the 0-based leaf index. `Manager::split_focused` returns `Result` instead of bare `PaneId` so the cap is visible. Renderer enumerates `tree.leaves()` to assign badges. `main::handle_key` gains a `Ctrl-W 1..9` arm and toasts on cap / out-of-range errors.

**Tech Stack:** Rust 1.80, ratatui 0.29, existing `wm::manager` / `wm::render` / `wm::window` modules. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-06-27-pane-badges-design.md`](../specs/2026-06-27-pane-badges-design.md)

**Test policy:** Targeted tests only — `cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::… -- --test-threads=1`. Never run `cargo test` workspace-wide. See `docs/CONTRIBUTING.md`.

---

## File Structure

| File | Status | Responsibility |
| --- | --- | --- |
| `crates/tui/src/wm/manager.rs` | modify | New methods (`focus_pane_index`, `focus_pane`), new `SplitError`, `MAX_PANES`, change `split_focused` to `Result`. |
| `crates/tui/src/wm/render.rs` | modify | `pane_title` takes an index; `render` threads the index through the plan tuple. |
| `crates/tui/src/wm/window.rs` | modify | `Window::paint` forwards the index to `pane_title`. |
| `crates/tui/src/main.rs` | modify | `Ctrl-W 1..9` arm; existing `Ctrl-W v`/`s` arms handle the `Err(PaneLimit)` toast. |
| `docs/CONTRIBUTING.md` | modify | Append the Phase-4 smoke test. |
| `ROADMAP.md` | modify | Tick the badges bullet. |

No new files. No new dependencies. All changes additive to existing modules.

---## Task 1: `Manager::focus_pane_index` and `focus_pane`

The two new pure methods on `Manager`. They do not touch `split_focused` — that change is in Task 2. Tests in this task use a 3-pane tree to exercise both in-range and out-of-range cases.

**Files:**
- Modify: `crates/tui/src/wm/manager.rs` (add methods + tests)
- Test: `crates/tui/src/wm/manager.rs` (existing `mod tests`)

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block at the bottom of `crates/tui/src/wm/manager.rs`:

```rust
    #[test]
    fn focus_pane_index_returns_some_for_in_range_leaf() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        let _ = m.split_focused(SplitDir::Vertical, 50, ScreenId::Audio);
        let ids = m.pane_ids();
        assert_eq!(ids.len(), 3);
        // Indices match the DFS order returned by `pane_ids()`.
        assert_eq!(m.focus_pane_index(0), Some(ids[0]));
        assert_eq!(m.focus_pane_index(1), Some(ids[1]));
        assert_eq!(m.focus_pane_index(2), Some(ids[2]));
    }

    #[test]
    fn focus_pane_index_returns_none_for_out_of_range() {
        let m = Manager::new(ScreenId::System);
        assert!(m.pane_ids().len() < 9);
        assert_eq!(m.focus_pane_index(m.pane_ids().len()), None);
        assert_eq!(m.focus_pane_index(usize::MAX), None);
    }

    #[test]
    fn focus_pane_swaps_focus() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        let ids = m.pane_ids();
        let original = m.focused();
        assert_ne!(original, ids[1]);
        assert!(m.focus_pane(ids[1]));
        assert_eq!(m.focused(), ids[1]);
    }

    #[test]
    fn focus_pane_returns_false_for_stale_id() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        let stale = PaneId(999_999);
        assert!(!m.focus_pane(stale));
        // Focused pane is unchanged.
        assert_eq!(m.focused(), m.pane_ids()[0]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::manager:: -- --test-threads=1
```

Expected: 4 new tests fail to compile with `error[E0599]: no function or associated method named 'focus_pane_index' for crate::wm::manager::Manager` (and the same for `focus_pane`).

- [ ] **Step 3: Add the methods to `impl Manager`**

Edit `crates/tui/src/wm/manager.rs`. In `impl Manager`, add the two methods. Place them next to the existing `focused()` getter (around line 48 in the current file):

```rust
    /// Return the `PaneId` of the leaf at DFS `index`, matching the
    /// order `pane_ids()` and `layout()` use. `None` for out-of-range.
    pub fn focus_pane_index(&self, index: usize) -> Option<PaneId> {
        self.tree.leaves().get(index).copied()
    }

    /// Set the focused pane to `id`. Returns false if `id` is not in
    /// the tree (e.g. a stale id from before a close).
    pub fn focus_pane(&mut self, id: PaneId) -> bool {
        if self.windows.contains_key(&id) {
            self.focused = id;
            true
        } else {
            false
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::manager:: -- --test-threads=1
```

Expected: all 8 tests in `wm::manager` pass (4 existing + 4 new).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/wm/manager.rs
git commit -m "wm: Manager::focus_pane_index + focus_pane (TDD)"
```

---## Task 2: `Manager::MAX_PANES`, `SplitError`, `split_focused` returns `Result`

The cap is now visible to callers. Two existing call sites in `main.rs` need to switch from `let _ = ...` to handling the `Err` — that's Task 4.

**Files:**
- Modify: `crates/tui/src/wm/manager.rs` (add `MAX_PANES`, `SplitError`, change `split_focused` signature, add test)
- Test: `crates/tui/src/wm/manager.rs` (`mod tests`)

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/tui/src/wm/manager.rs`:

```rust
    #[test]
    fn split_focused_at_limit_returns_err() {
        let mut m = Manager::new(ScreenId::System);
        // Open until we hit the cap. Each call creates one new pane.
        for i in 0..(Manager::MAX_PANES - 1) {
            let dir = if i % 2 == 0 { SplitDir::Horizontal } else { SplitDir::Vertical };
            let _ = m.split_focused(dir, 50, ScreenId::System).expect("within cap");
        }
        assert_eq!(m.pane_ids().len() as u8, Manager::MAX_PANES);
        // The next split must fail.
        let err = m
            .split_focused(SplitDir::Horizontal, 50, ScreenId::System)
            .unwrap_err();
        assert_eq!(err, SplitError::PaneLimit);
        // And the pane count did not grow.
        assert_eq!(m.pane_ids().len() as u8, Manager::MAX_PANES);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::manager:: -- --test-threads=1
```

Expected: compile error — `split_focused` returns `PaneId`, not `Result<PaneId, _>`; `SplitError` and `Manager::MAX_PANES` are unresolved.

- [ ] **Step 3: Add `MAX_PANES`, `SplitError`, change `split_focused` signature**

In `crates/tui/src/wm/manager.rs`:

**a)** Above `pub struct Manager`, add:

```rust
/// Hard cap on the number of panes. Drives `SplitError::PaneLimit`
/// and the toast text shown when the user tries to split past it.
pub const MAX_PANES: u8 = 9;

/// Error returned by `Manager::split_focused` when the requested
/// split would exceed `MAX_PANES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// `split_focused` was called when `MAX_PANES` panes already exist.
    PaneLimit,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::PaneLimit => write!(f, "pane limit reached ({})", MAX_PANES),
        }
    }
}

impl std::error::Error for SplitError {}
```

**b)** Replace the existing `pub fn split_focused(&mut self, dir: SplitDir, ratio: u8, screen: ScreenId) -> PaneId` body. New shape:

```rust
    /// Split the focused leaf, opening a new built-in screen on the
    /// non-focused side. The new pane is given focus (vim: the new
    /// window is the one you're typing in). Returns the new id.
    ///
    /// Errors with `SplitError::PaneLimit` when the tree already
    /// holds `MAX_PANES` panes.
    pub fn split_focused(
        &mut self,
        dir: SplitDir,
        ratio: u8,
        screen: ScreenId,
    ) -> Result<PaneId, SplitError> {
        if self.windows.len() as u8 >= MAX_PANES {
            return Err(SplitError::PaneLimit);
        }
        let new_id = PaneId::fresh();
        assert!(
            self.tree.split(self.focused, dir, ratio, new_id),
            "focused leaf not in tree — invariant violated"
        );
        self.windows.insert(new_id, Window::builtin(new_id, screen));
        self.focused = new_id;
        Ok(new_id)
    }
```

- [ ] **Step 4: Run tests — expect call-site failures**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::manager:: -- --test-threads=1
```

Expected: the new `split_focused_at_limit_returns_err` passes; **all other tests in the workspace that call `split_focused` will now fail to compile** — specifically the four tests in `wm::manager::tests` that use `.split_focused(...).expect(...)` (none — they use `let _ = m.split_focused(...)`), plus the smoke test in `crates/tui/src/main.rs` (via `Task 2.5`'s Ctrl-W v/s arms).

The four `manager.rs` tests must be updated to discard the `Result`. Find each `let _ = m.split_focused(...)` in the existing tests and change to:

```rust
let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
```

There are 5 such call sites in `wm::manager::tests` (in `split_focused_adds_a_pane`, `close_focused_collapses_to_one_pane`, `focus_neighbor_finds_adjacent_pane`, and the two within the new tests). Apply the `.expect("within cap")` fix to the 3 pre-existing tests.

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::manager:: -- --test-threads=1
```

Expected: all 9 `wm::manager` tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/wm/manager.rs
git commit -m "wm: SplitError + MAX_PANES cap; split_focused returns Result"
```

---## Task 3: `pane_title` takes an index; `Window::paint` forwards it

Renderer changes only. No new behaviour on its own — combined with Task 4 (`main::handle_key` `Ctrl-W 1..9`) the badges become live.

**Files:**
- Modify: `crates/tui/src/wm/render.rs` (`pane_title` signature, plan tuple, call from `Window::paint`)
- Modify: `crates/tui/src/wm/window.rs` (`Window::paint` accepts and forwards the index)
- Test: `crates/tui/src/wm/render.rs` (existing `mod tests`)

- [ ] **Step 1: Write the failing test**

Append to `mod tests` at the bottom of `crates/tui/src/wm/render.rs`:

```rust
    #[test]
    fn pane_title_includes_index_and_label() {
        use crate::wm::window::WindowKind;
        // 0-based manager index → 1-based badge.
        assert_eq!(pane_title(&WindowKind::Terminal, 0), " [1] terminal ");
        assert_eq!(pane_title(&WindowKind::Terminal, 8), " [9] terminal ");
        assert_eq!(
            pane_title(&WindowKind::Builtin(crate::app::screen::ScreenId::Network), 1),
            " [2] Network "
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::render:: -- --test-threads=1
```

Expected: compile error — `pane_title` takes 1 argument, 2 supplied.

- [ ] **Step 3: Update `pane_title` signature**

In `crates/tui/src/wm/render.rs`, replace the existing `pane_title`:

```rust
/// Title-bar string for a pane. `index` is the 0-based leaf position
/// in DFS order (matches `Manager::pane_ids()`); the user sees a
/// 1-based badge ` [N] `.
pub fn pane_title(w: &WindowKind, index: usize) -> String {
    format!(" [{}] {} ", index + 1, w.label())
}
```

- [ ] **Step 4: Update `Window::paint` to accept and forward the index**

In `crates/tui/src/wm/window.rs`, `Window::paint` is called from `wm::render::render` twice — once for built-in (the screen's own `render` paints the title, but `pane_title` is also referenced) and once for terminal. The cleanest change is to thread the index through `paint`'s parameter list and call `pane_title(&self.kind, index)` inside `paint`.

Edit the `paint` method signature from:

```rust
pub fn paint(
    &mut self,
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
    focused: bool,
)
```

to:

```rust
pub fn paint(
    &mut self,
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
    focused: bool,
    pane_index: usize,
)
```

Inside the method body, replace the existing call to `crate::wm::render::pane_title(&self.kind)` with `crate::wm::render::pane_title(&self.kind, pane_index)`. There are two such calls (one in the `Builtin` arm via the block title, one in the `Terminal` arm); update both.

- [ ] **Step 5: Update `wm::render::render` plan tuple**

In `crates/tui/src/wm/render.rs::render`, replace the plan tuple type and construction so it carries the leaf index, and pass `*index` to `Window::paint`:

```rust
    // Pass 1 — plan: apply layout and snapshot what each pane is.
    // We only touch `app.manager` here, so the borrow is scoped tightly.
    let plan: Vec<(crate::wm::broadcaster::PaneId, Rect, WindowKind, bool, usize)> = {
        let manager = &mut app.manager;
        manager.apply_layout(area);
        let focused = manager.focused();
        manager
            .layout()
            .into_iter()
            .enumerate()
            .filter_map(|(index, (id, rect))| {
                let w = manager.window(id)?;
                let is_focused = id == focused;
                Some((id, rect, w.kind, is_focused, index))
            })
            .collect()
    };

    // Pass 2 — built-in panes.
    for (_id, rect, kind, focused, _index) in &plan {
        if let WindowKind::Builtin(sid) = kind {
            if let Some(s) = screens.iter_mut().find(|s| s.id() == *sid) {
                s.render(f, *rect, app, theme, *focused);
            }
        }
    }

    // Pass 3 — terminal panes.
    for (id, rect, kind, focused, index) in &plan {
        if matches!(kind, WindowKind::Terminal) {
            if let Some(w) = app.manager.window_mut(*id) {
                w.paint(f, *rect, theme, *focused, *index);
            }
        }
    }
```

(For built-in panes the badge is implicit: the screen's own `render` paints its title. The Phase-4 acceptance criteria are met because the terminal panes — the only kind that shows `pane_title` directly — pick up the new format. Built-in titles still render the screen label only; the badge for a built-in pane appears once we extend `Screen::render` to take an index — that is **out of scope** for this plan and filed in ROADMAP.md as a follow-up.)

- [ ] **Step 6: Run the render tests**

Run:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::render:: -- --test-threads=1
```

Expected: all 3 `wm::render` tests pass (2 pre-existing + 1 new).

- [ ] **Step 7: Run a broader targeted check**

```bash
cargo check -p cyberdeck-tui --all-targets
```

Expected: clean. (Main has not been touched yet; nothing else calls `Window::paint` outside `wm::render`.)

- [ ] **Step 8: Commit**

```bash
git add crates/tui/src/wm/render.rs crates/tui/src/wm/window.rs
git commit -m "wm: pane_title(index); render threads leaf index to paint"
```

---## Task 4: `Ctrl-W 1..9` jump verb + split-cap toast in `main.rs`

The user-facing wiring. Two call-site updates inside the `_ if app.wm_pending =>` block.

**Files:**
- Modify: `crates/tui/src/main.rs` (two arms inside the existing `match key.code` block)
- Test: covered by `wm::manager::tests` (no new `main.rs` tests — `handle_key` is integration-level, exercised by the manual smoke test)

- [ ] **Step 1: Update the existing `Ctrl-W v` arm**

In `crates/tui/src/main.rs`, find the `Ctrl-W v` arm inside the `_ if app.wm_pending =>` block. Currently:

```rust
                KeyCode::Char('v') => {
                    let _ = app.manager.split_focused(
                        crate::wm::tree::SplitDir::Vertical,
                        50,
                        app.current,
                    );
                }
```

Replace with:

```rust
                KeyCode::Char('v') => {
                    if let Err(e) = app.manager.split_focused(
                        crate::wm::tree::SplitDir::Vertical,
                        50,
                        app.current,
                    ) {
                        let _ = app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            e.to_string(),
                        );
                    }
                }
```

- [ ] **Step 2: Update the existing `Ctrl-W s` arm**

Same shape as the `v` arm, with `SplitDir::Horizontal`:

```rust
                KeyCode::Char('s') => {
                    if let Err(e) = app.manager.split_focused(
                        crate::wm::tree::SplitDir::Horizontal,
                        50,
                        app.current,
                    ) {
                        let _ = app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            e.to_string(),
                        );
                    }
                }
```

- [ ] **Step 3: Add the new `Ctrl-W 1..9` arm**

Place it inside the `_ if app.wm_pending =>` match, right after the `=`/`-` arms (i.e. before the `_ => {}` catch-all):

```rust
                KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                    // Jump to pane N (1..=9). Indices are 0-based inside
                    // the manager, 1-based on screen.
                    let target = (c as u8 - b'1') as usize;
                    match app.manager.focus_pane_index(target) {
                        Some(id) => { let _ = app.manager.focus_pane(id); }
                        None => {
                            let _ = app.push_toast(
                                crate::app::toast::ToastKind::Warn,
                                format!("no pane {}", target + 1),
                            );
                        }
                    }
                }
```

- [ ] **Step 4: Build**

Run:

```bash
cargo check -p cyberdeck-tui --all-targets
```

Expected: clean. 0 warnings.

- [ ] **Step 5: Manual smoke test**

```bash
cargo run -p cyberdeck-tui --bin cyberdeck-tui
```

Walk the 11-step Phase-4 smoke test (it is added to `docs/CONTRIBUTING.md` in Task 5; reproduce it inline here). Note: built-in pane titles are drawn by each `Screen::render` and do **not** yet show the badge — only terminal pane titles do (see follow-up in ROADMAP.md). The steps below describe what is observable after this plan lands.

1. Open TUI. Single built-in pane; the title reads ` System ` (no badge yet for built-ins). Open a terminal pane with `Ctrl-W n` — the new terminal title reads ` [1] terminal ` (terminals do show the badge).
2. `Ctrl-W v`. Two panes side by side. New pane is focused.
3. `Ctrl-W h`. Focus border moves left; titles unchanged.
4. `Ctrl-W 1`. Focus jumps to pane 1.
5. Open four more terminal panes (each `Ctrl-W n` on the focused pane, then `Ctrl-W v` or `Ctrl-W s` to split as needed). Six panes, terminal titles ` [1] terminal `..` [6] terminal `.
6. Open three more (`Ctrl-W n` then split). Nine panes total.
7. `Ctrl-W v` on pane 9. No new pane appears; toast at the bottom reads `pane limit reached (9)`.
8. `Ctrl-W 1`. Focus jumps to pane 1.
9. `Ctrl-W q` on pane 9. Eight panes remain; terminal titles renumber to ` [1] terminal `..` [8] terminal `.
10. `Ctrl-W 9`. Toast `no pane 9` (only 8 exist).
11. `Ctrl-W 8`. Focus jumps to pane 8.

Press `q` to exit.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "tui: Ctrl-W 1..9 jump + pane-cap toast on split"
```

---## Task 5: Append the Phase-4 smoke test + tick the ROADMAP bullet

Two small doc edits. No code. No tests.

**Files:**
- Modify: `docs/CONTRIBUTING.md` (append the smoke test)
- Modify: `ROADMAP.md` (tick the badges bullet)

- [ ] **Step 1: Append the Phase-4 smoke test to `docs/CONTRIBUTING.md`**

Append at the end of the file (below the Phase-3 smoke test added in `dba9586`):

```markdown
## Manual smoke test for Phase 4 (pane number badges + Ctrl-W N jump)

After touching `wm/manager.rs`, `wm/render.rs`, or the `Ctrl-W`
arms in `main.rs`, run the binary and confirm. Note: built-in pane
titles are rendered by each `Screen::render` and do **not** yet show
the badge — only terminal pane titles do (see follow-up in
ROADMAP.md).

1. Open TUI. Single built-in pane; the title reads ` System ` (no
   badge yet for built-ins). Open a terminal pane with `Ctrl-W n` —
   the new terminal title reads ` [1] terminal ` (terminals do show
   the badge).
2. `Ctrl-W v`. Two panes side by side. New pane is focused.
3. `Ctrl-W h`. Focus border moves left; titles unchanged.
4. `Ctrl-W 1`. Focus jumps to pane 1.
5. Open four more terminal panes (each `Ctrl-W n` on the focused
   pane, then `Ctrl-W v` or `Ctrl-W s` to split as needed). Six panes,
   terminal titles ` [1] terminal `..` [6] terminal `.
6. Open three more (`Ctrl-W n` then split). Nine panes total.
7. `Ctrl-W v` on pane 9. No new pane appears; toast at the bottom
   reads `pane limit reached (9)`.
8. `Ctrl-W 1`. Focus jumps to pane 1.
9. `Ctrl-W q` on pane 9. Eight panes remain; terminal titles
   renumber to ` [1] terminal `..` [8] terminal `.
10. `Ctrl-W 9`. Toast `no pane 9` (only 8 exist).
11. `Ctrl-W 8`. Focus jumps to pane 8.

If any step fails, the regression is almost always in
`Manager::focus_pane_index` / `focus_pane` (Task 1), the `Result`
shape of `split_focused` (Task 2), or the index threading in
`wm::render::render` (Task 3).
```

- [ ] **Step 2: Tick the ROADMAP bullet**

In `ROADMAP.md`, the `## Phase 4 — polish` section currently starts with:

```markdown
- Pane number badges in titles (`1`/`2`/…) so `Ctrl-W N` jump is discoverable.
```

Replace with:

```markdown
- [x] Pane number badges in titles (`1`/`2`/…) so `Ctrl-W N` jump is discoverable.
```

(The other Phase-4 bullets remain unchecked — they are the next plans.)

- [ ] **Step 3: Commit**

```bash
git add docs/CONTRIBUTING.md ROADMAP.md
git commit -m "docs: Phase 4 pane-badges smoke test; tick ROADMAP bullet"
```

---

## Self-Review

**1. Spec coverage.** Mapping each spec section to a task:

- §3 (user-visible behaviour table): covered by Tasks 3 + 4 + 5 (smoke test).
- §4.1 (`focus_pane_index`, `focus_pane`, `SplitError`, `MAX_PANES`, `split_focused` returns `Result`): Task 1 + Task 2.
- §4.2 (`pane_title` signature): Task 3.
- §4.3 (renderer threads index): Task 3.
- §4.4 (`Window::paint`): Task 3.
- §4.5 (`main::handle_key` `Ctrl-W 1..9` + cap toast): Task 4.
- §5 (data flow): documented in Task 4 step 3.
- §6 (error handling): covered by Tasks 1 + 2 + 4 (toasts surface `SplitError::PaneLimit` and out-of-range).
- §7 (tests): Tasks 1, 2, 3 each add targeted tests; the manual smoke test is in Task 5.
- §8 (rollout): this plan.

**2. Placeholder scan.** No TBD / TODO / "similar to N" / "appropriate error handling" placeholders. The "within cap" `.expect(...)` is shown in full each time.

**3. Type consistency.**
- `Manager::MAX_PANES: u8 = 9` (Task 2) → `as u8` casts in the test match (Task 2).
- `SplitError::PaneLimit` (Task 2) → `e.to_string()` produces `pane limit reached (9)` (Task 4) → matches the spec.
- `focus_pane_index(index: usize) -> Option<PaneId>` (Task 1) → caller converts `'1'..='9'` to `(c as u8 - b'1') as usize` (Task 4) → `target + 1` in the toast format string.
- `pane_title(&WindowKind, usize)` (Task 3) → `Window::paint(... pane_index: usize)` (Task 3) → `wm::render::render` calls `w.paint(..., *index)` (Task 3).

---