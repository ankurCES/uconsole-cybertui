//! Packages screen: upgradable + search + install/remove.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::{App, ConfirmKind, InputKind, Modal, Region};
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
                // Open the search input modal so the user can type a query.
                // Submit is handled by `run_input` in `main.rs` (Task 3.1):
                // trimmed query is stashed on `app.packages_search_query` and
                // the modal is closed. Pre-fix, this arm spawned an
                // empty-string `cyberdeck_core::packages::search("")` task,
                // which silently produced a `"0 matches for ``"` toast and
                // never showed a modal.
                app.modal = Modal::Input {
                    prompt: "search packages".into(),
                    buf: String::new(),
                    kind: InputKind::PackageSearch,
                };
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
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
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
        let left_focused = matches!(app.region, Region::ContentLeft);
        let left = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(" upgradable ", theme.title()))
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
        let right_focused = matches!(app.region, Region::ContentRight);
        let right = Paragraph::new(lines).block(
            Block::default()
                .title(Span::styled(" actions ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(right_focused)),
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

#[cfg(test)]
mod tests {
    //! Tests for the Packages screen's key dispatcher.
    //!
    //! Module 3.1 introduced `InputKind::PackageSearch` and the submit arm in
    //! `run_input` that stashes a trimmed query on `app.packages_search_query`.
    //! Module 3.3 wires the user-facing affordance: pressing `s` on the
    //! Packages screen must open `Modal::Input(InputKind::PackageSearch, "")`
    //! so the user can type their query — instead of firing an empty-string
    //! search directly (the pre-fix behaviour, which produced a
    //! `"0 matches for ``"` toast and never showed a modal).

    use super::*;
    use crate::app::InputKind;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc;

    fn make_app() -> App {
        let (tx, rx) = mpsc::channel::<crate::app::action::Action>(8);
        App::new(tx, rx)
    }

    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Pressing `s` on the Packages screen must open the search input modal
    /// with an empty buffer — not fire an empty-string search against
    /// `pkgs_filter`. This is the user-facing half of the Module 3 fix
    /// (Module 3.1 added the submit arm; Module 3.3 wires the open).
    #[test]
    fn packages_screen_s_key_opens_search_input_modal() {
        let mut app = make_app();
        app.current = ScreenId::Packages;
        // Sanity: no modal at rest.
        assert!(matches!(app.modal, Modal::None));
        // Pre-fill the live filter so we can also assert the modal doesn't
        // copy it into its buffer (the modal opens empty; the user types
        // from scratch).
        app.pkgs_filter = "stale-filter".into();

        let mut screen = PackagesScreen;
        let consumed = screen.on_key(kc('s'), &mut app);

        assert!(consumed, "`s` must be consumed on the Packages screen");
        match &app.modal {
            Modal::Input { prompt, buf, kind } => {
                assert_eq!(*kind, InputKind::PackageSearch, "modal kind");
                assert!(buf.is_empty(), "modal buffer starts empty: got {buf:?}");
                // Prompt should mention searching so the user knows what
                // they're typing into. Pin a non-empty prompt — the exact
                // string is renderer copy and may evolve.
                assert!(!prompt.is_empty(), "modal prompt must not be empty");
            }
            other => panic!("expected Modal::Input(PackageSearch, \"\"), got {other:?}"),
        }

        // The previous broken behaviour spawned a tokio task that called
        // `cyberdeck_core::packages::search("")`. No Action::Run(...) was
        // ever queued, so we don't need to assert on the channel — the
        // modal assertion above is sufficient evidence that the new code
        // path is in effect.
    }

    /// Pressing `s` while a different modal is open should still flip the
    /// modal to the PackageSearch input. (Pre-existing modals get clobbered
    /// by this — same as the pre-fix `s` arm didn't check either, so this
    /// is a "no regression beyond prior behaviour" pin rather than a
    /// redesign.)
    #[test]
    fn packages_screen_s_key_overwrites_existing_help_modal() {
        let mut app = make_app();
        app.current = ScreenId::Packages;
        app.modal = Modal::Help;

        let mut screen = PackagesScreen;
        assert!(screen.on_key(kc('s'), &mut app));

        assert!(
            matches!(&app.modal, Modal::Input { kind: InputKind::PackageSearch, .. }),
            "Help modal must be replaced by PackageSearch input, got {:?}",
            app.modal
        );
    }
}