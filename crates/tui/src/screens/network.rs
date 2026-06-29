//! Network screen: interfaces + Wi-Fi (scan, connect, disconnect, toggle up/down).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
        // j/k (and Up/Down) move a unified "selected row" across the two
        // panes: the interface list (left) is indexed first, then the wifi
        // scan rows (right). This keeps the screen fully navigable from
        // the keyboard without needing Tab to flip between panes.
        let iface_count = app.live.interfaces.try_read().map(|v| v.len()).unwrap_or(0);
        let wifi_count = app.wifi_scan_results.len();
        let total = iface_count + wifi_count;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.net_selected = (app.net_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.net_selected = app.net_selected.saturating_sub(1);
                return true;
            }
            KeyCode::Char('h') | KeyCode::Left => {
                // Jump to the last iface (or 0 if there are none).
                app.net_selected = if iface_count > 0 { iface_count - 1 } else { 0 };
                return true;
            }
            KeyCode::Char('l') | KeyCode::Right => {
                // Jump to the first wifi row, if any.
                if wifi_count > 0 {
                    app.net_selected = iface_count;
                }
                return true;
            }
            KeyCode::Char('G') | KeyCode::End => {
                // Jump to the last item in the unified list (last wifi).
                if total > 0 {
                    app.net_selected = total - 1;
                }
                return true;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                // Jump back to the first iface row.
                app.net_selected = 0;
                return true;
            }
            KeyCode::Enter => {
                // Enter on a wifi row opens the connect flow.
                if app.net_selected >= iface_count {
                    if let Some(net) = app.wifi_scan_results.get(app.net_selected - iface_count) {
                        if net.security.is_empty() {
                            // Open network — connect straight away.
                            let ssid = net.ssid.clone();
                            let _ = app.tx.try_send(Action::Run(RunAction::WifiConnect {
                                ssid,
                                password: None,
                            }));
                        } else {
                            // Secured — prompt for password.
                            let ssid = net.ssid.clone();
                            app.pending_ssid = Some(ssid);
                            app.modal = Modal::Input {
                                prompt: "Wi-Fi password:".into(),
                                buf: String::new(),
                                kind: InputKind::WifiPassword,
                            };
                        }
                        return true;
                    }
                }
            }
            KeyCode::Char('r') => {
                // Dispatch through Action::Run so the existing
                // `RunAction::WifiScan` handler in main.rs populates
                // `app.wifi_scan_results` (the screen's render reads
                // from there). Calling `cyberdeck_core::net::wifi_scan`
                // directly and only emitting a toast used to leave
                // the wifi list empty — networks were parsed then
                // discarded.
                if app
                    .tx
                    .try_send(Action::Run(RunAction::WifiScan))
                    .is_err()
                {
                    // Channel full / closed — surface it rather than
                    // silently dropping the scan request.
                    app.push_toast(ToastKind::Error, "scan dispatch failed");
                }
                return true;
            }
            KeyCode::Char('c') => {
                app.modal = Modal::Input {
                    prompt: "Connect to SSID:".into(),
                    buf: String::new(),
                    kind: InputKind::ConnectSSID,
                };
                return true;
            }
            KeyCode::Char('d') => {
                app.modal = Modal::Confirm {
                    message: "Disconnect from Wi-Fi?".into(),
                    kind: crate::app::ConfirmKind::DisconnectWifi,
                    arg: String::new(),
                };
                return true;
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
                return true;
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

        // Reserve the bottom row for hints.
        let body_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(body_area);

        let iface_count = app.live.interfaces.try_read().map(|v| v.len()).unwrap_or(0);
        let wifi_count = app.wifi_scan_results.len();
        let total = iface_count + wifi_count;
        if total == 0 {
            app.net_selected = 0;
        } else if app.net_selected >= total {
            app.net_selected = total - 1;
        }

        // Left: interfaces
        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(ifaces) = app.live.interfaces.try_read() {
            for (i, iface) in ifaces.iter().enumerate() {
                let state_color = if iface.state.eq_ignore_ascii_case("up") {
                    theme.ok()
                } else {
                    theme.dim()
                };
                let mut line = Line::from(vec![
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
                let _ = i;
            }
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (no interfaces)",
                theme.dim(),
            ))));
        }
        let iface_selected = if app.net_selected < iface_count {
            Some(app.net_selected)
        } else {
            None
        };
        let left_h = cols[0].height as usize;
        let mut left_state = ListState::default().with_selected(iface_selected);
        *left_state.offset_mut() = compute_offset(iface_selected.unwrap_or(0), items.len(), left_h);
        let left = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" interfaces ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(false)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(left, cols[0], &mut left_state);

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
        let wifi_header_rows = if app.live.active_ssid.try_read().is_ok() {
            2
        } else {
            0
        };
        // `app.net_selected` indexes the combined iface+wifi list. For the
        // wifi list (right pane), we only show selection if we're past the
        // interface region, and the wifi-row index is shifted by the
        // header offset.
        let wifi_selected = if app.net_selected >= iface_count {
            let row = app.net_selected - iface_count + wifi_header_rows;
            if row < items.len() {
                Some(row)
            } else {
                None
            }
        } else {
            None
        };
        let right_h = cols[1].height as usize;
        let mut right_state = ListState::default().with_selected(wifi_selected);
        *right_state.offset_mut() =
            compute_offset(wifi_selected.unwrap_or(0), items.len(), right_h);
        let right = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" wifi ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(false)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(right, cols[1], &mut right_state);

        // Footer: hints + position.
        let pos = if total == 0 {
            "  no items".to_string()
        } else {
            format!("  {}/{}  ", app.net_selected + 1, total)
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(" r ", theme.key()),
            Span::styled("scan  ", theme.dim()),
            Span::styled(" c ", theme.key()),
            Span::styled("connect  ", theme.dim()),
            Span::styled(" d ", theme.key()),
            Span::styled("disconnect  ", theme.dim()),
            Span::styled(" ⏎ ", theme.key()),
            Span::styled("join  ", theme.dim()),
            Span::styled(" space ", theme.key()),
            Span::styled("iface up/down", theme.dim()),
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
    // Clamp `selected` into the valid range so a stale cursor can't
    // produce a negative or out-of-range offset.
    let sel = selected.min(total - 1);
    if sel >= visible {
        // Scroll down: keep the cursor at the bottom row of the visible
        // window. This is the common case after pressing Down past the
        // first page.
        sel - visible + 1
    } else {
        // Cursor is still in the first page; no scroll needed.
        0
    }
}

