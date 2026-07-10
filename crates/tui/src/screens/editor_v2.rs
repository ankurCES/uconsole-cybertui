use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct EditorScreenV2 {
    path: Option<String>,
    lines: Vec<String>,
    cursor_row: usize,
    view_offset: usize,
}

impl Default for EditorScreenV2 {
    fn default() -> Self {
        Self {
            path: None,
            lines: vec![String::new()],
            cursor_row: 0,
            view_offset: 0,
        }
    }
}

impl EditorScreenV2 {
    pub fn open(path: &str) -> Self {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.lines().map(|l| l.to_string()).collect()
        };
        Self {
            path: Some(path.to_string()),
            cursor_row: 0,
            view_offset: 0,
            lines,
        }
    }
}

impl ScreenV2 for EditorScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Editor }
    fn title(&self) -> &str { "Editor" }
    fn is_hidden(&self) -> bool { true }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = self.lines.len();
        match event {
            NavEvent::Up => {
                self.cursor_row = self.cursor_row.saturating_sub(1);
                if self.cursor_row < self.view_offset {
                    self.view_offset = self.cursor_row;
                }
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.cursor_row = (self.cursor_row + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let path_label = self.path.as_deref().unwrap_or("(no file)");
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ", theme.dim()),
                Span::styled(path_label.to_string(), Style::default().fg(theme.accent)),
                Span::styled(format!("  {}:{}", self.cursor_row + 1, 1), theme.dim()),
            ])).style(Style::default().bg(theme.bg)),
            chunks[0],
        );

        let visible_h = chunks[1].height as usize;
        let offset = if self.cursor_row >= self.view_offset + visible_h {
            self.cursor_row + 1 - visible_h
        } else {
            self.view_offset
        };

        let items: Vec<ListItem<'static>> = self.lines.iter()
            .skip(offset)
            .take(visible_h)
            .enumerate()
            .map(|(i, line)| {
                let row = offset + i;
                let line_style = if row == self.cursor_row {
                    Style::default().fg(theme.selection_fg).bg(theme.selection_bg)
                } else {
                    Style::default().fg(theme.fg)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:>4} ", row + 1), Style::default().fg(theme.dim)),
                    Span::styled(line.clone(), line_style),
                ]))
            })
            .collect();

        let count = items.len();
        let sel = if count == 0 { None } else {
            Some(self.cursor_row.saturating_sub(offset).min(count - 1))
        };
        let mut list_state = ListState::default().with_selected(sel);
        let list = List::new(items)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(true)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg));
        frame.render_stateful_widget(list, chunks[1], &mut list_state);
    }
}
