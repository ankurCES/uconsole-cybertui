use std::cell::Cell;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const CELL_H: u16 = 4;

static ITEMS: &[ScreenId] = &[
    ScreenId::System,
    ScreenId::Network,
    ScreenId::Bluetooth,
    ScreenId::Power,
    ScreenId::Display,
    ScreenId::Audio,
    ScreenId::Storage,
    ScreenId::LoRa,
    ScreenId::City,
    ScreenId::Intel,
    ScreenId::Recon,
    ScreenId::Files,
    ScreenId::Processes,
    ScreenId::Services,
    ScreenId::Packages,
    ScreenId::Logs,
    ScreenId::Settings,
];

pub struct MainMenuScreen {
    cursor: usize,
    scroll_offset: usize,
    // ponytail: Cell for render→nav cache; render is &self per trait contract
    cols: Cell<usize>,
    visible_rows: Cell<usize>,
}

impl Default for MainMenuScreen {
    fn default() -> Self {
        Self {
            cursor: 0,
            scroll_offset: 0,
            cols: Cell::new(5),
            visible_rows: Cell::new(3),
        }
    }
}

fn cols_for_width(inner_width: u16) -> usize {
    ((inner_width as usize) / 15).clamp(2, 6)
}

impl MainMenuScreen {
    fn clamp_scroll(&mut self) {
        let cursor_row = self.cursor / self.cols.get();
        let visible = self.visible_rows.get().max(1);
        if cursor_row < self.scroll_offset {
            self.scroll_offset = cursor_row;
        } else if cursor_row >= self.scroll_offset + visible {
            self.scroll_offset = cursor_row + 1 - visible;
        }
    }
}

impl ScreenV2 for MainMenuScreen {
    fn id(&self) -> ScreenId {
        ScreenId::MainMenu
    }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let cols = self.cols.get();
        let total = ITEMS.len();
        let rows = (total + cols - 1) / cols;
        let col = self.cursor % cols;
        let row = self.cursor / cols;

