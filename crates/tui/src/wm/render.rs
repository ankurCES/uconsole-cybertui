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
//!
//! The two-pass layout (collect plan → paint builtins → paint terminals)
//! exists to satisfy the borrow checker: `Window::paint` (terminal) needs
//! `&mut self` from `app.manager`, while `Screen::render` needs `&mut App`.
//! We can't hold both at once, so we plan under one borrow, release it,
//! then render each kind with its own disjoint borrow.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::app::screen::Screen;
use crate::app::App;
use crate::theme::Theme;
use crate::wm::window::WindowKind;

#[allow(dead_code)] // wired up in Task 2.3
pub fn render(
    f: &mut Frame,
    area: Rect,
    app: &mut App,
    screens: &mut [Box<dyn Screen>],
    theme: &Theme,
) {
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
}

/// Title-bar string for a pane. `index` is the 0-based leaf position
/// in DFS order (matches `Manager::pane_ids()`); the user sees a
/// 1-based badge ` [N] `.
#[allow(dead_code)] // wired up in Task 2.3
pub fn pane_title(w: &WindowKind, index: usize) -> String {
    format!(" [{}] {} ", index + 1, w.label())
}

/// Short right-aligned status hint for a terminal pane title bar.
/// `alive=true` → `" running "`, otherwise → `" exited "`. Used by
/// `Window::paint` to give the title bar a sense of liveness without
/// adding a second row.
pub fn terminal_status_hint(alive: bool) -> &'static str {
    if alive {
        " running "
    } else {
        " exited "
    }
}

#[cfg(test)]
mod tests {
    use crate::app::screen::ScreenId;
    use crate::wm::manager::Manager;
    use crate::wm::tree::SplitDir;
    use ratatui::layout::Rect;
    use super::{pane_title, terminal_status_hint};

    #[test]
    fn apply_layout_single_pane_uses_full_area() {
        // We can't easily assert on a `Frame` here, but we *can* assert
        // that `apply_layout` produced the right rects. The actual
        // pixel-level render is exercised by the manual smoke test in
        // Task 2.6.
        let mut m = Manager::new(ScreenId::System);
        let area = Rect::new(0, 0, 80, 24);
        m.apply_layout(area);
        let layout = m.layout();
        assert_eq!(layout, vec![(m.focused(), area)]);
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

    #[test]
    fn terminal_status_hint_reports_running_and_exited() {
        assert_eq!(terminal_status_hint(true), " running ");
        assert_eq!(terminal_status_hint(false), " exited ");
    }
}