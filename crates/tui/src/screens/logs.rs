//! Logs screen: tail `journalctl -f` into a scrollable buffer.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::{App, LogLine};
use crate::theme::Theme;
use chrono::Local;

pub struct LogsScreen;

impl Screen for LogsScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Logs
    }
    fn title(&self) -> &'static str {
        "Logs"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('c') => {
                app.logs.clear();
                app.logs_offset = 0;
                return true;
            }
            // Module 2.4 — `r` requests an immediate 60s fetch. The handler
            // is intentionally tiny: it enqueues `Action::RefreshLogs`
            // synchronously (the channel is bounded to 256 actions,
            // and this is a tiny control message — no payload). The
            // dispatcher arm (main.rs) does the journalctl invocation
            // on a Tokio task and routes results back through
            // `Action::LogPushed` so dedupe + ordering keep working.
            // The screen's `on_key` MUST stay non-blocking — a 60s
            // journalctl on a busy box can take hundreds of ms, so we
            // don't even attempt to send it from here.
            //
            // `try_send` is sync (no await) so the test can observe
            // the action with `rx.try_recv()` immediately after
            // `on_key` returns. If the channel is ever full we drop
            // the request silently — the 1Hz refiller will catch up.
            KeyCode::Char('r') => {
                let _ = app.tx.try_send(crate::app::action::Action::RefreshLogs);
                return true;
            }
            KeyCode::Char('f') => {
                // Spawn a one-shot journalctl -n 50 fetch (no -f so we don't block).
                // Module 2.3: `recent_since` returns (ts, message) tuples
                // parsed from `--output=json` so the rendered line carries
                // the event's actual journal time, not fetch time.
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, BufReader};
                    use tokio::process::Command;
                    let mut child = match Command::new("journalctl")
                        .args([
                            "-n", "50",
                            "--no-pager",
                            "-q",
                            "-o", "json",
                        ])
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                    {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx
                                .send(crate::app::action::Action::Toast(
                                    crate::app::toast::ToastKind::Error,
                                    format!("journalctl: {e}"),
                                ))
                                .await;
                            return;
                        }
                    };
                    let stdout = child.stdout.take().unwrap();
                    let mut lines = BufReader::new(stdout).lines();
                    use chrono::{DateTime, Utc};
                    while let Ok(Some(raw)) = lines.next_line().await {
                        if raw.is_empty() {
                            continue;
                        }
                        let v: serde_json::Value = match serde_json::from_slice(raw.as_bytes()) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let ts_us = v
                            .get("__REALTIME_TIMESTAMP")
                            .and_then(|x| x.as_str())
                            .and_then(|s| s.parse::<i64>().ok());
                        let ts = match ts_us {
                            Some(us) => DateTime::<Utc>::from_timestamp(
                                us / 1_000_000,
                                (us % 1_000_000).unsigned_abs() as u32 * 1_000,
                            )
                            .map(|dt| dt.with_timezone(&Local))
                            .unwrap_or_else(Local::now),
                            None => continue,
                        };
                        let msg = v
                            .get("MESSAGE")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        if msg.is_empty() {
                            continue;
                        }
                        let _ = tx
                            .send(crate::app::action::Action::LogPushed(LogLine {
                                ts,
                                message: msg,
                            }))
                            .await;
                    }
                });
                return true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.logs_offset = app.logs_offset.saturating_add(1);
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.logs_offset = app.logs_offset.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                app.logs_offset = app.logs_offset.saturating_add(10);
                return true;
            }
            KeyCode::PageUp => {
                app.logs_offset = app.logs_offset.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                // g = jump to top of buffer (oldest line).
                app.logs_offset = usize::MAX;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                // G = jump to live tail.
                app.logs_offset = 0;
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Logs ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Reserve bottom row for hints.
        let list_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let visible_h = list_area.height as usize;
        let total = app.logs.len();
        // `logs_offset` counts lines from the live tail (0 == newest). We
        // cap it at `total.saturating_sub(visible)` so we can't scroll
        // past the oldest line. usize::MAX from 'g' saturates to the cap.
        let max_off = total.saturating_sub(visible_h);
        if app.logs_offset > max_off {
            app.logs_offset = max_off;
        }
        // Build items from the slice we want to display: [total - visible - off, total - off)
        let end = total.saturating_sub(app.logs_offset);
        let start = end.saturating_sub(visible_h);
        let items: Vec<ListItem> = app.logs[start..end]
            .iter()
            .map(|l| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", l.ts.format("%H:%M:%S")), theme.dim()),
                    Span::styled(l.message.clone(), theme.fg),
                ]))
            })
            .collect();
        // ListState's offset is the topmost visible row index. With our
        // slice it's 0 — the slice *is* the visible window — but we still
        // surface a selection at the bottom row so the highlight bar
        // lands on the "current" tail line (matters when paused scrolling).
        let highlight = if items.is_empty() {
            None
        } else {
            Some(items.len() - 1)
        };
        let mut state = ListState::default().with_selected(highlight);
        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(list, list_area, &mut state);

        let mode = if app.logs_offset == 0 {
            "  ● live (G to scroll up, j/k step, PgUp/PgDn page)"
        } else {
            "  ⏸ paused — press G to jump back to live tail"
        };
        let pos = format!("  {} lines  ", total);
        // Module 2.4 — `r` triggers an immediate 60s fetch (vs. the 1Hz
        // refiller's 2s sliding window). The fetch happens off the UI
        // thread inside the dispatcher's `Action::RefreshLogs` arm, and
        // results flow back through the normal `LogPushed` pipeline so
        // dedupe + ordering keep working.
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(mode, theme.dim()),
            Span::raw("  "),
            Span::styled(" r ", theme.key()),
            Span::styled("refresh (live)  ", theme.dim()),
            Span::styled(" f ", theme.key()),
            Span::styled("fetch  ", theme.dim()),
            Span::styled(" c ", theme.key()),
            Span::styled("clear", theme.dim()),
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

