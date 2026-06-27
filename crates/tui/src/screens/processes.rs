//! Processes screen: ps table with sort and inline kill.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
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
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                app.proc_selected = app.proc_selected.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.proc_selected = app.proc_selected.saturating_sub(1)
            }
            KeyCode::Char('c') => app.proc_sort = crate::app::ProcessSort::Cpu,
            KeyCode::Char('m') => app.proc_sort = crate::app::ProcessSort::Mem,
            KeyCode::Char('p') => app.proc_sort = crate::app::ProcessSort::Pid,
            KeyCode::Char('t') => app.proc_sort = crate::app::ProcessSort::Time,
            KeyCode::Char('K') => {
                // Quick kill of the highlighted row.
                if let Some(p) = selected_proc(app) {
                    app.modal = Modal::Confirm {
                        message: format!("Kill pid {} ({}) with SIGTERM?", p.pid, p.command),
                        kind: ConfirmKind::Kill,
                        arg: p.pid.to_string(),
                    };
                }
            }
            KeyCode::Char('x') => {
                // Kill with explicit pid entry.
                app.modal = Modal::Input {
                    prompt: "Kill PID:".into(),
                    buf: String::new(),
                    kind: InputKind::KillPid,
                };
            }
            _ => return false,
        }
        true
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

        let mut items: Vec<ListItem> = Vec::new();
        items.push(ListItem::new(Line::from(Span::styled(
            format!(
                "  {:>7} {:<10} {:>5} {:>5} {:<8} {}",
                "PID", "USER", "CPU%", "MEM%", "STAT", "COMMAND"
            ),
            theme.title(),
        ))));
        if let Ok(p) = app.live.processes.try_read() {
            for (i, proc) in p.iter().enumerate() {
                if i == app.proc_selected {
                    let row = format!(
                        "  {:>7} {:<10} {:>5} {:>5} {:<8} {}",
                        proc.pid,
                        truncate(&proc.user, 10),
                        format!("{:.1}", proc.cpu),
                        format!("{:.1}", proc.mem),
                        proc.stat,
                        proc.command
                    );
                    items.push(ListItem::new(Line::from(Span::styled(
                        row,
                        ratatui::style::Style::default()
                            .fg(theme.selection_fg)
                            .bg(theme.selection_bg),
                    ))));
                } else {
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
        }
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        let hints = Paragraph::new(Line::from(vec![
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
