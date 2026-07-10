//! Network screen v2 — interface list (left) + WiFi / detail (right).
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const ZONES: &[Zone] = &[Zone::Left, Zone::Right];

pub struct NetworkScreenV2 {
    pub selected: usize,
}

impl Default for NetworkScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for NetworkScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Network }
    fn title(&self) -> &str { "Network" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼ select   ◀▶ pane   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.interfaces.try_read().map(|v| v.len()).unwrap_or(0);
        match event {
            NavEvent::Left  => { ctx.nav.focus_zone = 0; Consumed::Yes }
            NavEvent::Right => { ctx.nav.focus_zone = 1; Consumed::Yes }
            NavEvent::Tab   => { ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % ZONES.len(); Consumed::Yes }
            NavEvent::BackTab => {
                let n = ZONES.len();
                ctx.nav.focus_zone = (ctx.nav.focus_zone + n - 1) % n;
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let left_focused  = ctx.nav.focus_zone == 0;
        let right_focused = ctx.nav.focus_zone == 1;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // ── Left: interfaces ─────────────────────────────────────────────────
        let selected = if let Ok(ifaces) = ctx.live.interfaces.try_read() {
            let n = ifaces.len();
            self.selected.min(n.saturating_sub(1))
        } else { 0 };

        let items: Vec<ListItem<'static>> = if let Ok(ifaces) = ctx.live.interfaces.try_read() {
            ifaces.iter().map(|iface| {
                let state_style = if iface.state.eq_ignore_ascii_case("up") {
                    theme.ok()
                } else {
                    theme.dim()
                };
                let mut spans = vec![
                    Span::styled(format!("{:<12}", iface.name), Style::default().fg(theme.fg)),
                    Span::styled(format!("{:<6}", iface.state), state_style),
                ];
                if let Some(ip) = iface.ipv4.first() {
                    spans.push(Span::styled(format!(" {ip}"), Style::default().fg(theme.accent)));
                }
                ListItem::new(Line::from(spans))
            }).collect()
        } else {
            vec![ListItem::new(Line::from(Span::styled(
                "  (refreshing…)", Style::default().fg(theme.dim)
            )))]
        };

        let mut list_state = ListState::default().with_selected(if items.is_empty() { None } else { Some(selected) });
        let iface_list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" interfaces ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(left_focused)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(iface_list, cols[0], &mut list_state);

        // ── Right: detail for selected interface ─────────────────────────────
        let detail_lines: Vec<Line<'static>> = if let Ok(ifaces) = ctx.live.interfaces.try_read() {
            if let Some(iface) = ifaces.get(selected) {
                let mut v = vec![
                    Line::from(vec![
                        Span::styled("name   ", Style::default().fg(theme.dim)),
                        Span::styled(iface.name.clone(), Style::default().fg(theme.fg)),
                    ]),
                    Line::from(vec![
                        Span::styled("state  ", Style::default().fg(theme.dim)),
                        Span::styled(iface.state.clone(), if iface.state.eq_ignore_ascii_case("up") { theme.ok() } else { theme.dim() }),
                    ]),
                ];
                for ip in &iface.ipv4 {
                    v.push(Line::from(vec![
                        Span::styled("ipv4   ", Style::default().fg(theme.dim)),
                        Span::styled(ip.clone(), Style::default().fg(theme.accent)),
                    ]));
                }
                for ip in &iface.ipv6 {
                    v.push(Line::from(vec![
                        Span::styled("ipv6   ", Style::default().fg(theme.dim)),
                        Span::styled(ip.clone(), Style::default().fg(theme.fg)),
                    ]));
                }
                if let Ok(ssid) = ctx.live.active_ssid.try_read() {
                    if let Some(s) = ssid.as_ref() {
                        v.push(Line::from(vec![
                            Span::styled("ssid   ", Style::default().fg(theme.dim)),
                            Span::styled(s.clone(), Style::default().fg(theme.ok)),
                        ]));
                    }
                }
                v
            } else {
                vec![Line::from(Span::styled("(no interface selected)", Style::default().fg(theme.dim)))]
            }
        } else {
            vec![Line::from(Span::styled("(refreshing…)", Style::default().fg(theme.dim)))]
        };

        let detail = Paragraph::new(detail_lines)
            .block(Block::default()
                .title(Span::styled(" detail ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(right_focused)));
        frame.render_widget(detail, cols[1]);
    }
}
