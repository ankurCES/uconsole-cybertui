# Pane number badges + Ctrl-W N jump — design

**Date:** 2026-06-27
**Status:** proposed
**Phase:** 4 polish (first item in `ROADMAP.md`)
**Plan this implements parts of:** the
[`2026-06-27-uconsole-keymap-and-phase3-wm`](../plans/2026-06-27-uconsole-keymap-and-phase3-wm.md)
plan (Phase 3 is shipped; this is the next chunk).

---

## 1. Goal

Make pane focus discoverable. Today the WM draws a brighter border on
the focused pane and that's it; with two or three panes open the user
has to read the border to know which one their next key will land in.
The fix: number each pane in the title bar, and add a `Ctrl-W N` jump
verb so the user can move focus by typing the number.

**Why now.** Phase 3 is shipped (`dba9586`) and the WM is feature-complete
for v0. Phase 4 polish is the next chunk in `ROADMAP.md`; pane badges
are the smallest, most user-visible item on that list.

## 2. Non-goals

- Multi-digit pane numbers. v0 is `1..9` only.
- Persisting pane count or numbering across TUI restarts.
- Per-pane badge style configuration (e.g. user-chosen colours).
- `Ctrl-W 0` jump. `0` stays unbound (reserved for a possible future
  "last pane" verb; bare `0` already jumps to Settings).
- A pane-number read-out in the status bar. The title-bar badge is the
  single source of truth.
- Refactoring `Node::rotate` or the `Ctrl-W n` double-spawn — those
  are tracked separately in `ROADMAP.md` under "Known issues".

## 3. User-visible behaviour

| Trigger | Behaviour |
| --- | --- |
| Open the TUI | One pane, title reads ` [1] System ` |
| `Ctrl-W v` (split vertically) | Two panes: ` [1] System `, ` [2] System `. New pane is focused. |
| `Ctrl-W h` / `l` | Focus moves; badges do not change. |
| `Ctrl-W 1` .. `Ctrl-W 9` | Focus jumps to the pane whose badge shows that number. |
| `Ctrl-W 5` when only 2 panes exist | Toast: `warn: no pane 5` |
| Close a pane (`Ctrl-W q`) | Remaining panes renumber contiguously from 1. |
| Open a 10th pane | Blocked. `Ctrl-W v` / `Ctrl-W s` are no-ops with toast `warn: pane limit reached (9)` |
| Bare `1` .. `9` / `0` (no Ctrl-W) | Unchanged: jumps to a screen, not a pane. |

### Title-bar format

Before: ` System ` or ` terminal `.
After:  ` [1] System ` or ` [2] terminal `.

Badge is left-aligned, single space, square brackets, no padding.
Title is a single `Block::title` span — no right-alignment hack.

## 4. Architecture

### 4.1 `wm::manager` — new methods

```rust
/// Index of the pane in DFS order (matches `layout()` and `pane_ids()`).
/// Returns None for out-of-range indices. Pure, `&self`.
pub fn focus_pane_index(&self, index: usize) -> Option<PaneId>;

/// Set the focused pane. Returns false if `id` is not in the tree
/// (e.g. a stale id from before a close).
pub fn focus_pane(&mut self, id: PaneId) -> bool;

/// Currently returns bare `PaneId`. CHANGED to `Result<PaneId, SplitError>`
/// so the cap is visible to the caller. Pane 9 splits return
/// `Err(SplitError::PaneLimit)`.
pub fn split_focused(
    &mut self,
    dir: SplitDir,
    ratio: u8,
    screen: ScreenId,
) -> Result<PaneId, SplitError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// `split_focused` was called when 9 panes already exist.
    PaneLimit,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::PaneLimit => write!(f, "pane limit reached (9)"),
        }
    }
}

impl std::error::Error for SplitError {}
```

The cap is `Manager::MAX_PANES = 9`, exposed as an associated constant
so the limit is documented at the type and the toast message can cite
the number.

### 4.2 `wm::render::pane_title` — signature change

```rust
// Before:
pub fn pane_title(w: &WindowKind) -> String;

// After:
pub fn pane_title(w: &WindowKind, index: usize) -> String;
```

Renders ` [N] {label} ` where `N = index + 1` (manager is 0-indexed,
user sees 1-indexed). Only one caller (`Window::paint`) — easy to
update.

### 4.3 `wm::render::render` — thread the index through

The plan tuple grows by one `usize`:

```rust
let plan: Vec<(PaneId, Rect, WindowKind, bool, usize)> = ...;
//                                       ^^^^  added: 0-based pane index
```

Built via `.into_iter().enumerate()`. Passed to `Window::paint` as the
new 5th argument (or via a tiny struct if the tuple gets too long —
unlikely, 5 fields is fine).

### 4.4 `Window::paint` — picks up the new title

Already delegates title rendering to `wm::render::pane_title`. Just
takes the extra index argument and forwards it.

### 4.5 `main::handle_key` — Ctrl-W 1..9 + split cap errors

Two changes inside the existing `_ if app.wm_pending =>` arm:

