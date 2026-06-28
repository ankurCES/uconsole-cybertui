//! Floating popup helper — a centered, bordered block with a shadow band.
//!
//! Inspired by the overlay chrome in `moclg/orbital` (see
//! `docs/orbital-notes.md`): each popup is a `Block` with a centered
//! title, rendered after `Clear` so it owns the cells underneath, with
//! a one-cell "shadow" painted one column right and one row below to
//! suggest depth without an alpha channel.
//!
//! This is intentionally minimal: no focus stack, no animation, no
//! mouse. Screens that want a popup build a `Popup` and call
//! `render` from their `Screen::render`. The popup lives inside the
//! focused pane — full-screen overlays belong on the main loop, not
//! here.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::Theme;

/// A floating popup. `title` is centered on the top border. `body`
/// is a single-line prompt or a multi-line paragraph. `hint` is the
/// right-aligned keybinding hint at the bottom (e.g. `[enter] continue`).
pub struct Popup<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub hint: Option<&'a str>,
}

impl<'a> Popup<'a> {
    pub fn new(title: &'a str, body: &'a str) -> Self {
        Self { title, body, hint: None }
    }

    pub fn with_hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }
}

/// Center `child_w × child_h` inside `parent`, clamping to the parent
/// bounds. Returns a `Rect` with `width ≥ 3` and `height ≥ 3` so the
/// shadow band never overflows.
pub fn centered_rect(parent: Rect, child_w: u16, child_h: u16) -> Rect {
    // Reserve 1 col + 1 row for the shadow band.
    let max_w = parent.width.saturating_sub(2).max(3);
    let max_h = parent.height.saturating_sub(2).max(3);
    let w = child_w.clamp(3, max_w);
    let h = child_h.clamp(3, max_h);
    let x = parent.x + (parent.width.saturating_sub(w)) / 2;
    let y = parent.y + (parent.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// Paint `popup` centered inside `parent`. The shadow band is dropped
/// automatically when the parent is too small to spare a column and a
/// row, so very small panes still get a usable popup.
pub fn render(f: &mut Frame, parent: Rect, popup: Popup<'_>, theme: &Theme) {
    // Pick a size that fits the body. Heuristic: ~60% of parent width,
    // capped at the body's display width + padding.
    let body_w = popup
        .body
        .lines()
        .map(str::len)
        .max()
        .unwrap_or(0) as u16;
    let body_lines = popup.body.lines().count().max(1) as u16;

    let desired_w = (parent.width * 6 / 10).max(body_w + 6);
    let desired_h = body_lines + 4; // top border + bottom border + 1 padding each side

    let rect = centered_rect(parent, desired_w, desired_h);

    // Shadow band: one column right + one row below, as far as the
    // popup extends. Only drawn when there's room.
    let can_shadow = rect
        .right()
        .checked_add(1)
        .map_or(false, |x| x < parent.right())
        && rect
            .bottom()
            .checked_add(1)
            .map_or(false, |y| y < parent.bottom());
    if can_shadow {
        let shadow = Rect::new(rect.x + 1, rect.y + 1, rect.width, rect.height);
        // Reuse `theme.dim()` as a "behind-everything" tone — slightly
        // darker than the pane background so it reads as a shadow
        // rather than a highlight.
        let block = Block::default().style(theme.dim());
        // We deliberately don't paint `Clear` here — we want to dim
        // whatever's underneath rather than wipe it.
        f.render_widget(block, shadow);
    }

    // Clear inside the popup rect so the body doesn't show the pane
    // contents bleeding through.
    f.render_widget(Clear, rect);

    let title_text = format!(" {} ", popup.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.border_focus)
                .add_modifier(Modifier::BOLD),
        )
        .title(Line::from(Span::styled(title_text, theme.title())))
        .style(Style::default().bg(theme.bg));

    let inner = block.inner(rect);
    f.render_widget(block, rect);

    // Layout: body fills, hint pinned to the bottom row.
    let mut body_rect = inner;
    if let Some(hint) = popup.hint {
        if inner.height >= 2 {
            body_rect.height = inner.height - 1;
            let hint_rect = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
            let line = Line::from(Span::styled(
                format!("{hint:>width$}", width = inner.width as usize),
                theme.key(),
            ));
            f.render_widget(Paragraph::new(line), hint_rect);
        }
    }

    let paragraph = Paragraph::new(popup.body)
        .style(Style::default().fg(theme.fg))
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, body_rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_centers_within_parent() {
        let parent = Rect::new(0, 0, 80, 24);
        let r = centered_rect(parent, 24, 5);
        // (80 - 24) / 2 = 28, (24 - 5) / 2 = 9.
        assert_eq!(r, Rect::new(28, 9, 24, 5));
    }

    #[test]
    fn centered_rect_clamps_when_too_large() {
        let parent = Rect::new(0, 0, 80, 24);
        let r = centered_rect(parent, 200, 200);
        // Should be clamped so width ≤ 78 (parent - 2) and height ≤ 22
        // (parent - 2), respecting the 3-cell floor.
        assert!(r.width <= 78);
        assert!(r.height <= 22);
        assert!(r.width >= 3);
        assert!(r.height >= 3);
    }

    #[test]
    fn centered_rect_enforces_minimum_size() {
        // Even when caller asks for 1×1, we return at least 3×3 so the
        // shadow band math never panics on an empty rect.
        let parent = Rect::new(0, 0, 10, 10);
        let r = centered_rect(parent, 1, 1);
        assert_eq!(r.width, 3);
        assert_eq!(r.height, 3);
    }

    #[test]
    fn centered_rect_in_origin_offset_parent() {
        let parent = Rect::new(10, 5, 20, 10);
        let r = centered_rect(parent, 10, 4);
        // x = 10 + (20 - 10) / 2 = 15
        // y = 5 + (10 - 4) / 2 = 8
        assert_eq!(r, Rect::new(15, 8, 10, 4));
    }

    #[test]
    fn popup_new_defaults_hint_to_none() {
        let p = Popup::new("t", "b");
        assert!(p.hint.is_none());
    }

    #[test]
    fn popup_with_hint_stores_hint() {
        let p = Popup::new("t", "b").with_hint("[enter]");
        assert_eq!(p.hint, Some("[enter]"));
    }
}