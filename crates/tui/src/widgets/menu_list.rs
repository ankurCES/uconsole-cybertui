use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, StatefulWidget, Widget},
};

/// One display row passed to a `MenuList`.
pub struct MenuEntry<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
}

impl<'a> MenuEntry<'a> {
    pub fn new(glyph: &'a str, label: &'a str) -> Self {
        Self { glyph, label }
    }
}

/// Cursor + scroll-window state for `MenuList`.
///
/// `Copy` so screens can store it in `Cell<MenuListState>` and update
/// it from a `&self` render method via interior mutability.
#[derive(Copy, Clone, Default)]
pub struct MenuListState {
    pub selected: usize,
    pub offset:   usize,
}

impl MenuListState {
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self, len: usize) {
        if self.selected + 1 < len {
            self.selected += 1;
        }
    }

    /// Adjust `offset` so that `selected` stays inside the visible window.
    /// Called by `StatefulWidget::render` with the actual rendered row count.
    pub fn sync_offset(&mut self, len: usize, visible: usize) {
        if len == 0 {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        self.selected = self.selected.min(len - 1);
        let visible = visible.max(1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + visible {
            self.offset = self.selected + 1 - visible;
        }
    }
}

/// Scrollable selectable list. Implements `StatefulWidget` so callers
/// supply a `MenuListState` that persists cursor and scroll position.
pub struct MenuList<'a> {
    items:           &'a [MenuEntry<'a>],
    block:           Option<Block<'a>>,
    highlight_style: Style,
    normal_style:    Style,
}

impl<'a> MenuList<'a> {
    pub fn new(items: &'a [MenuEntry<'a>]) -> Self {
        Self {
            items,
            block:           None,
            highlight_style: Style::default().add_modifier(Modifier::REVERSED),
            normal_style:    Style::default(),
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }
}

impl<'a> StatefulWidget for MenuList<'a> {
    type State = MenuListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let inner = if let Some(block) = self.block {
            let inner = block.inner(area);
            block.render(area, buf);
            inner
        } else {
            area
        };

        let visible = inner.height as usize;
        state.sync_offset(self.items.len(), visible);

        for row in 0..visible {
            let idx = state.offset + row;
            if idx >= self.items.len() {
                break;
            }
            let item = &self.items[idx];
            let style = if idx == state.selected {
                self.highlight_style
            } else {
                self.normal_style
            };
            let text = format!(" {} {}", item.glyph, item.label);
            let padded = format!("{:<width$}", text, width = inner.width as usize);
            let span = Span::styled(padded, style);
            buf.set_span(inner.x, inner.y + row as u16, &span, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_up_at_zero_is_noop() {
        let mut s = MenuListState::default();
        s.move_up();
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn move_down_increments_selected() {
        let mut s = MenuListState::default();
        s.move_down(5);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn move_down_at_last_is_noop() {
        let mut s = MenuListState { selected: 4, offset: 0 };
        s.move_down(5);
        assert_eq!(s.selected, 4);
    }

    #[test]
    fn scroll_window_advances_when_selection_exceeds_visible() {
        let mut s = MenuListState::default();
        let len = 10;
        let visible = 3;
        for _ in 0..5 {
            s.move_down(len);
        }
        s.sync_offset(len, visible);
        // selected=5, visible=3 → offset ∈ {3,4,5} keeping sel in window
        assert!(s.selected >= s.offset, "selected must be ≥ offset");
        assert!(s.selected < s.offset + visible, "selected must be < offset+visible");
    }

    #[test]
    fn scroll_window_retreats_when_selection_goes_above() {
        let mut s = MenuListState { selected: 5, offset: 4 };
        s.move_up();
        s.sync_offset(10, 3);
        assert!(s.selected >= s.offset);
        assert!(s.selected < s.offset + 3);
    }

    #[test]
    fn sync_offset_noop_when_selection_already_visible() {
        let mut s = MenuListState { selected: 2, offset: 1 };
        s.sync_offset(10, 5); // visible=5, offset=1, selected=2 → already in [1,6)
        assert_eq!(s.offset, 1);
    }

    #[test]
    fn empty_list_does_not_panic() {
        let mut s = MenuListState::default();
        s.sync_offset(0, 5);
        assert_eq!(s.selected, 0);
        assert_eq!(s.offset, 0);
    }

    #[test]
    fn sync_clamps_selected_to_len() {
        let mut s = MenuListState { selected: 99, offset: 0 };
        s.sync_offset(3, 5);
        assert_eq!(s.selected, 2);
    }
}
