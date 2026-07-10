//! Recon screen v2 — 7-tab OSINT action console.
//! Single-pane: Tab/BackTab cycle arms, Char appends to query,
//! Confirm runs, Back clears or exits.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyberdeck_intel::recon::ReconTab;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::action::Action;
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const QUERY_MAX: usize  = 256;
const OUTPUT_CAP: usize = 128;
// Timeout for any single recon call — covers the SSL s_client hang case.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

pub struct ReconScreenV2 {
    pub tab:    ReconTab,
    pub query:  String,
    pub scroll: usize,
    // ponytail: shared mailbox — background task writes, render reads
    output: Arc<Mutex<Vec<String>>>,
}

impl Default for ReconScreenV2 {
    fn default() -> Self {
        Self {
            tab:    ReconTab::Dns,
            query:  String::new(),
            scroll: 0,
            output: Arc::new(Mutex::new(vec![
                "Recon — type a query, hit A to run.".into(),
            ])),
        }
    }
}

impl ScreenV2 for ReconScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Recon }
    fn title(&self) -> &str { "Recon" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "Tab arm   ▲▼ scroll   A run   B clear/back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Tab => {
                let n = ReconTab::ALL.len();
                let pos = ReconTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                self.tab = ReconTab::ALL[(pos + 1) % n];
                Consumed::Yes
            }
            NavEvent::BackTab => {
                let n = ReconTab::ALL.len();
                let pos = ReconTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                self.tab = ReconTab::ALL[(pos + n - 1) % n];
                Consumed::Yes
            }
            NavEvent::Up => {
                self.scroll = self.scroll.saturating_add(1);
                Consumed::Yes
            }
            NavEvent::Down => {
                self.scroll = self.scroll.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Char(c) => {
                if self.query.len() < QUERY_MAX { self.query.push(c); }
                Consumed::Yes
            }
            NavEvent::Backspace => {
                self.query.pop();
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let q = self.query.trim().to_string();
                if q.is_empty() {
                    *self.output.lock().unwrap() =
                        vec!["(empty query — type something first)".into()];
                    return Consumed::Yes;
                }
                // Signal running immediately so next render shows it.
                *self.output.lock().unwrap() = vec!["running…".into()];
                self.scroll = 0;
                let tab    = self.tab;
                let out_tx = Arc::clone(&self.output);
                let tx     = ctx.tx.clone();
                tokio::spawn(async move {
                    // spawn_blocking so we don't stall the tokio runtime.
                    // timeout guards against openssl s_client or whois hanging.
                    let result = tokio::time::timeout(
                        CALL_TIMEOUT,
                        tokio::task::spawn_blocking(move || match tab {
                            ReconTab::Dns       => cyberdeck_intel::recon::dns::run(&q),
                            ReconTab::Whois     => cyberdeck_intel::recon::whois::run(&q),
                            ReconTab::Ip        => cyberdeck_intel::recon::ip::run(&q),
                            ReconTab::Ssl       => cyberdeck_intel::recon::ssl::run(&q),
                            ReconTab::Cve       => cyberdeck_intel::recon::cve::run(&q),
                            ReconTab::Crypto    => cyberdeck_intel::recon::crypto::run(&q),
                            ReconTab::Sanctions => cyberdeck_intel::recon::sanctions::run(&q),
                        }),
                    )
                    .await
                    // timeout elapsed
                    .unwrap_or_else(|_| Ok(Err(anyhow::anyhow!("timed out after 15s"))))
                    // spawn_blocking join error
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("task panicked: {e}")));

                    let mut lines: Vec<String> = match result {
                        Ok(body) => body.lines().map(str::to_string).collect(),
                        Err(e)   => vec![format!("error: {e}")],
                    };
                    if lines.is_empty() { lines = vec!["(no output)".into()]; }
                    if lines.len() > OUTPUT_CAP {
                        lines.drain(0..lines.len() - OUTPUT_CAP);
                    }
                    *out_tx.lock().unwrap() = lines;
                    tx.try_send(Action::Tick).ok();
                });
                Consumed::Yes
            }
            NavEvent::Back => {
                if !self.query.is_empty() {
                    self.query.clear();
                    *self.output.lock().unwrap() =
                        vec!["Recon — type a query, hit A to run.".into()];
                    self.scroll = 0;
                } else {
                    ctx.go_back();
                }
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        // Vertical layout: tab strip (1 row) | query (1 row) | output
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);

        // ── Tab strip ────────────────────────────────────────────────────────
        let tab_spans: Vec<Span<'static>> = ReconTab::ALL.iter().flat_map(|t| {
            let active = *t == self.tab;
            let (label_style, sep_style) = if active {
                (Style::default().fg(theme.bg).bg(theme.accent), Style::default().fg(theme.accent))
            } else {
                (Style::default().fg(theme.dim), Style::default().fg(theme.dim))
            };
            [
                Span::styled(format!(" {} ", t.label()), label_style),
                Span::styled("│", sep_style),
            ]
        }).collect();
        frame.render_widget(Paragraph::new(Line::from(tab_spans)), rows[0]);

        // ── Query row ────────────────────────────────────────────────────────
        let output = self.output.lock().unwrap();
        let is_running = output.first().map(|s| s == "running…").unwrap_or(false);
        let q_display = if self.query.is_empty() {
            Span::styled("(type query, A to run)", Style::default().fg(theme.dim))
        } else {
            Span::styled(self.query.clone(), Style::default().fg(theme.fg))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("❯ ", Style::default().fg(theme.accent)),
                q_display,
                if is_running {
                    Span::styled(" …", Style::default().fg(theme.warn))
                } else {
                    Span::raw("")
                },
            ])),
            rows[1],
        );

        // ── Output pane ──────────────────────────────────────────────────────
        let visible = rows[2].height as usize;
        let total   = output.len();
        // scroll=0 → bottom of output; scroll up to see earlier lines
        let scroll  = self.scroll.min(total.saturating_sub(visible));
        let end     = total.saturating_sub(scroll);
        let start   = end.saturating_sub(visible);

        let out_lines: Vec<Line<'static>> = output[start..end].iter()
            .map(|s| Line::from(Span::styled(s.clone(), Style::default().fg(theme.fg))))
            .collect();
        drop(output); // release lock before widget render

        frame.render_widget(
            Paragraph::new(out_lines)
                .block(Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme.border))),
            rows[2],
        );
    }
}
