//! System screen: live values driven by the background refresh task.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::{App, Region};
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

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                // Scroll the right-hand log pane down (away from the tail).
                app.system_log_offset = app.system_log_offset.saturating_add(1);
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.system_log_offset = app.system_log_offset.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                app.system_log_offset = app.system_log_offset.saturating_add(10);
                return true;
            }
            KeyCode::PageUp => {
                app.system_log_offset = app.system_log_offset.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                // g = jump to top of log (oldest).
                app.system_log_offset = usize::MAX;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                // G = jump back to live tail.
                app.system_log_offset = 0;
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" System ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Reserve bottom row for hints.
        let body_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        // Two columns: facts left, recent log right.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(body_area);

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

        // Wrap so long fields (CPU model) stay inside the column.
        let left_focused = !matches!(app.region, Region::ContentRight);
        let left = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(Span::styled(" facts ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(left_focused)),
            )
            .wrap(Wrap { trim: false })
            .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
        f.render_widget(left, cols[0]);

        // Right: latest log lines. `system_log_offset` counts lines back
        // from the tail (0 == newest). Cap so we never scroll past the
        // oldest entry, even when the user holds PageDown.
        let right_area = cols[1];
        let visible_h = right_area.height as usize;
        let total = app.logs.len();
        let max_off = total.saturating_sub(visible_h);
        if app.system_log_offset > max_off {
            app.system_log_offset = max_off;
        }
        let end = total.saturating_sub(app.system_log_offset);
        let start = end.saturating_sub(visible_h);
        // We want newest at the bottom of the slice (most recent visible).
        let recent: Vec<ListItem> = if total == 0 {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no log lines yet)",
                theme.dim(),
            )))]
        } else {
            app.logs[start..end]
                .iter()
                .map(|l| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", l.ts.format("%H:%M:%S")), theme.dim()),
                        Span::styled(l.line.clone(), theme.fg),
                    ]))
                })
                .collect()
        };
        let highlight = if total == 0 {
            None
        } else {
            Some(recent.len().saturating_sub(1))
        };
        let mut state = ListState::default().with_selected(highlight);
        // Sub-focus border. When region is ContentRight the right pane
        // gets the brighter border so the user sees which column ↑/↓
        // will move in. Otherwise (Sidebar or ContentLeft) the left
        // pane — drawn further up — gets the focus border.
        let right_focused = matches!(app.region, Region::ContentRight);
        let right = List::new(recent)
            .block(
                Block::default()
                    .title(Span::styled(
                        format!(
                            " recent log ({}/{}) ",
                            end,
                            total
                        ),
                        theme.title(),
                    ))
                    .borders(Borders::ALL)
                    .border_style(theme.border(right_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(right, right_area, &mut state);

        let mode = if app.system_log_offset == 0 {
            "  ● live (j/k step, PgUp/PgDn page, G to live)"
        } else {
            "  ⏸ paused — press G to jump back to live tail"
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(mode, theme.dim()),
            Span::raw("  "),
            Span::styled(" r ", theme.key()),
            Span::styled("refresh  ", theme.dim()),
            Span::styled(" ? ", theme.key()),
            Span::styled("help", theme.dim()),
        ]));
        let hint_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        f.render_widget(hints, hint_area);
    }
}

fn trim(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}