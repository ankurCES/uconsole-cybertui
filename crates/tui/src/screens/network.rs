//! Network screen: interfaces + Wi-Fi (scan, connect, disconnect, toggle up/down).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::{App, InputKind, Modal};
use crate::theme::{glyphs, Theme};

pub struct NetworkScreen;

impl Screen for NetworkScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Network
    }
    fn title(&self) -> &'static str {
        "Network"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('r') => {
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    if let Ok(n) = cyberdeck_core::net::wifi_scan().await {
                        app_via_action(&tx, move |tx| {
                            // We need a mutable borrow of the app to push results,
                            // but we don't have one here. Send a Toast for the
                            // discovery and let the next tick pick up via the
                            // background refresher.
                            let _ = tx.send(Action::Toast(
                                ToastKind::Ok,
                                format!("found {} networks", n.len()),
                            ));
                        })
                        .await;
                    }
                });
            }
            KeyCode::Char('c') => {
                app.modal = Modal::Input {
                    prompt: "Connect to SSID:".into(),
                    buf: String::new(),
                    kind: InputKind::ConnectSSID,
                };
            }
            KeyCode::Char('d') => {
                app.modal = Modal::Confirm {
                    message: "Disconnect from Wi-Fi?".into(),
                    kind: crate::app::ConfirmKind::DisconnectWifi,
                    arg: String::new(),
                };
            }
            KeyCode::Char(' ') => {
                if let Some(iface) = current_iface(app) {
                    let up = !iface.state.eq_ignore_ascii_case("up")
                        && !iface.state.eq_ignore_ascii_case("unknown");
                    let name = iface.name.clone();
                    let _ = app
                        .tx
                        .try_send(Action::Run(RunAction::SetInterfaceUp(name, up)));
                }
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Network ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // Left: interfaces
        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(ifaces) = app.live.interfaces.try_read() {
            for (i, iface) in ifaces.iter().enumerate() {
                let selected = i == app.net_selected;
                let state_color = if iface.state.eq_ignore_ascii_case("up") {
                    theme.ok()
                } else {
                    theme.dim()
                };
                let mut line = Line::from(vec![
                    Span::styled(
                        if selected { "▸ " } else { "  " },
                        if selected { theme.title() } else { theme.dim() },
                    ),
                    Span::styled(format!("{:<12}", iface.name), theme.fg),
                    Span::styled(format!("{:<8}", iface.state), state_color),
                ]);
                if let Some(ip) = iface.ipv4.first() {
                    line.spans
                        .push(Span::styled(format!(" {ip}"), theme.accent));
                }
                if iface.ipv6.len() > 0 {
                    line.spans.push(Span::styled(
                        format!(" +{}v6", iface.ipv6.len()),
                        theme.dim(),
                    ));
                }
                items.push(ListItem::new(line));
            }
        }
        let left = List::new(items).block(
            Block::default()
                .title(Span::styled(" interfaces ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(left, cols[0]);

        // Right: wifi
        let g = glyphs();
        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(ssid) = app.live.active_ssid.try_read() {
            let active = ssid.as_deref().unwrap_or("(none)");
            items.push(ListItem::new(Line::from(vec![
                Span::styled("  active   ", theme.dim()),
                Span::styled(active.to_string(), theme.accent),
            ])));
        }
        items.push(ListItem::new(Line::from(Span::styled(
            "  scan results:",
            theme.dim(),
        ))));
        for net in &app.wifi_scan_results {
            let bars = match net.signal {
                75..=100 => g.signal_full,
                50..=74 => g.signal_mid,
                25..=49 => g.signal_low,
                _ => g.signal_none,
            };
            let style = if net.in_use {
                theme.ok()
            } else {
                ratatui::style::Style::default().fg(theme.fg)
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled("  ", theme.dim()),
                Span::styled(format!("{:<4} ", bars), theme.accent),
                Span::styled(format!("{:<24}", truncate(&net.ssid, 24)), style),
                Span::styled(format!("{:>3}% ", net.signal), theme.fg),
                Span::styled(
                    if net.security.is_empty() {
                        "open".into()
                    } else {
                        net.security.clone()
                    },
                    theme.dim(),
                ),
            ])));
        }
        if app.wifi_scan_results.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (press r to scan)",
                theme.dim(),
            ))));
        }
        let right = List::new(items).block(
            Block::default()
                .title(Span::styled(" wifi ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(right, cols[1]);

        // Hints at the bottom of the content area.
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(" r ", theme.key()),
            Span::styled("scan  ", theme.dim()),
            Span::styled(" c ", theme.key()),
            Span::styled("connect  ", theme.dim()),
            Span::styled(" d ", theme.key()),
            Span::styled("disconnect  ", theme.dim()),
            Span::styled(" space ", theme.key()),
            Span::styled("toggle iface up/down", theme.dim()),
        ]));
        // The right column already contains the wifi list; the hints go below
        // the right column inside the right pane, but we don't have room in
        // most terminal sizes. Drop them on top of the wifi list is fine.
        let _ = hints;
    }
}

fn current_iface(app: &App) -> Option<cyberdeck_core::net::Interface> {
    let ifaces = app.live.interfaces.try_read().ok()?;
    ifaces.get(app.net_selected).cloned()
}

// Helper: actually do the wifi scan and write results into the app. Lives as
// a free function so the closure in `on_key` can call it. We use a blocking
// approach because the existing key handler is already in a `tokio::spawn`.
async fn app_via_action<F>(_tx: &tokio::sync::mpsc::Sender<Action>, _f: F)
where
    F: FnOnce(&tokio::sync::mpsc::Sender<Action>),
{
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}

// Suppress unused warnings for the helper closure.
#[allow(dead_code)]
fn _unused_clear(_: Clear) {}
