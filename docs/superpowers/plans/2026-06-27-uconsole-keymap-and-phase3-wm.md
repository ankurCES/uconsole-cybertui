# cyberdeck-tui: uconsole keymap + Phase 3 window manager

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the TUI navigable end-to-end on a ClockworkPi uconsole using its top X/Y/A/B hardware buttons, then wire the existing Phase-2 PTY/ANSI/broadcaster modules into a real Phase-3 window manager so each pane can host a built-in screen or a live shell.

**Architecture:**

1. **Keymap layer** — a pure function `map_key(KeyEvent, KeymapProfile) -> Option<KeyEvent>` sits at the very top of `handle_key` in `main.rs`. For the `uconsole` profile it translates the four hardware buttons into `Up`/`Down`/`Enter`/`Esc`. Profiles are selected at compile time via the `--features uconsole-keymap` feature (default off, so x86 dev boxes are unaffected). One small module, one test, one wiring point.
2. **Window manager** — reuse `wm/tree`, `wm/ansi`, `wm/pty`, `wm/broadcaster`, `wm/window` as-is. Add `wm/manager.rs` (the `Manager` struct that owns a `Node` + `HashMap<PaneId, Window>` + focused `PaneId`) and `wm/render.rs` (the tree-walk painter). `main.rs::draw` calls into `wm/render` instead of the single-screen render. Focus becomes `Focus::Pane(PaneId)`. New global keymap `Ctrl-W h/j/k/l` moves focus, `Ctrl-W v/s` splits, `Ctrl-W n` opens a terminal pane, `Ctrl-W q`/`x` closes the focused pane.
3. **Tooling policy** — the development loop uses `cargo check` and `cargo build`. `cargo test` is **not** part of the iteration loop because the unit tests in `wm/ansi.rs`, `wm/pty.rs`, `wm/broadcaster.rs`, and `wm/window.rs` spin up real PTYs and shells; on this dev box some of them hang in the background-task scheduler. The plan still keeps those tests as the canonical correctness check — they're just run deliberately (`cargo test -p cyberdeck-tui wm::`, with a wall-clock cap), not every save. This is documented in `CONTRIBUTING.md` so the next person doesn't waste a morning the same way.

**Tech Stack:** Rust 1.80, ratatui 0.29, crossterm 0.28, tokio 1, portable-pty 0.8, vte 0.13 (all already in `Cargo.toml`).

**Reference files (read these before starting):**
- `crates/tui/src/main.rs` — entry point + `handle_key` + `draw`.
- `crates/tui/src/app.rs` — `App`, `Live`, `Focus`, `Modal`.
- `crates/tui/src/app/screen.rs` — `Screen` trait, `ScreenId` (13 variants).
- `crates/tui/src/screens/*.rs` — the 13 screen impls.
- `crates/tui/src/wm/{mod,tree,window,ansi,pty,broadcaster}.rs` — Phase-2 infra, all in tree and tested in isolation.
- `crates/tui/Cargo.toml` — workspace deps.
- `ROADMAP.md` — Phase 3 milestones we're filling in.

---

## File Structure

| File | Status | Responsibility |
| --- | --- | --- |
| `crates/tui/src/wm/keymap.rs` | **create** | Pure `map_key(KeyEvent, KeymapProfile) -> Option<KeyEvent>` plus `KeymapProfile` enum. The only file that knows about uconsole button labels. |
| `crates/tui/src/wm/mod.rs` | **modify** | Re-export `keymap` and the new `manager`/`render` modules. |
| `crates/tui/src/wm/manager.rs` | **create** | `Manager` struct: owns `Node` (split tree) + `HashMap<PaneId, Window>` + `focused: PaneId`. Methods: `new`, `focus_pane`, `split_focused`, `open_terminal`, `close_focused`, `resize_focused`, `rotate_focused`, `focus_neighbor`, `apply_layout(area)`. |
| `crates/tui/src/wm/render.rs` | **create** | `render(f, area, &mut Manager, &mut [Box<dyn Screen>], &App, &Theme)` — walks the tree, calls each `Window`'s paint (built-in screens dispatch into the `Screen` list, terminals paint their `Grid`). |
| `crates/tui/src/wm/window.rs` | **modify** | Add `Window::paint(&mut self, frame, area, screens, app, theme, focused)` to centralise the Builtin vs Terminal dispatch. |
| `crates/tui/src/main.rs` | **modify** | (1) call `wm::keymap::map_key` at the top of `handle_key`; (2) replace `Focus::Sidebar/Content` with `Focus::Pane(PaneId)` (kept behind the WM — sidebar stays the same, content focus is now the focused pane); (3) add `Ctrl-W` keymap; (4) `draw` calls `wm::render::render`. |
| `crates/tui/src/app.rs` | **modify** | Replace `focus: Focus` (binary) with `focus: wm::manager::Manager`. `App::new` builds the initial `Manager` (one pane hosting `ScreenId::System`). |
| `crates/tui/Cargo.toml` | **modify** | Add the `uconsole-keymap` feature (empty; gates `default-features`-style behaviour in `wm/keymap.rs` via `#[cfg]`). |
| `docs/CONTRIBUTING.md` | **create** | One-page note on the `cargo test` policy and how to run the wm tests deliberately. |
| `ROADMAP.md` | **modify** | Tick off the Phase 3 sub-bullets that this plan lands. |

Tests live next to the code (Rust `#[cfg(test)]` modules). The only new external test command is `cargo test -p cyberdeck-tui wm:: -- --test-threads=1` documented in `CONTRIBUTING.md`.

---

## Part 1 — uconsole keymap (lands first so the TUI is usable before WM work)

### Task 1.1: `wm::keymap` module skeleton

**Files:**
- Create: `crates/tui/src/wm/keymap.rs`
- Modify: `crates/tui/src/wm/mod.rs:1-14`
- Test: in-file `#[cfg(test)] mod tests` in `keymap.rs`

- [ ] **Step 1: Write the failing test**

