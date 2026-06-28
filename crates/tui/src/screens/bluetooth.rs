//! Bluetooth screen: list devices, pair/connect/trust.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
        let total = app.live.bluetooth.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.bt_selected = (app.bt_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.bt_selected = app.bt_selected.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total > 0 {
                    app.bt_selected = (app.bt_selected + 10).min(total - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.bt_selected = app.bt_selected.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.bt_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total > 0 {
                    app.bt_selected = total - 1;
                }
                return true;
            }
            _ => {}
        }
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

        // Reserve the bottom row for hints.
        let list_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let total = app.live.bluetooth.try_read().map(|v| v.len()).unwrap_or(0);
        if total == 0 {
            app.bt_selected = 0;
        } else if app.bt_selected >= total {
            app.bt_selected = total - 1;
        }

        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(devs) = app.live.bluetooth.try_read() {
            if devs.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "  (no devices — install bluetoothctl + bluez, then press P to power on)",
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
                    Span::styled(format!("{:<17}", d.mac), theme.fg),
                    Span::styled(format!("{:<24}", truncate(&d.name, 24)), theme.fg),
                    Span::styled(format!("{:<10}", state), status),
                    Span::styled(format!("{:<10}", signal), theme.dim()),
                    Span::styled(format!("[{trusted}]"), theme.warn()),
                ])));
            }
        }
        let visible_h = list_area.height as usize;
        let offset = compute_offset(app.bt_selected, items.len(), visible_h);
        let mut state = ListState::default().with_selected(if total > 0 {
            Some(app.bt_selected)
        } else {
            None
        });
        *state.offset_mut() = offset;
        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(list, list_area, &mut state);

        let pos = if total == 0 {
            "  no devices".to_string()
        } else {
            format!("  {}/{}  ", app.bt_selected + 1, total)
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
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

/// Compute the scroll offset that keeps `selected` visible inside a window
/// of `visible` rows drawn from a list of `total` items. Top-aligned:
/// shifts only when the cursor scrolls past the bottom (or top) edge of
/// the visible window, so the view visually tracks the cursor immediately
/// instead of waiting until the cursor reaches the middle (which is what a
/// centred offset does, and which makes long lists look frozen at the top
/// until you've already half-scrolled). PgUp/PgDn still feel symmetric
/// because each call recomputes from the current cursor.
fn compute_offset(selected: usize, total: usize, visible: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    let sel = selected.min(total - 1);
    if sel >= visible {
        sel - visible + 1
    } else {
        0
    }
}

fn selected_device(app: &App) -> Option<cyberdeck_core::bluetooth::BtDevice> {
    let d = app.live.bluetooth.try_read().ok()?;
    let n = d.len();
    if n == 0 {
        return None;
    }
    let idx = app.bt_selected.min(n - 1);
    d.get(idx).cloned()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use cyberdeck_core::bluetooth::BtDevice;
    use tokio::sync::mpsc;

    fn kc(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn make_app() -> App {
        let (tx, rx) = mpsc::channel(16);
        App::new(tx, rx)
    }

    async fn push_bt(app: &App, n: usize) {
        let devs: Vec<BtDevice> = (0..n)
            .map(|i| BtDevice {
                mac: format!("AA:BB:CC:DD:EE:{i:02X}"),
                name: format!("dev-{i:02}"),
                paired: false,
                connected: false,
                trusted: false,
                rssi: Some(-50),
            })
            .collect();
        *app.live.bluetooth.write().await = devs;
    }

    // Pressing `j`/`Down` walks `bt_selected` from 0 through the device list.
    #[tokio::test]
    async fn arrows_walk_bt_device_list() {
        let mut app = make_app();
        push_bt(&app, 5).await;
        let mut screen = BluetoothScreen;

        assert_eq!(app.bt_selected, 0);
        for expected in [1usize, 2, 3, 4] {
            assert!(screen.on_key(kc(KeyCode::Down), &mut app));
            assert_eq!(app.bt_selected, expected);
        }
        // One more Down should clamp at the last item (4).
        screen.on_key(kc(KeyCode::Down), &mut app);
        assert_eq!(app.bt_selected, 4);
    }

    // `End` jumps to the last device in a long list.
    #[tokio::test]
    async fn end_jumps_to_last_bt_device() {
        let mut app = make_app();
        push_bt(&app, 30).await;
        let mut screen = BluetoothScreen;

        assert!(screen.on_key(kc(KeyCode::End), &mut app));
        assert_eq!(app.bt_selected, 29);
    }

    // With 30 devices and a 10-row viewport, the offset must clamp so the
    // selected row stays visible. This is the "right-side pane scrollable"
    // guarantee from the user's bug report.
    #[tokio::test]
    async fn bt_pane_scrolls_with_30_devices() {
        let mut app = make_app();
        push_bt(&app, 30).await;
        let mut screen = BluetoothScreen;

        // Jump to last device.
        screen.on_key(kc(KeyCode::End), &mut app);
        assert_eq!(app.bt_selected, 29);

        // Reproduce the list-pane scroll logic from render().
        let items_len = 30;
        let visible_h = 10;
        let offset = compute_offset(29, items_len, visible_h);
        // offset should clamp to items_len - visible_h = 30 - 10 = 20
        assert_eq!(offset, 20);
        // The selected row is visible: offset <= selected < offset + visible.
        assert!(offset <= 29 && 29 < offset + visible_h);

        // Now jump back to top — offset should reset to 0.
        screen.on_key(kc(KeyCode::Home), &mut app);
        assert_eq!(app.bt_selected, 0);
        let offset = compute_offset(0, items_len, visible_h);
        assert_eq!(offset, 0);
    }
}