        match event {
            NavEvent::Right => {
                self.cursor = (self.cursor + 1) % total;
                self.clamp_scroll();
                Consumed::Yes
            }
            NavEvent::Left => {
                self.cursor = (self.cursor + total - 1) % total;
                self.clamp_scroll();
                Consumed::Yes
            }
            NavEvent::Down => {
                let next_row = (row + 1) % rows;
                let candidate = next_row * cols + col;
                self.cursor = if candidate < total { candidate } else { col.min(total - 1) };
                self.clamp_scroll();
                Consumed::Yes
            }
            NavEvent::Up => {
                let prev_row = (row + rows - 1) % rows;
                let candidate = prev_row * cols + col;
                self.cursor = if candidate < total { candidate } else { total - 1 };
                self.clamp_scroll();
                Consumed::Yes
            }
            NavEvent::Confirm => {
                ctx.navigate_to(ITEMS[self.cursor]);
                Consumed::Yes
            }
            NavEvent::Back => {
                ctx.navigate_to(ScreenId::Screensaver);
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let crumb: Vec<&str> = ctx.nav.stack.breadcrumb().map(|id| id.label()).collect();
        frame.render_widget(
            Paragraph::new(crumb.join(" > ")).alignment(Alignment::Left),
            chunks[0],
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" ▦ MENU ", theme.title()));
        let inner = block.inner(chunks[1]);
        frame.render_widget(block, chunks[1]);

        let cols = cols_for_width(inner.width);
        let cell_w = inner.width / cols as u16;
        let visible_rows = (inner.height / CELL_H) as usize;
        self.cols.set(cols);
        self.visible_rows.set(visible_rows);

        let total = ITEMS.len();
        let total_rows = (total + cols - 1) / cols;
        let scroll = self.scroll_offset;

        if scroll > 0 && inner.height > 0 {
            let x = inner.x + inner.width / 2;
            frame.render_widget(Paragraph::new("▲"), Rect { x, y: inner.y, width: 1, height: 1 });
        }
        if scroll + visible_rows < total_rows && inner.height > 0 {
            let x = inner.x + inner.width / 2;
            let y = inner.y + inner.height.saturating_sub(1);
            frame.render_widget(Paragraph::new("▼"), Rect { x, y, width: 1, height: 1 });
        }

        for (i, &id) in ITEMS.iter().enumerate() {
            let grid_row = i / cols;
            if grid_row < scroll || grid_row >= scroll + visible_rows {
                continue;
            }
            let col = (i % cols) as u16;
            let row = (grid_row - scroll) as u16;
            let x = inner.x + col * cell_w;
            let y = inner.y + row * CELL_H;

            if y + CELL_H > inner.y + inner.height {
                break;
            }

            let cell_rect = Rect { x, y, width: cell_w, height: CELL_H };
            let focused = i == self.cursor;

            let cell_block = Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(focused));
            let cell_inner = cell_block.inner(cell_rect);
            frame.render_widget(cell_block, cell_rect);

            let text_style = if focused {
                Style::default().fg(theme.accent)
            } else {
                Style::default().fg(theme.fg)
            };

            if cell_inner.height >= 1 {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(id.glyph(), text_style)))
                        .alignment(Alignment::Center),
                    Rect { height: 1, ..cell_inner },
                );
            }
            if cell_inner.height >= 2 {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(id.label(), text_style)))
                        .alignment(Alignment::Center),
                    Rect { y: cell_inner.y + 1, height: 1, ..cell_inner },
                );
            }
        }
    }

    fn focusable_zones(&self) -> &[Zone] {
        &[Zone::Main]
    }

    fn hint(&self) -> &str {
        "▲▼◀▶ move  A select  B back"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nav_right(m: &mut MainMenuScreen) {
        let total = ITEMS.len();
        m.cursor = (m.cursor + 1) % total;
        m.clamp_scroll();
    }
    fn nav_left(m: &mut MainMenuScreen) {
        let total = ITEMS.len();
        m.cursor = (m.cursor + total - 1) % total;
        m.clamp_scroll();
    }
    fn nav_down(m: &mut MainMenuScreen) {
        let cols = m.cols.get();
        let total = ITEMS.len();
        let rows = (total + cols - 1) / cols;
        let col = m.cursor % cols;
        let row = m.cursor / cols;
        let next_row = (row + 1) % rows;
        let candidate = next_row * cols + col;
        m.cursor = if candidate < total { candidate } else { col.min(total - 1) };
        m.clamp_scroll();
    }
    fn nav_up(m: &mut MainMenuScreen) {
        let cols = m.cols.get();
        let total = ITEMS.len();
        let rows = (total + cols - 1) / cols;
        let col = m.cursor % cols;
        let row = m.cursor / cols;
        let prev_row = (row + rows - 1) % rows;
        let candidate = prev_row * cols + col;
        m.cursor = if candidate < total { candidate } else { total - 1 };
        m.clamp_scroll();
    }

    #[test]
    fn right_wraps_to_start() {
        let mut m = MainMenuScreen { cursor: ITEMS.len() - 1, ..Default::default() };
        nav_right(&mut m);
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn left_wraps_to_end() {
        let mut m = MainMenuScreen::default();
        nav_left(&mut m);
        assert_eq!(m.cursor, ITEMS.len() - 1);
    }

    #[test]
    fn down_moves_to_next_row() {
        let mut m = MainMenuScreen::default(); // cols=5
        nav_down(&mut m);
        assert_eq!(m.cursor, 5);
    }

    #[test]
    fn up_from_row0_wraps_to_last_row() {
        let mut m = MainMenuScreen::default(); // cols=5, cursor=0
        nav_up(&mut m);
        let cols = m.cols.get();
        let total = ITEMS.len();
        let rows = (total + cols - 1) / cols;
        assert_eq!(m.cursor, (rows - 1) * cols);
    }

    #[test]
    fn cols_for_width_bounds() {
        assert_eq!(cols_for_width(80), 5);  // 78/15=5
        assert_eq!(cols_for_width(40), 2);  // 38/15=2
        assert_eq!(cols_for_width(120), 6); // 118/15=7 → capped at 6
        assert_eq!(cols_for_width(10), 2);  // 8/15=0 → clamped to 2
    }

    #[test]
    fn scroll_clamps_down_on_cursor_advance() {
        let mut m = MainMenuScreen {
            visible_rows: Cell::new(1),
            ..Default::default()
        };
        // cursor starts at row 0, visible_rows=1 → only row 0 visible
        nav_down(&mut m); // cursor moves to row 1
        assert_eq!(m.scroll_offset, 1);
    }

    #[test]
    fn scroll_clamps_up_on_cursor_retreat() {
        let mut m = MainMenuScreen {
            cursor: 5, // row 1 with cols=5
            scroll_offset: 1,
            visible_rows: Cell::new(1),
            ..Default::default()
        };
        nav_up(&mut m); // cursor moves to row 0
        assert_eq!(m.scroll_offset, 0);
    }
}
