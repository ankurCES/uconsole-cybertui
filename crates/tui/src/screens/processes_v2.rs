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

pub struct ProcessesScreenV2 {
    selected: usize,
    sort_by_mem: bool,
}

impl Default for ProcessesScreenV2 {
    fn default() -> Self { Self { selected: 0, sort_by_mem: false } }
}

impl ScreenV2 for ProcessesScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Processes }
    fn title(&self) -> &str { "Processes" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   A kill   s sort   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.processes.try_read().map(|v| v.len()).unwrap_or(0);
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
                let procs = ctx.live.processes.try_read().ok();
                if let Some(ps) = procs {
                    let mut sorted: Vec<_> = ps.iter().collect();
                    if self.sort_by_mem {
                        sorted.sort_by(|a, b| b.mem.partial_cmp(&a.mem).unwrap_or(std::cmp::Ordering::Equal));
                    } else {
                        sorted.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
                    }
                    let sel = self.selected.min(sorted.len().saturating_sub(1));
                    if let Some(p) = sorted.get(sel) {
                        let pid = p.pid;
                        let cmd = trunc(&p.command, 20);
                        drop(ps);
                        ctx.open_modal(Box::new(RunActionModal::new(
                            format!("Kill PID {} ({})?", pid, cmd),
                            Action::Run(RunAction::ProcessKill(pid)),
                        )));
                    }
                }
                Consumed::Yes
            }
            NavEvent::Char('s') => {
                self.sort_by_mem = !self.sort_by_mem;
                self.selected = 0;
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

        // Column header
        let sort_label = if self.sort_by_mem { "MEM%" } else { "CPU%" };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<8}", "PID"), theme.dim()),
                Span::styled(format!("{:<12}", "USER"), theme.dim()),
                Span::styled(format!("{:<6}", sort_label), theme.dim()),
                Span::styled("COMMAND", theme.dim()),
            ])).style(Style::default().bg(theme.bg)),
            chunks[0],
        );

        let items: Vec<ListItem<'static>> = if let Ok(procs) = ctx.live.processes.try_read() {
            if procs.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  (no processes)", theme.dim())))]
            } else {
                let mut sorted: Vec<_> = procs.iter().collect();
                if self.sort_by_mem {
                    sorted.sort_by(|a, b| b.mem.partial_cmp(&a.mem).unwrap_or(std::cmp::Ordering::Equal));
                } else {
                    sorted.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
                }
                sorted.iter().map(|p| {
                    let val = if self.sort_by_mem { p.mem } else { p.cpu };
                    let pct_style = if val > 50.0 { theme.warn() } else { Style::default().fg(theme.fg) };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:<8}", p.pid), Style::default().fg(theme.dim)),
                        Span::styled(format!("{:<12}", trunc(&p.user, 11)), Style::default().fg(theme.fg)),
                        Span::styled(format!("{:<6.1}", val), pct_style),
                        Span::styled(trunc(&p.command, 30), Style::default().fg(theme.fg)),
                    ]))
                }).collect()
            }
        } else {
            vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))]
        };

        let count = items.len();
        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(
                    if self.sort_by_mem { " Processes (sort: mem) " } else { " Processes (sort: cpu) " },
                    theme.title(),
                ))
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