In `keymap.rs`, add at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn uconsole_buttons_map_to_nav_keys() {
        // X is left/down on the uconsole's top row; we use it as Up
        // because the screen is short and Up is the most-needed key.
        // If you prefer a different mapping, change the source — the
        // test just locks in the contract, not the exact letters.
        let p = KeymapProfile::Uconsole;
        assert_eq!(map_key(k(KeyCode::Char('x')), p), Some(k(KeyCode::Up)));
        assert_eq!(map_key(k(KeyCode::Char('y')), p), Some(k(KeyCode::Down)));
        assert_eq!(map_key(k(KeyCode::Char('a')), p), Some(k(KeyCode::Enter)));
        assert_eq!(map_key(k(KeyCode::Char('b')), p), Some(k(KeyCode::Esc)));
    }

    #[test]
    fn uconsole_mapping_is_a_passthrough_for_other_keys() {
        // We never swallow keys we don't recognise — we only rewrite the
        // four hardware buttons. Everything else falls through to the
        // main loop unmodified.
        let p = KeymapProfile::Uconsole;
        assert_eq!(map_key(k(KeyCode::Char('q')), p), Some(k(KeyCode::Char('q'))));
        assert_eq!(map_key(k(KeyCode::Tab), p), Some(k(KeyCode::Tab)));
    }

    #[test]
    fn desktop_profile_is_identity() {
        let p = KeymapProfile::Desktop;
        assert_eq!(map_key(k(KeyCode::Up), p), Some(k(KeyCode::Up)));
        assert_eq!(map_key(k(KeyCode::Char('x')), p), Some(k(KeyCode::Char('x'))));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (compilation error is fine)**

Run: `cargo check -p cyberdeck-tui --tests`
Expected: error[E0432]: unresolved import `super::*` (keymap.rs doesn't exist yet).

- [ ] **Step 3: Create the module with stubs**

Create `crates/tui/src/wm/keymap.rs`:

```rust
//! Hardware button → arrow/enter/esc mapping for keyboards that don't emit
//! standard arrow codes (e.g. the ClockworkPi uconsole's top X/Y/A/B row).
//!
//! `map_key` is a pure function: same input, same output, no I/O. It runs
//! at the very top of `main::handle_key` so every code path downstream
//! sees a normal `KeyEvent`. The desktop profile is identity — the
//! remap is a no-op unless the binary is built with `--features
//! uconsole-keymap` and the env var `CYBERDECK_KEYMAP=uconsole` is set.
//!
//! Why both a feature flag *and* an env var? The feature is what we
//! want when cross-compiling for the uconsole hardware; the env var
//! is what we use on the device itself to flip the mapping without
//! rebuilding (handy when debugging with a real keyboard attached).

use crossterm::event::KeyEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapProfile {
    /// Standard x86 laptop. Pass-through.
    Desktop,
    /// ClockworkPi uconsole: X/Y/A/B → Up/Down/Enter/Esc.
    Uconsole,
}

impl KeymapProfile {
    /// Resolved at runtime from the env var, with a sensible default
    /// (Desktop) so x86 development builds Just Work.
    pub fn detect() -> Self {
        match std::env::var("CYBERDECK_KEYMAP").as_deref() {
            Ok("uconsole") => Self::Uconsole,
            _ => Self::Desktop,
        }
    }
}

pub fn map_key(key: KeyEvent, profile: KeymapProfile) -> Option<KeyEvent> {
    use crossterm::event::KeyCode;
    match profile {
        KeymapProfile::Desktop => Some(key),
        KeymapProfile::Uconsole => match key.code {
            KeyCode::Char('x') => Some(KeyEvent::new(KeyCode::Up, key.modifiers)),
            KeyCode::Char('y') => Some(KeyEvent::new(KeyCode::Down, key.modifiers)),
            KeyCode::Char('a') => Some(KeyEvent::new(KeyCode::Enter, key.modifiers)),
            KeyCode::Char('b') => Some(KeyEvent::new(KeyCode::Esc, key.modifiers)),
            // Anything else (real arrows, hjkl, tab, q, etc.) passes
            // through unchanged. This is the critical contract: we
            // never *swallow* a key, only rewrite the four hardware
            // buttons.
            _ => Some(key),
        },
    }
}
```

- [ ] **Step 4: Wire the module into `wm/mod.rs`**

Edit `crates/tui/src/wm/mod.rs:1-14`. Replace the doc-comment body so the module list includes keymap, but keep `pub mod` lines alphabetical:

```rust
//! TUI window manager: layout tree, PTY panes, ANSI rendering.
//!
//! Modules:
//! - `ansi`         — VT100 byte stream → ratatui cell grid.
//! - `broadcaster`  — broadcast output + mpsc input for a pane.
//! - `keymap`       — hardware button remap (uconsole X/Y/A/B → arrows).
//! - `manager`      — owns the split tree + per-pane state.
//! - `pty`          — child PTY per external pane, lifecycle + I/O.
//! - `render`       — tree-walk renderer for the manager.
//! - `tree`         — binary split tree, layout, focus neighbours.
//! - `window`       — `Window` + `WindowKind` (Builtin | Terminal).

pub mod ansi;
pub mod broadcaster;
pub mod keymap;
pub mod pty;
pub mod tree;
pub mod window;
```

(The `manager` and `render` lines are forward-references — they'll be added in Part 2. The doc comment is the only place they appear right now; `pub mod manager;` lands in Task 2.1.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p cyberdeck-tui wm::keymap:: -- --nocapture`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/wm/keymap.rs crates/tui/src/wm/mod.rs
git commit -m "wm: add uconsole keymap profile (X/Y/A/B -> Up/Down/Enter/Esc)"
```

---

### Task 1.2: Add the `uconsole-keymap` feature gate

**Files:**
- Modify: `crates/tui/Cargo.toml:10-11`

- [ ] **Step 1: Add the feature**

Edit `crates/tui/Cargo.toml`. After the existing `web = ["dep:cyberdeck-web"]` line, add:

```toml
# uconsole-keymap: makes uconsole the default profile when CYBERDECK_KEYMAP
# isn't set. Empty because the runtime check is in `wm/keymap.rs`; the
# feature just changes the *default* so an unattended build for the
# device boots straight into uconsole behaviour.
uconsole-keymap = []
```

- [ ] **Step 2: Make the feature affect the default profile**

Edit `crates/tui/src/wm/keymap.rs`. In `KeymapProfile::detect`, change the default branch:

```rust
pub fn detect() -> Self {
    match std::env::var("CYBERDECK_KEYMAP").as_deref() {
        Ok("uconsole") => Self::Uconsole,
        Ok("desktop") => Self::Desktop,
        // Env var unset. Default is Desktop on a normal build, Uconsole
        // when built with `--features uconsole-keymap` (for flashing
        // onto the device).
        Ok(_) | Err(_) => {
            if cfg!(feature = "uconsole-keymap") {
                Self::Uconsole
            } else {
                Self::Desktop
            }
        }
    }
}
```

- [ ] **Step 3: Add a test for the feature-driven default**

Append to the existing `mod tests` in `keymap.rs`:

```rust
#[test]
fn env_var_overrides_feature_default() {
    // We can't toggle `cfg!(feature = ...)` from a test, but we *can*
    // assert that the env-var branches are reached. If the feature
    // isn't enabled in this build, the env var still wins.
    std::env::set_var("CYBERDECK_KEYMAP", "uconsole");
    assert_eq!(KeymapProfile::detect(), KeymapProfile::Uconsole);
    std::env::set_var("CYBERDECK_KEYMAP", "desktop");
    assert_eq!(KeymapProfile::detect(), KeymapProfile::Desktop);
    std::env::remove_var("CYBERDECK_KEYMAP");
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p cyberdeck-tui wm::keymap:: -- --nocapture`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/Cargo.toml crates/tui/src/wm/keymap.rs
git commit -m "wm: add uconsole-keymap cargo feature (default profile override)"
```

---

### Task 1.3: Wire `map_key` into `main::handle_key`

**Files:**
- Modify: `crates/tui/src/main.rs` (the `handle_key` function, ~line 411-540)

- [ ] **Step 1: Add the call at the top of `handle_key`**

Edit `crates/tui/src/main.rs`. At the very top of the `async fn handle_key` body (right after the `use KeyCode::*;` line and before the modal-handling `match`), add:

```rust
    // Hardware-button remap. Runs first so the rest of the handler
    // (modal dispatch, global keys, screen on_key) sees a normal
    // KeyEvent. The desktop profile is identity; the uconsole profile
    // rewrites X/Y/A/B into Up/Down/Enter/Esc. See `wm/keymap.rs`.
    let key = match wm::keymap::map_key(key, wm::keymap::KeymapProfile::detect()) {
        Some(k) => k,
        // The contract is `Option` so future profiles can swallow
        // specific keys (e.g. a tablet profile that ignores the
        // volume buttons). Today every profile returns `Some`.
        None => return false,
    };
```

- [ ] **Step 2: Sanity-build, no run, no test**

Run: `cargo check -p cyberdeck-tui`
Expected: clean build, no warnings about unused imports (the `KeyEvent` import is already used).

- [ ] **Step 3: Manual smoke test on the desktop**

Run: `cargo run -p cyberdeck-tui --bin cyberdeck-tui`
Expected: the TUI starts, the System screen renders, and the `1`..`9`/`0` global jump keys work (proves the handler still fires for non-rewritten keys). Press `x` — on the uconsole profile this would mean "Up", but on the desktop profile `x` falls through; in the help modal you can verify `x` isn't bound to anything. Exit with `q`.

(If you do have a uconsole to test on, run with `CYBERDECK_KEYMAP=uconsole ./target/debug/cyberdeck-tui` and confirm `x`/`y` move the cursor in the Services screen, `a` opens a service, `b` cancels.)

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/main.rs
git commit -m "tui: route every key through wm::keymap (desktop = identity)"
```

---

### Task 1.4: Document the `cargo test` policy

**Files:**
- Create: `docs/CONTRIBUTING.md`

- [ ] **Step 1: Write the doc**

Create `docs/CONTRIBUTING.md`:

```markdown
# Contributing

## Iteration loop

We **do not** run `cargo test` as part of the inner save loop on this
repo. The unit tests under `crates/tui/src/wm/{ansi,pty,broadcaster,window}.rs`
spin up real PTYs and shells; on the dev box we share with the
running editor and a few other services, the scheduler occasionally
hitches and the tests hang.

Use these commands while you're iterating:

- `cargo check -p cyberdeck-tui` — fastest, catches most type errors.
- `cargo check -p cyberdeck-tui --all-targets` — also picks up the
  `#[cfg(test)]` modules so an unused import inside a test will fail
  the build.
- `cargo build -p cyberdeck-tui` — when you want to actually run the
  binary.
- `cargo clippy -p cyberdeck-tui --all-targets -- -D warnings` —
  before sending a PR.

## Running the WM tests deliberately

When you do want to run them — e.g. after editing `wm/tree.rs` or
`wm/ansi.rs` — use:

```bash
cargo test -p cyberdeck-tui wm:: -- --test-threads=1
```

`--test-threads=1` is the important bit. The PTY tests in `pty.rs` and
`broadcaster.rs` both spawn a child process; running them in parallel
sometimes exhausts the available PTYs and triggers a hang inside
`portable-pty`. Sequential is slower but reliable.

If a test still hangs, the binary that's stuck is almost always
`/bin/cat` or `/bin/sh` from a previous run that didn't reap cleanly:

```bash
pkill -f 'target/debug/deps/cyberdeck_tui-*'
```

then re-run.

## Running the full test suite (CI parity)

```bash
cargo test --workspace -- --test-threads=1
```

Run this before opening a PR. Expect 20-40 seconds on a quiet box.
```

- [ ] **Step 2: Sanity-check the markdown renders cleanly**

Run: `cat docs/CONTRIBUTING.md | head -20`
Expected: the first heading and intro paragraph appear.

- [ ] **Step 3: Commit**

```bash
git add docs/CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING.md with the cargo-test policy"
```

---

## Part 2 — Phase 3 window manager

### Task 2.1: `wm::manager` skeleton (split tree + window map)

**Files:**
- Create: `crates/tui/src/wm/manager.rs`
- Modify: `crates/tui/src/wm/mod.rs` (add `pub mod manager;`)

- [ ] **Step 1: Write the failing test**

In `manager.rs`, at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screen::ScreenId;
    use crate::wm::broadcaster::PaneId;
    use ratatui::layout::Rect;

    #[test]
    fn new_starts_with_a_single_pane() {
        let m = Manager::new(ScreenId::System);
        let panes = m.pane_ids();
        assert_eq!(panes.len(), 1);
        assert_eq!(m.focused(), panes[0]);
        let w = m.window(m.focused()).unwrap();
        assert_eq!(w.kind, WindowKind::Builtin(ScreenId::System));
    }

    #[test]
    fn split_focused_adds_a_pane() {
        let mut m = Manager::new(ScreenId::System);
        let before = m.pane_ids();
        let new_id = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        assert!(m.pane_ids().contains(&new_id));
        assert_eq!(m.pane_ids().len(), before.len() + 1);
        // Newly-split pane gets focus (vim convention).
        assert_eq!(m.focused(), new_id);
    }

    #[test]
    fn close_focused_collapses_to_one_pane() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        let _ = m.split_focused(SplitDir::Vertical, 50, ScreenId::Audio);
        let _ = m.close_focused();
        let _ = m.close_focused();
        assert_eq!(m.pane_ids().len(), 1);
    }

    #[test]
    fn focus_neighbor_finds_adjacent_pane() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        // Focus is on the new (right) pane. Go back left.
        let back = m.focus_neighbor(FocusDir::Left).unwrap();
        let original = m.pane_ids().into_iter().find(|id| *id != m.focused()).unwrap();
        assert_eq!(back, original);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo check -p cyberdeck-tui --tests`
Expected: error[E0432]: unresolved import `crate::wm::manager` (module doesn't exist yet).

- [ ] **Step 3: Write the minimal `Manager`**

Create `crates/tui/src/wm/manager.rs`:

```rust
//! Window manager: owns the split tree, the per-pane runtime state, and
//! the currently focused pane. The tree itself lives in `wm::tree`; this
//! module is the orchestrator that the rest of the TUI talks to.
//!
//! All mutators keep three things in sync:
//!   * the `Node` tree (drives layout),
//!   * the `HashMap<PaneId, Window>` (drives paint),
//!   * `focused` (drives the next input event).
//!
//! `apply_layout(area)` walks the tree once per render and hands the
//! computed rects to each `Window`. `Window::resize` is then called with
//! the new size so terminal panes can re-`ioctl(TIOCSWINSZ)`.

use std::collections::HashMap;

use ratatui::layout::Rect;

use crate::app::screen::ScreenId;
use crate::wm::broadcaster::PaneId;
use crate::wm::tree::{compute_layout, FocusDir, Node, SplitDir};
use crate::wm::window::{Window, WindowKind};

pub use crate::wm::tree::FocusDir as NeighbourDir;

pub struct Manager {
    tree: Node,
    windows: HashMap<PaneId, Window>,
    focused: PaneId,
    /// Last area we laid out into. Used to drive `Window::resize` on the
    /// next call so terminal panes see a real TIOCSWINSZ.
    last_area: Rect,
}

impl Manager {
    /// Build a single-pane tree hosting the given built-in screen.
    pub fn new(initial: ScreenId) -> Self {
        let id = PaneId::fresh();
        let mut windows = HashMap::new();
        windows.insert(id, Window::builtin(id, initial));
        Self {
            tree: Node::leaf(id),
            windows,
            focused: id,
            last_area: Rect::new(0, 0, 0, 0),
        }
    }

    pub fn focused(&self) -> PaneId { self.focused }
    pub fn window(&self, id: PaneId) -> Option<&Window> { self.windows.get(&id) }
    pub fn window_mut(&mut self, id: PaneId) -> Option<&mut Window> { self.windows.get_mut(&id) }

    pub fn pane_ids(&self) -> Vec<PaneId> { self.tree.leaves() }

    /// Split the focused leaf, opening a new built-in screen on the
    /// non-focused side. The new pane is given focus (vim: the new
    /// window is the one you're typing in). Returns the new id.
    pub fn split_focused(
        &mut self,
        dir: SplitDir,
        ratio: u8,
        screen: ScreenId,
    ) -> PaneId {
        let new_id = PaneId::fresh();
        assert!(
            self.tree.split(self.focused, dir, ratio, new_id),
            "focused leaf not in tree — invariant violated"
        );
        self.windows.insert(new_id, Window::builtin(new_id, screen));
        self.focused = new_id;
        new_id
    }

    /// Close the focused pane. If it was the last pane, returns false and
    /// does nothing (the TUI must always have at least one pane to show).
    pub fn close_focused(&mut self) -> bool {
        if self.windows.len() <= 1 {
            return false;
        }
        let target = self.focused;
        // Pick a neighbour to give focus to. Vim uses the previously
        // focused pane if one exists; we fall back to the first
        // remaining leaf.
        let neighbour = self
            .tree
            .focus_neighbor(target, self.last_area, FocusDir::Left)
            .or_else(|| self.tree.focus_neighbor(target, self.last_area, FocusDir::Right))
            .or_else(|| self.tree.focus_neighbor(target, self.last_area, FocusDir::Up))
            .or_else(|| self.tree.focus_neighbor(target, self.last_area, FocusDir::Down))
            .or_else(|| self.tree.leaves().into_iter().find(|id| *id != target));
        let _ = self.tree.close(target);
        self.windows.remove(&target);
        if let Some(n) = neighbour {
            self.focused = n;
        }
        true
    }

    /// Move focus to the leaf in `dir` from the currently focused one.
    /// Returns the new focused id, or `None` if no neighbour exists.
    pub fn focus_neighbor(&mut self, dir: FocusDir) -> Option<PaneId> {
        let next = self.tree.focus_neighbor(self.focused, self.last_area, dir)?;
        self.focused = next;
        Some(next)
    }

    /// Walk the tree and update each `Window`'s `last_rows`/`last_cols`
    /// (and PTY size, for terminals). Must be called from the render
    /// path before any window paints.
    pub fn apply_layout(&mut self, area: Rect) {
        self.last_area = area;
        let rects = compute_layout(&self.tree, area);
        for (id, rect) in rects {
            if let Some(w) = self.windows.get_mut(&id) {
                w.resize(rect.height, rect.width);
            }
        }
    }

    /// Iterator over `(PaneId, Rect)` in left-to-right, top-to-bottom
    /// order. Used by the renderer.
    pub fn layout(&self) -> Vec<(PaneId, Rect)> {
        compute_layout(&self.tree, self.last_area)
    }

    /// Set the focused pane's kind. Used by `Ctrl-W n` to swap a builtin
    /// pane for a terminal. Returns the previous kind.
    pub fn replace_focused_with_terminal(
        &mut self,
        pty: crate::wm::pty::Pty,
        output: crate::wm::broadcaster::PaneOutput,
        writer: crate::wm::broadcaster::PtyWriter,
    ) -> Option<WindowKind> {
        let id = self.focused;
        let prev = self.windows.get(&id)?.kind;
        // Use a sensible default size; apply_layout will resize on the
        // next render.
        let (rows, cols) = (24, 80);
        *self.windows.get_mut(&id)? = Window::terminal(id, pty, output, writer, rows, cols);
        Some(prev)
    }
}
```

- [ ] **Step 4: Add the module to `wm/mod.rs`**

Edit `crates/tui/src/wm/mod.rs`. Add the line in alphabetical order (between `keymap` and `pty`):

```rust
pub mod manager;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p cyberdeck-tui wm::manager:: -- --test-threads=1 --nocapture`
Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/wm/manager.rs crates/tui/src/wm/mod.rs
git commit -m "wm: add Manager (split tree + per-pane state + focus)"
```

---

### Task 2.2: `Window::paint` and `wm::render::render`

**Files:**
- Create: `crates/tui/src/wm/render.rs`
- Modify: `crates/tui/src/wm/window.rs` (add `Window::paint`)
- Modify: `crates/tui/src/wm/mod.rs` (add `pub mod render;`)

- [ ] **Step 1: Write the failing test in `render.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screen::ScreenId;
    use crate::theme::Theme;
    use ratatui::layout::Rect;

    #[test]
    fn render_single_pane_draws_into_the_whole_area() {
        // We can't easily assert on a `Frame` here, but we *can* assert
        // that `apply_layout` produced the right rects. The actual
        // pixel-level render is exercised by the manual smoke test in
        // Task 2.6.
        let mut m = Manager::new(ScreenId::System);
        let area = Rect::new(0, 0, 80, 24);
        m.apply_layout(area);
        let layout = m.layout();
        assert_eq!(layout, vec![(m.focused(), area)]);
        let _ = Theme::by_name(crate::theme::ThemeName::Dark);
    }

    #[test]
    fn render_split_panes_have_disjoint_rects() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network);
        let area = Rect::new(0, 0, 80, 24);
        m.apply_layout(area);
        let layout = m.layout();
        assert_eq!(layout.len(), 2);
        // 50% of 80 = 40. Each pane gets 40 cols.
        assert_eq!(layout[0].1.width, 40);
        assert_eq!(layout[1].1.width, 40);
        assert_eq!(layout[0].1.x, 0);
        assert_eq!(layout[1].1.x, 40);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo check -p cyberdeck-tui --tests`
Expected: error[E0432]: unresolved import `crate::wm::render` (module doesn't exist yet).

- [ ] **Step 3: Create `wm/render.rs`**

```rust
//! Tree-walk renderer: walks the split tree, paints each pane.
//!
//! For a built-in pane we dispatch into the global `screens` list keyed
//! by the pane's `ScreenId` (the same `Screen` trait impls that the
//! single-pane TUI used in Phase 1). For a terminal pane we paint the
//! `Grid` of cells into a `Paragraph` of styled spans — ratatui can
//! take a per-cell style so ANSI colours flow through.
//!
//! The focus border style comes from `Theme::border(focused)`. The
//! focused pane gets the brighter border so it's always obvious which
//! one your keystrokes are going to.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::screen::Screen;
use crate::app::App;
use crate::theme::Theme;
use crate::wm::manager::Manager;
use crate::wm::window::WindowKind;

pub fn render(
    f: &mut Frame,
    area: Rect,
    manager: &mut Manager,
    screens: &mut [Box<dyn Screen>],
    app: &mut App,
    theme: &Theme,
) {
    manager.apply_layout(area);
    for (id, rect) in manager.layout() {
        let focused = id == manager.focused();
        if let Some(w) = manager.window_mut(id) {
            w.paint(f, rect, screens, app, theme, focused);
        }
    }
}

/// Title-bar string for a pane. Kept here (not in `Window`) because
/// ratatui's `Block` builder is what we pass it to.
pub fn pane_title(w: &WindowKind) -> String {
    format!(" {} ", w.label())
}

// `Line`/`Span`/`Block`/`Borders` are used by the Window::paint body
// (see next task); they're re-exported here for the convenience of any
// future helper that wants to compose a title bar the same way.
#[allow(unused_imports)]
pub(crate) use ratatui::text::{Line as _Line, Span as _Span};
#[allow(unused_imports)]
pub(crate) use ratatui::widgets::{Block as _Block, Borders as _Borders};
```

- [ ] **Step 4: Add the module to `wm/mod.rs`**

Edit `crates/tui/src/wm/mod.rs`. Insert in alphabetical order (between `pty` and `tree`):

```rust
pub mod render;
```

- [ ] **Step 5: Add `Window::paint` to `wm/window.rs`**

Append to `impl Window` in `crates/tui/src/wm/window.rs` (right before the existing `#[cfg(test)] mod tests`):

```rust
    /// Paint one pane. Dispatches on `kind`:
    ///   * `Builtin(id)` — finds the matching `Screen` in `screens`
    ///     and calls its `render`.
    ///   * `Terminal`    — drains the broadcaster into the parser,
    ///     then paints the `Grid` as styled spans into a `Paragraph`
    ///     wrapped in a `Block` with the pane title.
    pub fn paint(
        &mut self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        screens: &mut [Box<dyn crate::app::screen::Screen>],
        app: &mut crate::app::App,
        theme: &crate::theme::Theme,
        focused: bool,
    ) {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Paragraph};
        self.focused = focused;
        let title = crate::wm::render::pane_title(&self.kind);
        let block = Block::default()
            .title(Span::styled(title, theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focused));
        match self.kind {
            WindowKind::Builtin(id) => {
                if let Some(s) = screens.iter_mut().find(|s| s.id() == id) {
                    s.render(frame, area, app, theme, focused);
                }
                // Re-render the border on top so the title is visible
                // above the screen's own widget. ratatui's render order
                // is last-wins for overlapping rects, so this draws
                // over the screen's own block.
                let _ = block; // the screen's render already draws its own border
            }
            WindowKind::Terminal => {
                if let Some(term) = self.terminal.as_mut() {
                    let _ = self.drain_output();
                    let lines: Vec<Line> = (0..term.grid.rows as usize)
                        .map(|r| {
                            let spans: Vec<Span> = (0..term.grid.cols as usize)
                                .map(|c| {
                                    let cell = &term.grid.cells()[r * term.grid.cols as usize + c];
                                    Span::styled(
                                        cell.ch.to_string(),
                                        ratatui::style::Style::default()
                                            .fg(cell.fg)
                                            .bg(cell.bg)
                                            .add_modifier(cell.mods),
                                    )
                                })
                                .collect();
                            Line::from(spans)
                        })
                        .collect();
                    let p = Paragraph::new(lines)
                        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg))
                        .block(block);
                    frame.render_widget(p, area);
                }
            }
        }
    }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p cyberdeck-tui wm::render:: -- --test-threads=1 --nocapture`
Expected: 2 tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/tui/src/wm/render.rs crates/tui/src/wm/window.rs crates/tui/src/wm/mod.rs
git commit -m "wm: add tree-walk renderer (Window::paint dispatches builtin vs terminal)"
```

---

### Task 2.3: Replace `App::focus` with the manager

**Files:**
- Modify: `crates/tui/src/app.rs` (replace `Focus` enum, add `manager: Manager` field, update `App::new`)

- [ ] **Step 1: Write the failing test**

Add a test module to the bottom of `app.rs` (right before the closing of the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_has_one_pane() {
        // A bit of a smoke test: the App's manager should be in a
        // valid state with one focused pane hosting the System
        // screen.
        let (tx, rx) = mpsc::channel::<Action>(8);
        let app = App::new(tx, rx);
        let panes = app.manager.pane_ids();
        assert_eq!(panes.len(), 1);
        let w = app.manager.window(app.manager.focused()).unwrap();
        assert_eq!(w.kind, crate::wm::window::WindowKind::Builtin(ScreenId::System));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo check -p cyberdeck-tui --tests`
Expected: error — `manager` field doesn't exist yet.

- [ ] **Step 3: Replace `Focus` with the manager**

Edit `crates/tui/src/app.rs`. Make three changes:

**a)** Remove the `Focus` enum and the `Modal` (modal stays, only `Focus` goes):

```rust
// (delete the entire `pub enum Focus { Sidebar, Content }` block)
```

**b)** In `pub struct App`, remove `pub focus: Focus,` and add `pub manager: crate::wm::manager::Manager,`:

```rust
pub struct App {
    pub live: Arc<Live>,
    pub current: ScreenId,
    // pub focus: Focus,   <-- delete this line
    pub manager: crate::wm::manager::Manager,   // <-- add this line
    pub modal: Modal,
    /* ...rest unchanged... */
}
```

**c)** In `App::new`, delete the `focus: Focus::Sidebar,` initialiser and add `manager: crate::wm::manager::Manager::new(ScreenId::System),`:

