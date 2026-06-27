//! Cross-cutting widgets: header (live values), sidebar (screen list),
//! status bar (keymap hints + clock), toast overlay.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::theme::{glyphs, Theme};

pub fn header_lines(app: &App) -> Vec<Line<'static>> {
    let g = glyphs();
    let mut spans: Vec<Span<'static>> = vec![
        " cyberdeck ".into(),
        Span::styled(
            "▸ ",
            ratatui::style::Style::default().fg(Theme::by_name(crate::theme::ThemeName::Dark).dim),
        ),
    ];
    if let Ok(info) = app.live.info.try_read() {
        spans.push(format!("{} ", info.hostname).into());
        spans.push(Span::styled(
            "· ",
            ratatui::style::Style::default().fg(Theme::by_name(crate::theme::ThemeName::Dark).dim),
        ));
        spans.push(format!("{} ", info.os).into());
        spans.push(Span::styled(
            "· ",
            ratatui::style::Style::default().fg(Theme::by_name(crate::theme::ThemeName::Dark).dim),
        ));
        if let Ok(ssid) = app.live.active_ssid.try_read() {
            spans.push(format!("{} {} ", g.wifi, ssid.as_deref().unwrap_or("—")).into());
        }
        if let Ok(ifaces) = app.live.interfaces.try_read() {
            if let Some(primary) = ifaces.iter().find(|i| !i.ipv4.is_empty()) {
                spans.push(
                    format!(
                        "{} {} {} ",
                        g.net,
                        primary.name,
                        primary.ipv4.first().cloned().unwrap_or_default()
                    )
                    .into(),
                );
            }
        }
        if let Ok(b) = app.live.battery.try_read() {
            if let Some(bat) = b.as_ref() {
                spans.push(format!("{} {}% ", g.bat, bat.capacity).into());
            }
        }
        if let Ok(th) = app.live.thermals.try_read() {
            if let Some(t) = th.first() {
                spans.push(format!("{} {:.0}°C ", g.temp, t.temp_c).into());
            }
        }
    }
    if let Ok(enabled) = app.live.web_enabled.try_read() {
        if *enabled {
            if let Ok(url) = app.live.web_url.try_read() {
                if let Some(u) = url.as_ref() {
                    spans.push(Span::styled(
                        format!(" web: {u} "),
                        ratatui::style::Style::default()
                            .fg(Theme::by_name(crate::theme::ThemeName::Dark).accent_2),
                    ));
                }
            }
        }
    }
    vec![Line::from(spans)]
}

pub fn draw_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let p = Paragraph::new(header_lines(app))
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(theme.border(false)),
        );
    f.render_widget(p, area);
}

pub fn draw_sidebar(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = matches!(app.focus, Focus::Sidebar);
    let items: Vec<ListItem> = crate::app::screen::ScreenId::ALL
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let selected = *id == app.current;
            let prefix = if selected { g().arrow } else { " " };
            let num = format!("{:>2}", i + 1);
            let label = id.label();
            let glyph = id.glyph();
            let line = Line::from(vec![
                Span::styled(
                    format!("{prefix} "),
                    if selected {
                        ratatui::style::Style::default()
                            .fg(theme.accent)
                            .add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        ratatui::style::Style::default().fg(theme.dim)
                    },
                ),
                Span::styled(format!("{num} "), theme.dim()),
                Span::styled(format!("{glyph} "), theme.accent),
                Span::styled(
                    label.to_string(),
                    if selected {
                        ratatui::style::Style::default()
                            .fg(theme.fg)
                            .add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        ratatui::style::Style::default().fg(theme.fg)
                    },
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(Span::styled(" screens ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(focused)),
        )
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
    f.render_widget(list, area);
}

fn g() -> &'static crate::theme::Glyphs {
    glyphs()
}

pub fn draw_status(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let hints = vec![
        Span::styled(" ↑↓ ", theme.key()),
        Span::styled("nav ", theme.dim()),
        Span::styled(" enter ", theme.key()),
        Span::styled("open ", theme.dim()),
        Span::styled(" r ", theme.key()),
        Span::styled("refresh ", theme.dim()),
        Span::styled(" : ", theme.key()),
        Span::styled("palette ", theme.dim()),
        Span::styled(" ? ", theme.key()),
        Span::styled("help ", theme.dim()),
        Span::styled(" q ", theme.key()),
        Span::styled("quit ", theme.dim()),
    ];
    let mut spans = hints;
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        app.clock.format("%H:%M:%S").to_string(),
        theme.accent,
    ));
    let line = Line::from(spans);
    let p = Paragraph::new(line)
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(theme.border(false)),
        );
    f.render_widget(p, area);
}

pub fn draw_toasts(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.toasts.is_empty() {
        return;
    }
    let w = (area.width as i32 - 4).max(8) as u16;
    let h = app.toasts.len() as u16;
    let x = area.x + (area.width.saturating_sub(w + 2)) / 2;
    let y = area.y + area.height.saturating_sub(h + 2);
    let rect = Rect::new(x, y, w + 2, h + 2);
    let items: Vec<ListItem> = app
        .toasts
        .iter()
        .map(|t| {
            let style = match t.kind {
                crate::app::toast::ToastKind::Info => theme.dim(),
                crate::app::toast::ToastKind::Ok => theme.ok(),
                crate::app::toast::ToastKind::Warn => theme.warn(),
                crate::app::toast::ToastKind::Error => theme.error(),
            };
            ListItem::new(Line::from(Span::styled(t.text.clone(), style)))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(false))
            .title(Span::styled(" notifications ", theme.title())),
    );
    f.render_widget(list, rect);
}

pub fn chunks(area: Rect) -> (Rect, Rect, Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(20)])
        .split(outer[1]);
    (outer[0], body[0], body[1])
}
