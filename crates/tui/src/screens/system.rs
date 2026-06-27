//! System screen: live values driven by the background refresh task.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::{glyphs, Theme};
use cyberdeck_core::sys;

pub struct SystemScreen;

impl Screen for SystemScreen {
    fn id(&self) -> ScreenId {
        ScreenId::System
    }
    fn title(&self) -> &'static str {
        "System Status"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" System ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Two columns: facts left, recent log right.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(inner);

        // Left: facts. Read live data; fall back to placeholder if not ready.
        let g = glyphs();
        let info = app.live.info.clone();
        let bat = app.live.battery.clone();
        let th = app.live.thermals.clone();
        let mut lines: Vec<Line> = Vec::new();
        if let Ok(info) = info.try_read() {
            lines.push(Line::from(vec![
                Span::styled("hostname  ", theme.dim()),
                Span::styled(info.hostname.clone(), theme.fg),
            ]));
            lines.push(Line::from(vec![
                Span::styled("kernel    ", theme.dim()),
                Span::styled(info.kernel.clone(), theme.fg),
            ]));
            lines.push(Line::from(vec![
                Span::styled("os        ", theme.dim()),
                Span::styled(info.os.clone(), theme.fg),
            ]));
            lines.push(Line::from(vec![
                Span::styled("arch      ", theme.dim()),
                Span::styled(info.arch.clone(), theme.fg),
            ]));
            lines.push(Line::from(vec![
                Span::styled("uptime    ", theme.dim()),
                Span::styled(sys::format_uptime(info.uptime_secs), theme.fg),
            ]));
            lines.push(Line::from(vec![
                Span::styled("load      ", theme.dim()),
                Span::styled(
                    format!(
                        "{:.2} {:.2} {:.2}",
                        info.loadavg.0, info.loadavg.1, info.loadavg.2
                    ),
                    theme.fg,
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("cpu       ", theme.dim()),
                Span::styled(
                    format!("{} × {}", info.cpu_count, trim(&info.cpu_model, 40)),
                    theme.fg,
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("memory    ", theme.dim()),
                Span::styled(sys::format_mem(&info.memory), theme.fg),
            ]));
        }
        if let Ok(bat) = bat.try_read() {
            if let Some(b) = bat.as_ref() {
                let cap_style = if b.capacity < 20 {
                    theme.error()
                } else if b.capacity < 50 {
                    theme.warn()
                } else {
                    theme.ok()
                };
                let eta = b
                    .time_to_full
                    .as_deref()
                    .or(b.time_to_empty.as_deref())
                    .map(|s| format!(" · ETA {s}"))
                    .unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled("battery   ", theme.dim()),
                    Span::styled(format!("{}% · {}{}", b.capacity, b.status, eta), cap_style),
                ]));
                if let Some(w) = b.power_now_w {
                    lines.push(Line::from(vec![
                        Span::styled("power     ", theme.dim()),
                        Span::styled(format!("{w:.2} W"), theme.fg),
                    ]));
                }
            }
        }
        if let Ok(th) = th.try_read() {
            for r in th.iter().take(3) {
                let style = if r.temp_c > 75.0 {
                    theme.error()
                } else if r.temp_c > 60.0 {
                    theme.warn()
                } else {
                    theme.ok()
                };
                lines.push(Line::from(vec![
                    Span::styled("thermal   ", theme.dim()),
                    Span::styled(format!("{} {:.1}°C", r.label, r.temp_c), style),
                ]));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("{}  auto-refresh every 1s", g.arrow),
            theme.dim(),
        )));

        let left = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(theme.border(false)),
            )
            .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
        f.render_widget(left, cols[0]);

        // Right: latest log lines.
        let recent: Vec<ListItem> = app
            .logs
            .iter()
            .rev()
            .take(20)
            .map(|l| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", l.ts.format("%H:%M:%S")), theme.dim()),
                    Span::styled(l.line.clone(), theme.fg),
                ]))
            })
            .collect();
        let right = List::new(recent).block(
            Block::default()
                .title(Span::styled(" recent log ", theme.title()))
                .borders(Borders::NONE),
        );
        f.render_widget(right, cols[1]);
    }
}

fn trim(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

// Suppress the unused-import warning for Wrap (kept for future paragraphs that wrap).
#[allow(dead_code)]
fn _wrap_import(_: Wrap) {}
