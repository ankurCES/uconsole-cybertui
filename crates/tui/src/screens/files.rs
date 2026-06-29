//! Files screen: two-pane browser (cwd on the left, selected dir on the right).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::cyberdeck_core_files::DirEntry;
use crate::app::screen::{Screen, ScreenId};
use crate::app::{App, Region};
use crate::theme::Theme;
use std::path::PathBuf;

pub struct FilesScreen;

impl Screen for FilesScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Files
    }
    fn title(&self) -> &'static str {
        "Files"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !app.files_entries.is_empty() {
                    app.files_selected =
                        (app.files_selected + 1).min(app.files_entries.len() - 1);
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.files_selected = app.files_selected.saturating_sub(1);
                true
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if !app.files_entries.is_empty() {
                    app.files_selected = (app.files_selected + 10)
                        .min(app.files_entries.len() - 1);
                }
                true
            }
            KeyCode::PageUp => {
                app.files_selected = app.files_selected.saturating_sub(10);
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.files_selected = 0;
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !app.files_entries.is_empty() {
                    app.files_selected = app.files_entries.len() - 1;
                }
                true
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(parent) = app.files_cwd.parent() {
                    app.files_cwd = parent.to_path_buf();
                    app.files_selected = 0;
                    refresh(app);
                }
                true
            }
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if let Some(entry) = app.files_entries.get(app.files_selected).cloned() {
                    if entry.is_dir {
                        app.files_cwd = entry.path.clone();
                        app.files_selected = 0;
                        refresh(app);
                    } else {
                        app.files_right = entry.path.clone();
                        refresh_right(app);
                    }
                }
                true
            }
            // Module 4 — open the in-TUI editor on `app.files_right`.
            // The right pane is where the user "locks in" a file (Enter
            // on a non-directory drops its path there and refreshes the
            // right listing), so `e` reads from there. We only open the
            // editor if the path is a real file — `is_file()` is false
            // for missing paths, directories, and broken symlinks, all
            // of which would otherwise trigger a confusing read-only
            // fallback. Returns `true` so the key is consumed.
            KeyCode::Char('e') => {
                if app.files_right.is_file() {
                    App::enter_editor(app, app.files_right.clone());
                } else {
                    app.status_message =
                        Some("editor: not a regular file".to_string());
                }
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Files ", theme.title()))
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

        // Clamp selection to bounds.
        if app.files_entries.is_empty() {
            app.files_selected = 0;
        } else if app.files_selected >= app.files_entries.len() {
            app.files_selected = app.files_entries.len() - 1;
        }

        // Left: cwd listing
        let left_items: Vec<ListItem> = app
            .files_entries
            .iter()
            .enumerate()
            .map(|(_i, e)| {
                let marker = if e.is_dir { "▸" } else { " " };
                let line = Line::from(vec![
                    Span::styled(
                        format!("{marker} "),
                        if e.is_dir {
                            ratatui::style::Style::default().fg(theme.accent)
                        } else {
                            ratatui::style::Style::default().fg(theme.dim)
                        },
                    ),
                    Span::styled(format!("{:<32}", truncate(&e.name, 32)), theme.fg),
                    Span::styled(
                        if e.is_dir {
                            String::new()
                        } else {
                            format!("{:>10}", format_size(e.size))
                        },
                        theme.dim(),
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();
        let left_h = cols[0].height as usize;
        let left_total = left_items.len();
        let left_offset = compute_offset(app.files_selected, left_total, left_h);
        let mut left_state = ListState::default()
            .with_selected(if left_total > 0 {
                Some(app.files_selected)
            } else {
                None
            });
        *left_state.offset_mut() = left_offset;
        let left_focused = !matches!(app.region, Region::ContentRight);
        let left = List::new(left_items)
            .block(
                Block::default()
                    .title(Span::styled(
                        format!(" {} ", app.files_cwd.display()),
                        theme.title(),
                    ))
                    .borders(Borders::ALL)
                    .border_style(theme.border(left_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(left, cols[0], &mut left_state);

        // Right: selected dir contents (or "select a directory" hint)
        let right_items: Vec<ListItem> = if app.files_right_entries.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no selection — press space on a directory)",
                theme.dim(),
            )))]
        } else {
            app.files_right_entries
                .iter()
                .map(|e| {
                    let marker = if e.is_dir { "▸" } else { " " };
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{marker} "),
                            if e.is_dir {
                                ratatui::style::Style::default().fg(theme.accent)
                            } else {
                                ratatui::style::Style::default().fg(theme.dim)
                            },
                        ),
                        Span::styled(format!("{:<32}", truncate(&e.name, 32)), theme.fg),
                        Span::styled(
                            if e.is_dir {
                                String::new()
                            } else {
                                format!("{:>10}", format_size(e.size))
                            },
                            theme.dim(),
                        ),
                    ]))
                })
                .collect()
        };
        // The right pane is read-only (peek), so no selection row.
        let right_h = cols[1].height as usize;
        let right_total = right_items.len();
        // Surface the position of the right pane by clipping to the
        // bottom: if there are more entries than the visible window, show
        // the tail of the listing (it's a "peek", not a picker).
        let right_offset = if right_total > right_h {
            right_total - right_h
        } else {
            0
        };
        let mut right_state = ListState::default();
        *right_state.offset_mut() = right_offset;
        let right_focused = matches!(app.region, Region::ContentRight);
        let right = List::new(right_items)
            .block(
                Block::default()
                    .title(Span::styled(
                        format!(" {} ", app.files_right.display()),
                        theme.title(),
                    ))
                    .borders(Borders::ALL)
                    .border_style(theme.border(right_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            );
        f.render_stateful_widget(right, cols[1], &mut right_state);

        let pos = if left_total == 0 {
            "  (empty)".to_string()
        } else {
            format!(
                "  {}/{}  ",
                app.files_selected + 1,
                left_total
            )
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(pos, theme.dim()),
            Span::styled(" j/k ", theme.key()),
            Span::styled("nav  ", theme.dim()),
            Span::styled(" PgUp/PgDn ", theme.key()),
            Span::styled("page  ", theme.dim()),
            Span::styled(" g/G ", theme.key()),
            Span::styled("top/bot  ", theme.dim()),
            Span::styled(" h ", theme.key()),
            Span::styled("up  ", theme.dim()),
            Span::styled(" l/⏎ ", theme.key()),
            Span::styled("open  ", theme.dim()),
            Span::styled(" space ", theme.key()),
            Span::styled("peek", theme.dim()),
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

fn refresh(app: &mut App) {
    app.files_entries = read_dir(&app.files_cwd, app.files_show_hidden);
    if app.files_entries.is_empty() {
        app.files_selected = 0;
    } else if app.files_selected >= app.files_entries.len() {
        app.files_selected = app.files_entries.len() - 1;
    }
}

fn refresh_right(app: &mut App) {
    app.files_right_entries = read_dir(&app.files_right, app.files_show_hidden);
}

fn read_dir(p: &PathBuf, show_hidden: bool) -> Vec<DirEntry> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let meta = entry.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            v.push(DirEntry {
                name,
                path: entry.path(),
                is_dir,
                size,
            });
        }
    }
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    v
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

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1}{}", size, UNITS[unit])
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}