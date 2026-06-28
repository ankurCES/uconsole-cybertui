//! Packages screen: upgradable + search + install/remove.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
        let total_up = app.live.upgradable.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total_up > 0 {
                    app.pkg_selected = (app.pkg_selected + 1).min(total_up - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.pkg_selected = app.pkg_selected.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total_up > 0 {
                    app.pkg_selected = (app.pkg_selected + 10).min(total_up - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.pkg_selected = app.pkg_selected.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.pkg_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total_up > 0 {
                    app.pkg_selected = total_up - 1;
                }
                return true;
            }
            _ => {}
        }
        match key.code {
            KeyCode::Char('u') => {
                let _ = app.tx.try_send(Action::Run(RunAction::PackageUpdate));
                return true;
            }
            KeyCode::Char('U') => {
                let _ = app.tx.try_send(Action::Run(RunAction::PackageUpgrade));
                return true;
            }
            KeyCode::Char('i') => {
                if let Some(p) = selected_upgradable(app) {
                    let _ = app.tx.try_send(Action::Run(RunAction::PackageInstall(p)));
                }
                return true;
            }
            KeyCode::Char('r') => {
                if let Some(p) = selected_upgradable(app) {
                    app.modal = Modal::Confirm {
                        message: format!("Remove package `{p}`?"),
                        kind: ConfirmKind::Remove,
                        arg: p,
                    };
                }
                return true;
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
                return true;
            }
            KeyCode::Char('/') => {
                // Toggle focus to the filter (typing would normally edit the
                // field; for now we just clear it so the user can re-type).
                app.pkgs_filter.clear();
                return true;
            }
            _ => return false,
        }
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

        // Left: upgradable — now a real scrollable list.
        let mut items: Vec<ListItem> = Vec::new();
        if let Ok(u) = app.live.upgradable.try_read() {
            for p in u.iter() {
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("▲ ", theme.warn()),
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
        let total = items.len();
        if total == 0 || app.pkg_selected >= total {
            app.pkg_selected = 0;
        }
        let left_h = cols[0].height.saturating_sub(1) as usize; // border
        let offset = compute_offset(app.pkg_selected, total, left_h);
        let mut state = ListState::default().with_selected(if total > 0 {
            Some(app.pkg_selected)
        } else {
            None
        });
        *state.offset_mut() = offset;
        let left = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" upgradable ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(false)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(left, cols[0], &mut state);

        // Right: search box + actions
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("  search: ", theme.dim()),
            Span::styled(format!("{}_", app.pkgs_filter), theme.accent),
        ]));
        lines.push(Line::from(""));
        if !app.pkg_search_results.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    "  matches: {}  ({}/{})",
                    app.pkg_search_results.len(),
                    app.pkg_search_offset + 1,
                    app.pkg_search_results.len()
                ),
                theme.accent,
            )));
        }
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
            Span::styled("clear filter", theme.dim()),
        ]));
        lines.push(Line::from(Span::styled(
            "  j/k scroll upgradable list",
            theme.dim(),
        )));
        let right = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(right, cols[1]);
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

fn selected_upgradable(app: &App) -> Option<String> {
    let u = app.live.upgradable.try_read().ok()?;
    if u.is_empty() {
        return None;
    }
    let idx = app.pkg_selected.min(u.len() - 1);
    u.get(idx).map(|p| p.name.clone())
}