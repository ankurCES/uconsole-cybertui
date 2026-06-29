//! Display screen: outputs + brightness slider.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::{App, Region};
use crate::theme::Theme;

pub struct DisplayScreen;

impl Screen for DisplayScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Display
    }
    fn title(&self) -> &'static str {
        "Display"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        let total = app.live.displays.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.display_selected = (app.display_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.display_selected = app.display_selected.saturating_sub(1);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.display_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total > 0 {
                    app.display_selected = total - 1;
                }
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total > 0 {
                    app.display_selected = (app.display_selected + 5).min(total - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.display_selected = app.display_selected.saturating_sub(5);
                return true;
            }
            KeyCode::Left => {
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    if let Ok(cur) = cyberdeck_core::display::brightness().await {
                        let next = cur.saturating_sub(5);
                        match cyberdeck_core::display::set_brightness(next).await {
                            Ok(_) => {
                                let _ = tx
                                    .send(Action::Toast(
                                        ToastKind::Info,
                                        format!("brightness {next}%"),
                                    ))
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Action::Toast(ToastKind::Error, format!("{e}")))
                                    .await;
                            }
                        }
                    }
                });
                return true;
            }
            KeyCode::Right => {
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    if let Ok(cur) = cyberdeck_core::display::brightness().await {
                        let next = (cur + 5).min(100);
                        let _ = tx.send(Action::Run(RunAction::SetBrightness(next))).await;
                    }
                });
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Display ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Reserve bottom row for hints.
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

        // Left: outputs
        let total = app.live.displays.try_read().map(|v| v.len()).unwrap_or(0);
        if total == 0 {
            app.display_selected = 0;
        } else if app.display_selected >= total {
            app.display_selected = total - 1;
        }

        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(d) = app.live.displays.try_read() {
            if d.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "  (no outputs — install wlr-randr or xrandr)",
                    theme.dim(),
                ))));
            }
            for o in d.iter() {
                let enabled = if o.enabled { theme.ok() } else { theme.dim() };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<12}", o.name), theme.fg),
                    Span::styled(
                        format!("{:<6}", if o.enabled { "on" } else { "off" }),
                        enabled,
                    ),
                    Span::styled(format!("{:<14}", o.mode), theme.accent),
                    Span::styled(format!("scale {:.2}", o.scale), theme.dim()),
                ])));
            }
        }
        let left_h = cols[0].height as usize;
        let offset = compute_offset(app.display_selected, items.len(), left_h);
        let mut state = ListState::default().with_selected(if total > 0 {
            Some(app.display_selected)
        } else {
            None
        });
        *state.offset_mut() = offset;
        let left_focused = matches!(app.region, Region::ContentLeft);
        let left = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" outputs ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(left_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(left, cols[0], &mut state);

        // Right: brightness
        let tx = app.tx.clone();
        let mut lines: Vec<Line> = Vec::new();
        // Read the current brightness synchronously via the shared runtime.
        let cur = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(cyberdeck_core::display::brightness())
        });
        match cur {
            Ok(c) => {
                let bar = brightness_bar(c);
                lines.push(Line::from(Span::styled(
                    format!("  {bar}  {c:>3}%"),
                    theme.accent,
                )));
                lines.push(Line::from(Span::styled(
                    "  ← / → to dim / brighten (±5%)",
                    theme.dim(),
                )));
            }
            Err(e) => {
                lines.push(Line::from(Span::styled(
                    format!("  brightness unavailable: {e}"),
                    theme.warn(),
                )));
            }
        }
        let _ = tx;
        let right_focused = matches!(app.region, Region::ContentRight);
        let right = Paragraph::new(lines).block(
            Block::default()
                .title(Span::styled(" brightness ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(right_focused)),
        );
        f.render_widget(right, cols[1]);

        // Footer: position + hints.
        let pos = if total == 0 {
            "  no outputs".to_string()
        } else {
            format!(
                "  {}/{}  ",
                app.display_selected + 1,
                total
            )
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(" j/k ", theme.key()),
            Span::styled("scroll  ", theme.dim()),
            Span::styled(" ←/→ ", theme.key()),
            Span::styled("brightness ±5%", theme.dim()),
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

fn brightness_bar(pct: u8) -> String {
    let filled = (pct as usize) / 5;
    let empty = 20 - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

// Suppress unused warning if Borders import ever becomes truly unused.
#[allow(dead_code)]
fn _b(_: Borders) {}