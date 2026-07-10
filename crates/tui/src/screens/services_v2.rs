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

pub struct ServicesScreenV2 {
    selected: usize,
}

impl Default for ServicesScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for ServicesScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Services }
    fn title(&self) -> &str { "Services" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Left, Zone::Right] }
    fn hint(&self) -> &str { "▲▼ scroll   A start/stop   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.services.try_read().map(|v| v.len()).unwrap_or(0);
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let svcs = ctx.live.services.try_read().ok();
                if let Some(ss) = svcs {
                    let sel = self.selected.min(ss.len().saturating_sub(1));
                    if let Some(svc) = ss.get(sel) {
                        let running = svc.active == "active";
                        let unit = svc.unit.clone();
                        let (msg, action) = if running {
                            (format!("Stop {}?", unit), Action::Run(RunAction::ServiceStop(unit)))
                        } else {
                            (format!("Start {}?", unit), Action::Run(RunAction::ServiceStart(unit)))
                        };
                        drop(ss);
                        ctx.open_modal(Box::new(RunActionModal::new(msg, action)));
                    }
                }
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // ── Left: service list ───────────────────────────────────────────────
        let items: Vec<ListItem<'static>> = if let Ok(svcs) = ctx.live.services.try_read() {
            if svcs.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  (no services)", theme.dim())))]
            } else {
                svcs.iter().map(|s| {
                    let dot = if s.active == "active" { "●" } else { "○" };
                    let style = if s.active == "active" { theme.ok() } else { theme.dim() };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", dot), style),
                        Span::styled(trunc(&s.unit, 30), Style::default().fg(theme.fg)),
                    ]))
                }).collect()
            }
        } else {
            vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))]
        };

        let count = ctx.live.services.try_read().map(|v| v.len()).unwrap_or(0);
        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" services ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(ctx.nav.focus_zone == 0)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, cols[0], &mut list_state);

        // ── Right: service detail ────────────────────────────────────────────
        let detail_lines: Vec<Line<'static>> = if let Ok(svcs) = ctx.live.services.try_read() {
            let sel = self.selected.min(svcs.len().saturating_sub(1));
            if let Some(s) = svcs.get(sel) {
                let active_style = if s.active == "active" { theme.ok() } else { theme.dim() };
                vec![
                    Line::from(vec![Span::styled("unit   ", theme.dim()), Span::styled(s.unit.clone(), Style::default().fg(theme.fg))]),
                    Line::from(vec![Span::styled("load   ", theme.dim()), Span::styled(s.load.clone(), Style::default().fg(theme.fg))]),
                    Line::from(vec![Span::styled("active ", theme.dim()), Span::styled(s.active.clone(), active_style)]),
                    Line::from(vec![Span::styled("sub    ", theme.dim()), Span::styled(s.sub.clone(), Style::default().fg(theme.fg))]),
                    Line::from(""),
                    Line::from(Span::styled(s.description.clone(), Style::default().fg(theme.dim))),
                    Line::from(""),
                    Line::from(Span::styled("  A = start / stop", theme.dim())),
                ]
            } else {
                vec![Line::from(Span::styled("(no service selected)", theme.dim()))]
            }
        } else {
            vec![Line::from(Span::styled("(refreshing…)", theme.dim()))]
        };

        frame.render_widget(
            Paragraph::new(detail_lines)
                .block(Block::default()
                    .title(Span::styled(" detail ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(ctx.nav.focus_zone == 1))),
            cols[1],
        );
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}
