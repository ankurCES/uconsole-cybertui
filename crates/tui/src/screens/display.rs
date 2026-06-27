//! Display screen: outputs + brightness slider.

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

pub struct DisplayScreen;

impl Screen for DisplayScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Display
    }
    fn title(&self) -> &'static str {
        "Display"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
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
            }
            KeyCode::Right => {
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    if let Ok(cur) = cyberdeck_core::display::brightness().await {
                        let next = (cur + 5).min(100);
                        let _ = tx.send(Action::Run(RunAction::SetBrightness(next))).await;
                    }
                });
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Display ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(inner);

        // Left: outputs
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
                    Span::styled("  ", theme.dim()),
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
        let left = List::new(items).block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(theme.border(false)),
        );
        f.render_widget(left, cols[0]);

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
        let right = Paragraph::new(lines).block(
            Block::default()
                .title(Span::styled(" brightness ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(right, cols[1]);
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
