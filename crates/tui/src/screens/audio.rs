//! Audio screen: sinks list with inline volume control.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
        let total = app.live.sinks.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.audio_selected = (app.audio_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.audio_selected = app.audio_selected.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total > 0 {
                    app.audio_selected = (app.audio_selected + 5).min(total - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.audio_selected = app.audio_selected.saturating_sub(5);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.audio_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total > 0 {
                    app.audio_selected = total - 1;
                }
                return true;
            }
            _ => {}
        }
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

        // Reserve bottom row for hints.
        let list_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let total = app.live.sinks.try_read().map(|v| v.len()).unwrap_or(0);
        if total == 0 {
            app.audio_selected = 0;
        } else if app.audio_selected >= total {
            app.audio_selected = total - 1;
        }

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
        let visible_h = list_area.height as usize;
        let offset = compute_offset(app.audio_selected, items.len(), visible_h);
        let mut state = ListState::default().with_selected(if total > 0 {
            Some(app.audio_selected)
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

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(inner);
        let _ = cols;

        // Footer hint.
        let pos = if total == 0 {
            "  no sinks".to_string()
        } else {
            format!(
                "  {}/{}  ",
                app.audio_selected + 1,
                total
            )
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(" j/k ", theme.key()),
            Span::styled("scroll  ", theme.dim()),
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

fn selected_sink(app: &App) -> Option<cyberdeck_core::audio::Sink> {
    let s = app.live.sinks.try_read().ok()?;
    let n = s.len();
    if n == 0 {
        return None;
    }
    let idx = app.audio_selected.min(n - 1);
    s.get(idx).cloned()
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