fn current_iface(app: &App) -> Option<cyberdeck_core::net::Interface> {
    let ifaces = app.live.interfaces.try_read().ok()?;
    let n = ifaces.len();
    if n == 0 {
        return None;
    }
    let idx = app.net_selected.min(n - 1);
    ifaces.get(idx).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};
    use cyberdeck_core::net::{Interface, WifiNetwork};
    use tokio::sync::mpsc;

    fn kc(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn make_app() -> App {
        let (tx, rx) = mpsc::channel(16);
        App::new(tx, rx)
    }

    async fn push_ifaces(app: &App, n: usize) {
        let ifaces: Vec<Interface> = (0..n)
            .map(|i| Interface {
                name: format!("eth{i}"),
                state: "up".into(),
                mac: format!("00:11:22:33:44:{i:02x}"),
                ipv4: vec!["10.0.0.1".into()],
                ipv6: vec![],
            })
            .collect();
        *app.live.interfaces.write().await = ifaces;
    }

    fn push_wifi(app: &mut App, n: usize) {
        app.wifi_scan_results = (0..n)
            .map(|i| WifiNetwork {
                ssid: format!("net-{:02}", i),
                signal: 50,
                security: "WPA2".into(),
                in_use: false,
            })
            .collect();
    }

    // Pressing `j`/`Down` walks the unified `net_selected` cursor from the
    // iface region into the wifi region. Without auto-scan + the unified
    // cursor, the user sees an empty wifi list and `Down` does nothing.
    #[tokio::test]
    async fn arrows_walk_iface_into_wifi_region() {
        let mut app = make_app();
        push_ifaces(&app, 2).await;
        push_wifi(&mut app, 30);
        let mut screen = NetworkScreen;

        // Start at iface row 0.
        assert_eq!(app.net_selected, 0);
        // Press Down 3 times: should walk iface[0]→iface[1]→wifi[0]→wifi[1].
        for expected in [1usize, 2, 3] {
            assert!(screen.on_key(kc(KeyCode::Down), &mut app));
            assert_eq!(
                app.net_selected, expected,
                "after Down, net_selected should be {expected}"
            );
        }
    }

    // `G` (End) jumps the cursor to the last wifi network, not the last
    // iface. This is the "wifi menu navigatable" guarantee.
    #[tokio::test]
    async fn end_jumps_to_last_wifi_network() {
        let mut app = make_app();
        push_ifaces(&app, 2).await;
        push_wifi(&mut app, 30);
        let mut screen = NetworkScreen;

        assert!(screen.on_key(kc(KeyCode::End), &mut app));
        // iface_count + wifi_count - 1 = 2 + 30 - 1 = 31
        assert_eq!(app.net_selected, 31);
    }

    // Wifi pane render with 30 networks: the wifi pane's `wifi_selected`
    // shifts from None (iface region) to Some(row) when the cursor enters
    // the wifi region. After pressing Down enough times, the wifi pane
    // should show a selection on the last wifi row, and `compute_offset`
    // should shift the view so the selected row is visible.
    #[tokio::test]
    async fn wifi_pane_scrolls_with_30_networks() {
        let mut app = make_app();
        push_ifaces(&app, 2).await;
        push_wifi(&mut app, 30);
        let mut screen = NetworkScreen;

        // Walk to the last wifi network.
        for _ in 0..31 {
            screen.on_key(kc(KeyCode::Down), &mut app);
        }
        assert_eq!(app.net_selected, 31);

        // Reproduce the wifi-pane selection logic from render().
        let iface_count = 2;
        let wifi_header_rows = 2; // active + "scan results:"
        let items_len = wifi_header_rows + 30; // 32
        let right_h = 10;
        let wifi_selected = if app.net_selected >= iface_count {
            let row = app.net_selected - iface_count + wifi_header_rows;
            if row < items_len {
                Some(row)
            } else {
                None
            }
        } else {
            None
        };
        // selected row = 31 - 2 + 2 = 31
        assert_eq!(wifi_selected, Some(31));
        // offset should clamp to items_len - right_h = 32 - 10 = 22
        let offset = compute_offset(31, items_len, right_h);
        assert_eq!(offset, 22);
        // The selected row is visible: offset <= selected < offset + visible.
        assert!(offset <= 31 && 31 < offset + right_h);
    }

    // Pressing `r` must dispatch `Action::Run(RunAction::WifiScan)` so the
    // handler at main.rs populates `app.wifi_scan_results`. Before the fix,
    // the screen spawned its own task and only emitted a Toast — networks
    // were parsed then discarded, so the user saw an empty wifi list.
    #[tokio::test]
    async fn r_key_dispatches_wifi_scan_run_action() {
        let mut app = make_app();
        push_ifaces(&app, 1).await;
        let mut screen = NetworkScreen;

        assert!(screen.on_key(kc(KeyCode::Char('r')), &mut app));

        // Drain the channel: the screen must have sent exactly one
        // Action::Run(RunAction::WifiScan).
        let sent = app.rx.try_recv().expect("expected a queued action");
        match sent {
            Action::Run(RunAction::WifiScan) => {}
            other => panic!("expected RunAction::WifiScan, got {other:?}"),
        }
        // And nothing else leaked into the channel.
        assert!(app.rx.try_recv().is_err());
    }
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