#[cfg(test)]
mod tests {
    //! Module 2.4 — pin the `r` handler's contract:
    //! - It routes through the action channel (no in-line `journalctl`
    //!   spawn), so the UI thread never blocks on a process invocation.
    //! - It dispatches `Action::RefreshLogs`, which the dispatcher arm
    //!   in `main.rs` catches and turns into a 60s `recent_since` fetch
    //!   on a Tokio task.
    //! - It returns `true` so the screen consumes the key (matches the
    //!   other Logs-screen handlers' contract).
    use super::*;
    use crate::app::action::Action;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    fn fresh_app_with_observer() -> (App, mpsc::Receiver<Action>) {
        // `App::new` consumes both endpoints of its channel, but the
        // field is effectively dead — the actual dispatcher lives in
        // `main.rs` and consumes a separate `rx`. For tests we hand a
        // dummy `tx`/`rx` pair to `App::new` (so it constructs) and a
        // *separate* pair as our observation channel. The screen
        // sends via `app.tx`, which is the App's own clone — we
        // observe through the second channel.
        //
        // Concretely: build the App's channel (consumed by App::new),
        // then build a second channel and *replace* `app.tx` with its
        // sender. That gives us the contract: "the screen uses
        // `app.tx` to send" without fighting `App::new`'s ownership
        // of `rx`.
        let (_app_tx, app_rx) = mpsc::channel::<Action>(8);
        let (tx, rx) = mpsc::channel::<Action>(8);
        let mut app = App::new(_app_tx, app_rx);
        app.tx = tx;
        (app, rx)
    }

    fn key_r() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('r'), crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn logs_screen_r_sends_refresh_logs_action() {
        // Synchronous test: drive the screen's on_key handler and assert
        // an `Action::RefreshLogs` arrives on the observation channel.
        // We use a dedicated runtime so we can poll the receiver
        // without an async fn signature.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (mut app, mut rx) = fresh_app_with_observer();
            app.current = ScreenId::Logs;

            let mut screen = LogsScreen;
            let start = Instant::now();
            let consumed = screen.on_key(key_r(), &mut app);
            let elapsed = start.elapsed();

            assert!(
                consumed,
                "r must be consumed by the Logs screen (it has its own handler)"
            );
            assert!(
                elapsed < Duration::from_millis(50),
                "r handler must be non-blocking (elapsed = {:?})",
                elapsed
            );

            // The dispatcher hasn't run yet, but the action is queued
            // on the channel the screen sent it through.
            let action = rx.try_recv().expect("r must enqueue Action::RefreshLogs");
            assert!(
                matches!(action, Action::RefreshLogs),
                "r must enqueue Action::RefreshLogs, got {:?}",
                action
            );
        });
    }

    #[test]
    fn logs_screen_other_keys_still_unaffected_by_r_handler() {
        // `c` still clears the buffer; `r` does not. This pins that
        // the new arm doesn't accidentally shadow the existing
        // handlers.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (mut app, mut rx) = fresh_app_with_observer();
            app.current = ScreenId::Logs;
            // Seed a line so we can prove `c` cleared it.
            app.logs.push(crate::app::LogLine {
                ts: Local::now(),
                message: "seed".into(),
            });

            let mut screen = LogsScreen;
            let consumed = screen.on_key(
                KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::NONE),
                &mut app,
            );
            assert!(consumed, "c must still be consumed");
            assert!(app.logs.is_empty(), "c must still clear the buffer");
            assert!(
                rx.try_recv().is_err(),
                "c must not enqueue an Action (no channel send)"
            );
        });
    }
}
