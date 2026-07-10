use std::process::Command;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct LogsScreenV2 {
    lines: Vec<String>,
    offset: usize,
    loaded: bool,
}

impl Default for LogsScreenV2 {
    fn default() -> Self { Self { lines: Vec::new(), offset: 0, loaded: false } }
}

impl LogsScreenV2 {
    fn load(&mut self) {
        self.lines.clear();
        let out = Command::new("journalctl")
            .args(["-n", "500", "--no-pager", "-o", "short"])
            .output();
        match out {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                for line in text.lines().rev().take(500) {
                    self.lines.push(line.to_string());
                }
                self.lines.reverse();
            }
            Err(_) => {
                self.lines.push("(journalctl unavailable)".to_string());
            }
        }
        self.offset = self.lines.len().saturating_sub(1);
        self.loaded = true;
    }
}

impl ScreenV2 for LogsScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Logs }
    fn title(&self) -> &str { "Logs" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   r refresh   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        if !self.loaded { self.load(); }
        let count = self.lines.len();
        match event {
            NavEvent::Up => {
                self.offset = self.offset.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.offset = (self.offset + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Char('r') => {
                self.load();
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let items: Vec<ListItem<'static>> = if !self.loaded {
            vec![ListItem::new(Line::from(Span::styled("  (press any key to load…)", theme.dim())))]
        } else if self.lines.is_empty() {
            vec![ListItem::new(Line::from(Span::styled("  (no log entries)", theme.dim())))]
        } else {
            self.lines.iter().map(|l| {
                let style = if l.contains("error") || l.contains("Error") || l.contains("ERROR") {
                    theme.error()
                } else if l.contains("warn") || l.contains("Warn") || l.contains("WARN") {
                    theme.warn()
                } else {
                    Style::default().fg(theme.fg)
                };
                ListItem::new(Line::from(Span::styled(l.clone(), style)))
            }).collect()
        };

        let count = items.len();
        let sel = if count == 0 { None } else { Some(self.offset.min(count - 1)) };
        let mut list_state = ListState::default().with_selected(sel);
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" Logs ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(true)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut list_state);
    }
}
