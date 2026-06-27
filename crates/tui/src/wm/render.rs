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
    let plan: Vec<(crate::wm::broadcaster::PaneId, Rect, WindowKind, bool)> = {
        let manager = &mut app.manager;
        manager.apply_layout(area);
        let focused = manager.focused();
        manager
            .layout()
            .into_iter()
            .filter_map(|(id, rect)| {
                let w = manager.window(id)?;
                let is_focused = id == focused;
                Some((id, rect, w.kind, is_focused))
            })
            .collect()
    };

    // Pass 2 — built-in panes: dispatch into `Screen::render`, which
    // needs `&mut App` + `&mut [Box<dyn Screen>]`. No manager borrow.
    for (id, rect, kind, focused) in &plan {
        if let WindowKind::Builtin(sid) = kind {
            if let Some(s) = screens.iter_mut().find(|s| s.id() == *sid) {
                s.render(f, *rect, app, theme, *focused);
            }
            let _ = id; // id unused for builtins
        }
    }

    // Pass 3 — terminal panes: needs `&mut Window`, so we re-borrow
    // `&mut app.manager`. This pass doesn't touch `app` outside
    // `app.manager`, so it doesn't conflict with pass 2 (sequential).
    for (id, _rect, kind, focused) in &plan {
        if matches!(kind, WindowKind::Terminal) {
            if let Some(w) = app.manager.window_mut(*id) {
                w.paint(f, *_rect, theme, *focused);
            }
        }
    }
}

/// Title-bar string for a pane. Kept here (not in `Window`) because
/// ratatui's `Block` builder is what we pass it to.
#[allow(dead_code)] // wired up in Task 2.3
pub fn pane_title(w: &WindowKind) -> String {
    format!(" {} ", w.label())
}

#[cfg(test)]
mod tests {
    use crate::app::screen::ScreenId;
    use crate::wm::manager::Manager;
    use crate::wm::tree::SplitDir;
    use ratatui::layout::Rect;

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
}