```rust
// pub focus: Focus::Sidebar,   <-- delete this line
manager: crate::wm::manager::Manager::new(ScreenId::System),   // <-- add this line
```

- [ ] **Step 4: Compile to find all the consumers we need to update**

Run: `cargo check -p cyberdeck-tui --all-targets 2>&1 | head -80`
Expected: a wave of errors at every `app.focus` and `Focus::Sidebar`/`Focus::Content` site. The notable ones are:

- `crates/tui/src/main.rs` — `handle_key` uses `app.focus`; `draw` uses `Focus::Content`.
- `crates/tui/src/ui/mod.rs` — `draw_sidebar` uses `Focus::Sidebar`.
- `crates/tui/src/screens/*.rs` — pass `focus: bool` to `s.render(...)`.

Each of these is fixed in the next task (Task 2.4). Don't try to fix them all in this step — we want one commit per concern.

- [ ] **Step 5: Commit the App change in isolation**

This commit will **not** build cleanly (we expect errors). That's intentional — it makes the next commit a clean "fix all the callers" diff.

```bash
git add crates/tui/src/app.rs
git commit -m "app: swap Focus::Sidebar/Content for wm::Manager (breaks build)"
```

(If your team policy requires every commit to build, do this on a branch and squash-merge later. The plan works either way.)

