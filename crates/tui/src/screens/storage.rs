//! Storage screen: df + lsblk summary.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

pub struct StorageScreen;

impl Screen for StorageScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Storage
    }
    fn title(&self) -> &'static str {
        "Storage"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        // Each filesystem emits two rows (data + bar), so the underlying
        // item count we want to navigate is the number of filesystems.
        let total = app.live.filesystems.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.storage_selected = (app.storage_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.storage_selected = app.storage_selected.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total > 0 {
                    app.storage_selected = (app.storage_selected + 5).min(total - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.storage_selected = app.storage_selected.saturating_sub(5);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.storage_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total > 0 {
                    app.storage_selected = total - 1;
                }
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Storage ", theme.title()))
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

        let total = app.live.filesystems.try_read().map(|v| v.len()).unwrap_or(0);
        if total == 0 {
            app.storage_selected = 0;
        } else if app.storage_selected >= total {
            app.storage_selected = total - 1;
        }

        let mut items: Vec<ListItem> = Vec::new();
        items.push(ListItem::new(Line::from(Span::styled(
            format!(
                "  {:<24} {:<8} {:<6} {:<6} {:<6} {:<4}  {}",
                "source", "fstype", "size", "used", "avail", "use%", "mount"
            ),
            theme.title(),
        ))));
        // Track which rows belong to the selected filesystem so we can
        // highlight both the data row and its usage bar together.
        let mut selected_rows: Vec<usize> = Vec::new();
        if let Ok(fs) = app.live.filesystems.try_read() {
            for (i, m) in fs.iter().enumerate() {
                let style = if m.use_pct > 90 {
                    theme.error()
                } else if m.use_pct > 75 {
                    theme.warn()
                } else {
                    ratatui::style::Style::default().fg(theme.fg)
                };
                let row_idx = items.len();
                if i == app.storage_selected {
                    selected_rows.push(row_idx);
                }
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(format!("{:<24}", m.source), theme.fg),
                    Span::styled(format!("{:<8}", m.fstype), theme.dim()),
                    Span::styled(format!("{:<6}", m.size), theme.fg),
                    Span::styled(format!("{:<6}", m.used), theme.fg),
                    Span::styled(format!("{:<6}", m.avail), theme.fg),
                    Span::styled(format!("{:<4}", format!("{}%", m.use_pct)), style),
                    Span::styled(format!("  {}", m.mounted_on), theme.accent),
                ])));
                let bar = usage_bar(m.use_pct);
                let bar_idx = items.len();
                if i == app.storage_selected {
                    selected_rows.push(bar_idx);
                }
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(format!("  {bar}"), style),
                ])));
            }
        }
        if items.len() == 1 {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (no filesystems reported)",
                theme.dim(),
            ))));
        }
        let visible_h = list_area.height as usize;
        // Highlight lands on the data row (the first of the pair). We
        // scroll so that data row is visible.
        let highlight_target = selected_rows.first().copied().unwrap_or(0);
        let offset = compute_offset(highlight_target, items.len(), visible_h);
        // Selection in ListState indexes into `items`. We highlight the
        // data row (first of the pair); the bar is just visually adjacent.
        let sel = if total > 0 { Some(highlight_target) } else { None };
        let mut state = ListState::default().with_selected(sel);
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

        // Footer hint.
        let pos = if total == 0 {
            "  no filesystems".to_string()
        } else {
            format!(
                "  {}/{}  ",
                app.storage_selected + 1,
                total
            )
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(" j/k ", theme.key()),
            Span::styled("scroll  ", theme.dim()),
            Span::styled(" PgUp/PgDn ", theme.key()),
            Span::styled("page  ", theme.dim()),
            Span::styled(" g/G ", theme.key()),
            Span::styled("top/bot", theme.dim()),
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

fn usage_bar(pct: u8) -> String {
    let filled = (pct as usize) / 5; // 0..=20
    let empty = 20 - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}