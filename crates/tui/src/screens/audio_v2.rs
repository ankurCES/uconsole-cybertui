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

pub struct AudioScreenV2 {
    selected: usize,
}

impl Default for AudioScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for AudioScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Audio }
    fn title(&self) -> &str { "Audio" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Left, Zone::Right] }
    fn hint(&self) -> &str { "▲▼ scroll   ◀▶ volume   m mute   A default   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.sinks.try_read().map(|v| v.len()).unwrap_or(0);
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Left | NavEvent::Right => {
                if let Ok(sinks) = ctx.live.sinks.try_read() {
                    let sel = self.selected.min(sinks.len().saturating_sub(1));
                    if let Some(sink) = sinks.get(sel) {
                        let new_vol = if matches!(event, NavEvent::Left) {
                            sink.volume.saturating_sub(5)
                        } else {
                            (sink.volume + 5).min(150)
                        };
                        let target = sink.id.to_string();
                        drop(sinks);
                        ctx.queue_action(Action::Run(RunAction::SetVolume { target, percent: new_vol }));
                    }
                }
                Consumed::Yes
            }
            NavEvent::Char('m') => {
                if let Ok(sinks) = ctx.live.sinks.try_read() {
                    let sel = self.selected.min(sinks.len().saturating_sub(1));
                    if let Some(sink) = sinks.get(sel) {
                        let target = sink.id.to_string();
                        let mute = !sink.muted;
                        drop(sinks);
                        ctx.queue_action(Action::Run(RunAction::MuteSink { target, mute }));
                    }
                }
                Consumed::Yes
            }
            NavEvent::Confirm => {
                if let Ok(sinks) = ctx.live.sinks.try_read() {
                    let sel = self.selected.min(sinks.len().saturating_sub(1));
                    if let Some(sink) = sinks.get(sel) {
                        if !sink.default {
                            let id = sink.id.to_string();
                            let name = trunc(&sink.name, 20);
                            drop(sinks);
                            ctx.open_modal(Box::new(RunActionModal::new(
                                format!("Set '{}' as default sink?", name),
                                Action::Run(RunAction::SetDefaultSink(id)),
                            )));
                        }
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
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        // ── Left: sink list ──────────────────────────────────────────────────
        let items: Vec<ListItem<'static>> = if let Ok(sinks) = ctx.live.sinks.try_read() {
            if sinks.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  (no audio sinks)", theme.dim())))]
            } else {
                sinks.iter().map(|s| {
                    let dot = if s.default { "★" } else { "○" };
                    let mute_tag = if s.muted { " [M]" } else { "" };
                    let dot_style = if s.default { theme.ok() } else { theme.dim() };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", dot), dot_style),
                        Span::styled(format!("{}{}", trunc(&s.name, 24), mute_tag), Style::default().fg(theme.fg)),
                    ]))
                }).collect()
            }
        } else {
            vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))]
        };

        let count = ctx.live.sinks.try_read().map(|v| v.len()).unwrap_or(0);
        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" sinks ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(ctx.nav.focus_zone == 0)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, cols[0], &mut list_state);

        // ── Right: sink detail ───────────────────────────────────────────────
        let detail_lines: Vec<Line<'static>> = if let Ok(sinks) = ctx.live.sinks.try_read() {
            let sel = self.selected.min(sinks.len().saturating_sub(1));
            if let Some(s) = sinks.get(sel) {
                let bar_w = 16usize;
                let filled = (s.volume as usize * bar_w / 100).min(bar_w);
                let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(bar_w - filled));
                let vol_style = if s.volume > 100 { theme.warn() } else { theme.ok() };
                vec![
                    Line::from(vec![Span::styled("name   ", theme.dim()), Span::styled(s.name.clone(), Style::default().fg(theme.fg))]),
                    Line::from(vec![Span::styled("id     ", theme.dim()), Span::styled(s.id.to_string(), theme.dim())]),
                    Line::from(vec![Span::styled("volume ", theme.dim()), Span::styled(format!("{} {}%", bar, s.volume), vol_style)]),
                    Line::from(vec![Span::styled("muted  ", theme.dim()), Span::styled(if s.muted { "yes" } else { "no" }, if s.muted { theme.warn() } else { theme.ok() })]),
                    Line::from(vec![Span::styled("default", theme.dim()), Span::styled(if s.default { " ★ yes" } else { " no" }, if s.default { theme.ok() } else { theme.dim() })]),
                    Line::from(""),
                    Line::from(Span::styled(trunc(&s.description, 30), theme.dim())),
                    Line::from(""),
                    Line::from(Span::styled("  ◀/▶ volume  m mute", theme.dim())),
                    Line::from(Span::styled("  A set as default", theme.dim())),
                ]
            } else {
                vec![Line::from(Span::styled("(no sink selected)", theme.dim()))]
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