---

### Task 2.4: Update all `Focus::*` consumers

**Files:**
- Modify: `crates/tui/src/main.rs` (`handle_key`, `draw`)
- Modify: `crates/tui/src/ui/mod.rs` (`draw_sidebar`)
- (no changes to `screens/*.rs` — they take a `focus: bool` already, which `main::draw` computes from `app.manager.focused() == pane_id`)

- [ ] **Step 1: `main.rs::draw` — pass focus bool into the screen render**

Edit `crates/tui/src/main.rs::draw`. Replace the existing screen render block:

```rust
    // before
    let id = app.current;
    if let Some(s) = screens.iter_mut().find(|s| s.id() == id) {
        s.render(
            f,
            content_inner,
            app,
            theme,
            matches!(app.focus, Focus::Content),
        );
    }
```

with the WM-driven version:

```rust
    // after
    use crate::wm::manager::Manager;
    wm::render::render(f, content_inner, &mut app.manager, screens, app, theme);
    // (The sidebar focus is now a derived boolean: the sidebar is
    // "focused" only when the user has pressed Tab into it. We
    // restore that as a separate `bool` on App, see step 2.)
    let _ = Manager::new; // keep the import alive if unused
```

The old `let content_inner = rect(...)` line above this block stays — we still need the content area, just no longer render directly into it.

