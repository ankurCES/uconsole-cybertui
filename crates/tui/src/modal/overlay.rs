use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Clear};
use ratatui::Frame;

use super::Modal;

/// Center a `(width × height)` box inside `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Dim the full terminal area, then render `modal` in a centered popup.
///
/// Width/height are hardcoded for an 80×24 target:
///   - 48 cols wide (60% of 80), 7 rows tall.
pub fn render_modal_overlay(frame: &mut Frame, area: Rect, modal: &dyn Modal, theme: &Theme) {
    // Dim background — a full-area Clear + dark block dims whatever was drawn behind.
    let dim_block = Block::default().style(Style::default().bg(theme.bg).fg(theme.dim));
    frame.render_widget(Clear, area);
    frame.render_widget(dim_block, area);

    let popup = centered_rect(48, 7, area);
    modal.render(frame, popup, theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_fits_inside_area() {
        let area = Rect { x: 0, y: 0, width: 80, height: 24 };
        let r = centered_rect(48, 7, area);
        assert!(r.x + r.width <= area.x + area.width);
        assert!(r.y + r.height <= area.y + area.height);
    }

    #[test]
    fn centered_rect_clamps_when_larger_than_area() {
        let area = Rect { x: 0, y: 0, width: 20, height: 5 };
        let r = centered_rect(48, 7, area);
        assert_eq!(r.width, 20);
        assert_eq!(r.height, 5);
    }
}
