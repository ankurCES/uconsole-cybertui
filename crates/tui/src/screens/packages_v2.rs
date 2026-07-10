use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct PackagesScreenV2 {
    selected: usize,
}

impl Default for PackagesScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for PackagesScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Packages }
    fn title(&self) -> &str { "Packages" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.upgradable.try_read().map(|v| v.len()).unwrap_or(0);
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let upgradable = ctx.live.upgradable.try_read().ok();
        let count = upgradable.as_ref().map(|v| v.len()).unwrap_or(0);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        // Header
        let header = Paragraph::new(Line::from(vec![
            Span::styled("  upgradable: ", theme.dim()),
            Span::styled(count.to_string(), theme.warn()),
            Span::styled(" package(s)", theme.dim()),
        ])).block(Block::default()
            .title(Span::styled(" Packages ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(true)));
        frame.render_widget(header, chunks[0]);

        // Package list
        let items: Vec<ListItem<'static>> = if let Some(pkgs) = upgradable {
            if pkgs.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  all packages up to date", theme.ok())))]
            } else {
                pkgs.iter().map(|p| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("  {:<30}", trunc(&p.name, 30)), Style::default().fg(theme.fg)),
                        Span::styled(trunc(&p.version, 20), Style::default().fg(theme.accent)),
                    ]))
                }).collect()
            }
        } else {
            vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))]
        };

        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" upgradable ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, chunks[1], &mut list_state);
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}