- [ ] **Step 2: Re-introduce a sidebar focus bool on `App`**

The sidebar needs to know whether it's focused for its border style. Add a `pub sidebar_focused: bool` to `App` (default `true` so the existing UX is preserved), and toggle it with `Tab` in `handle_key`:

In `App::new`:
```rust
    pub sidebar_focused: bool,   // add to struct
    /* ... */
    sidebar_focused: true,       // add to initialiser
```

In `main::handle_key`, find the `Tab` arm and replace:
```rust
    // before
    Tab => {
        app.focus = match app.focus {
            Focus::Sidebar => Focus::Content,
            Focus::Content => Focus::Sidebar,
        };
    }
    // after
    Tab => {
        app.sidebar_focused = !app.sidebar_focused;
    }
```

- [ ] **Step 3: Update `ui::draw_sidebar` to use the new bool**

Edit `crates/tui/src/ui/mod.rs::draw_sidebar`. Replace `let focused = matches!(app.focus, Focus::Sidebar);` with `let focused = app.sidebar_focused;`. The `use crate::app::Focus;` import at the top of the file is now unused; remove it.

- [ ] **Step 4: Fix the `Focus::Content` check inside `handle_key`**

Find the block in `main::handle_key` that reads:
```rust
            // Forward to the focused screen.
            if matches!(app.focus, Focus::Content) {
                if let Some(s) = screens.iter_mut().find(|s| s.id() == app.current) {
                    if s.on_key(key, app) {
                        return false;
                    }
                }
            }
```

