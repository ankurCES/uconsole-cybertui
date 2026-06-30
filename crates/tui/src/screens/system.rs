//! System screen: live values driven by the background refresh task.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::{App, Region};
use crate::theme::{glyphs, Theme};
use cyberdeck_core::sys;

pub struct SystemScreen;

impl Screen for SystemScreen {
    fn id(&self) -> ScreenId {
        ScreenId::System
    }
    fn title(&self) -> &'static str {
        "System Status"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                // Scroll the right-hand log pane down (away from the tail).
                app.system_log_offset = app.system_log_offset.saturating_add(1);
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.system_log_offset = app.system_log_offset.saturating_sub(1);
                return true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                app.system_log_offset = app.system_log_offset.saturating_add(10);
                return true;
            }
            KeyCode::PageUp => {
                app.system_log_offset = app.system_log_offset.saturating_sub(10);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                // g = jump to top of log (oldest).
                app.system_log_offset = usize::MAX;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                // G = jump back to live tail.
                app.system_log_offset = 0;
                return true;
            }
            // Module 2.4 — `r` requests an immediate 60s fetch for
            // the right-hand "recent log" pane. Same contract as the
            // Logs screen: the handler is a tiny enqueue
            // (`try_send` on `app.tx`), the actual journalctl call
            // lives in the dispatcher's `Action::RefreshLogs` arm.
            // The 1Hz refiller continues to feed live updates in
            // parallel via the same `LogPushed` pipeline.
            KeyCode::Char('r') => {
                let _ = app.tx.try_send(crate::app::action::Action::RefreshLogs);
                return true;
            }
            // Module 6.3 — `t` toggles the process-tree view. The flag is
            // on `App` (so other screens can read it without traversing a
            // screen-local) and the render branch in `render` decides
            // whether to draw the indented tree or the default facts
            // pane. The handler is purely a state flip — no Action is
            // enqueued — so the next frame picks up the new mode.
            KeyCode::Char('t') => {
                app.proc_tree_view = !app.proc_tree_view;
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" System ", theme.title()))
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

        // Two columns: facts left, recent log right.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(body_area);

        // Left pane: either the default facts view, or the process tree
        // (Module 6.3) when `proc_tree_view` is on. The right pane
        // (recent log) is unchanged between the two modes — switching
        // modes only swaps the left, so the user keeps their scroll
        // position in the log.
        if app.proc_tree_view {
            render_proc_tree_pane(f, cols[0], &app.proc_tree, theme, app.region == Region::ContentLeft);
        } else {
            render_facts_pane(f, cols[0], app, theme);
        }
        // Right: latest log lines. `system_log_offset` counts lines back
        // from the tail (0 == newest). Cap so we never scroll past the
        // oldest entry, even when the user holds PageDown.
        let right_area = cols[1];
        let visible_h = right_area.height as usize;
        let total = app.logs.len();
        let max_off = total.saturating_sub(visible_h);
        if app.system_log_offset > max_off {
            app.system_log_offset = max_off;
        }
        let end = total.saturating_sub(app.system_log_offset);
        let start = end.saturating_sub(visible_h);
        // We want newest at the bottom of the slice (most recent visible).
        let recent: Vec<ListItem> = if total == 0 {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no log lines yet)",
                theme.dim(),
            )))]
        } else {
            app.logs[start..end]
                .iter()
                .map(|l| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", l.ts.format("%H:%M:%S")), theme.dim()),
                        Span::styled(l.message.clone(), theme.fg),
                    ]))
                })
                .collect()
        };
        let highlight = if total == 0 {
            None
        } else {
            Some(recent.len().saturating_sub(1))
        };
        let mut state = ListState::default().with_selected(highlight);
        // Sub-focus border. When region is ContentRight the right pane
        // gets the brighter border so the user sees which column ↑/↓
        // will move in. Otherwise (Sidebar or ContentLeft) the left
        // pane — drawn further up — gets the focus border.
        let right_focused = matches!(app.region, Region::ContentRight);
        let right = List::new(recent)
            .block(
                Block::default()
                    .title(Span::styled(
                        format!(
                            " recent log ({}/{}) ",
                            end,
                            total
                        ),
                        theme.title(),
                    ))
                    .borders(Borders::ALL)
                    .border_style(theme.border(right_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(right, right_area, &mut state);

        let mode = if app.system_log_offset == 0 {
            "  ● live (j/k step, PgUp/PgDn page, G to live)"
        } else {
            "  ⏸ paused — press G to jump back to live tail"
        };
        // Module 6.3 — `t` toggles the process-tree view; show the
        // current mode so the user knows which pane they're in. The
        // hint stays compact (one row) so the right-hand log pane
        // doesn't lose a line.
        let tree_hint = if app.proc_tree_view {
            "  ▦ tree"
        } else {
            ""
        };
        let tree_key = Span::styled(" t ", theme.key());
        let tree_label = Span::styled("process tree  ", theme.dim());
        let mut hint_spans: Vec<Span> = vec![
            Span::styled(mode, theme.dim()),
            Span::raw("  "),
            Span::styled(" r ", theme.key()),
            // Module 2.4 — match the Logs screen's hint so the user
            // sees the same affordance everywhere `r` is bound.
            Span::styled("refresh (live)  ", theme.dim()),
            tree_key,
            tree_label,
        ];
        if !tree_hint.is_empty() {
            hint_spans.push(Span::styled(tree_hint, theme.ok()));
        }
        hint_spans.extend([
            Span::styled(" ? ", theme.key()),
            Span::styled("help", theme.dim()),
        ]);
        let hints = Paragraph::new(Line::from(hint_spans));
        let hint_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        f.render_widget(hints, hint_area);
    }
}

fn trim(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

// ---------------------------------------------------------------------------
// Module 6.3 — System-screen sub-renderers. Splitting the left pane into
// facts vs tree keeps `Screen::render` readable: the dispatcher just
// branches on `app.proc_tree_view` and the two halves are independently
// testable. The right pane (recent log) is unchanged.
// ---------------------------------------------------------------------------

/// Default left pane: live system facts (hostname, kernel, load, CPU, memory,
/// battery, thermals). Same content as the pre-Module-6 System screen.
fn render_facts_pane(f: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let g = glyphs();
    let info = app.live.info.clone();
    let bat = app.live.battery.clone();
    let th = app.live.thermals.clone();
    let mut lines: Vec<Line> = Vec::new();
    if let Ok(info) = info.try_read() {
        lines.push(Line::from(vec![
            Span::styled("hostname  ", theme.dim()),
            Span::styled(info.hostname.clone(), theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("kernel    ", theme.dim()),
            Span::styled(info.kernel.clone(), theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("os        ", theme.dim()),
            Span::styled(info.os.clone(), theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("arch      ", theme.dim()),
            Span::styled(info.arch.clone(), theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("uptime    ", theme.dim()),
            Span::styled(sys::format_uptime(info.uptime_secs), theme.fg),
        ]));
        lines.push(Line::from(vec![
            Span::styled("load      ", theme.dim()),
            Span::styled(
                format!(
                    "{:.2} {:.2} {:.2}",
                    info.loadavg.0, info.loadavg.1, info.loadavg.2
                ),
                theme.fg,
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("cpu       ", theme.dim()),
            Span::styled(
                format!("{} × {}", info.cpu_count, trim(&info.cpu_model, 40)),
                theme.fg,
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("memory    ", theme.dim()),
            Span::styled(sys::format_mem(&info.memory), theme.fg),
        ]));
    }
    if let Ok(bat) = bat.try_read() {
        if let Some(b) = bat.as_ref() {
            let cap_style = if b.capacity < 20 {
                theme.error()
            } else if b.capacity < 50 {
                theme.warn()
            } else {
                theme.ok()
            };
            let eta = b
                .time_to_full
                .as_deref()
                .or(b.time_to_empty.as_deref())
                .map(|s| format!(" · ETA {s}"))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled("battery   ", theme.dim()),
                Span::styled(format!("{}% · {}{}", b.capacity, b.status, eta), cap_style),
            ]));
            if let Some(w) = b.power_now_w {
                lines.push(Line::from(vec![
                    Span::styled("power     ", theme.dim()),
                    Span::styled(format!("{w:.2} W"), theme.fg),
                ]));
            }
        }
    }
    if let Ok(th) = th.try_read() {
        for r in th.iter().take(3) {
            let style = if r.temp_c > 75.0 {
                theme.error()
            } else if r.temp_c > 60.0 {
                theme.warn()
            } else {
                theme.ok()
            };
            lines.push(Line::from(vec![
                Span::styled("thermal   ", theme.dim()),
                Span::styled(format!("{} {:.1}°C", r.label, r.temp_c), style),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{}  auto-refresh every 1s", g.arrow),
        theme.dim(),
    )));

    // Wrap so long fields (CPU model) stay inside the column.
    let left = Paragraph::new(lines)
        .block(
            Block::default()
                .title(Span::styled(" facts ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(true)),
        )
        .wrap(Wrap { trim: false })
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
    f.render_widget(left, area);
}

/// Tree view left pane: renders `app.proc_tree` as an indented list of
/// processes keyed by ppid. Orphan ppids (parent not in the snapshot)
/// are treated as roots. Empty snapshot → placeholder line.
fn render_proc_tree_pane(
    f: &mut Frame,
    area: Rect,
    procs: &[cyberdeck_core::process::ProcEntry],
    theme: &Theme,
    focused: bool,
) {
    let lines = build_proc_tree_lines(procs);
    let display_lines: Vec<Line> = if lines.is_empty() {
        vec![Line::from(Span::styled(
            "  (no /proc snapshot yet — refreshes every 15s)",
            theme.dim(),
        ))]
    } else {
        lines
            .iter()
            .map(|s| Line::from(s.as_str()))
            .collect()
    };
    let title = if procs.is_empty() {
        " tree (empty) "
    } else {
        " tree "
    };
    let pane = Paragraph::new(display_lines)
        .block(
            Block::default()
                .title(Span::styled(title, theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(focused)),
        )
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
    f.render_widget(pane, area);
}

/// Build the indented, parent-child indented process tree as a Vec of
/// pre-rendered String lines. Module 6.4 tests pin the indent-by-depth
/// contract via this helper.
///
/// Layout: each row reads `{indent}{pid} {comm} {cmdline-clipped}`. The
/// indent is 2 spaces per depth level. Roots are processes whose ppid
/// is 0 or whose parent PID isn't present in the snapshot (orphan
/// handling — common when a parent has exited between snapshots).
/// Children at each level are sorted by PID for deterministic output.
pub(crate) fn build_proc_tree_lines(
    procs: &[cyberdeck_core::process::ProcEntry],
) -> Vec<String> {
    use std::collections::HashMap;

    if procs.is_empty() {
        return Vec::new();
    }

    // Map parent PID -> child entries. A missing parent means we'll
    // treat the child as a root in the second pass.
    let mut by_ppid: HashMap<u32, Vec<&cyberdeck_core::process::ProcEntry>> =
        HashMap::new();
    for p in procs {
        by_ppid.entry(p.ppid).or_default().push(p);
    }
    let by_pid: HashMap<u32, &cyberdeck_core::process::ProcEntry> =
        procs.iter().map(|p| (p.pid, p)).collect();

    // Roots = ppid == 0 OR parent not in snapshot.
    let mut roots: Vec<&cyberdeck_core::process::ProcEntry> = procs
        .iter()
        .filter(|p| p.ppid == 0 || !by_pid.contains_key(&p.ppid))
        .collect();
    roots.sort_by_key(|p| p.pid);

    let mut out: Vec<String> = Vec::new();
    fn walk(
        node: &cyberdeck_core::process::ProcEntry,
        depth: usize,
        by_ppid: &HashMap<u32, Vec<&cyberdeck_core::process::ProcEntry>>,
        out: &mut Vec<String>,
    ) {
        let indent = "  ".repeat(depth);
        let label = if node.cmdline.is_empty() {
            format!("{} {}", node.pid, node.comm)
        } else {
            // Cap cmdline so a long invocation doesn't blow the column
            // width — the column is 60% of the content pane, which is
            // narrower than the user might expect on a small terminal.
            let clipped: String = node.cmdline.chars().take(48).collect();
            if node.cmdline.chars().count() > 48 {
                format!("{} {} ({clipped}…)", node.pid, node.comm)
            } else {
                format!("{} {} ({clipped})", node.pid, node.comm)
            }
        };
        out.push(format!("{indent}{label}"));
        if let Some(children) = by_ppid.get(&node.pid) {
            let mut sorted = children.clone();
            sorted.sort_by_key(|c| c.pid);
            for c in sorted {
                walk(c, depth + 1, by_ppid, out);
            }
        }
    }

    for r in roots {
        walk(r, 0, &by_ppid, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    //! Module 2.4 — pin the System screen's `r` handler. The right
    //! "recent log" pane is the Logs screen's more compact cousin;
    //! pressing `r` here should also enqueue `Action::RefreshLogs`,
    //! so the dispatcher triggers the same 60s `recent_since` fetch
    //! and routes results back via `LogPushed`.
    use super::*;
    use crate::app::action::Action;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    fn fresh_app_with_observer() -> (App, mpsc::Receiver<Action>) {
        // `App::new` consumes both endpoints of its channel, but the
        // field is effectively dead (the dispatcher lives in
        // `main.rs`). Hand a dummy pair to `App::new`, then overwrite
        // `app.tx` with a fresh sender so we can observe what the
        // screen sends.
        let (_app_tx, app_rx) = mpsc::channel::<Action>(8);
        let (tx, rx) = mpsc::channel::<Action>(8);
        let mut app = App::new(_app_tx, app_rx);
        app.tx = tx;
        (app, rx)
    }

    #[test]
    fn system_screen_r_sends_refresh_logs_action() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (mut app, mut rx) = fresh_app_with_observer();
            app.current = ScreenId::System;

            let mut screen = SystemScreen;
            let start = Instant::now();
            let consumed = screen.on_key(
                KeyEvent::new(KeyCode::Char('r'), crossterm::event::KeyModifiers::NONE),
                &mut app,
            );
            let elapsed = start.elapsed();

            assert!(
                consumed,
                "r must be consumed by the System screen (it has its own handler)"
            );
            assert!(
                elapsed < Duration::from_millis(50),
                "r handler must be non-blocking (elapsed = {:?})",
                elapsed
            );

            let action = rx.try_recv().expect("r must enqueue Action::RefreshLogs");
            assert!(
                matches!(action, Action::RefreshLogs),
                "r must enqueue Action::RefreshLogs, got {:?}",
                action
            );
        });
    }

    // -------------------------------------------------------------------------
    // Module 6.3 — `t` on the System screen toggles `proc_tree_view`.
    // The flag defaults to false (facts view); pressing `t` flips it
    // once, pressing again flips it back. The handler doesn't enqueue
    // any Action — it only mutates App state — so the test only
    // observes the bool.
    // -------------------------------------------------------------------------

    #[test]
    fn system_screen_t_toggles_proc_tree_view() {
        let (mut app, _rx) = fresh_app_with_observer();
        app.current = ScreenId::System;
        assert!(
            !app.proc_tree_view,
            "proc_tree_view must default to false"
        );

        let mut screen = SystemScreen;
        let consumed = screen.on_key(
            KeyEvent::new(KeyCode::Char('t'), crossterm::event::KeyModifiers::NONE),
            &mut app,
        );
        assert!(consumed, "t must be consumed by the System screen");
        assert!(app.proc_tree_view, "first t press must turn tree view on");

        let consumed = screen.on_key(
            KeyEvent::new(KeyCode::Char('t'), crossterm::event::KeyModifiers::NONE),
            &mut app,
        );
        assert!(consumed, "t must remain consumed after the first press");
        assert!(
            !app.proc_tree_view,
            "second t press must turn tree view back off"
        );
    }

    #[test]
    fn system_screen_non_t_keys_return_false() {
        // Sanity: the `t` handler doesn't accidentally swallow unrelated
        // keys. We sample a couple of letters that the System screen
        // doesn't currently bind.
        let (mut app, _rx) = fresh_app_with_observer();
        app.current = ScreenId::System;
        let mut screen = SystemScreen;
        for ch in ['x', 'y', 'z'] {
            let consumed = screen.on_key(
                KeyEvent::new(KeyCode::Char(ch), crossterm::event::KeyModifiers::NONE),
                &mut app,
            );
            assert!(!consumed, "char '{ch}' must NOT be consumed by System");
        }
        assert!(
            !app.proc_tree_view,
            "random keys must not flip proc_tree_view"
        );
    }
}
