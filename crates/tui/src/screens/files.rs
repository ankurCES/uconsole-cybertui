//! Files screen: two-pane browser (cwd on the left, selected dir on the right).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::cyberdeck_core_files::DirEntry;
use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
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
                if app.files_selected + 1 < app.files_entries.len() {
                    app.files_selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.files_selected = app.files_selected.saturating_sub(1);
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(parent) = app.files_cwd.parent() {
                    app.files_cwd = parent.to_path_buf();
                    app.files_selected = 0;
                    refresh(app);
                }
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
            }
            KeyCode::Char(' ') => {
                // Move the selected entry into the right pane for inspection.
                if let Some(entry) = app.files_entries.get(app.files_selected).cloned() {
                    if entry.is_dir {
                        app.files_right = entry.path.clone();
                        refresh_right(app);
                    }
                }
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Files ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // Left: cwd listing
        let left_items: Vec<ListItem> = app
            .files_entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let selected = i == app.files_selected;
                let marker = if e.is_dir { "▸" } else { " " };
                let line = Line::from(vec![
                    Span::styled(
                        if selected { "▸ " } else { "  " },
                        if selected { theme.title() } else { theme.dim() },
                    ),
                    Span::styled(
                        format!("{marker} "),
                        if e.is_dir {
                            ratatui::style::Style::default().fg(theme.accent)
                        } else {
                            ratatui::style::Style::default().fg(theme.dim)
                        },
                    ),
                    Span::styled(
                        format!("{:<32}", truncate(&e.name, 32)),
                        if selected { theme.fg } else { theme.fg },
                    ),
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
        let left = List::new(left_items).block(
            Block::default()
                .title(Span::styled(
                    format!(" {} ", app.files_cwd.display()),
                    theme.title(),
                ))
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(left, cols[0]);

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
                        Span::styled("  ", theme.dim()),
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
        let right = List::new(right_items).block(
            Block::default()
                .title(Span::styled(
                    format!(" {} ", app.files_right.display()),
                    theme.title(),
                ))
                .borders(Borders::ALL)
                .border_style(theme.border(false)),
        );
        f.render_widget(right, cols[1]);

        let hints = Paragraph::new(Line::from(vec![
            Span::styled(" j/k ", theme.key()),
            Span::styled("nav  ", theme.dim()),
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
