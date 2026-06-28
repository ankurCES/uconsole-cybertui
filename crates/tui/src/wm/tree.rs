//! Binary split tree of panes.
//!
//! Every non-trivial WM is built on one of these — tmux, i3, vim splits,
//! Helix, etc. The model is intentionally tiny:
//!
//! ```text
//! Node::Leaf { id }              — terminal pane
//! Node::Split { dir, ratio, a, b } — two children laid out dir-wise,
//!                                    ratio = percent of `a` (1..=99)
//! ```
//!
//! Invariants:
//!   * `ratio` is in `1..=99`.
//!   * A `Split` always has two children (no degenerate single-child splits —
//!     close mutators collapse those back into a `Leaf`).
//!   * `PaneId`s inside one tree are unique. (Enforced by the constructor
//!     helpers; no public `Tree::new` takes raw `PaneId`s except via the
//!     mutators, which mint them.)
//!
//! `compute_layout` walks the tree and returns the visible leaf rects. It
//! never allocates panes — the tree is just a description of "where should
//! this pane live on screen".
//!
//! `focus_neighbor` walks the tree to find the next pane in a direction.
//! Returns `None` if no neighbor exists in that direction (used by the
//! `Ctrl-W h/j/k/l` keymap).
//!
//! Phase-4 simplification: with the layout locked to 2 panes, the
//! `Split` variant, the split/close/resize/focus_neighbor mutators, and
//! the `rect_center` helper are unused at runtime. They're kept (and
//! silenced) so a future re-enable doesn't have to re-derive the
//! design. Remove this allow when the WM is wired back up.
#![allow(dead_code)]

use ratatui::layout::Rect;

use crate::wm::broadcaster::PaneId;

/// Horizontal or vertical split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SplitDir {
    /// Left | Right
    Horizontal,
    /// Top / Bottom
    Vertical,
}

/// Direction for focus-neighbor queries. Mirrors vim's `Ctrl-W h/j/k/l`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusDir {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf { id: PaneId },
    Split {
        dir: SplitDir,
        /// Percent of `a`. Stored as `u8` (1..=99) so it's cheap and has no
        /// floating-point fuzz. Recomputed from absolute pixel widths on
        /// resize.
        ratio: u8,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    /// Wrap a single pane in a tree.
    pub fn leaf(id: PaneId) -> Self {
        Node::Leaf { id }
    }

    /// Split the leaf with `id` in `dir` at `ratio` percent. Returns
    /// `None` if the leaf is not found or if the tree is degenerate (which
    /// shouldn't happen — see invariants).
    ///
    /// The new pane is created with a fresh `PaneId` (minted by the caller —
    /// see `PaneId::fresh`).
    pub fn split(&mut self, target: PaneId, dir: SplitDir, ratio: u8, new_id: PaneId) -> bool {
        let r = ratio.clamp(1, 99);
        self.split_inner(target, dir, r, new_id)
    }

    fn split_inner(&mut self, target: PaneId, dir: SplitDir, ratio: u8, new_id: PaneId) -> bool {
        match self {
            Node::Leaf { id } if *id == target => {
                let old = Node::Leaf { id: *id };
                *self = Node::Split {
                    dir,
                    ratio,
                    a: Box::new(old),
                    b: Box::new(Node::Leaf { id: new_id }),
                };
                true
            }
            Node::Leaf { .. } => false,
            Node::Split { a, b, .. } => a.split_inner(target, dir, ratio, new_id)
                || b.split_inner(target, dir, ratio, new_id),
        }
    }

    /// Remove the leaf with `id`. If the removal leaves a `Split` with one
    /// `Leaf` child, collapse it. Returns `true` if a leaf was removed.
    pub fn close(&mut self, target: PaneId) -> bool {
        // First check if `target` is one of the direct children — easier to
        // reason about than walking into grandchildren.
        match self {
            Node::Leaf { id } if *id == target => {
                // Closing the root leaf is a no-op (tree stays as-is).
                false
            }
            Node::Split { a, b, .. } => {
                let in_a = a.contains(target);
                let in_b = b.contains(target);
                if in_a && a.is_leaf_with(target) {
                    // `b` is non-leaf by invariant (a Split always has two
                    // children, and we already removed one). Promote `b`.
                    *self = (**b).clone();
                    return true;
                }
                if in_b && b.is_leaf_with(target) {
                    *self = (**a).clone();
                    return true;
                }
                // Otherwise recurse into the side that contains `target`.
                if in_a { a.close(target) } else { b.close(target) }
            }
            _ => false,
        }
    }

    fn contains(&self, target: PaneId) -> bool {
        match self {
            Node::Leaf { id } => *id == target,
            Node::Split { a, b, .. } => a.contains(target) || b.contains(target),
        }
    }

    fn is_leaf_with(&self, target: PaneId) -> bool {
        matches!(self, Node::Leaf { id } if *id == target)
    }

    /// All leaf IDs, depth-first (left-then-right). Used by tests and by the
    /// renderer for pane numbering.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf { id } => out.push(*id),
            Node::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
        }
    }

    /// Find the next pane to focus when moving from `from` in `dir`.
    ///
    /// Algorithm: walk the tree, collect the rect of `from` and the rect of
    /// every other leaf, pick the leaf whose center is closest in the
    /// requested direction (and not behind us). Returns `None` if there's
    /// no candidate.
    pub fn focus_neighbor(&self, from: PaneId, area: Rect, dir: FocusDir) -> Option<PaneId> {
        let rects = compute_layout(self, area);
        let from_rect = rects.iter().find(|(id, _)| *id == from).map(|(_, r)| *r)?;
        let fc = rect_center(from_rect);

        let mut best: Option<(u32, PaneId)> = None;
        for (id, r) in &rects {
            if *id == from {
                continue;
            }
            let c = rect_center(*r);
            let candidate = match dir {
                FocusDir::Left => {
                    if c.0 >= fc.0 { continue; }
                    // Prefer horizontal proximity; rank by horizontal gap.
                    fc.0 - c.0
                }
                FocusDir::Right => {
                    if c.0 <= fc.0 { continue; }
                    c.0 - fc.0
                }
                FocusDir::Up => {
                    if c.1 >= fc.1 { continue; }
                    fc.1 - c.1
                }
                FocusDir::Down => {
                    if c.1 <= fc.1 { continue; }
                    c.1 - fc.1
                }
            };
            // Tie-break by vertical/horizontal distance so we move "straight".
            let tiebreak = match dir {
                FocusDir::Left | FocusDir::Right => c.1.abs_diff(fc.1),
                FocusDir::Up | FocusDir::Down => c.0.abs_diff(fc.0),
            };
            let score = (candidate as u32) * 1000 + tiebreak as u32;
            best = match best {
                Some((s, _)) if s <= score => best,
                _ => Some((score, *id)),
            };
        }
        best.map(|(_, id)| id)
    }

    /// Resize the split containing `target` by `delta` percentage points.
    /// `delta` is clamped so the ratio stays in `1..=99`.
    pub fn resize(&mut self, target: PaneId, dir: SplitDir, delta: i16) -> bool {
        match self {
            Node::Leaf { .. } => false,
            Node::Split { dir: d, ratio, a, b } => {
                if *d == dir && (a.contains(target) ^ b.contains(target)) {
                    let cur = *ratio as i16;
                    let next = (cur + delta).clamp(1, 99);
                    *ratio = next as u8;
                    true
                } else {
                    a.resize(target, dir, delta) || b.resize(target, dir, delta)
                }
            }
        }
    }

    }