```rust
// New: jump arm, placed alongside the existing verbs.
KeyCode::Char(c) if ('1'..='9').contains(&c) => {
    let target = (c as u8 - b'1') as usize;
    match app.manager.focus_pane_index(target) {
        Some(id) => { let _ = app.manager.focus_pane(id); }
        None => {
            let _ = app.push_toast(
                ToastKind::Warn,
                format!("no pane {}", target + 1),
            );
        }
    }
}

// Existing Ctrl-W v / s arms: change from
//     let _ = app.manager.split_focused(...);
// to
//     if let Err(e) = app.manager.split_focused(...) {
//         let _ = app.push_toast(ToastKind::Warn, e.to_string());
//     }
```

Bare `1`..`9` / `0` at `main.rs:616-625` is unchanged — that's screen
jump, not pane jump.

## 5. Data flow — Ctrl-W 2 with 3 panes

```
User presses Ctrl-W then 2
    ↓
crossterm event → KeyEvent
    ↓
main::handle_key:
  1. keymap (uconsole / desktop)         ← unchanged
  2. modal dispatch                       ← unchanged
  3. global-keys match                    ← unchanged
  4. first Ctrl-W: arm wm_pending         ← unchanged (existing)
  5. second key '2' (event B): _ if app.wm_pending
       KeyCode::Char('2') arm:
         target = 1
         app.manager.focus_pane_index(1) → Some(pane_id_2)
         app.manager.focus_pane(pane_id_2) → self.focused = pane_id_2
    ↓
redraw flagged → run_app loop → wm::render::render
    ↓
plan[i] now carries indices 0..k-1, Window::paint calls
pane_title(&w.kind, i) for each pane
    ↓
title bars now show " [1] X " / " [2] Y " / " [3] Z "
```

## 6. Error handling

| Error | Surface | Recoverable? |
| --- | --- | --- |
| `Ctrl-W N` with no pane at that index | `app.push_toast(Warn, "no pane N")` | Yes — user can `Ctrl-W 1..9` again or pick a valid number |
| `Ctrl-W v/s` at pane limit | `app.push_toast(Warn, "pane limit reached (9)")` | Yes — close a pane with `Ctrl-W q` |
| `Manager::focus_pane(stale_id)` | Returns `false`; caller (currently `handle_key`) treats as no-op | Yes — UI redraws with the focused pane unchanged |
| Invalid PaneId reaching `focus_pane_index` | Impossible — index is computed from the manager's own `tree.leaves()` | n/a |

The toast queue is the existing `App::toasts: Vec<Toast>` rendered by
`ui::draw_toasts`. No new plumbing.

## 7. Testing

Targeted tests only, per the repo's `cargo test` policy
(`docs/CONTRIBUTING.md`).

### `wm::manager::tests`

- `focus_pane_index_returns_some_for_in_range_leaf`
- `focus_pane_index_returns_none_for_out_of_range`
- `focus_pane_swaps_focus`
- `focus_pane_returns_false_for_stale_id`
- `split_focused_at_limit_returns_err`

Run with:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::manager:: \
    -- --test-threads=1
```

### `wm::render::tests`

Existing tests stay. Add:

- `pane_title_includes_index_and_label`

Run with:

```bash
cargo test -p cyberdeck-tui --bin cyberdeck-tui wm::render:: \
    -- --test-threads=1
```

### Manual smoke test (added to `docs/CONTRIBUTING.md`)

1. Open TUI. Title reads ` [1] System `.
2. `Ctrl-W v`. Two panes: ` [1] System `, ` [2] System `.
3. `Ctrl-W h`. Focus border moves left; both badges unchanged.
4. `Ctrl-W 1`. Focus jumps to pane 1.
5. Split four more times (via `Ctrl-W v` and `Ctrl-W s`). Six panes,
   badges ` [1] `..` [6] `.
6. Open three more (via `Ctrl-W v` until 9 panes).
7. `Ctrl-W v` on pane 9. No new pane appears; toast at the bottom
   reads `pane limit reached (9)`.
8. `Ctrl-W 1`. Focus jumps to pane 1.
9. `Ctrl-W q` on pane 9. Eight panes remain; badges ` [1] `..` [8] `.
10. `Ctrl-W 9`. Toast `no pane 9` (only 8 exist).
11. `Ctrl-W 8`. Focus jumps to pane 8.

## 8. Rollout

One commit. No migration; no backward compat needed (the title format
change is purely additive).

```
docs/superpowers/specs/2026-06-27-pane-badges-design.md   ← this file
crates/tui/src/wm/manager.rs     (new methods, SplitError, MAX_PANES)
crates/tui/src/wm/render.rs      (pane_title signature, plan tuple)
crates/tui/src/wm/window.rs      (paint takes index)
crates/tui/src/main.rs           (Ctrl-W 1..9 arm; split Err toast)
docs/CONTRIBUTING.md             (append the smoke test)
ROADMAP.md                       (tick the badges bullet)
```

## 9. Open questions

None. The three brainstorming questions are resolved (cap = 9, badge
placement = left, cap-reached behaviour = block with toast).