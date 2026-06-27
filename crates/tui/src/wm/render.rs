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
// `Line`/`Span`/`Block`/`Borders` are used by the Window::paint body
// (see `window.rs`); re-exported here with `_`-prefixed aliases below
// for the convenience of any future helper that wants to compose a
// title bar the same way.
#[allow(unused_imports)]
use ratatui::text::{Line, Span};
#[allow(unused_imports)]
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::screen::Screen;
use crate::app::App;
use crate::theme::Theme;
use crate::wm::manager::Manager;
use crate::wm::window::WindowKind;

#[allow(dead_code)] // wired up in Task 2.3
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
#[allow(dead_code)] // wired up in Task 2.3
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screen::ScreenId;
    use crate::theme::Theme;
    use crate::wm::tree::SplitDir;
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