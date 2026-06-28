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
            KeyCode::Char('f') => {
                // Spawn a one-shot journalctl -n 50 fetch (no -f so we don't block).
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, BufReader};
                    use tokio::process::Command;
                    let mut child = match Command::new("journalctl")
                        .args(["-n", "50", "--no-pager", "-q"])
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
                    while let Ok(Some(line)) = lines.next_line().await {
                        let _ = tx
                            .send(crate::app::action::Action::LogPushed(LogLine {
                                ts: Local::now(),
                                line,
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
                    Span::styled(l.line.clone(), theme.fg),
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
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(mode, theme.dim()),
            Span::raw("  "),
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