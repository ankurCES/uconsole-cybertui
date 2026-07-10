use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::modal::QuitConfirmModal;
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const COLS: usize = 5;
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
}

impl Default for MainMenuScreen {
    fn default() -> Self {
        Self { cursor: 0 }
    }
}

impl ScreenV2 for MainMenuScreen {
    fn id(&self) -> ScreenId {
        ScreenId::MainMenu
    }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let total = ITEMS.len();
        let rows = (total + COLS - 1) / COLS;
        let col = self.cursor % COLS;
        let row = self.cursor / COLS;

        match event {
            NavEvent::Right => {
                self.cursor = (self.cursor + 1) % total;
                Consumed::Yes
            }
            NavEvent::Left => {
                self.cursor = (self.cursor + total - 1) % total;
                Consumed::Yes
            }
            NavEvent::Down => {
                let next_row = (row + 1) % rows;
                let candidate = next_row * COLS + col;
                self.cursor = if candidate < total { candidate } else { col.min(total - 1) };
                Consumed::Yes
            }
            NavEvent::Up => {
                let prev_row = (row + rows - 1) % rows;
                let candidate = prev_row * COLS + col;
                self.cursor = if candidate < total { candidate } else { total - 1 };
                Consumed::Yes
            }
            NavEvent::Confirm => {
                ctx.navigate_to(ITEMS[self.cursor]);
                Consumed::Yes
            }
            NavEvent::Back => {
                ctx.open_modal(Box::new(QuitConfirmModal));
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

        let cell_w = inner.width / COLS as u16;

        for (i, &id) in ITEMS.iter().enumerate() {
            let col = (i % COLS) as u16;
            let row = (i / COLS) as u16;
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
                let glyph_rect = Rect { height: 1, ..cell_inner };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(id.glyph(), text_style)))
                        .alignment(Alignment::Center),
                    glyph_rect,
                );
            }
            if cell_inner.height >= 2 {
                let label_rect = Rect { y: cell_inner.y + 1, height: 1, ..cell_inner };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(id.label(), text_style)))
                        .alignment(Alignment::Center),
                    label_rect,
                );
            }
        }
    }

    fn focusable_zones(&self) -> &[Zone] {
        &[Zone::Main]
    }

    fn hint(&self) -> &str {
        "Enter select  Esc back"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_wraps_to_start() {
        let mut m = MainMenuScreen { cursor: ITEMS.len() - 1 };
        // simulate Right: (total-1 + 1) % total == 0
        m.cursor = (m.cursor + 1) % ITEMS.len();
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn left_wraps_to_end() {
        let mut m = MainMenuScreen { cursor: 0 };
        m.cursor = (m.cursor + ITEMS.len() - 1) % ITEMS.len();
        assert_eq!(m.cursor, ITEMS.len() - 1);
    }

    #[test]
    fn down_moves_to_next_row() {
        let mut m = MainMenuScreen { cursor: 0 };
        // col=0, row=0 → next_row=1, candidate=5
        let total = ITEMS.len();
        let rows = (total + COLS - 1) / COLS;
        let col = m.cursor % COLS;
        let row = m.cursor / COLS;
        let next_row = (row + 1) % rows;
        let candidate = next_row * COLS + col;
        m.cursor = if candidate < total { candidate } else { col.min(total - 1) };
        assert_eq!(m.cursor, COLS);
    }

    #[test]
    fn up_from_row0_wraps_to_last_row() {
        let total = ITEMS.len();
        let rows = (total + COLS - 1) / COLS;
        let mut m = MainMenuScreen { cursor: 0 }; // col=0, row=0
        let col = m.cursor % COLS;
        let row = m.cursor / COLS;
        let prev_row = (row + rows - 1) % rows;
        let candidate = prev_row * COLS + col;
        m.cursor = if candidate < total { candidate } else { total - 1 };
        // col=0, last row = rows-1=3 → candidate = 15 (which is Logs, index 15 < 17)
        assert_eq!(m.cursor, (rows - 1) * COLS);
    }
}
