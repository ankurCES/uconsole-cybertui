//! System screen v2 — CPU/RAM/uptime facts (left) + log placeholder (right).
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use cyberdeck_core::sys;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const ZONES: &[Zone] = &[Zone::Left, Zone::Right];

pub struct SystemScreenV2 {
    pub log_offset: usize,
}

impl Default for SystemScreenV2 {
    fn default() -> Self { Self { log_offset: 0 } }
}

impl ScreenV2 for SystemScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::System }
    fn title(&self) -> &str { "System" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼ scroll   ◀▶ pane   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let zone = ZONES.get(ctx.nav.focus_zone).copied().unwrap_or(Zone::Left);
        match event {
            NavEvent::Left  => { ctx.nav.focus_zone = 0; Consumed::Yes }
            NavEvent::Right => { ctx.nav.focus_zone = 1; Consumed::Yes }
            NavEvent::Tab   => { ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % ZONES.len(); Consumed::Yes }
            NavEvent::BackTab => {
                let n = ZONES.len();
                ctx.nav.focus_zone = (ctx.nav.focus_zone + n - 1) % n;
                Consumed::Yes
            }
            NavEvent::Up if zone == Zone::Right => {
                self.log_offset = self.log_offset.saturating_add(1);
                Consumed::Yes
            }
            NavEvent::Down if zone == Zone::Right => {
                self.log_offset = self.log_offset.saturating_sub(1);
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

        // ── Left: live system facts ──────────────────────────────────────────
        let mut lines: Vec<Line<'static>> = Vec::new();

        if let Ok(info) = ctx.live.info.try_read() {
            let push = |lines: &mut Vec<Line<'static>>, label: &'static str, val: String| {
                lines.push(Line::from(vec![
                    Span::styled(label, Style::default().fg(theme.dim)),
                    Span::styled(val, Style::default().fg(theme.fg)),
                ]));
            };
            push(&mut lines, "hostname  ", info.hostname.clone());
            push(&mut lines, "kernel    ", info.kernel.clone());
            push(&mut lines, "os        ", info.os.clone());
            push(&mut lines, "arch      ", info.arch.clone());
            push(&mut lines, "uptime    ", sys::format_uptime(info.uptime_secs));
            push(&mut lines, "load      ", format!(
                "{:.2} {:.2} {:.2}", info.loadavg.0, info.loadavg.1, info.loadavg.2
            ));
            push(&mut lines, "cpu       ", format!(
                "{} × {}", info.cpu_count,
                info.cpu_model.chars().take(40).collect::<String>()
            ));
            push(&mut lines, "memory    ", sys::format_mem(&info.memory));
        } else {
            lines.push(Line::from(Span::styled("(refreshing…)", Style::default().fg(theme.dim))));
        }

        if let Ok(bat) = ctx.live.battery.try_read() {
            if let Some(b) = bat.as_ref() {
                let style = if b.capacity < 20 { theme.error() }
                    else if b.capacity < 50 { theme.warn() }
                    else { theme.ok() };
                let eta = b.time_to_full.as_deref()
                    .or(b.time_to_empty.as_deref())
                    .map(|s| format!(" · ETA {s}"))
                    .unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled("battery   ", Style::default().fg(theme.dim)),
                    Span::styled(format!("{}% · {}{}", b.capacity, b.status, eta), style),
                ]));
            }
        }

        if let Ok(th) = ctx.live.thermals.try_read() {
            for r in th.iter().take(3) {
                let style = if r.temp_c > 75.0 { theme.error() }
                    else if r.temp_c > 60.0 { theme.warn() }
                    else { theme.ok() };
                lines.push(Line::from(vec![
                    Span::styled("thermal   ", Style::default().fg(theme.dim)),
                    Span::styled(format!("{} {:.1}°C", r.label, r.temp_c), style),
                ]));
            }
        }

        let left = Paragraph::new(lines)
            .block(Block::default()
                .title(Span::styled(" facts ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(left_focused)))
            .wrap(Wrap { trim: false });
        frame.render_widget(left, cols[0]);

        // ── Right: log placeholder (log channel wires in S6) ────────────────
        let right = Paragraph::new(Span::styled(
            " log output (wires in S6) ",
            Style::default().fg(theme.dim),
        ))
        .block(Block::default()
            .title(Span::styled(" log ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(right_focused)));
        frame.render_widget(right, cols[1]);
    }
}
