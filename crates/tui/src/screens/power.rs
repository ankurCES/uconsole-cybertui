//! Power screen: battery, governor, suspend/hibernate/reboot/shutdown.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::{App, ConfirmKind, Modal};
use crate::theme::{glyphs, Theme};

pub struct PowerScreen;

impl Screen for PowerScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Power
    }
    fn title(&self) -> &'static str {
        "Power"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            KeyCode::Char('s') => {
                let _ = app.tx.try_send(Action::Run(RunAction::Suspend));
            }
            KeyCode::Char('h') => {
                let _ = app.tx.try_send(Action::Run(RunAction::Hibernate));
            }
            KeyCode::Char('r') => {
                app.modal = Modal::Confirm {
                    message: "Reboot the system?".into(),
                    kind: ConfirmKind::Reboot,
                    arg: String::new(),
                };
            }
            KeyCode::Char('p') => {
                app.modal = Modal::Confirm {
                    message: "Shut down the system?".into(),
                    kind: ConfirmKind::Shutdown,
                    arg: String::new(),
                };
            }
            KeyCode::Char('g') => {
                // Cycle governor between the two most common choices.
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    if let Ok(cur) = cyberdeck_core::power::cpu_governor().await {
                        let next = if cur.governor == "performance" {
                            "powersave"
                        } else {
                            "performance"
                        };
                        match cyberdeck_core::power::set_governor(next).await {
                            Ok(_) => {
                                let _ = tx
                                    .send(Action::Toast(
                                        ToastKind::Ok,
                                        format!("governor → {next}"),
                                    ))
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Action::Toast(ToastKind::Error, format!("{e}")))
                                    .await;
                            }
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
            .title(Span::styled(" Power ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // Left: battery.
        let g = glyphs();
        let mut left_lines: Vec<Line> = Vec::new();
        if let Ok(b) = app.live.battery.try_read() {
            if let Some(bat) = b.as_ref() {
                let bar = battery_bar(bat.capacity);
                let style = if bat.capacity < 20 {
                    theme.error()
                } else if bat.capacity < 50 {
                    theme.warn()
                } else {
                    theme.ok()
                };
                left_lines.push(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(bar, style),
                    Span::styled(format!(" {:>3}%", bat.capacity), style),
                ]));
                left_lines.push(Line::from(vec![
                    Span::styled("  status   ", theme.dim()),
                    Span::styled(bat.status.clone(), theme.fg),
                ]));
                if let Some(t) = &bat.time_to_full {
                    left_lines.push(Line::from(vec![
                        Span::styled("  to full  ", theme.dim()),
                        Span::styled(t.clone(), theme.fg),
                    ]));
                }
                if let Some(t) = &bat.time_to_empty {
                    left_lines.push(Line::from(vec![
                        Span::styled("  to empty ", theme.dim()),
                        Span::styled(t.clone(), theme.fg),
                    ]));
                }
                if let Some(w) = bat.power_now_w {
                    left_lines.push(Line::from(vec![
                        Span::styled("  power    ", theme.dim()),
                        Span::styled(format!("{w:.2} W"), theme.fg),
                    ]));
                }
                if let Some(h) = &bat.health {
                    left_lines.push(Line::from(vec![
                        Span::styled("  health   ", theme.dim()),
                        Span::styled(h.clone(), theme.fg),
                    ]));
                }
            } else {
                left_lines.push(Line::from(Span::styled(
                    "  (no battery — running on AC)",
                    theme.dim(),
                )));
            }
        }
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled("  actions:", theme.title())));
        left_lines.push(Line::from(vec![
            Span::styled("  s ", theme.key()),
            Span::styled("suspend  ", theme.dim()),
            Span::styled(" h ", theme.key()),
            Span::styled("hibernate", theme.dim()),
        ]));
        left_lines.push(Line::from(vec![
            Span::styled("  r ", theme.key()),
            Span::styled("reboot   ", theme.dim()),
            Span::styled(" p ", theme.key()),
            Span::styled("poweroff", theme.dim()),
        ]));
        let _ = g;
        let left = Paragraph::new(left_lines).block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(theme.border(false)),
        );
        f.render_widget(left, cols[0]);

        // Right: governor + thermals.
        let mut right: Vec<ListItem> = Vec::new();
        if let Ok(info) = app.live.info.try_read() {
            right.push(ListItem::new(Line::from(vec![
                Span::styled("  cpu      ", theme.dim()),
                Span::styled(
                    format!("{} × {}", info.cpu_count, truncate(&info.cpu_model, 36)),
                    theme.fg,
                ),
            ])));
        }
        // CPU governor (live read of /sys).
        if let Ok(g) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(cyberdeck_core::power::cpu_governor())
        }) {
            right.push(ListItem::new(Line::from(vec![
                Span::styled("  driver   ", theme.dim()),
                Span::styled(g.driver, theme.fg),
            ])));
            right.push(ListItem::new(Line::from(vec![
                Span::styled("  governor ", theme.dim()),
                Span::styled(g.governor.clone(), theme.accent),
            ])));
            right.push(ListItem::new(Line::from(Span::styled(
                format!("  available: {}", g.available.join(", ")),
                theme.dim(),
            ))));
        }
        if let Ok(th) = app.live.thermals.try_read() {
            right.push(ListItem::new(Line::from("")));
            for r in th.iter() {
                let style = if r.temp_c > 75.0 {
                    theme.error()
                } else if r.temp_c > 60.0 {
                    theme.warn()
                } else {
                    theme.ok()
                };
                right.push(ListItem::new(Line::from(vec![
                    Span::styled("  thermal  ", theme.dim()),
                    Span::styled(format!("{:<14}", r.label), theme.fg),
                    Span::styled(format!("{:.1}°C", r.temp_c), style),
                ])));
            }
        }
        right.push(ListItem::new(Line::from("")));
        right.push(ListItem::new(Line::from(Span::styled(
            "  g toggle governor (performance↔powersave)",
            theme.dim(),
        ))));
        let list = List::new(right).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, cols[1]);
    }
}

fn battery_bar(cap: u8) -> String {
    let filled = (cap as usize) / 10;
    let empty = 10 - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

// Suppress unused warning if Borders import ever becomes truly unused.
#[allow(dead_code)]
fn _b(_: Borders) {}
