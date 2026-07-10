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

const ZONES: &[Zone] = &[Zone::Left, Zone::Right];

pub struct BluetoothScreenV2 {
    selected: usize,
}

impl Default for BluetoothScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for BluetoothScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Bluetooth }
    fn title(&self) -> &str { "Bluetooth" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼ scroll   A connect/disconnect   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.bluetooth.try_read().map(|v| v.len()).unwrap_or(0);
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
                let devices = ctx.live.bluetooth.try_read().ok();
                if let Some(devs) = devices {
                    if let Some(dev) = devs.get(self.selected) {
                        let (msg, action) = if dev.connected {
                            (
                                format!("Disconnect {}?", dev.name),
                                Action::Run(RunAction::BluetoothDisconnect(dev.mac.clone())),
                            )
                        } else {
                            (
                                format!("Connect to {}?", dev.name),
                                Action::Run(RunAction::BluetoothConnect(dev.mac.clone())),
                            )
                        };
                        drop(devs);
                        ctx.open_modal(Box::new(RunActionModal::new(msg, action)));
                    }
                }
                Consumed::Yes
            }
            NavEvent::Char('s') => {
                ctx.queue_action(Action::Run(RunAction::BluetoothScan));
                Consumed::Yes
            }
            NavEvent::Back => {
                ctx.go_back();
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // ── Left: device list ────────────────────────────────────────────────
        let (items, selected) = if let Ok(devs) = ctx.live.bluetooth.try_read() {
            let n = devs.len();
            let sel = self.selected.min(n.saturating_sub(1));
            let list: Vec<ListItem<'static>> = if devs.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  (no devices — press s to scan)", theme.dim())))]
            } else {
                devs.iter().map(|d| {
                    let status = if d.connected { "●" } else if d.paired { "○" } else { "·" };
                    let style = if d.connected { theme.ok() } else { theme.dim() };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", status), style),
                        Span::styled(d.name.clone(), Style::default().fg(theme.fg)),
                    ]))
                }).collect()
            };
            (list, if n == 0 { None } else { Some(sel) })
        } else {
            (vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))], None)
        };

        let mut list_state = ListState::default().with_selected(selected);
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" bluetooth ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(ctx.nav.focus_zone == 0)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, cols[0], &mut list_state);

        // ── Right: device detail ─────────────────────────────────────────────
        let detail_lines: Vec<Line<'static>> = if let Ok(devs) = ctx.live.bluetooth.try_read() {
            let sel = self.selected.min(devs.len().saturating_sub(1));
            if let Some(d) = devs.get(sel) {
                vec![
                    Line::from(vec![Span::styled("name      ", theme.dim()), Span::styled(d.name.clone(), Style::default().fg(theme.fg))]),
                    Line::from(vec![Span::styled("mac       ", theme.dim()), Span::styled(d.mac.clone(), Style::default().fg(theme.accent))]),
                    Line::from(vec![Span::styled("paired    ", theme.dim()), Span::styled(bool_label(d.paired), Style::default().fg(if d.paired { theme.ok } else { theme.dim }))]),
                    Line::from(vec![Span::styled("connected ", theme.dim()), Span::styled(bool_label(d.connected), Style::default().fg(if d.connected { theme.ok } else { theme.dim }))]),
                    Line::from(vec![Span::styled("trusted   ", theme.dim()), Span::styled(bool_label(d.trusted), Style::default().fg(if d.trusted { theme.ok } else { theme.dim }))]),
                    if let Some(rssi) = d.rssi {
                        Line::from(vec![Span::styled("rssi      ", theme.dim()), Span::styled(format!("{rssi} dBm"), Style::default().fg(theme.fg))])
                    } else {
                        Line::from(Span::styled("rssi      —", theme.dim()))
                    },
                    Line::from(""),
                    Line::from(Span::styled("  A = connect/disconnect", theme.dim())),
                    Line::from(Span::styled("  s = scan for devices", theme.dim())),
                ]
            } else {
                vec![Line::from(Span::styled("(no device selected)", theme.dim()))]
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

fn bool_label(b: bool) -> &'static str { if b { "yes" } else { "no" } }
