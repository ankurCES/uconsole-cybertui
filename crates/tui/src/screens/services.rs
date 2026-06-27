//! Services screen: list systemd units and act on them with single keys.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

pub struct ServicesScreen;

impl Screen for ServicesScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Services
    }
    fn title(&self) -> &'static str {
        "Services"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        if let Some(unit) = selected_unit(app) {
            let act = match key.code {
                KeyCode::Char('s') => Some(RunAction::ServiceStart(unit.clone())),
                KeyCode::Char('S') => Some(RunAction::ServiceStop(unit.clone())),
                KeyCode::Char('R') => Some(RunAction::ServiceRestart(unit.clone())),
                KeyCode::Char('e') => Some(RunAction::ServiceEnable(unit.clone())),
                KeyCode::Char('E') => Some(RunAction::ServiceDisable(unit.clone())),
                _ => None,
            };
            if let Some(a) = act {
                let _ = app.tx.try_send(Action::Run(a));
                return true;
            }
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                app.svc_selected = app.svc_selected.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.svc_selected = app.svc_selected.saturating_sub(1)
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Services ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut items: Vec<ListItem> = Vec::new();
        let services = app.live.services.clone();
        if let Ok(sv) = services.try_read() {
            let max = inner.height.saturating_sub(1) as usize;
            let start = app.svc_selected.saturating_sub(max / 2);
            let end = (start + max).min(sv.len());
            for (i, s) in sv.iter().enumerate().take(end).skip(start) {
                let selected = i == app.svc_selected;
                let active_color = match s.active.as_str() {
                    "active" => theme.ok(),
                    "failed" => theme.error(),
                    "inactive" => theme.dim(),
                    _ => theme.warn(),
                };
                let line = Line::from(vec![
                    Span::styled(
                        if selected { "▸ " } else { "  " },
                        if selected { theme.title() } else { theme.dim() },
                    ),
                    Span::styled(
                        format!("{:<36}", s.unit),
                        if selected { theme.fg } else { theme.fg },
                    ),
                    Span::styled(format!("{:<10}", s.active), active_color),
                    Span::styled(format!("{:<10}", s.sub), theme.dim()),
                    Span::styled(truncate(&s.description, 50), theme.dim()),
                ]);
                items.push(ListItem::new(line));
            }
        }
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        // Hints footer
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(" s ", theme.key()),
            Span::styled("start  ", theme.dim()),
            Span::styled(" S ", theme.key()),
            Span::styled("stop  ", theme.dim()),
            Span::styled(" R ", theme.key()),
            Span::styled("restart  ", theme.dim()),
            Span::styled(" e ", theme.key()),
            Span::styled("enable  ", theme.dim()),
            Span::styled(" E ", theme.key()),
            Span::styled("disable", theme.dim()),
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

fn selected_unit(app: &App) -> Option<String> {
    let sv = app.live.services.try_read().ok()?;
    sv.get(app.svc_selected).map(|s| s.unit.clone())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

// Suppress the unused warning on `Borders` import in case the macros change.
#[allow(dead_code)]
fn _b(_: Borders) {}
