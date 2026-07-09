use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::app::screen::ScreenId;
use crate::app::{App, Modal, Wizard};
use crate::theme::Theme;

fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
    Rect::new(x, y, w, h)
}

pub fn modal_input_lines(prompt: &str, buf: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(prompt.to_string()),
        Line::from(format!("> {buf}")),
        Line::from(vec![
            Span::raw("  "),
            Span::raw("[ OK ]"),
            Span::raw("      "),
            Span::raw("[ Cancel ]"),
        ]),
    ]
}

pub fn modal_secret_lines(prompt: &str, buf: &str) -> Vec<Line<'static>> {
    let masked: String = std::iter::repeat('•').take(buf.chars().count()).collect();
    vec![
        Line::from(prompt.to_string()),
        Line::from(format!("> {masked}▏")),
        Line::from(vec![
            Span::raw("  "),
            Span::raw("[ OK ]"),
            Span::raw("      "),
            Span::raw("[ Cancel ]"),
        ]),
    ]
}

pub fn palette_actions() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = Vec::new();
    for id in ScreenId::ALL {
        v.push(("screen", format!("Go to {}", id.label())));
    }
    v.push(("action", "Reboot".into()));
    v.push(("action", "Shutdown".into()));
    v.push(("action", "Suspend".into()));
    v.push(("action", "Hibernate".into()));
    v.push(("action", "Refresh all".into()));
    v.push(("action", "Start web server".into()));
    v.push(("action", "Stop web server".into()));
    v
}

