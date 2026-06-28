//! Services screen: list systemd units and act on them with single keys.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
        // Always let j/k/Up/Down move the selection so the list is
        // navigable while the content pane is focused. The action keys
        // (s/S/R/e/E) operate on whatever is currently highlighted.
        let total = app.live.services.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.svc_selected = (app.svc_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.svc_selected = app.svc_selected.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total > 0 {
                    let step = 10usize;
                    app.svc_selected = (app.svc_selected + step).min(total - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.svc_selected = app.svc_selected.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.svc_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total > 0 {
                    app.svc_selected = total - 1;
                }
                return true;
            }
            _ => {}
        }
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
        false
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Services ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Reserve the bottom row for hints.
        let list_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let mut items: Vec<ListItem> = Vec::new();
        let total = app.live.services.try_read().map(|v| v.len()).unwrap_or(0);
        // Clamp selection in case the list shrank.
        if total == 0 {
            app.svc_selected = 0;
        } else if app.svc_selected >= total {
            app.svc_selected = total - 1;
        }
        if let Ok(sv) = app.live.services.try_read() {
            for (i, s) in sv.iter().enumerate() {
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
                    Span::styled(format!("{:<36}", truncate(&s.unit, 36)), theme.fg),
                    Span::styled(format!("{:<10}", s.active), active_color),
                    Span::styled(format!("{:<10}", s.sub), theme.dim()),
                    Span::styled(truncate(&s.description, 60), theme.dim()),
                ]);
                items.push(ListItem::new(line));
            }
        }
        let visible_h = list_area.height as usize;
        // Centre the cursor when possible so PgUp/PgDown land cleanly.
        let offset = compute_offset(app.svc_selected, items.len(), visible_h);
        let mut state = ListState::default().with_selected(Some(app.svc_selected));
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

        // Scroll indicator.
        let indicator = if total == 0 {
            "  no services".to_string()
        } else {
            format!(
                "  {}/{}  (j/k nav, PgUp/PgDn page, g/G top/bottom)",
                app.svc_selected + 1,
                total
            )
        };
        let footer = Line::from(vec![
            Span::styled(
                indicator,
                theme.dim(),
            ),
            Span::raw("  "),
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
        ]);
        let hint_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        f.render_widget(Paragraph::new(footer).style(theme.fg), hint_area);
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

fn selected_unit(app: &App) -> Option<String> {
    let sv = app.live.services.try_read().ok()?;
    sv.get(app.svc_selected).map(|s| s.unit.clone())
}

#[cfg(test)]
mod offset_tests {
    use super::compute_offset;

    // The list is shorter than the visible window: no scroll, offset = 0.
    #[test]
    fn short_list_no_scroll() {
        assert_eq!(compute_offset(0, 5, 20), 0);
        assert_eq!(compute_offset(4, 5, 20), 0);
    }

    // The cursor is still in the first page (selected < visible): no scroll.
    // This is the key behavioural difference from a centred offset: with a
    // centred offset, pressing Down from row 0 in a 100-item, 10-visible
    // list would jump the view to start at row 0 (because half = 5, so
    // selected.saturating_sub(5) = 0). With top-aligned, the view stays
    // pinned at 0 until the cursor passes row 9, then shifts to keep
    // the cursor at the bottom row. That means the view visually tracks
    // the cursor immediately instead of looking frozen at the top.
    #[test]
    fn first_page_no_scroll() {
        assert_eq!(compute_offset(0, 100, 10), 0);
        assert_eq!(compute_offset(5, 100, 10), 0);
        assert_eq!(compute_offset(9, 100, 10), 0);
    }

    // Once the cursor passes the bottom of the visible window, the view
    // shifts so the cursor sits at the bottom row of the visible window.
    #[test]
    fn scrolls_when_cursor_passes_bottom() {
        // visible=10, selected=10 → offset=1 (rows 1..=10 visible)
        assert_eq!(compute_offset(10, 100, 10), 1);
        assert_eq!(compute_offset(20, 100, 10), 11);
        assert_eq!(compute_offset(50, 100, 10), 41);
    }