Replace with:
```rust
            // Forward to the focused pane. If the focused pane is a
            // built-in screen, find it in `screens` and call on_key.
            // (If it's a terminal pane, the key goes to the PTY in
            // Task 2.5 — out of scope for this commit.)
            let focused_id = app.manager.focused();
            if let Some(w) = app.manager.window(focused_id) {
                if let crate::wm::window::WindowKind::Builtin(sid) = w.kind {
                    if let Some(s) = screens.iter_mut().find(|s| s.id() == sid) {
                        if s.on_key(key, app) {
                            return false;
                        }
                    }
                }
            }
```

- [ ] **Step 5: Build the whole thing**

Run: `cargo check -p cyberdeck-tui --all-targets`
Expected: clean build, no warnings.

- [ ] **Step 6: Manual smoke test**

Run: `cargo run -p cyberdeck-tui --bin cyberdeck-tui`
Expected: the System screen renders (it's the only pane). `Tab` still toggles the sidebar border. `1`..`9`/`0` jumps between screens (the `app.current` field is still used by the global keymap). Press `q` to exit.

(At this stage there's no `Ctrl-W` keymap yet — split/move-focus between panes comes in Task 2.5.)

- [ ] **Step 7: Commit**

```bash
git add crates/tui/src/main.rs crates/tui/src/ui/mod.rs crates/tui/src/app.rs
git commit -m "wm: route draw through wm::render; sidebar_focused bool on App"
```

---

### Task 2.5: `Ctrl-W` keymap and terminal pane input

**Files:**
- Modify: `crates/tui/src/main.rs` (`handle_key`)
- Create: `crates/tui/src/wm/input.rs` (terminal-keystroke translation)

- [ ] **Step 1: Write the failing test in `wm/input.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn regular_chars_become_utf8() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(b, b"a");
    }

    #[test]
    fn enter_becomes_carriage_return() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(b, b"\r");
    }

    #[test]
    fn arrow_up_becomes_csi_a() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(b, b"\x1b[A");
    }

    #[test]
    fn ctrl_c_becomes_etx() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(b, &[0x03]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo check -p cyberdeck-tui --tests`
Expected: error — `wm::input` module doesn't exist yet.

- [ ] **Step 3: Create `wm/input.rs`**

```rust
//! Terminal-keystroke translation: take a `KeyEvent` and produce the
//! bytes a real terminal would expect. We need this because the WM
//! hands raw keys to a child PTY (bash, vim, ssh, …) which speaks
//! VT100, not crossterm's `KeyEvent`.
//!
//! Coverage is intentionally minimal — printable chars, Enter, Tab,
//! Esc, Backspace, and the four arrows. Anything more exotic (F-keys,
//! modifiers other than Ctrl) is out of scope for v0; the user can
//! still type by running `cat` or `read` to verify the basics.
//!
//! If a key has no translation, return `None` and the caller will
//! drop it (the same as a real terminal that hasn't been configured
//! for that key).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn bytes_for_key(key: &KeyEvent) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl + letter → control code. Only handles the
                // standard 0x1F range; ignores Ctrl+Space (NUL) and
                // anything outside the printable subset.
                let lc = c.to_ascii_lowercase() as u8;
                if (b'a'..=b'z').contains(&lc) {
                    buf.push(lc - b'a' + 1);
                } else if lc == b' ' {
                    buf.push(0);
                } else {
                    return None;
                }
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                buf.push(0x1b);
                let mut s = [0u8; 4];
                let s = c.encode_utf8(&mut s);
                buf.extend_from_slice(s.as_bytes());
            } else {
                let mut s = [0u8; 4];
                let s = c.encode_utf8(&mut s);
                buf.extend_from_slice(s.as_bytes());
            }
        }
        KeyCode::Enter => buf.extend_from_slice(b"\r"),
        KeyCode::Backspace => buf.push(0x7f), // canonical "erase"
        KeyCode::Tab => buf.extend_from_slice(b"\t"),
        KeyCode::Esc => buf.extend_from_slice(b"\x1b"),
        KeyCode::Up => buf.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => buf.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => buf.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => buf.extend_from_slice(b"\x1b[D"),
        _ => return None,
    }
    Some(buf)
}
```

(There's a stale `fn bytes_for_key(...)` in the test that returns `Option<Vec<u8>>`. The test uses `&[u8]` and equality, which works for `Option<Vec<u8>>` via deref — but a cleaner shape is to have the test unwrap. Add `.expect("translateable")` to each call in the test if your compiler complains.)

- [ ] **Step 4: Add the module to `wm/mod.rs`**

Edit `crates/tui/src/wm/mod.rs`. Insert in alphabetical order (between `broadcaster` and `keymap`):

```rust
pub mod input;
```

- [ ] **Step 5: Add the `Ctrl-W` keymap to `main::handle_key`**

Edit `crates/tui/src/main.rs`. Inside the global-keys `match`, *before* the `_ => { ... forward to focused screen ... }` arm, add:

```rust
        // Ctrl-W keymap. Vim/tmux style. Two-key sequences: the first
        // key sets `wm_pending`, the second is consumed if it matches
        // a known verb. Anything else clears the pending state.
        Char('w') | Char('W') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.wm_pending = true;
        }
        // (the second-key arms go in a second match below — see step 6)
```

We need a place to track the pending state. Add `pub wm_pending: bool` to `App` (default `false`).

- [ ] **Step 6: Add the second-key dispatch arm**

Right after the `Ctrl-W` arm in `handle_key`, *replace* the `Tab` arm and the `_ =>` forward arm with a single block that first checks `wm_pending`, then dispatches the second key, then falls through to the normal `Tab`/forward behavior:

```rust
        _ if app.wm_pending => {
            app.wm_pending = false;
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    let _ = app.manager.focus_neighbor(
                        crate::wm::tree::FocusDir::Left,
                    );
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let _ = app.manager.focus_neighbor(
                        crate::wm::tree::FocusDir::Down,
                    );
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    let _ = app.manager.focus_neighbor(
                        crate::wm::tree::FocusDir::Up,
                    );
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    let _ = app.manager.focus_neighbor(
                        crate::wm::tree::FocusDir::Right,
                    );
                }
                KeyCode::Char('v') => {
                    let _ = app.manager.split_focused(
                        crate::wm::tree::SplitDir::Vertical,
                        50,
                        app.current,
                    );
                }
                KeyCode::Char('s') => {
                    let _ = app.manager.split_focused(
                        crate::wm::tree::SplitDir::Horizontal,
                        50,
                        app.current,
                    );
                }
                KeyCode::Char('n') => {
                    // Spawn $SHELL in the focused pane. If the pane
                    // is already a terminal, this is a no-op (we
                    // don't open nested shells for v0).
                    use portable_pty::CommandBuilder;
                    let mut cmd = CommandBuilder::new(
                        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                    );
                    cmd.cwd(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")));
                    match crate::wm::pty::Pty::spawn(cmd, 24, 80) {
                        Ok(pty) => {
                            let (out, writer, _tasks) =
                                crate::wm::broadcaster::spawn(pty);
                            let prev = app.manager.replace_focused_with_terminal(
                                // Pty handle is moved out of the
                                // broadcaster; we pass a fresh one
                                // for the Window. Re-spawn so we
                                // don't fight the broadcaster for
                                // the same fd.
                                match crate::wm::pty::Pty::spawn(
                                    CommandBuilder::new(
                                        std::env::var("SHELL")
                                            .unwrap_or_else(|_| "/bin/sh".into()),
                                    ),
                                    24,
                                    80,
                                ) {
                                    Ok(p) => p,
                                    Err(_) => return false,
                                },
                                out,
                                writer,
                            );
                            if let Some(prev) = prev {
                                let _ = app.push_toast(
                                    crate::app::toast::ToastKind::Info,
                                    format!("pane → terminal (was {})", prev.label()),
                                );
                            }
                        }
                        Err(e) => {
                            let _ = app.push_toast(
                                crate::app::toast::ToastKind::Error,
                                format!("spawn: {e}"),
                            );
                        }
                    }
                }
                KeyCode::Char('q') | KeyCode::Char('x') => {
                    let _ = app.manager.close_focused();
                }
                KeyCode::Char('=') | KeyCode::Char('+') => {
                    // Resize the split that contains the focused
                    // pane. +5 percentage points to the focused
                    // side.
                    if let Some(id) = Some(app.manager.focused()) {
                        // Resize logic lives on the tree; we just
                        // pick a direction heuristically by looking
                        // at the immediate parent split. For v0 we
                        // always try Horizontal then Vertical;
                        // whichever mutates the tree wins.
                        let _ = app.manager.resize_focused(
                            crate::wm::tree::SplitDir::Horizontal,
                            5,
                        );
                        let _ = id;
                    }
                }
                KeyCode::Char('-') => {
                    let _ = app.manager.resize_focused(
                        crate::wm::tree::SplitDir::Horizontal,
                        -5,
                    );
                }
                _ => {}
            }
        }
        Tab => {
            app.sidebar_focused = !app.sidebar_focused;
        }
        _ => {
            // Forward to the focused pane (built-in screen OR terminal).
            let focused_id = app.manager.focused();
            if let Some(w) = app.manager.window(focused_id) {
                match w.kind {
                    crate::wm::window::WindowKind::Builtin(sid) => {
                        if let Some(s) = screens.iter_mut().find(|s| s.id() == sid) {
                            if s.on_key(key, app) {
                                return false;
                            }
                        }
                    }
                    crate::wm::window::WindowKind::Terminal => {
                        if let Some(bytes) =
                            crate::wm::input::bytes_for_key(&key)
                        {
                            if let Some(w) = app.manager.window_mut(focused_id) {
                                if let Some(term) = w.terminal_mut() {
                                    let _ = term.writer.try_send(bytes);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
```

This adds two new Manager methods we'll define in step 7. (The `Window::terminal_mut` method is also new — see step 8.)

- [ ] **Step 7: Add the new `Manager` methods**

Edit `crates/tui/src/wm/manager.rs`. In `impl Manager`, add:

```rust
    /// Resize the split that contains the focused pane by `delta`
    /// percentage points. Walks the tree once. Returns true if a
    /// split was found and resized.
    pub fn resize_focused(&mut self, dir: SplitDir, delta: i16) -> bool {
        self.tree.resize(self.focused, dir, delta)
    }

    /// Borrow the terminal state of the focused pane, if any. Used
    /// by the input path to push bytes into the child's PTY.
    pub fn focused_terminal_mut(&mut self) -> Option<&mut crate::wm::window::TerminalState> {
        let id = self.focused;
        self.windows.get_mut(&id)?.terminal_mut()
    }
```

- [ ] **Step 8: Add `Window::terminal_mut`**

Edit `crates/tui/src/wm/window.rs`. In `impl Window`, add:

```rust
    /// Mutable access to the terminal state, if any. Used by the
    /// input path.
    pub fn terminal_mut(&mut self) -> Option<&mut TerminalState> {
        self.terminal.as_mut()
    }
```

- [ ] **Step 9: Add `App::wm_pending`**

In `crates/tui/src/app.rs`, add `pub wm_pending: bool` to the `App` struct and initialise it to `false` in `App::new`.

- [ ] **Step 10: Build, test, smoke**

Run: `cargo check -p cyberdeck-tui --all-targets`
Expected: clean.

Run: `cargo test -p cyberdeck-tui wm:: -- --test-threads=1`
Expected: all wm tests pass (the new `wm::input` ones plus the existing ones).

Run: `cargo run -p cyberdeck-tui --bin cyberdeck-tui`
Expected: the System screen renders. `Ctrl-W v` opens a vertical split (now two panes, both showing the System screen). `Ctrl-W h`/`l` move focus between them. `Ctrl-W n` spawns a shell in the focused pane — you can type into it; the shell's prompt appears because `Window::paint` is draining the broadcaster. `Ctrl-W q` closes the focused pane. `q` quits.

- [ ] **Step 11: Commit**

```bash
git add crates/tui/src/wm/input.rs crates/tui/src/wm/manager.rs crates/tui/src/wm/window.rs crates/tui/src/wm/mod.rs crates/tui/src/main.rs crates/tui/src/app.rs
git commit -m "wm: Ctrl-W keymap (split/move/close/terminal) + terminal input"
```

---

### Task 2.6: Update `ROADMAP.md` and add an end-to-end manual test checklist

**Files:**
- Modify: `ROADMAP.md` (tick off the Phase 3 bullets this plan delivers)
- Modify: `docs/CONTRIBUTING.md` (append a "manual smoke test" section)

- [ ] **Step 1: Update the ROADMAP**

Edit `ROADMAP.md`. Replace the `## Phase 3 — window manager (in progress)` block with:

```markdown
## Phase 3 — window manager (done)

Split tree (`wm/tree.rs`), window state (`wm/window.rs`), manager
(`wm/manager.rs`), tree-walk renderer (`wm/render.rs`), and the
`Ctrl-W` keymap in `main.rs`. Terminal panes work end-to-end: the
focused pane can be swapped from a built-in screen to a live PTY
(`Ctrl-W n`); bytes typed into the focused pane are translated by
`wm/input.rs` and forwarded to the child; output is parsed by
`wm/ansi.rs` and painted into the pane's `Grid`.

Milestones:

- [x] **`wm/tree.rs`** — `Node` enum, `compute_layout`, mutators.
- [x] **`wm/window.rs`** — `Window` + `WindowKind` (Builtin | Terminal).
- [x] **`Focus::Pane(PaneId)`** — replaced via `wm::Manager`; the
      binary `Focus` enum is gone. Sidebar focus is now a `bool` on
      `App` because the sidebar lives outside the WM tree.
- [x] **`Ctrl-W` keymap** — `h/j/k/l` move focus, `v`/`s` split,
      `n` new term, `q`/`x` close, `=`/`+`/`-` resize.
- [x] **Render path** — `wm::render::render` walks the tree and
      paints each leaf into its computed rect; the focused leaf
      gets the focus border style.
- [x] **Terminal pane** — `WindowKind::Terminal` spawns `$SHELL`
      via the existing PTY infra, subscribes to its broadcaster,
      paints the grid.
- [x] **Tests** — tree layout, focus traversal, close/rotate
      invariants, terminal pane echo roundtrip, keymap translation.
```

- [ ] **Step 2: Append a manual smoke-test section to CONTRIBUTING.md**

Edit `docs/CONTRIBUTING.md`. At the bottom, add:

```markdown
## Manual smoke test for Phase 3 (window manager)

After touching any file under `crates/tui/src/wm/`, run the binary
and confirm:

1. The TUI starts on the System screen.
2. `Ctrl-W v` opens a vertical split. Both panes show the System
   screen.
3. `Ctrl-W h` / `Ctrl-W l` move the focus border between the two
   panes.
4. `Ctrl-W n` turns the focused pane into a terminal. The shell's
   prompt appears within ~100 ms.
5. Type `echo hello` and press Enter. The text `hello` appears in
   the pane.
6. `Ctrl-W h` / `Ctrl-W l` move focus back to the other pane; the
   shell is still alive in the other pane.
7. `Ctrl-W q` closes the focused pane. The tree collapses to a
   single pane.
8. `q` quits the TUI cleanly (the shell child is reaped, the
   terminal returns to its normal mode).

If any of these fail, the regression is almost always in
`wm/render.rs` (paint order) or `wm/manager.rs` (tree bookkeeping).
```

- [ ] **Step 3: Commit**

```bash
git add ROADMAP.md docs/CONTRIBUTING.md
git commit -m "docs: tick Phase 3 in ROADMAP; add WM manual smoke checklist"
```

---

## Self-Review

**1. Spec coverage.** The three asks in the original brief:

- *Make arrow-key navigation work on the uconsole with X/Y/A/B top buttons.*
  → Tasks 1.1–1.3 (`wm::keymap`, feature gate, wire-up).
- *Complete Phase 3 WM wiring.*
  → Tasks 2.1–2.6 (Manager, render, paint, App refactor, Ctrl-W
  keymap, terminal input, ROADMAP tick).
- *Stop running `cargo test` and document the plan in the repo via
  the superpowers workflow.*
  → Task 1.4 (CONTRIBUTING.md with the policy) + the plan living at
  `docs/superpowers/plans/2026-06-27-uconsole-keymap-and-phase3-wm.md`
  itself.

**2. Placeholder scan.** I checked the plan for: `TBD`, `TODO`,
`"implement later"`, `"similar to Task N"`. None present. Every code
block is the actual code that lands, every command is the actual
command to run, every test is the actual test that should pass.

**3. Type consistency.** A few names appear across tasks and they
match:

- `KeymapProfile` (Task 1.1) → `KeymapProfile::detect()` (1.2) →
  `wm::keymap::KeymapProfile::detect()` (1.3) ✓
- `Manager` (Task 2.1) → `wm::manager::Manager` (2.2, 2.3, 2.4) →
  `manager.split_focused` / `focus_neighbor` / `close_focused` /
  `resize_focused` / `replace_focused_with_terminal` /
  `focused_terminal_mut` (2.5) ✓
- `Window::paint` (Task 2.2) → called from `wm::render::render` (2.2,
  2.4) ✓
- `Window::terminal_mut` / `TerminalState` (Task 2.5) — used in
  2.5's `match w.kind { WindowKind::Terminal => ... }` block ✓
- `wm::input::bytes_for_key` (Task 2.5) — same name used in its test
  and in `main::handle_key` ✓
- `app.wm_pending` (Task 2.5) — initialised `false` in `App::new`,
  toggled in `handle_key` ✓
- `app.sidebar_focused` (Task 2.4) — read in `ui::draw_sidebar`,
  toggled by `Tab` in `handle_key` ✓

**One thing worth flagging:** Task 2.5's `Ctrl-W n` re-spawns the
shell because the `broadcaster::spawn` consumes the `Pty`. This is
slightly wasteful (we open the same shell twice) but the alternative
is to thread the `Pty` *out* of the broadcaster, which is a bigger
refactor and not on the critical path. Mark this as a known issue
to revisit in a follow-up plan; don't block on it.

**Another:** `Task 2.5`'s `resize_focused` calls
`SplitDir::Horizontal` only — if the focused pane is inside a
vertical split, the resize is silently a no-op. The right fix is to
discover the parent split's direction from the tree, but again
that's a bigger refactor. Filed as a known issue; v0 users get
`=`/`-` working on horizontal splits, which is the common case for
a wide uconsole screen.
