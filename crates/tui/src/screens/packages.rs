//! Packages screen: upgradable + search + install/remove.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::{App, ConfirmKind, Modal};
use crate::theme::Theme;

pub struct PackagesScreen;

impl Screen for PackagesScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Packages
    }
    fn title(&self) -> &'static str {
        "Packages"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('u') => {
                let _ = app.tx.try_send(Action::Run(RunAction::PackageUpdate));
            }
            KeyCode::Char('U') => {
                let _ = app.tx.try_send(Action::Run(RunAction::PackageUpgrade));
            }
            KeyCode::Char('i') => {
                if let Some(p) = selected_upgradable(app) {
                    let _ = app.tx.try_send(Action::Run(RunAction::PackageInstall(p)));
                }
            }
            KeyCode::Char('r') => {
                if let Some(p) = selected_upgradable(app) {
                    app.modal = Modal::Confirm {
                        message: format!("Remove package `{p}`?"),
                        kind: ConfirmKind::Remove,
                        arg: p,
                    };
                }
            }
            KeyCode::Char('s') => {
                let tx = app.tx.clone();
                let q = app.pkgs_filter.clone();
                tokio::spawn(async move {
                    match cyberdeck_core::packages::search(&q).await {
                        Ok(v) => {
                            let _ = tx
                                .send(Action::Toast(
                                    ToastKind::Info,
                                    format!("{} matches for `{}`", v.len(), q),
                                ))
                                .await;
                        }
                        Err(e) => {
                            let _ = tx
                                .send(Action::Toast(ToastKind::Error, format!("{e}")))
                                .await;
                        }
                    }
                });
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Packages ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(inner);

        // Left: upgradable
        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(u) = app.live.upgradable.try_read() {
            for p in u.iter().take(inner.height as usize) {
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ▲ ", theme.warn()),
                    Span::styled(p.name.clone(), theme.fg),
                ])));
            }
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (system up to date — press u to refresh)",
                theme.dim(),
            ))));
        }
        let left = List::new(items).block(
            Block::default()
                .title(Span::styled(" upgradable ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(left, cols[0]);

        // Right: search box + actions
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("  search: ", theme.dim()),
            Span::styled(format!("{}_", app.pkgs_filter), theme.accent),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  actions", theme.title())));
        lines.push(Line::from(vec![
            Span::styled("  u ", theme.key()),
            Span::styled("apt update       ", theme.dim()),
            Span::styled(" U ", theme.key()),
            Span::styled("apt upgrade", theme.dim()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  i ", theme.key()),
            Span::styled("install row      ", theme.dim()),
            Span::styled(" r ", theme.key()),
            Span::styled("remove row", theme.dim()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  s ", theme.key()),
            Span::styled("search           ", theme.dim()),
            Span::styled(" / ", theme.key()),
            Span::styled("type to filter", theme.dim()),
        ]));
        let right = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(right, cols[1]);
    }
}

fn selected_upgradable(app: &App) -> Option<String> {
    // Selection on packages screen is implicit = the first upgradable (no
    // list navigation yet — keep it simple until the Settings→Bindings UX
    // is built).
    let u = app.live.upgradable.try_read().ok()?;
    u.first().map(|p| p.name.clone())
}
