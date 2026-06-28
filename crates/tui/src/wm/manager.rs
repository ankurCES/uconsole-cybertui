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

/// Error returned by `Manager::split_focused` when the requested
/// split would exceed `Manager::MAX_PANES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// `split_focused` was called when `Manager::MAX_PANES` panes already exist.
    PaneLimit,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::PaneLimit => write!(f, "pane limit reached ({})", Manager::MAX_PANES),
        }
    }
}

impl std::error::Error for SplitError {}

pub struct Manager {
    tree: Node,
    windows: HashMap<PaneId, Window>,
    focused: PaneId,
    /// Last area we laid out into. Used to drive `Window::resize` on the
    /// next call so terminal panes see a real TIOCSWINSZ.
    last_area: Rect,
}

impl Manager {
    /// Hard cap on the number of panes a single `Manager` can hold.
    /// Reaching this cap causes `split_focused` to return
    /// `SplitError::PaneLimit` rather than grow the tree further.
    pub const MAX_PANES: u8 = 9;

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

    /// Return the `PaneId` of the leaf at DFS `index`, matching the
    /// order `pane_ids()` and `layout()` use. `None` for out-of-range.
    #[allow(dead_code)] // consumed by plan §2.5 (pane enumeration); see also `pane_ids`.
    pub fn focus_pane_index(&self, index: usize) -> Option<PaneId> {
        self.tree.leaves().get(index).copied()
    }

    /// Set the focused pane to `id`. Returns false if `id` is not in
    /// the tree (e.g. a stale id from before a close).
    #[allow(dead_code)] // consumed by plan §2.5 (pane enumeration); see also `pane_ids`.
    pub fn focus_pane(&mut self, id: PaneId) -> bool {
        if self.windows.contains_key(&id) {
            self.focused = id;
            true
        } else {
            false
        }
    }

    /// Resize the split that contains the focused pane by `delta`
    /// percentage points. Walks the tree once. Returns true if a
    /// split was found and resized.
    pub fn resize_focused(&mut self, dir: SplitDir, delta: i16) -> bool {
        self.tree.resize(self.focused, dir, delta)
    }

    /// Borrow the terminal state of the focused pane, if any. Used
    /// by the input path to push bytes into the child's PTY.
    // Public API consumed only by tests and the planned Ctrl-W forwarding
    // path (see docs/superpowers/plans/.../2026-06-27-...md §2.5).
    #[allow(dead_code)]
    pub fn focused_terminal_mut(&mut self) -> Option<&mut crate::wm::window::TerminalState> {
        let id = self.focused;
        self.windows.get_mut(&id)?.terminal_mut()
    }

    // Public API used by app.rs tests and future pane enumeration (see plan §2.5).
    #[allow(dead_code)]
    pub fn pane_ids(&self) -> Vec<PaneId> { self.tree.leaves() }

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
        if self.windows.len() as u8 >= Self::MAX_PANES {
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

    /// Swap the focused pane's kind to a built-in screen. No-op if the
    /// pane is already showing that screen. Returns the previous kind.
    ///
    /// The 2-pane layout (sidebar + content) drives the content pane
    /// from `app.current` via this method: selecting a screen on the
    /// left calls `set_pane_kind(Builtin(ScreenId::N))` so the right
    /// side redraws with the new screen on the next frame.
    pub fn set_pane_kind(&mut self, kind: WindowKind) -> Option<WindowKind> {
        let id = self.focused;
        let w = self.windows.get_mut(&id)?;
        if w.kind == kind {
            return Some(kind);
        }
        // Built-in panes don't own terminal state, so swapping out a
        // terminal here would leak the PTY/broadcaster. Reject that
        // direction; the call sites only ever set Builtin.
        if !matches!(kind, WindowKind::Builtin(_)) {
            return None;
        }
        let prev = w.kind;
        // Drop any terminal state (PTY + broadcaster subscription) when
        // swapping a terminal pane to a built-in. We don't expect this
        // in the 2-pane flow (no Ctrl-W n), but the safe thing is to
        // release it cleanly.
        if matches!(prev, WindowKind::Terminal) {
            w.clear_terminal();
        }
        w.kind = kind;
        Some(prev)
    }
}

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
        let new_id = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
        assert!(m.pane_ids().contains(&new_id));
        assert_eq!(m.pane_ids().len(), before.len() + 1);
        // Newly-split pane gets focus (vim convention).
        assert_eq!(m.focused(), new_id);
    }

    #[test]
    fn close_focused_collapses_to_one_pane() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
        let _ = m.split_focused(SplitDir::Vertical, 50, ScreenId::Audio).expect("within cap");
        let _ = m.close_focused();
        let _ = m.close_focused();
        assert_eq!(m.pane_ids().len(), 1);
    }

    #[test]
    fn focus_neighbor_finds_adjacent_pane() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
        // Give the tree a real area so focus_neighbor can compute centers.
        m.apply_layout(Rect::new(0, 0, 80, 24));
        // Capture the original (left) pane id before we move focus.
        let original = m.pane_ids()[0];
        // Focus is on the new (right) pane. Go back left.
        let back = m.focus_neighbor(FocusDir::Left).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn focus_pane_index_returns_some_for_in_range_leaf() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
        let _ = m.split_focused(SplitDir::Vertical, 50, ScreenId::Audio).expect("within cap");
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
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
        let ids = m.pane_ids();
        // After split, the new pane (ids[1]) is focused, not the original (ids[0]).
        let original_focus = m.focused();
        assert_eq!(original_focus, ids[1]);
        assert_ne!(original_focus, ids[0]);
        // Focus the original (now unfocused) pane.
        assert!(m.focus_pane(ids[0]));
        assert_eq!(m.focused(), ids[0]);
    }

    #[test]
    fn focus_pane_returns_false_for_stale_id() {
        let mut m = Manager::new(ScreenId::System);
        let _ = m.split_focused(SplitDir::Horizontal, 50, ScreenId::Network).expect("within cap");
        let stale = PaneId(999_999);
        assert!(!m.focus_pane(stale));
        // Focused pane is unchanged.
        assert_eq!(m.focused(), m.pane_ids()[1]);
    }

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

    #[test]
    fn set_pane_kind_swaps_focused_builtin_screen() {
        // The 2-pane layout drives the content pane from `app.current`
        // via `set_pane_kind`. Verify the contract: swaps the kind,
        // returns the previous kind, leaves no terminal state behind.
        let mut m = Manager::new(ScreenId::System);
        let prev = m.set_pane_kind(WindowKind::Builtin(ScreenId::Network));
        assert_eq!(prev, Some(WindowKind::Builtin(ScreenId::System)));
        let w = m.window(m.focused()).unwrap();
        assert_eq!(w.kind, WindowKind::Builtin(ScreenId::Network));
        assert!(!w.is_terminal());
    }

    #[test]
    fn set_pane_kind_is_noop_when_already_target() {
        let mut m = Manager::new(ScreenId::System);
        let prev = m.set_pane_kind(WindowKind::Builtin(ScreenId::System));
        assert_eq!(prev, Some(WindowKind::Builtin(ScreenId::System)));
    }
}
