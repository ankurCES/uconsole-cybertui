//! Logs screen: tail `journalctl -f` into a scrollable buffer.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
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
            KeyCode::Char('c') => app.logs.clear(),
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
                        let entry = crate::app::action::Action::Toast(
                            crate::app::toast::ToastKind::Info,
                            format!("log fetched: {} line(s) so far", line.len()),
                        );
                        // Push a log line directly into the app.
                        let _ = tx
                            .send(crate::app::action::Action::LogPushed(LogLine {
                                ts: Local::now(),
                                line,
                            }))
                            .await;
                        let _ = entry;
                    }
                });
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Logs ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let items: Vec<ListItem> = app
            .logs
            .iter()
            .rev()
            .take(inner.height as usize)
            .map(|l| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", l.ts.format("%H:%M:%S")), theme.dim()),
                    Span::styled(l.line.clone(), theme.fg),
                ]))
            })
            .collect();
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        let hints = Paragraph::new(Line::from(vec![
            Span::styled(" f ", theme.key()),
            Span::styled("fetch last 50  ", theme.dim()),
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
