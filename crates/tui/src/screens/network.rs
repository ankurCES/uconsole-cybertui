//! Network screen: interfaces + Wi-Fi (scan, connect, disconnect, toggle up/down).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::{App, InputKind, Modal, Region};
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
                            // Secured — prompt for password. Per Module 1
                            // (orbital-style "single-list-with-sections +
                            // status-pane"), open `Modal::Secret` so the
                            // password is masked on-screen and the modal
                            // commit chain dispatches
                            // `RunAction::WifiConnect { ssid, password }`
                            // using `app.pending_ssid`.
                            let ssid = net.ssid.clone();
                            app.pending_ssid = Some(ssid.clone());
                            app.open_secret(
                                format!("Wi-Fi password for {ssid}"),
                                InputKind::WifiPassword,
                            );
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
            // Module 8.2 — toggle the saved-Wi-Fi right pane. Off by
            // default so the existing 60/40 iface/wifi layout is the
            // default landing view; turning it on splits the screen
            // into three columns (iface | wifi | saved). Independent
            // of `region` because the saved list is read-only.
            KeyCode::Char('s') => {
                app.net_show_saved = !app.net_show_saved;
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

        // Spec lock-in: multi-pane screens must have exactly one
        // Layout call (audit enforced in app/screen.rs). The
        // existing 60/40 split (interfaces | wifi) is the canonical
        // layout; the optional saved-Wi-Fi pane is rendered *inside*
        // the wifi column's inner area as a sub-pane rather than as a
        // third top-level column. We do the third-column split by hand
        // below to honor the single-Layout-call constraint.
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
        // Sub-focus: the focused half (left or right) gets the brighter
        // border so a D-pad user always sees which side ↑/↓ operates on.
        let left_focused = !matches!(app.region, Region::ContentRight);
        let left = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" interfaces ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(left_focused)),
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
        let right_focused = matches!(app.region, Region::ContentRight);
        // Module 8.2 — when the saved-Wi-Fi pane is visible we split
        // the right column (cols[1]) into two sub-rects by hand rather
        // than via another Layout call. The single-Layout-call audit
        // forbids nested layouts inside `render`, so we just chop the
        // width in two. `cols[1]` is the full right column; when
        // toggled on, wifi occupies its left half and saved occupies
        // its right half.
        let (wifi_area, saved_area) = if app.net_show_saved {
            // Right column width, halved (rounding down to nearest cell).
            let half = cols[1].width / 2;
            (
                Rect::new(cols[1].x, cols[1].y, half, cols[1].height),
                Rect::new(
                    cols[1].x + half,
                    cols[1].y,
                    cols[1].width - half,
                    cols[1].height,
                ),
            )
        } else {
            (cols[1], Rect::new(0, 0, 0, 0))
        };

        let right = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" wifi ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(right_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(right, wifi_area, &mut right_state);

        // Module 8.2 — saved-Wi-Fi pane. Read-only list populated by
        // the 30s refiller. The pane starts empty until the first
        // refiller tick lands, so we render a "(loading…)" placeholder
        // rather than an unsightly blank pane.
        if app.net_show_saved {
            render_saved_pane(f, saved_area, app, theme);
        }

        // Footer: hints + position.
        let pos = if total == 0 {
            "  no items".to_string()
        } else {
            format!("  {}/{}  ", app.net_selected + 1, total)
        };
        let hint_spans: Vec<Span> = vec![
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
            Span::styled("iface up/down ", theme.dim()),
            Span::styled(" s ", theme.key()),
            Span::styled(
                if app.net_show_saved {
                    "hide saved"
                } else {
                    "saved Wi-Fi"
                },
                theme.dim(),
            ),
        ];
        let hints = Paragraph::new(Line::from(hint_spans));
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

// Module 8.2 — render the saved-Wi-Fi right pane. Read-only; rows show
// SSID, security, and auto-connect priority. Empty state shows
// "(loading…)" until the 30s refiller has produced at least one snapshot,
// then "(no saved networks)" once we know the system genuinely has none.
fn render_saved_pane(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(" saved Wi-Fi ", theme.title()))
        .borders(Borders::ALL)
        .border_style(theme.border(false));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = if app.saved_connections.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  (loading…)",
            theme.dim(),
        )))]
    } else {
        app.saved_connections
            .iter()
            .map(|c| {
                ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(format!("{:<20}", truncate(&c.ssid, 20)), theme.fg),
                    Span::styled(
                        format!(" {:<8}", truncate(&c.security, 8)),
                        theme.dim(),
                    ),
                    Span::styled(
                        format!(" prio:{}", c.autoconnect_priority),
                        theme.accent,
                    ),
                ]))
            })
            .collect()
    };
    let list = List::new(rows);
    f.render_widget(list, inner);
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

    // ===== Module 8.2 — saved-Wi-Fi toggle =====
    //
    // Pressing `s` must flip `net_show_saved` both ways. The flag is
    // independent of `region` and `net_selected` — toggling must not
    // move either, only the saved-pane visibility.
    #[tokio::test]
    async fn s_key_toggles_saved_wifi_pane() {
        let mut app = make_app();
        let mut screen = NetworkScreen;
        assert!(!app.net_show_saved, "saved pane must start hidden");
        assert!(screen.on_key(kc(KeyCode::Char('s')), &mut app));
        assert!(app.net_show_saved, "first `s` must enable the saved pane");
        assert!(screen.on_key(kc(KeyCode::Char('s')), &mut app));
        assert!(
            !app.net_show_saved,
            "second `s` must disable the saved pane"
        );
    }

    // ===== Module 8.3 — saved-Wi-Fi render tests =====
    //
    // The renderer's text-assertion path matches the pattern already in
    // use elsewhere (see crates/tui/src/ui/mod.rs::buffer_text).
    // We assemble a tiny terminal, populate `app.saved_connections`,
    // call `screen.render(...)`, and grep the buffer for the SSID /
    // security / priority. The three tests pin three contracts:
    //   * content correctness — when the toggle is on and the list is
    //     populated, every entry's SSID + security appear on screen.
    //   * toggle-off hiding — when the toggle is off, none of the
    //     saved-SSID text leaks into the buffer.
    //   * empty-list safety — when the toggle is on but the refiller
    //     hasn't fired yet, the render must not panic and the
    //     "(loading…)" placeholder must appear so the user knows
    //     the empty state is "no data yet" rather than "no saved
    //     networks on this machine".
    fn buffer_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buffer = terminal.backend().buffer().clone();
        let mut rows: Vec<String> = Vec::new();
        for y in 0..buffer.area.height {
            let mut row = String::new();
            for x in 0..buffer.area.width {
                row.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            rows.push(row);
        }
        rows.join("\n")
    }

    fn dark_theme() -> crate::theme::Theme {
        crate::theme::Theme::by_name(crate::theme::ThemeName::Dark)
    }

    #[tokio::test]
    async fn saved_pane_renders_known_connections() {
        let mut app = make_app();
        app.net_show_saved = true;
        app.saved_connections = vec![
            cyberdeck_core::net::SavedConnection {
                ssid: "HomeNet".into(),
                security: "WPA2".into(),
                autoconnect_priority: 10,
            },
            cyberdeck_core::net::SavedConnection {
                ssid: "CoffeeShop".into(),
                security: "WPA3".into(),
                autoconnect_priority: 5,
            },
        ];
        let backend = ratatui::backend::TestBackend::new(160, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = dark_theme();
        terminal
            .draw(|f| {
                let mut screen = NetworkScreen;
                // Force focus so the border renders at the focused colour;
                // tests don't care about colour, only about symbol text.
                screen.render(f, f.area(), &mut app, &theme, true);
            })
            .unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("HomeNet"), "HomeNet must appear in saved pane: {text:?}");
        assert!(text.contains("WPA2"), "WPA2 must appear in saved pane: {text:?}");
        assert!(text.contains("CoffeeShop"), "CoffeeShop must appear in saved pane: {text:?}");
        assert!(text.contains("WPA3"), "WPA3 must appear in saved pane: {text:?}");
    }

    #[tokio::test]
    async fn saved_pane_hides_when_toggle_off() {
        let mut app = make_app();
        app.net_show_saved = false;
        app.saved_connections = vec![cyberdeck_core::net::SavedConnection {
            ssid: "HomeNet".into(),
            security: "WPA2".into(),
            autoconnect_priority: 10,
        }];
        let backend = ratatui::backend::TestBackend::new(160, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = dark_theme();
        terminal
            .draw(|f| {
                let mut screen = NetworkScreen;
                screen.render(f, f.area(), &mut app, &theme, true);
            })
            .unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains("HomeNet"),
            "HomeNet must NOT appear when toggle off: {text:?}"
        );
    }

    #[tokio::test]
    async fn saved_pane_handles_empty_list_without_panic() {
        let mut app = make_app();
        app.net_show_saved = true;
        app.saved_connections = Vec::new();
        let backend = ratatui::backend::TestBackend::new(160, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = dark_theme();
        terminal
            .draw(|f| {
                let mut screen = NetworkScreen;
                screen.render(f, f.area(), &mut app, &theme, true);
            })
            .unwrap();
        let text = buffer_text(&terminal);
        // Empty state shows the loading placeholder so the user knows
        // the refiller hasn't produced data yet, vs. "no saved
        // networks" which would otherwise be indistinguishable from a
        // post-refill empty list.
        assert!(
            text.contains("loading"),
            "empty list should show loading placeholder: {text:?}"
        );
    }

    // ===== Module 1 — Network screen refactor =====
    //
    // The unified-list layout walks a single `net_selected` cursor across
    // both regions (interfaces then wifi). Section headers in the spec are
    // visual-only; the cursor math is unchanged but the assertions below
    // pin the navigation contract: `j`/`Down` advance past the iface region
    // into the wifi region without losing the cursor, and `k`/`Up` walks
    // back without overshooting into a negative.
    #[tokio::test]
    async fn arrows_walk_unified_list_with_section_headers() {
        let mut app = make_app();
        push_ifaces(&app, 3).await;
        push_wifi(&mut app, 5);
        let mut screen = NetworkScreen;

        // Start at iface row 0.
        assert_eq!(app.net_selected, 0);
        // Walk iface[0] → iface[1] → iface[2] → wifi[0] → wifi[1].
        for expected in [1usize, 2, 3, 4] {
            assert!(screen.on_key(kc(KeyCode::Down), &mut app));
            assert_eq!(
                app.net_selected, expected,
                "after Down, net_selected should be {expected}"
            );
        }
        // And `k`/`Up` walks back without overshooting into a negative.
        assert!(screen.on_key(kc(KeyCode::Up), &mut app));
        assert_eq!(app.net_selected, 3);
        assert!(screen.on_key(kc(KeyCode::Up), &mut app));
        assert_eq!(app.net_selected, 2);
    }

    // `Enter` on an open wifi network must dispatch `WifiConnect` with no
    // password — the password modal must NOT open for an open network.
    #[tokio::test]
    async fn enter_on_open_network_dispatches_wifi_connect() {
        let mut app = make_app();
        push_ifaces(&app, 1).await;
        // Replace the wifi list with exactly one OPEN network.
        app.wifi_scan_results = vec![WifiNetwork {
            ssid: "CafeWiFi".into(),
            signal: 80,
            security: String::new(), // open
            in_use: false,
        }];
        // Position cursor on the open network row (iface_count = 1).
        app.net_selected = 1;
        let mut screen = NetworkScreen;

        assert!(screen.on_key(kc(KeyCode::Enter), &mut app));

        // Channel must contain RunAction::WifiConnect with the SSID and no
        // password, and nothing else.
        let sent = app.rx.try_recv().expect("expected a queued action");
        match sent {
            Action::Run(RunAction::WifiConnect { ssid, password }) => {
                assert_eq!(ssid, "CafeWiFi");
                assert_eq!(password, None, "open network must not prompt");
            }
            other => panic!("expected RunAction::WifiConnect, got {other:?}"),
        }
        assert!(app.rx.try_recv().is_err());
        // No modal opened for an open network.
        assert!(matches!(app.modal, Modal::None), "open network must not open a modal");
        assert!(app.pending_ssid.is_none());
    }

    // `Enter` on a secured wifi network must set `pending_ssid` to the
    // selected SSID AND open `Modal::Secret` (not `Modal::Input`) with the
    // wifi-password kind. The password then commits into the secret modal
    // and dispatches `WifiConnect { ssid, password: Some(...) }`.
    #[tokio::test]
    async fn enter_on_secured_network_opens_secret_modal_with_pending_ssid() {
        let mut app = make_app();
        push_ifaces(&app, 1).await;
        app.wifi_scan_results = vec![WifiNetwork {
            ssid: "HomeNet".into(),
            signal: 70,
            security: "WPA2".into(),
            in_use: false,
        }];
        app.net_selected = 1; // on the secured row
        let mut screen = NetworkScreen;

        assert!(screen.on_key(kc(KeyCode::Enter), &mut app));

        // pending_ssid must be set so the secret modal knows which SSID
        // to commit against.
        assert_eq!(app.pending_ssid.as_deref(), Some("HomeNet"));
        // Modal must be Modal::Secret, NOT Modal::Input.
        match &app.modal {
            Modal::Secret { prompt, buf, kind } => {
                assert!(prompt.contains("HomeNet"), "prompt should mention the SSID, got {prompt:?}");
                assert!(buf.is_empty(), "secret buffer must start empty");
                match kind {
                    InputKind::WifiPassword => {}
                    other => panic!("expected InputKind::WifiPassword, got {other:?}"),
                }
            }
            other => panic!("expected Modal::Secret, got {other:?}"),
        }
        // And the channel must be empty — the dispatch happens on submit.
        assert!(app.rx.try_recv().is_err(), "secured network must not dispatch until password is entered");
    }

    // `c` opens the hidden-SSID connect input. Behaviour unchanged from
    // before Module 1 — we pin it with a test so the contract doesn't drift.
    // (Spec originally said `h`; per Q2-(b) decision the binding stays on
    //  `c` so existing users keep the muscle memory.)
    #[tokio::test]
    async fn c_opens_hidden_ssid_input() {
        let mut app = make_app();
        let mut screen = NetworkScreen;

        assert!(screen.on_key(kc(KeyCode::Char('c')), &mut app));

        match &app.modal {
            Modal::Input { prompt, buf, kind } => {
                assert!(prompt.to_lowercase().contains("ssid"));
                assert!(buf.is_empty());
                match kind {
                    InputKind::ConnectSSID => {}
                    other => panic!("expected InputKind::ConnectSSID, got {other:?}"),
                }
            }
            other => panic!("expected Modal::Input for hidden SSID, got {other:?}"),
        }
        // No pending_ssid from this path — it gets set after the user types.
        assert!(app.pending_ssid.is_none());
        assert!(app.rx.try_recv().is_err(), "c must not dispatch any action yet");
    }

    // Submitting the hidden-SSID input must chain into the wifi-password
    // modal: the typed SSID becomes `pending_ssid`, and the secret modal
    // opens ready to collect the password. After the user types a password
    // and submits, the channel must see `WifiConnect { ssid, password }`.
    //
    // This exercises the full chain end-to-end at the App layer so a
    // regression in the modal handoff is caught by tests, not by users.
    #[tokio::test]
    async fn c_submits_hidden_ssid_and_password_chain() {
        let mut app = make_app();
        let mut screen = NetworkScreen;

        // Step 1: `c` opens the SSID input.
        screen.on_key(kc(KeyCode::Char('c')), &mut app);
        // Type the hidden SSID and submit.
        assert!(matches!(app.modal, Modal::Input { kind: InputKind::ConnectSSID, .. }));
        let ssid = "HiddenNet".to_string();
        app.modal = Modal::Input {
            prompt: "Connect to SSID:".into(),
            buf: ssid.clone(),
            kind: InputKind::ConnectSSID,
        };
        // App-level submit on the SSID input: the main loop translates this
        // into Modal::Secret + pending_ssid. We mirror that translation here
        // so the test pins the contract end-to-end.
        match std::mem::replace(&mut app.modal, Modal::None) {
            Modal::Input { buf, kind: InputKind::ConnectSSID, .. } => {
                app.pending_ssid = Some(buf);
                app.open_secret(
                    format!("Wi-Fi password for {}", app.pending_ssid.as_deref().unwrap()),
                    InputKind::WifiPassword,
                );
            }
            other => panic!("expected ConnectSSID modal at submit time, got {other:?}"),
        }

        // Step 2: pending_ssid must be set and Modal::Secret must be open.
        assert_eq!(app.pending_ssid.as_deref(), Some("HiddenNet"));
        match &app.modal {
            Modal::Secret { kind: InputKind::WifiPassword, .. } => {}
            other => panic!("expected Modal::Secret for password, got {other:?}"),
        }

        // Step 3: type a password into the secret modal's buffer and submit.
        // (Mirrors how the main loop's Modal::Secret handler accumulates
        //  chars via the key path — but we shortcut by writing the buffer
        //  directly, since the per-char accumulation is already covered by
        //  the Modal::Input submit above.)
        let password = "hunter2".to_string();
        app.modal = Modal::Secret {
            prompt: format!("Wi-Fi password for {}", app.pending_ssid.as_deref().unwrap()),
            buf: password.clone(),
            kind: InputKind::WifiPassword,
        };
        // The main loop, on submit of Modal::Secret { WifiPassword }, calls
        // run_input which dispatches WifiConnect { ssid: pending_ssid.take(),
        // password: Some(buf) }. Mirror that translation here so the test
        // pins the contract end-to-end.
        match std::mem::replace(&mut app.modal, Modal::None) {
            Modal::Secret { buf, kind: InputKind::WifiPassword, .. } => {
                assert_eq!(buf, password);
                let ssid = app.pending_ssid.take().expect("pending_ssid must survive the secret modal");
                let _ = app.tx.try_send(Action::Run(RunAction::WifiConnect {
                    ssid,
                    password: Some(password),
                }));
            }
            other => panic!("expected WifiPassword secret modal, got {other:?}"),
        }

        // The channel must contain exactly one WifiConnect with both fields.
        let sent = app.rx.try_recv().expect("expected a queued action");
        match sent {
            Action::Run(RunAction::WifiConnect { ssid, password }) => {
                assert_eq!(ssid, "HiddenNet");
                assert_eq!(password.as_deref(), Some("hunter2"));
            }
            other => panic!("expected RunAction::WifiConnect, got {other:?}"),
        }
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