pub fn draw_modal(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    match &app.modal {
        Modal::None => {}
        Modal::Help => {
            crate::wm::popup::render_with_hints(
                f,
                area,
                "help",
                &[
                    ("region · sidebar", ""),
                    ("↑/↓ j/k", "move cursor"),
                    ("enter / →", "open screen"),
                    ("1..9 0", "jump to screen"),
                    ("region · content", ""),
                    ("↑/↓ j/k", "scroll list"),
                    ("←/h", "step back (or sidebar)"),
                    ("→/l", "step right (multi-pane)"),
                    ("tab", "next screen"),
                    ("shift-tab", "previous screen"),
                    ("esc", "back (sub-screen · or leave to launcher)"),
                    ("b",   "back to launcher"),
                    ("anytime", ""),
                    ("?", "this help"),
                    (":", "command palette"),
                    ("f10 / alt+f", "open menu bar"),
                    ("←/→", "cycle tabs"),
                    ("tab", "next screen"),
                    ("shift-tab", "previous screen"),
                    ("esc", "close menu / modal"),
                    ("r", "refresh current screen"),
                    ("q", "quit"),
                    ("menu · file", ""),
                    ("refresh all", "scan wifi/bluetooth/reload"),
                    ("command palette…", "open command palette"),
                    ("quit", "exit the tui"),
                    ("menu · view", ""),
                    ("units: metric", "°C, km/h"),
                    ("units: imperial", "°F, mph"),
                    ("toggle traffic overlay", "city map traffic"),
                    ("toggle weather panel", "city weather pane"),
                    ("menu · tools", ""),
                    ("rescan wi-fi", "trigger wifi scan"),
                    ("rescan bluetooth", "trigger bluetooth scan"),
                    ("toggle web server", "start/stop http"),
                    ("menu · help", ""),
                    ("show help (?)", "this overlay"),
                    ("toast log (T)", "view all toasts"),
                ],
                theme,
            );
        }
        Modal::CommandPalette => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let mut lines: Vec<Line> = vec![Line::from(format!(":{}", app.palette_buf))];
            let actions = palette_actions();
            let q = app.palette_buf.to_lowercase();
            let filtered: Vec<_> = actions
                .iter()
                .filter(|(_, label)| q.is_empty() || label.to_lowercase().contains(&q))
                .take(8)
                .collect();
            for (i, (_, label)) in filtered.iter().enumerate() {
                let style = if i == app.palette_idx {
                    ratatui::style::Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg)
                } else {
                    ratatui::style::Style::default().fg(theme.fg)
                };
                lines.push(Line::from(Span::styled(
                    label.to_string(),
                    style,
                )));
            }
            let w = 50.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + area.height.saturating_sub(h + 2);
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(" command palette ")
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            f.render_widget(p, rect);
        }
        Modal::Confirm { message, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
            let lines = vec![
                Line::from(message.clone()),
                Line::from(""),
                Line::from("Press Y to confirm, N/Esc to cancel."),
            ];
            let w = 60.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" confirm ")
                        .borders(Borders::ALL)
                        .border_style(theme.warn()),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(p, rect);
        }
        Modal::Input { prompt, buf, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let lines = modal_input_lines(prompt, buf);
            let w = 60.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(" input ")
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            f.render_widget(p, rect);
        }
        Modal::Secret { prompt, buf, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let lines = modal_secret_lines(prompt, buf);
            let w = 60.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(" password ")
                    .borders(Borders::ALL)
                    .border_style(theme.warn()),
            );
            f.render_widget(p, rect);
        }
        Modal::Choice { prompt, options, cursor, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
            let lines: Vec<Line> = vec![Line::from(prompt.clone()), Line::from("")];
            let max_visible = 12usize;
            let start = if *cursor >= max_visible { cursor + 1 - max_visible } else { 0 };
            let end = (start + max_visible).min(options.len());
            let items: Vec<ListItem> = options[start..end]
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let real_i = start + i;
                    let style = if real_i == *cursor {
                        ratatui::style::Style::default()
                            .fg(theme.selection_fg)
                            .bg(theme.selection_bg)
                    } else {
                        ratatui::style::Style::default().fg(theme.fg)
                    };
                    ListItem::new(Line::from(Span::styled(opt.label.clone(), style)))
                })
                .collect();
            let total = options.len();
            let title = format!(" pick ({}/{}) ", cursor.saturating_add(1).min(total.max(1)), total);
            let w = 60.min(area.width.saturating_sub(4));
            let h = ((end - start) as u16 + 4).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            lines.iter().for_each(|l| {
                f.render_widget(
                    Paragraph::new(l.clone()),
                    Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), 1),
                );
            });
            let list = List::new(items).block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            let list_rect = Rect::new(
                rect.x,
                rect.y + 3,
                rect.width,
                rect.height.saturating_sub(3),
            );
            f.render_widget(list, list_rect);
        }
        Modal::Wizard(w) => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
            let (header, body) = match w {
                Wizard::WifiEnterprise { ssid, step, eap, identity, password, anon_or_cert } => {
                    let h = format!("Wi-Fi Enterprise — {ssid}");
                    let b = match step {
                        0 => "Pick EAP method (PEAP/TTLS/TLS/PWD) and press Enter.".to_string(),
                        1 => format!(
                            "Identity: {}",
                            identity.as_deref().unwrap_or("(typing)")
                        ),
                        2 => match eap.as_deref() {
                            Some("TLS") => format!(
                                "Path to client certificate: {}",
                                anon_or_cert.as_deref().unwrap_or("(typing)")
                            ),
                            _ => format!(
                                "Password: {}",
                                if password.is_some() { "•••" } else { "(typing)" }
                            ),
                        },
                        _ => "Ready to connect.".to_string(),
                    };
                    (h, b)
                }
            };
            let lines = vec![Line::from(header), Line::from(""), Line::from(Span::styled(body, theme.warn()))];
            let w_ = 60.min(area.width.saturating_sub(4));
            let h_ = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w_)) / 2;
            let y = area.y + (area.height.saturating_sub(h_)) / 2;
            let rect = rect(x, y, w_, h_);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" wizard ")
                        .borders(Borders::ALL)
                        .border_style(theme.border(true)),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(p, rect);
        }
        Modal::Progress { label, done, total, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};
            let w_ = 60.min(area.width.saturating_sub(4));
            let h_ = 5u16.min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w_)) / 2;
            let y = area.y + (area.height.saturating_sub(h_)) / 2;
            let rect = rect(x, y, w_, h_);
            f.render_widget(Clear, rect);
            let header = Paragraph::new(Line::from(label.clone())).block(
                Block::default()
                    .title(" working ")
                    .borders(Borders::ALL)
                    .border_style(theme.warn()),
            );
            f.render_widget(header, Rect::new(rect.x, rect.y, rect.width, 3));
            let pct = if *total == 0 {
                None
            } else {
                Some(((done.saturating_mul(100)) / total).min(100) as u16)
            };
            let gauge_rect = Rect::new(
                rect.x + 1,
                rect.y + 3,
                rect.width.saturating_sub(2),
                1,
            );
            let label = if let Some(p) = pct {
                format!("{done}/{total} ({p}%)")
            } else {
                "…".to_string()
            };
            let gauge = Gauge::default()
                .gauge_style(theme.warn())
                .label(label)
                .ratio(pct.map(|p| p as f64 / 100.0).unwrap_or(0.0));
            f.render_widget(gauge, gauge_rect);
        }
        Modal::AuthFailure { command, stderr, retry: _ } => {
            let body = format!(
                "Authentication failed: {command}\n\n{}\n\nPress R to retry, Esc to cancel.",
                stderr
            );
            crate::wm::popup::render(
                f,
                area,
                crate::wm::popup::Popup::new("auth required", &body)
                    .with_hint("[r] retry   [esc] cancel"),
                theme,
            );
        }
        Modal::ToastLog => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let total = app.toast_history.len();
            let h = (total.min(area.height.saturating_sub(4) as usize) as u16)
                .max(3)
                .min(area.height.saturating_sub(4));
            let w = 70.min(area.width.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h + 2)) / 2;
            let rect = rect(x, y, w, h + 2);
            f.render_widget(Clear, rect);

            let visible = h as usize;
            let max_off = total.saturating_sub(visible);
            let offset = app.toast_log_offset.min(max_off);

            let lines: Vec<Line> = if total == 0 {
                vec![Line::from("(no toasts yet — try something first)")]
            } else {
                app.toast_history
                    .iter()
                    .rev()
                    .skip(offset)
                    .take(visible)
                    .map(|t| {
                        let prefix = match t.kind {
                            crate::app::toast::ToastKind::Info => "ℹ",
                            crate::app::toast::ToastKind::Ok => "✓",
                            crate::app::toast::ToastKind::Warn => "⚠",
                            crate::app::toast::ToastKind::Error => "✗",
                        };
                        Line::from(format!(
                            "{} {} {}",
                            t.ts.format("%H:%M:%S"),
                            prefix,
                            t.message
                        ))
                    })
                    .collect()
            };

            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(format!(
                        " toast log ({}/{}) ",
                        offset.saturating_add(1).min(total.max(1)),
                        total
                    ))
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            f.render_widget(p, rect);
            let hint_y = rect.y.saturating_add(rect.height);
            if hint_y < area.y + area.height {
                f.render_widget(
                    Paragraph::new(Line::from("[ ↑/↓ ] scroll   [ esc ] close"))
                        .alignment(ratatui::layout::Alignment::Center),
                    Rect::new(
                        x,
                        hint_y,
                        w,
                        1,
                    ),
                );
            }
        }
    }
}
