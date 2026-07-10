use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::modal::RunActionModal;
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct DisplayScreenV2 {
    selected: usize,
    brightness: u8,
}

impl Default for DisplayScreenV2 {
    fn default() -> Self { Self { selected: 0, brightness: 100 } }
}

impl ScreenV2 for DisplayScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Display }
    fn title(&self) -> &str { "Display" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   ◀▶ brightness   A apply   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.displays.try_read().map(|v| v.len()).unwrap_or(0);
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Left => {
                self.brightness = self.brightness.saturating_sub(10);
                Consumed::Yes
            }
            NavEvent::Right => {
                self.brightness = (self.brightness + 10).min(100);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                ctx.open_modal(Box::new(RunActionModal::new(
                    format!("Set brightness to {}%?", self.brightness),
                    Action::Run(RunAction::SetBrightness(self.brightness)),
                )));
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
            .constraints([Constraint::Length(5), Constraint::Min(0)])
            .split(area);

        // Brightness control
        let bar_w = 20usize;
        let filled = (self.brightness as usize * bar_w / 100).min(bar_w);
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(bar_w - filled));
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  brightness  ", theme.dim()),
                    Span::styled(bar, theme.ok()),
                    Span::styled(format!(" {}%", self.brightness), Style::default().fg(theme.fg)),
                ]),
                Line::from(""),
                Line::from(Span::styled("  ◀/▶ adjust   A to apply", theme.dim())),
            ]).block(Block::default()
                .title(Span::styled(" brightness ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false))),
            chunks[0],
        );

        // Display list
        let items: Vec<ListItem<'static>> = if let Ok(displays) = ctx.live.displays.try_read() {
            if displays.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  (no displays detected)", theme.dim())))]
            } else {
                displays.iter().map(|d| {
                    let enabled_style = if d.enabled { theme.ok() } else { theme.dim() };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", if d.enabled { "●" } else { "○" }), enabled_style),
                        Span::styled(format!("{:<12}", trunc(&d.name, 12)), Style::default().fg(theme.fg)),
                        Span::styled(trunc(&d.mode, 16), Style::default().fg(theme.accent)),
                        Span::styled(format!("  ×{:.1}", d.scale), theme.dim()),
                    ]))
                }).collect()
            }
        } else {
            vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))]
        };

        let count = ctx.live.displays.try_read().map(|v| v.len()).unwrap_or(0);
        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" displays ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(true)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, chunks[1], &mut list_state);
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}