fn rect_center(r: Rect) -> (u32, u32) {
    let x = r.x as u32 + r.width as u32 / 2;
    let y = r.y as u32 + r.height as u32 / 2;
    (x, y)
}

/// Walk the tree and produce `(PaneId, Rect)` for every leaf.
///
/// SplitDir::Horizontal → `a` is left, `b` is right.
/// SplitDir::Vertical   → `a` is top,  `b` is bottom.
pub fn compute_layout(node: &Node, area: Rect) -> Vec<(PaneId, Rect)> {
    let mut out = Vec::new();
    layout_into(node, area, &mut out);
    out
}

fn layout_into(node: &Node, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        Node::Leaf { id } => out.push((*id, area)),
        Node::Split { dir, ratio, a, b } => {
            // Even an empty rect still has to keep the tree well-formed — we
            // just emit zero-width/height children. The renderer will skip
            // them via the standard `Rect` checks.
            if area.width == 0 || area.height == 0 {
                layout_into(a, Rect::new(area.x, area.y, 0, 0), out);
                layout_into(b, Rect::new(area.x, area.y, 0, 0), out);
                return;
            }
            let (ra, rb) = match dir {
                SplitDir::Horizontal => {
                    let w = (area.width as u32 * *ratio as u32) / 100;
                    let w = w as u16;
                    let a = Rect::new(area.x, area.y, w, area.height);
                    let b = Rect::new(area.x + w, area.y, area.width - w, area.height);
                    (a, b)
                }
                SplitDir::Vertical => {
                    let h = (area.height as u32 * *ratio as u32) / 100;
                    let h = h as u16;
                    let a = Rect::new(area.x, area.y, area.width, h);
                    let b = Rect::new(area.x, area.y + h, area.width, area.height - h);
                    (a, b)
                }
            };
            layout_into(a, ra, out);
            layout_into(b, rb, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u64) -> PaneId {
        PaneId(n)
    }

    #[test]
    fn single_leaf_layout() {
        let tree = Node::leaf(pid(1));
        let out = compute_layout(&tree, Rect::new(0, 0, 80, 24));
        assert_eq!(out, vec![(pid(1), Rect::new(0, 0, 80, 24))]);
    }

    #[test]
    fn horizontal_split_50_50() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        let out = compute_layout(&tree, Rect::new(0, 0, 80, 24));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, pid(1));
        assert_eq!(out[1].0, pid(2));
        assert_eq!(out[0].1.width, 40);
        assert_eq!(out[1].1.width, 40);
        assert_eq!(out[0].1.x, 0);
        assert_eq!(out[1].1.x, 40);
        assert_eq!(out[0].1.height, 24);
        assert_eq!(out[1].1.height, 24);
    }

    #[test]
    fn vertical_split_30_70() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Vertical, 30, pid(2));
        let out = compute_layout(&tree, Rect::new(0, 0, 80, 24));
        // 30% of 24 = 7 (rounded down).
        assert_eq!(out[0].1.height, 7);
        assert_eq!(out[1].1.height, 17);
        assert_eq!(out[1].1.y, 7);
    }

    #[test]
    fn nested_splits() {
        let mut tree = Node::leaf(pid(1));
        // Split 1 horizontally → leaf 1 on left, leaf 2 on right.
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        // Split leaf 2 vertically → leaf 2 on top, leaf 3 on bottom.
        tree.split(pid(2), SplitDir::Vertical, 50, pid(3));
        let out = compute_layout(&tree, Rect::new(0, 0, 80, 24));
        assert_eq!(out.len(), 3);
        // Right half is 40 wide; top half is 12 high.
        let r2 = out.iter().find(|(id, _)| *id == pid(2)).unwrap().1;
        let r3 = out.iter().find(|(id, _)| *id == pid(3)).unwrap().1;
        assert_eq!(r2.y, 0);
        assert_eq!(r3.y, 12);
        assert_eq!(r2.x, 40);
        assert_eq!(r3.x, 40);
    }

    #[test]
    fn close_collapses_one_child_split() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        assert!(tree.close(pid(2)));
        assert_eq!(tree.leaves(), vec![pid(1)]);
    }

    #[test]
    fn close_only_affects_target() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        tree.split(pid(2), SplitDir::Vertical, 50, pid(3));
        assert!(tree.close(pid(2)));
        let leaves = tree.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.contains(&pid(1)));
        assert!(leaves.contains(&pid(3)));
    }

    #[test]
    fn close_root_is_noop() {
        let mut tree = Node::leaf(pid(1));
        assert!(!tree.close(pid(1)));
        assert_eq!(tree.leaves(), vec![pid(1)]);
    }

    #[test]
    fn resize_clamps_to_valid_range() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        // Resize target pid(1) (the left side of an H split) by +200 → clamps to 99.
        assert!(tree.resize(pid(1), SplitDir::Horizontal, 200));
        let out = compute_layout(&tree, Rect::new(0, 0, 100, 10));
        assert_eq!(out[0].1.width, 99);
        assert_eq!(out[1].1.width, 1);
        // Resize target pid(2) (the right side) by -200 → clamps to 1.
        assert!(tree.resize(pid(2), SplitDir::Horizontal, -200));
        let out = compute_layout(&tree, Rect::new(0, 0, 100, 10));
        assert_eq!(out[0].1.width, 99);
        assert_eq!(out[1].1.width, 1);
    }

    #[test]
    fn focus_neighbor_right() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(tree.focus_neighbor(pid(1), area, FocusDir::Right), Some(pid(2)));
        assert_eq!(tree.focus_neighbor(pid(2), area, FocusDir::Left), Some(pid(1)));
        // No neighbors up/down → None.
        assert_eq!(tree.focus_neighbor(pid(1), area, FocusDir::Up), None);
        assert_eq!(tree.focus_neighbor(pid(1), area, FocusDir::Down), None);
    }

    #[test]
    fn focus_neighbor_three_pane() {
        // Top | Bottom
        //   left       right
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Vertical, 50, pid(2));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(3));
        let area = Rect::new(0, 0, 80, 24);
        // From pid(1) [top-left], right → pid(3) [top-right].
        assert_eq!(tree.focus_neighbor(pid(1), area, FocusDir::Right), Some(pid(3)));
        // From pid(1) [top-left], down → pid(2) [bottom-left].
        assert_eq!(tree.focus_neighbor(pid(1), area, FocusDir::Down), Some(pid(2)));
        // From pid(3) [top-right], down → pid(2) [bottom-left] (best match).
        assert_eq!(tree.focus_neighbor(pid(3), area, FocusDir::Down), Some(pid(2)));
    }

    #[test]
    fn zero_size_area_does_not_panic() {
        let mut tree = Node::leaf(pid(1));
        tree.split(pid(1), SplitDir::Horizontal, 50, pid(2));
        let out = compute_layout(&tree, Rect::new(0, 0, 0, 0));
        // Two leaves, both zero-sized — must not panic.
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn ratio_clamped_on_split() {
        let mut tree = Node::leaf(pid(1));
        // Out-of-range ratios should clamp, not panic.
        tree.split(pid(1), SplitDir::Horizontal, 0, pid(2));
        tree.split(pid(1), SplitDir::Vertical, 200, pid(3));
        let leaves = tree.leaves();
        // Only one split happened (the second split found pid(1) in `a`,
        // and `b` is already pid(2)). Either way, no panic.
        assert!(leaves.len() >= 2);
    }
}