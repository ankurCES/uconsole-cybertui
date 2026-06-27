//! Audio screen: sinks list with inline volume control.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::App;
use crate::theme::Theme;

pub struct AudioScreen;

impl Screen for AudioScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Audio
    }
    fn title(&self) -> &'static str {
        "Audio"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        if let Some(sink) = selected_sink(app) {
            match key.code {
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    let next = (sink.volume + 5).min(150);
                    let _ = app.tx.try_send(Action::Run(RunAction::SetVolume {
                        target: sink.name.clone(),
                        percent: next,
                    }));
                    return true;
                }
                KeyCode::Char('-') => {
                    let next = sink.volume.saturating_sub(5);
                    let _ = app.tx.try_send(Action::Run(RunAction::SetVolume {
                        target: sink.name.clone(),
                        percent: next,
                    }));
                    return true;
                }
                KeyCode::Char('m') => {
                    let _ = app.tx.try_send(Action::Run(RunAction::MuteSink {
                        target: sink.name.clone(),
                        mute: !sink.muted,
                    }));
                    return true;
                }
                KeyCode::Char('d') => {
                    let _ = app
                        .tx
                        .try_send(Action::Run(RunAction::SetDefaultSink(sink.name.clone())));
                    return true;
                }
                _ => {}
            }
        }
        let _ = key;
        let _ = ToastKind::Info;
        false
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Audio ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(s) = app.live.sinks.try_read() {
            if s.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "  (no sinks — install pipewire-audio or pulseaudio)",
                    theme.dim(),
                ))));
            }
            for sink in s.iter() {
                let marker = if sink.default { "◉" } else { "○" };
                let vol_style = if sink.muted {
                    theme.warn()
                } else {
                    ratatui::style::Style::default().fg(theme.fg)
                };
                let bar = vol_bar(sink.volume);
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(
                        format!("{marker} "),
                        if sink.default {
                            ratatui::style::Style::default().fg(theme.accent)
                        } else {
                            ratatui::style::Style::default().fg(theme.dim)
                        },
                    ),
                    Span::styled(format!("{:<48}", truncate(&sink.description, 48)), theme.fg),
                    Span::styled(format!("{:<4}", sink.id.to_string()), theme.dim()),
                    Span::styled(format!("  {bar}"), vol_style),
                    Span::styled(format!(" {:>3}%", sink.volume), vol_style),
                    if sink.muted {
                        Span::styled(" muted", theme.warn())
                    } else {
                        Span::raw("")
                    },
                ])));
            }
        }
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(40)])
            .split(inner);
        let _ = cols;

        // Footer hint.
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(" +/- ", theme.key()),
            Span::styled("vol  ", theme.dim()),
            Span::styled(" m ", theme.key()),
            Span::styled("mute  ", theme.dim()),
            Span::styled(" d ", theme.key()),
            Span::styled("set default", theme.dim()),
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

fn selected_sink(app: &App) -> Option<cyberdeck_core::audio::Sink> {
    // For the audio screen, the "selected" sink is whichever one is default.
    let s = app.live.sinks.try_read().ok()?;
    s.iter()
        .find(|x| x.default)
        .cloned()
        .or_else(|| s.first().cloned())
}

fn vol_bar(pct: u8) -> String {
    let filled = ((pct as usize) / 5).min(20);
    let empty = 20 - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}
