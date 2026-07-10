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

const GOVERNORS: &[&str] = &["performance", "ondemand", "conservative", "powersave"];

pub struct PowerScreenV2 {
    governor_sel: usize,
}

impl Default for PowerScreenV2 {
    fn default() -> Self { Self { governor_sel: 0 } }
}

impl ScreenV2 for PowerScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Power }
    fn title(&self) -> &str { "Power" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ governor   A apply   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Up => {
                self.governor_sel = self.governor_sel.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down => {
                self.governor_sel = (self.governor_sel + 1).min(GOVERNORS.len() - 1);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let gov = GOVERNORS[self.governor_sel];
                ctx.open_modal(Box::new(RunActionModal::new(
                    format!("Set governor to {}?", gov),
                    Action::Run(RunAction::SetGovernor(gov.to_owned())),
                )));
                Consumed::Yes
            }
            NavEvent::Char('s') => {
                ctx.open_modal(Box::new(RunActionModal::new(
                    "Suspend?",
                    Action::Run(RunAction::Suspend),
                )));
                Consumed::Yes
            }
            NavEvent::Char('r') => {
                ctx.open_modal(Box::new(RunActionModal::new(
                    "Reboot?",
                    Action::Run(RunAction::Reboot),
                )));
                Consumed::Yes
            }
            NavEvent::Char('p') => {
                ctx.open_modal(Box::new(RunActionModal::new(
                    "Shutdown?",
                    Action::Run(RunAction::Shutdown),
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
            .constraints([Constraint::Length(8), Constraint::Min(0)])
            .split(area);

        // ── Battery info ─────────────────────────────────────────────────────
        let batt_lines: Vec<Line<'static>> = if let Ok(b) = ctx.live.battery.try_read() {
            if let Some(b) = b.as_ref() {
                let status_style = match b.status.as_str() {
                    "Charging" => theme.ok(),
                    "Full"     => theme.ok(),
                    _          => theme.warn(),
                };
                let bar = capacity_bar(b.capacity, 20);
                let mut v = vec![
                    Line::from(vec![
                        Span::styled("  status    ", theme.dim()),
                        Span::styled(b.status.clone(), status_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  capacity  ", theme.dim()),
                        Span::styled(format!("{}% {}", b.capacity, bar), Style::default().fg(if b.capacity > 20 { theme.ok } else { theme.error })),
                    ]),
                ];
                if let Some(ttf) = &b.time_to_full {
                    v.push(Line::from(vec![Span::styled("  to full   ", theme.dim()), Span::styled(ttf.clone(), Style::default().fg(theme.fg))]));
                }
                if let Some(tte) = &b.time_to_empty {
                    v.push(Line::from(vec![Span::styled("  remaining ", theme.dim()), Span::styled(tte.clone(), Style::default().fg(theme.fg))]));
                }
                if let Some(pw) = b.power_now_w {
                    v.push(Line::from(vec![Span::styled("  power now ", theme.dim()), Span::styled(format!("{:.1} W", pw), Style::default().fg(theme.fg))]));
                }
                v
            } else {
                vec![Line::from(Span::styled("  no battery detected", theme.dim()))]
            }
        } else {
            vec![Line::from(Span::styled("  (refreshing…)", theme.dim()))]
        };

        frame.render_widget(
            Paragraph::new(batt_lines)
                .block(Block::default()
                    .title(Span::styled(" battery ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(false))),
            chunks[0],
        );

        // ── Governor selector ────────────────────────────────────────────────
        let gov_items: Vec<ListItem<'static>> = GOVERNORS.iter().map(|&g| {
            ListItem::new(Line::from(Span::styled(format!("  {}", g), Style::default().fg(theme.fg))))
        }).collect();

        let mut gov_state = ListState::default().with_selected(Some(self.governor_sel));
        let gov_list = List::new(gov_items)
            .block(Block::default()
                .title(Span::styled(" governor (A to apply) ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(true)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(gov_list, chunks[1], &mut gov_state);

        // Hint overlay at bottom-right of governor block
        if chunks[1].height > 4 {
            let hint = Paragraph::new(Line::from(Span::styled(
                "  s=suspend  r=reboot  p=poweroff",
                theme.dim(),
            )));
            let hr = Rect {
                x: chunks[1].x + 1,
                y: chunks[1].y + chunks[1].height - 2,
                width: chunks[1].width.saturating_sub(2),
                height: 1,
            };
            frame.render_widget(hint, hr);
        }
    }
}

fn capacity_bar(pct: u8, width: usize) -> String {
    let filled = (pct as usize * width / 100).min(width);
    let empty = width - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}
