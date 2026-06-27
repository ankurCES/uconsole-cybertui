//! Bluetooth screen: list devices, pair/connect/trust.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::App;
use crate::theme::Theme;

pub struct BluetoothScreen;

impl Screen for BluetoothScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Bluetooth
    }
    fn title(&self) -> &'static str {
        "Bluetooth"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        if let Some(dev) = selected_device(app) {
            let act = match key.code {
                KeyCode::Char('p') => Some(RunAction::BluetoothPair(dev.mac.clone())),
                KeyCode::Char('c') => Some(RunAction::BluetoothConnect(dev.mac.clone())),
                KeyCode::Char('C') => Some(RunAction::BluetoothDisconnect(dev.mac.clone())),
                KeyCode::Char('t') => Some(RunAction::BluetoothTrust(dev.mac.clone())),
                _ => None,
            };
            if let Some(a) = act {
                let _ = app.tx.try_send(Action::Run(a));
                return true;
            }
        }
        match key.code {
            KeyCode::Char('P') => {
                // Toggle adapter power
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    let res = cyberdeck_core::bluetooth::list().await;
                    let on = res.as_ref().map(|v| v.is_empty()).unwrap_or(true);
                    match cyberdeck_core::bluetooth::adapter_power(on).await {
                        Ok(_) => {
                            let _ = tx
                                .send(Action::Toast(
                                    ToastKind::Ok,
                                    format!("bluetooth {}", if on { "on" } else { "off" }),
                                ))
                                .await;
                        }
                        Err(e) => {
                            let _ = tx
                                .send(Action::Toast(ToastKind::Error, format!("{e}")))
                                .await;
                        }
                    }
                });
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Bluetooth ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(devs) = app.live.bluetooth.try_read() {
            if devs.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "  (no devices — install bluetoothctl + bluez)",
                    theme.dim(),
                ))));
            }
            for d in devs.iter() {
                let status = if d.connected {
                    theme.ok()
                } else if d.paired {
                    ratatui::style::Style::default().fg(theme.accent)
                } else {
                    theme.dim()
                };
                let state = if d.connected {
                    "connected"
                } else if d.paired {
                    "paired"
                } else {
                    "—"
                };
                let signal = d
                    .rssi
                    .map(|r| format!("{r} dBm"))
                    .unwrap_or_else(|| "—".into());
                let trusted = if d.trusted { "✓" } else { " " };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(format!("{:<17}", d.mac), theme.fg),
                    Span::styled(format!("{:<24}", truncate(&d.name, 24)), theme.fg),
                    Span::styled(format!("{:<10}", state), status),
                    Span::styled(format!("{:<10}", signal), theme.dim()),
                    Span::styled(format!("[{trusted}]"), theme.warn()),
                ])));
            }
        }
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        let hints = Paragraph::new(Line::from(vec![
            Span::styled(" p ", theme.key()),
            Span::styled("pair  ", theme.dim()),
            Span::styled(" c ", theme.key()),
            Span::styled("connect  ", theme.dim()),
            Span::styled(" C ", theme.key()),
            Span::styled("disconnect  ", theme.dim()),
            Span::styled(" t ", theme.key()),
            Span::styled("trust  ", theme.dim()),
            Span::styled(" P ", theme.key()),
            Span::styled("adapter power", theme.dim()),
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

fn selected_device(app: &App) -> Option<cyberdeck_core::bluetooth::BtDevice> {
    let d = app.live.bluetooth.try_read().ok()?;
    // Selection: the first connected device, else the first paired, else first.
    d.iter()
        .find(|x| x.connected)
        .or_else(|| d.iter().find(|x| x.paired))
        .or_else(|| d.first())
        .cloned()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

#[allow(dead_code)]
fn _b(_: Borders) {}
