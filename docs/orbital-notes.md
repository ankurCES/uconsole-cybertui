# Orbital (moclg/orbital) — notes & adoption plan

Captured during the Phase 6 UX sweep on PR #5. Goal: borrow the
**chrome + popup** ideas that map cleanly onto our `wm::{manager,tree,render,window}`
without dragging in a Z-stack, mouse, or a real compositor.

## What orbital does well

1. **Overlay layering with priority input.** Their `Explorer` overlay is a
   full-screen `Block::default().borders(Borders::ALL).border_type(BorderType::Double)`
   drawn after `frame.render_widget(Clear, rect)` so the underlying dashboard
   is wiped inside the overlay's rect. Input is routed with a single
   `if self.explorer.active` guard at the top of the main loop — the
   overlay owns the keystrokes until it closes.

2. **A small, opinionated theme.** `theme.rs` is a `pub struct Theme` with
   `const Color` palette and a handful of `Style` builders:
   `border_focused`, `border_unfocused`, `title_focused`, `title_unfocused`,
   `highlight`, `key_hint`, `text`, `label`, `good`, `warn`, `bad`, `accent`.
   ~80 lines total. Every widget imports the same five builders, so the
   look is consistent across panels without anyone hand-rolling styles.

3. **Bordered block + title for every panel.** Each widget renders inside
   `Block::default().borders(Borders::ALL)` with a title, never bare text.
   The double border (`BorderType::Double`) is reserved for "this is special"
   (overlays, focused panels). Standard panels use the default single.

4. **Boot sequence + a `?` overlay for help.** Same primitive — bordered
   block, centered title — used for both. Means the overlay infrastructure
   is one thing, not five.

5. **Tab/Shift-Tab navigation with hidden-widget skipping.** Tabs step over
   `!is_visible()` widgets in a small loop, so panels can opt out
   conditionally without breaking the focus order.

6. **Status hints in the chrome.** Footer/header text right-aligned in the
   title bar carries transient state (`Deleted successfully`, blocked path
   errors) without needing a separate toast line.

## What orbital deliberately does NOT do (and we shouldn't either)

- **No Z-stack / floating windows.** Overlays are full-screen modals.
  We will follow the same constraint — popups float visually but live
  semantically on top of a single focused pane.
- **No mouse.** Orbital is keyboard-first; we are too. No mouse hooks here.
- **No scrollbars.** Lists scroll inside their panel; we already do the same.
- **No drag/resize.** Compositor-level concerns.

## What we'll adopt in PR #5

Two contained changes that ride on top of our existing tree model:

### A. `wm::popup` module with a shadow band

New file `crates/tui/src/wm/popup.rs`:

- `pub struct Popup { rect: Rect, title: String, hint: Option<String> }`
- `pub fn render(f: &mut Frame, popup: &Popup)` — paints
  `Clear` over the inner rect, a single-bordered block with a centered
  title, and an ASCII "shadow band" one column right and one row below
  using `Theme::SURFACE` style against the pane background. Gives the
  visual sensation of a floating panel without an alpha channel.
- Screens ask for a popup instead of hand-rolling centering math.

**Canary:** migrate `AuthFailure` (the smallest modal) to use the new
helper. Other modals (`Secret`, `Choice`, `Wizard`, `Progress`) keep their
old path in this PR; they migrate in follow-ups so we don't bundle a
modal-stack refactor with chrome polish.

**Tests:** pure layout-math tests for `popup::centered_rect(parent, w, h)`
that assert:
- center within a 80×24 rect yields `Rect::new(28, 9, 24, 5)` for `w=24, h=5`
- width/height clamp to parent bounds when requested is larger
- minimum 3×3 is enforced (so the shadow band always fits or is dropped)

### B. Title-bar polish in `wm::render::render_title`

Today the badge `[N]` already appears (via `pane_title`), but built-in
panes that come up *first* (single-pane TUI) never get the badge because
the manual smoke test in CONTRIBUTING notes that only terminal titles
show it. Closing that:

- Extend the existing `pane_title` test contract so built-in panes always
  emit ` [N] Screen ` (already true today — `pane_title` is keyed on
  `WindowKind::Builtin(ScreenId)`, the badge has always been there).
  Verify no regressions in the manual smoke test (Phase 4 checklist in
  CONTRIBUTING already covers this).
- Add a right-aligned status hint on terminal panes: ` running ` (while
  alive) or ` exited [N] ` after the PTY reaps. Single span, Theme::DIM.

We will NOT touch the tree, the focus protocol, the modal flag set, or
the screen trait shape.

## Out of scope (follow-ups)

- Modal stack (today modals are booleans on `App`).
- Floating/reparentable windows with a Z axis.
- Mouse-driven drag/resize of panes.
- Color/contrast palette rewrite (our `theme.rs` already has focused/
  unfocused builders; we're not refactoring it in this PR).