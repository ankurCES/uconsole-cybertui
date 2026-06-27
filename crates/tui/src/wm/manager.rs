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
        // Give the tree a real area so focus_neighbor can compute centers.
        m.apply_layout(Rect::new(0, 0, 80, 24));
        // Capture the original (left) pane id before we move focus.
        let original = m.pane_ids()[0];
        // Focus is on the new (right) pane. Go back left.
        let back = m.focus_neighbor(FocusDir::Left).unwrap();
        assert_eq!(back, original);
    }
}
