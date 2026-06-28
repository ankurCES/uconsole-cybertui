//! Processes screen: ps table with sort and inline kill.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::{App, ConfirmKind, InputKind, Modal};
use crate::theme::Theme;

pub struct ProcessesScreen;

impl Screen for ProcessesScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Processes
    }
    fn title(&self) -> &'static str {
        "Processes"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        let total = app.live.processes.try_read().map(|v| v.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if total > 0 {
                    app.proc_selected = (app.proc_selected + 1).min(total - 1);
                }
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.proc_selected = app.proc_selected.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if total > 0 {
                    let step = 10usize;
                    app.proc_selected = (app.proc_selected + step).min(total - 1);
                }
                return true;
            }
            KeyCode::PageUp => {
                app.proc_selected = app.proc_selected.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.proc_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if total > 0 {
                    app.proc_selected = total - 1;
                }
                return true;
            }
            _ => {}
        }
        match key.code {
            KeyCode::Char('c') => {
                app.proc_sort = crate::app::ProcessSort::Cpu;
                return true;
            }
            KeyCode::Char('m') => {
                app.proc_sort = crate::app::ProcessSort::Mem;
                return true;
            }
            KeyCode::Char('p') => {
                app.proc_sort = crate::app::ProcessSort::Pid;
                return true;
            }
            KeyCode::Char('t') => {
                app.proc_sort = crate::app::ProcessSort::Time;
                return true;
            }
            KeyCode::Char('K') => {
                // Quick kill of the highlighted row.
                if let Some(p) = selected_proc(app) {
                    app.modal = Modal::Confirm {
                        message: format!("Kill pid {} ({}) with SIGTERM?", p.pid, p.command),
                        kind: ConfirmKind::Kill,
                        arg: p.pid.to_string(),
                    };
                }
                return true;
            }
            KeyCode::Char('x') => {
                // Kill with explicit pid entry.
                app.modal = Modal::Input {
                    prompt: "Kill PID:".into(),
                    buf: String::new(),
                    kind: InputKind::KillPid,
                };
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(
                format!(
                    " Processes (sort: {}) ",
                    match app.proc_sort {
                        crate::app::ProcessSort::Cpu => "cpu",
                        crate::app::ProcessSort::Mem => "mem",
                        crate::app::ProcessSort::Pid => "pid",
                        crate::app::ProcessSort::Time => "time",
                    }
                ),
                theme.title(),
            ))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Reserve header row + footer hint row.
        let list_area = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(2),
        );

        let total = app.live.processes.try_read().map(|v| v.len()).unwrap_or(0);
        if total == 0 {
            app.proc_selected = 0;
        } else if app.proc_selected >= total {
            app.proc_selected = total - 1;
        }

        let mut items: Vec<ListItem> = Vec::new();
        // Header row (kept at the top, fixed).
        items.push(ListItem::new(Line::from(Span::styled(
            format!(
                "  {:>7} {:<10} {:>5} {:>5} {:<8} {}",
                "PID", "USER", "CPU%", "MEM%", "STAT", "COMMAND"
            ),
            theme.title(),
        ))));
        if let Ok(p) = app.live.processes.try_read() {
            for proc in p.iter() {
                let row = format!(
                    "  {:>7} {:<10} {:>5} {:>5} {:<8} {}",
                    proc.pid,
                    truncate(&proc.user, 10),
                    format!("{:.1}", proc.cpu),
                    format!("{:.1}", proc.mem),
                    proc.stat,
                    proc.command
                );
                items.push(ListItem::new(Line::from(Span::styled(row, theme.fg))));
            }
        }
        // `selection` in ListState is the index into `items`. We want the
        // highlighted process row, which sits 1 below the header.
        let sel_in_items = if total == 0 {
            None
        } else {
            Some(app.proc_selected + 1)
        };
        let visible_h = list_area.height as usize;
        let offset = compute_offset_list(sel_in_items.unwrap_or(0), items.len(), visible_h);
        let mut state = ListState::default().with_selected(sel_in_items);
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

        // Header line sits above the list (so it stays visible while we
        // scroll the table body).
        let header = Paragraph::new(Line::from(Span::styled(
            format!(
                "  {:>7} {:<10} {:>5} {:>5} {:<8} {}",
                "PID", "USER", "CPU%", "MEM%", "STAT", "COMMAND"
            ),
            theme.title(),
        )));
        let header_area = Rect::new(inner.x, inner.y, inner.width, 1);
        f.render_widget(header, header_area);

        // Footer: position indicator + hints.
        let indicator = if total == 0 {
            "  no processes".to_string()
        } else {
            format!(
                "  {}/{}  (j/k nav, PgUp/PgDn page, g/G top/bottom)",
                app.proc_selected + 1,
                total
            )
        };
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(indicator, theme.dim()),
            Span::raw("  "),
            Span::styled(" c ", theme.key()),
            Span::styled("cpu  ", theme.dim()),
            Span::styled(" m ", theme.key()),
            Span::styled("mem  ", theme.dim()),
            Span::styled(" p ", theme.key()),
            Span::styled("pid  ", theme.dim()),
            Span::styled(" t ", theme.key()),
            Span::styled("time  ", theme.dim()),
            Span::styled(" K ", theme.key()),
            Span::styled("kill row  ", theme.dim()),
            Span::styled(" x ", theme.key()),
            Span::styled("kill by pid", theme.dim()),
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
fn compute_offset_list(selected: usize, total: usize, visible: usize) -> usize {
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

fn selected_proc(app: &App) -> Option<cyberdeck_core::process::Process> {
    let p = app.live.processes.try_read().ok()?;
    p.get(app.proc_selected).cloned()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

// Keep the Borders import alive in case layout changes.
#[allow(dead_code)]
fn _b(_: Borders) {}