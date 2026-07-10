//! Recon screen v2 — 7-tab OSINT action console.
//! Single-pane: Tab/BackTab cycle arms, Char appends to query,
//! Confirm runs, Back clears or exits.
use cyberdeck_intel::recon::ReconTab;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const QUERY_MAX: usize  = 256;
const OUTPUT_CAP: usize = 128; // max lines kept

pub struct ReconScreenV2 {
    pub tab:     ReconTab,
    pub query:   String,
    pub output:  Vec<String>,
    pub scroll:  usize,
    pub running: bool,
}

impl Default for ReconScreenV2 {
    fn default() -> Self {
        Self {
            tab:     ReconTab::Dns,
            query:   String::new(),
            output:  vec!["Recon — type a query, hit A to run.".into()],
            scroll:  0,
            running: false,
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
                self.scroll = self.scroll.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down => {
                self.scroll = self.scroll.saturating_add(1);
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
                self.run_query();
                Consumed::Yes
            }
            NavEvent::Back => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.output = vec!["Recon — type a query, hit A to run.".into()];
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
        frame.render_widget(
            Paragraph::new(Line::from(tab_spans)),
            rows[0],
        );

        // ── Query row ────────────────────────────────────────────────────────
        let q_display = if self.query.is_empty() {
            Span::styled("(type query, A to run)", Style::default().fg(theme.dim))
        } else {
            Span::styled(self.query.clone(), Style::default().fg(theme.fg))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("❯ ", Style::default().fg(theme.accent)),
                q_display,
                if self.running { Span::styled(" …", Style::default().fg(theme.warn)) } else { Span::raw("") },
            ])),
            rows[1],
        );

        // ── Output pane ──────────────────────────────────────────────────────
        let visible = rows[2].height as usize;
        let total   = self.output.len();
        let scroll  = self.scroll.min(total.saturating_sub(visible));
        let end     = total.saturating_sub(scroll);
        let start   = end.saturating_sub(visible);

        let out_lines: Vec<Line<'static>> = self.output[start..end].iter()
            .map(|s| Line::from(Span::styled(s.clone(), Style::default().fg(theme.fg))))
            .collect();

        frame.render_widget(
            Paragraph::new(out_lines)
                .block(Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme.border))),
            rows[2],
        );
    }
}

impl ReconScreenV2 {
    fn run_query(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.output = vec!["(empty query — type something first)".into()];
            return;
        }
        self.running = true;
        let result = match self.tab {
            ReconTab::Dns      => cyberdeck_intel::recon::dns::run(&q),
            ReconTab::Whois    => cyberdeck_intel::recon::whois::run(&q),
            ReconTab::Ip       => cyberdeck_intel::recon::ip::run(&q),
            ReconTab::Ssl      => cyberdeck_intel::recon::ssl::run(&q),
            ReconTab::Cve      => cyberdeck_intel::recon::cve::run(&q),
            ReconTab::Crypto   => cyberdeck_intel::recon::crypto::run(&q),
            ReconTab::Sanctions => cyberdeck_intel::recon::sanctions::run(&q),
        };
        self.running = false;
        match result {
            Ok(body) => {
                self.output = body.lines().map(str::to_string).collect();
                if self.output.is_empty() { self.output = vec!["(no output)".into()]; }
            }
            Err(e) => { self.output = vec![format!("error: {e}")]; }
        }
        // Cap buffer
        if self.output.len() > OUTPUT_CAP {
            self.output.drain(0..self.output.len() - OUTPUT_CAP);
        }
        self.scroll = 0;
    }
}