    // At the bottom of the list: offset = total - visible.
    #[test]
    fn clamps_to_bottom() {
        assert_eq!(compute_offset(99, 100, 10), 90);
        assert_eq!(compute_offset(100, 100, 10), 90);
    }

    // A stale cursor (selected >= total) must not produce an out-of-range
    // offset that would panic the render path.
    #[test]
    fn stale_cursor_does_not_panic() {
        assert_eq!(compute_offset(200, 100, 10), 90);
    }

    // PgUp/PgDn symmetry: pressing Down by `visible` rows from row 0
    // lands you at row `visible`, which means offset becomes 1 and the
    // cursor is at the bottom row of the visible window. Pressing Up
    // by `visible` rows from there lands you at row 0, which means
    // offset becomes 0 again. So PgUp/PgDn feel symmetric without
    // needing a centred offset.
    #[test]
    fn pgup_pgdn_symmetric() {
        let v = 10;
        // Start at 0, press PgDn: cursor at 10, offset at 1.
        let down = compute_offset(10, 100, v);
        assert_eq!(down, 1);
        // From 10, press PgUp: cursor at 0, offset at 0.
        let up = compute_offset(0, 100, v);
        assert_eq!(up, 0);
    }

    // End-to-end render test: build a 50-item List, set offset=20 via
    // `*state.offset_mut()`, render to a TestBackend, and verify that
    // only items 20..=29 appear on screen. This proves the view actually
    // visually clips — not just that `compute_offset` returns the right
    // number. Without this, a regression where ratatui silently
    // overwrites our offset write (it does recompute it from `selected`
    // in 0.29) would go undetected.
    #[test]
    fn render_clips_to_offset() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use ratatui::widgets::{List, ListItem, ListState};

        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        let items: Vec<ListItem> = (0..50)
            .map(|i| ListItem::new(format!("item-{:02}", i)))
            .collect();
        let mut state = ListState::default().with_selected(Some(25));
        // Top-aligned offset for selected=25, total=50, visible=10:
        // sel - visible + 1 = 16.
        *state.offset_mut() = compute_offset(25, items.len(), 10);

        terminal
            .draw(|f| {
                let list = List::new(items.clone()).highlight_symbol("> ");
                f.render_stateful_widget(list, f.area(), &mut state);
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        // Collect the text content of each rendered row.
        let mut rows: Vec<String> = Vec::new();
        for y in 0..buffer.area.height {
            let mut row = String::new();
            for x in 0..buffer.area.width {
                row.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            rows.push(row);
        }
        // First visible row should contain item-16 (offset=16), not
        // item-00. If ratatui had ignored our offset write and centered
        // selected=25 (which would give offset=20, item-20), we'd see
        // item-20 — that's the centred behaviour the user reported as
        // "view doesn't scroll". We assert the top-aligned result.
        let first = rows.iter().find(|r| r.contains("item-")).cloned().unwrap_or_default();
        assert!(
            first.contains("item-16"),
            "first visible row should be item-16 (offset=16), got: {:?}",
            first
        );
        // Last visible row should be item-25 (the selected row, at the
        // bottom of the visible window).
        let last = rows.iter().rev().find(|r| r.contains("item-")).cloned().unwrap_or_default();
        assert!(
            last.contains("item-25"),
            "last visible row should be item-25 (selected), got: {:?}",
            last
        );
        // Items before the offset (item-00 through item-15) must NOT
        // appear — the view clipped them.
        for r in &rows {
            for i in 0..16 {
                let needle = format!("item-{:02}", i);
                assert!(
                    !r.contains(&needle),
                    "row {:?} contains {} (should be clipped)",
                    r,
                    needle
                );
            }
        }
    }
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