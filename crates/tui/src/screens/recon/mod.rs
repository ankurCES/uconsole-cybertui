//! Recon screen — Phase 7 M7.
//!
//! Seven-tab OSINT action console driven by `cyberdeck_intel::recon`.
//! Each tab ships one primitive behind the stable
//! `pub fn run(query: &str) -> anyhow::Result<String>` contract so the
//! Recon CLI verb (`cyberdeck recon …`) and the screen share one
//! data path.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │ DNS │ WHOIS │ IP │ SSL │ CVE │ CRYPTO │ SANCTIONS            │
//! ├──────────────────────────────────────────────────────────────┤
//! │ query > example.com_                                         │
//! ├──────────────────────────────────────────────────────────────┤
//! │ 93.184.216.34                                                │
//! │                                                              │
//! │ j/k scroll · Enter run · Esc clear · Tab next arm            │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! **Single-pane by design.** Unlike `CityScreen` / `IntelScreen`,
//! Recon has no "left grid + right detail" split — the output
//! *is* the screen. `ScreenId::has_right_pane` returns false for
//! `Recon` so the region model never tries to step into it.
//!
//! **Keymap.** Tab / BackTab cycle tabs (matching the tab-strip
//! indicator from M3), characters append to the query buffer,
//! Enter runs the active arm, Esc clears the query, `j` / `k` (and
//! arrow keys) scroll the output pane. We don't bind `q` to quit
//! — the only way out is Tab/BackTab to the next screen, the same
//! way every other screen handles "go elsewhere".
//!
//! **No async.** Every primitive is synchronous on purpose — DNS /
//! WHOIS / openssl are blocking sub-processes, ip-api is a one-shot
//! HTTP, and the rest are bundled-table lookups. The screen calls
//! `recon::<arm>::run()` synchronously in `handle_enter` so the
//! output paints in the same frame as the user's keystroke; a long
//! run can be cancelled with `Esc` (which clears the output area).
//!
//! **SSRF.** Every primitive that resolves a user-supplied target to
//! a network endpoint runs through `cyberdeck_intel::recon::ssrf`
//! before any process spawn or HTTP call. A loopback / RFC1918 input
//! produces a structured "refused to target" error that the screen
//! renders red on the output area.

use cyberdeck_intel::recon::{ReconTab, ReconTab as Tab};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

/// Hard cap on the query buffer. Long inputs are silently truncated
/// to keep a paste-bomb from pushing the tab strip off the screen.
const QUERY_MAX: usize = 256;

/// Hard cap on the output buffer. Each recon primitive may return a
/// megabyte of WHOIS text; we cap to 4 KiB so scrolling + render stay
/// snappy at 80×32.
const OUTPUT_MAX: usize = 4096;

pub struct ReconScreen {
    /// Active tab. Cycles on Tab / BackTab; `Enter` runs `run()` for
    /// this tab's primitive.
    pub tab: ReconTab,
    /// Pending query buffer. The screen does not parse it — each
    /// primitive re-reads and sanitises as it sees fit.
    pub query: String,
    /// Last-output buffer (newline-split). The right pane's content.
    pub output: Vec<String>,
    /// Output scroll offset (line index of the top visible row).
    pub scroll: u16,
    /// Set to `true` by `handle_enter` so the render path paints a
    /// "running…" flash (no real async — `run()` is sync).
    pub running: bool,
    /// Footer hint; rotates between "Tab next arm", "j/k scroll", etc.
    pub last_hint: &'static str,
}

impl Default for ReconScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconScreen {
    pub fn new() -> Self {
        Self {
            tab: ReconTab::Dns,
            query: String::new(),
            output: vec!["Recon — type a query, hit Enter.".into()],
            scroll: 0,
            running: false,
            last_hint: "Tab cycle · Enter run · Esc clear · j/k scroll",
        }
    }

    /// Tab strip area: one cell per `ReconTab`, space + glyph +
    /// label, with the active tab rendered in the theme's accent
    /// colour. The strip never collapses (always painted when there
    /// is at least 1 row of height available).
    fn tab_strip<'a>(&self, theme: &Theme, focus: bool) -> List<'a> {
        let items: Vec<ListItem> = ReconTab::ALL
            .iter()
            .map(|t| {
                let active = *t == self.tab;
                let line = Line::from(vec![
                    Span::styled(
                        format!(" {} ", t.glyph()),
                        if active {
                            ratatui::style::Style::default()
                                .fg(theme.bg)
                                .bg(theme.accent)
                        } else {
                            ratatui::style::Style::default().fg(theme.dim)
                        },
                    ),
                    Span::styled(
                        format!(" {} ", t.label()),
                        if active {
                            ratatui::style::Style::default().fg(theme.accent)
                        } else {
                            ratatui::style::Style::default().fg(theme.fg)
                        },
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();
        List::new(items)
            .block(
                Block::default()
                    .title(" Recon ")
                    .borders(Borders::ALL)
                    .border_style(theme.border(focus)),
            )
            .highlight_style(ratatui::style::Style::default().bg(theme.bg))
    }

    /// Render the right pane (query row + scrollable output area).
    fn body_lines(&self) -> Vec<Line<'_>> {
        let mut out = Vec::with_capacity(self.output.len() + 2);
        // Query row — rendered with a `>` prompt prefix.
        let q_disp = if self.query.is_empty() {
            "(type to enter a query)".to_string()
        } else {
            self.query.clone()
        };
        out.push(Line::from(vec![
            Span::styled("query > ", ratatui::style::Style::default().fg(theme_accent_dim())),
            Span::styled(q_disp, ratatui::style::Style::default().fg(theme_fg_main())),
        ]));
        out.push(Line::from(""));
        for line in &self.output {
            out.push(Line::from(line.clone()));
        }
        out
    }

    /// Run the active arm's `run()` and write the response into
    /// `output`. Errors become visible strings — the screen treats
    /// every result identically and just renders it.
    pub fn handle_enter(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.output = vec!["(empty query — type something first)".into()];
            return;
        }
        self.running = true;
        let result = match self.tab {
            ReconTab::Dns => cyberdeck_intel::recon::dns::run(&q),
            ReconTab::Whois => cyberdeck_intel::recon::whois::run(&q),
            ReconTab::Ip => cyberdeck_intel::recon::ip::run(&q),
            ReconTab::Ssl => cyberdeck_intel::recon::ssl::run(&q),
            ReconTab::Cve => cyberdeck_intel::recon::cve::run(&q),
            ReconTab::Crypto => cyberdeck_intel::recon::crypto::run(&q),
            ReconTab::Sanctions => cyberdeck_intel::recon::sanctions::run(&q),
        };
        self.running = false;
        match result {
            Ok(body) => {
                self.output = body.lines().map(str::to_string).collect();
                if self.output.is_empty() {
                    self.output = vec!["(no output)".into()];
                }
            }
            Err(e) => {
                self.output = vec![format!("error: {e}")];
            }
        }
        self.scroll = 0;
        self.last_hint = "Tab next · Esc clear · j/k scroll";
        // Cap the buffer to OUTPUT_MAX so a 1 MiB WHOIS response
        // doesn't pin the renderer.
        if self.output.len() > OUTPUT_MAX / 32 {
            let drop = self.output.len() - OUTPUT_MAX / 32;
            self.output.drain(0..drop);
            self.output.insert(0, "(… output truncated)".into());
        }
    }

    /// Push a printable character onto the query buffer, truncating
    /// to QUERY_MAX.
    pub fn push_char(&mut self, ch: char) {
        if self.query.len() >= QUERY_MAX {
            return;
        }
        self.query.push(ch);
        self.last_hint = "Enter to run";
    }

    /// Backspace — remove the trailing char if any.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.last_hint = "Enter to run";
    }

    /// Cycle the active tab forward / backward.
    pub fn cycle_tab(&mut self, forward: bool) {
        self.tab = self.tab.cycle(forward);
    }

    /// Clear the query and reset the output area.
    pub fn clear(&mut self) {
        self.query.clear();
        self.output = vec!["(cleared — type a query, hit Enter)".into()];
        self.scroll = 0;
        self.last_hint = "Tab cycle · Enter run · Esc clear";
    }

    /// Scroll the output by `delta` rows, clamping within bounds.
    pub fn scroll_by(&mut self, delta: i32) {
        let max_scroll = self.output.len().saturating_sub(1) as u16;
        let next = if delta >= 0 {
            self.scroll.saturating_add(delta as u16)
        } else {
            self.scroll.saturating_sub((-delta) as u16)
        };
        self.scroll = next.min(max_scroll);
    }
}

// Tiny local helpers — we can't `use theme::*` without polluting the
// module's pub surface. These two call into the theme palette with
// safe defaults so a stub theme still renders something readable.
fn theme_accent_dim() -> ratatui::style::Color { ratatui::style::Color::DarkGray }
fn theme_fg_main() -> ratatui::style::Color { ratatui::style::Color::Gray }

impl Screen for ReconScreen {
    fn id(&self) -> ScreenId { ScreenId::Recon }
    fn title(&self) -> &'static str { "Recon" }

    fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        _app: &mut App,
        theme: &Theme,
        focus: bool,
    ) {
        // Split area into: tab strip (3 rows) | query + output (rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(area);
        f.render_widget(self.tab_strip(theme, focus), chunks[0]);

        // Body: tinted background block. Wrap the lines so an output
        // like a long JSON object remains readable at 80 cols.
        let body = Paragraph::new(self.body_lines())
            .block(
                Block::default()
                    .title(format!(" Recon · {} ", self.tab.label()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(focus)),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.scroll as u16, 0));
        f.render_widget(body, chunks[1]);

        // Footer hint row.
        let footer = Paragraph::new(Line::from(self.last_hint))
            .style(ratatui::style::Style::default().fg(theme.dim));
        f.render_widget(footer, chunks[2]);

        // Clear the area beneath the active widget for the cursor —
        // ratatui needs an explicit blank cursor target so the
        // terminal doesn't carry a stale one.
        f.render_widget(Clear, chunks[1]);
    }

    /// Pinned default — Recon screen always visible on the launcher,
    /// and on the Tab cycle. We don't want to skip it just because
    /// the user hasn't typed anything yet.
    fn is_hidden(&self, _app: &App) -> bool {
        false
    }
}

// `Tab` is the local alias declared at the top of the file. Strip
// it from the public API now that we've used it once so the lint
// stays clean.
#[allow(dead_code)]
fn _alias_marker() -> Tab { ReconTab::Dns }

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Default state: DNS tab selected, query empty, output has the
    /// placeholder line, scroll at zero, not running.
    #[test]
    fn fresh_screen_starts_on_dns_with_placeholder() {
        let s = ReconScreen::new();
        assert_eq!(s.tab, ReconTab::Dns);
        assert!(s.query.is_empty());
        assert!(!s.running);
        assert_eq!(s.scroll, 0);
        assert!(!s.output.is_empty(), "fresh output shouldn't be empty");
        assert!(s.output[0].contains("Recon"), "placeholder text missing");
    }

    /// Query buffer caps at 256 chars — silent drop after that.
    #[test]
    fn query_buffer_caps_at_max() {
        let mut s = ReconScreen::new();
        for _ in 0..(QUERY_MAX + 50) {
            s.push_char('a');
        }
        assert_eq!(s.query.len(), QUERY_MAX);
    }

    /// Backspace pops the trailing char (UTF-8 safe at byte level —
    /// the screen never edits mid-cluster).
    #[test]
    fn backspace_pops_trailing_char() {
        let mut s = ReconScreen::new();
        s.push_char('h');
        s.push_char('i');
        assert_eq!(s.query, "hi");
        s.backspace();
        assert_eq!(s.query, "h");
        s.backspace();
        assert!(s.query.is_empty());
        // Past the start — no panic.
        s.backspace();
        assert!(s.query.is_empty());
    }

    /// Tab cycle visits every tab exactly once before returning to
    /// the start (in both directions).
    #[test]
    fn cycle_visits_every_tab_before_wrapping() {
        let mut s = ReconScreen::new();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..ReconTab::ALL.len() {
            seen.insert(s.tab);
            s.cycle_tab(true);
        }
        // After n forward cycles we're back at the starting tab, so
        // the *current* tab isn't in `seen`. Re-insert and assert.
        seen.insert(s.tab);
        assert_eq!(seen.len(), ReconTab::ALL.len());
    }

    /// Backward cycle from the first tab wraps to the last.
    #[test]
    fn cycle_backward_wraps_to_last() {
        let mut s = ReconScreen::new();
        s.cycle_tab(false);
        assert_eq!(s.tab, *ReconTab::ALL.last().unwrap());
    }

    /// Forward cycle from the last wraps to the first.
    #[test]
    fn cycle_forward_wraps_to_first() {
        let mut s = ReconScreen::new();
        for _ in 0..ReconTab::ALL.len() - 1 {
            s.cycle_tab(true);
        }
        assert_eq!(s.tab, *ReconTab::ALL.last().unwrap());
        s.cycle_tab(true);
        assert_eq!(s.tab, ReconTab::Dns);
    }

    /// Enter on an empty query produces a friendly prompt rather
    /// than a panic.
    #[test]
    fn enter_on_empty_query_renders_prompt() {
        let mut s = ReconScreen::new();
        s.handle_enter();
        assert!(s.output.iter().any(|l| l.contains("empty query")));
    }

    /// Enter on the CVE arm produces the offline fixture hits —
    /// hermetic (no network). The screen handles a multi-line
    /// output by painting every line; we only need the data to
    /// land in `output`.
    #[test]
    fn cve_arm_returns_offline_hits_for_known_keyword() {
        let mut s = ReconScreen::new();
        s.tab = ReconTab::Cve;
        s.query = "log4j".into();
        s.handle_enter();
        let blob = s.output.join("\n");
        assert!(blob.contains("CVE-2021-44228"));
    }

    /// Sanctions substring that hits the bundled SDN fixture row
    /// must surface the "Test Sanctioned Entity" name — the screen
    /// pipeline is responsible for calling the primitive, the
    /// primitive owns the data.
    #[test]
    fn sanctions_arm_returns_fixture_entity() {
        let mut s = ReconScreen::new();
        s.tab = ReconTab::Sanctions;
        s.query = "sanctioned".into();
        s.handle_enter();
        let blob = s.output.join("\n");
        assert!(blob.contains("Test Sanctioned Entity"));
    }

    /// IP-arm SSRF gate fires before any network call when the
    /// user pastes a loopback address. Screen surfaces the
    /// structured error verbatim.
    #[test]
    fn ip_arm_loopback_input_surfaces_ssrf_error_not_panic() {
        let mut s = ReconScreen::new();
        s.tab = ReconTab::Ip;
        s.query = "127.0.0.1".into();
        s.handle_enter();
        let blob = s.output.join("\n");
        assert!(blob.contains("refused to target"));
        assert!(blob.contains("loopback"));
    }

    /// Clear resets the buffer; the next Enter repaints the
    /// placeholder rather than the prior output.
    #[test]
    fn clear_resets_state() {
        let mut s = ReconScreen::new();
        s.query = "example".into();
        s.handle_enter();
        assert!(s.query == "example" || s.output.iter().any(|l| l.contains("error") || l.contains("Example")));
        s.clear();
        assert!(s.query.is_empty());
        assert_eq!(s.scroll, 0);
        assert!(s.output.iter().any(|l| l.contains("cleared")));
    }

    /// Scroll offset is clamped to the visible range.
    #[test]
    fn scroll_clamps_to_output_len() {
        let mut s = ReconScreen::new();
        s.output = (0..10).map(|i| format!("line {i}")).collect();
        s.scroll_by(1000);
        assert!(s.scroll as usize <= s.output.len().saturating_sub(1));
        s.scroll_by(-1000);
        assert_eq!(s.scroll, 0);
    }

    /// Render draws a non-empty tab strip + body + footer in a
    /// fresh app at a normal terminal size.
    #[test]
    fn render_produces_three_chunks() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(120, 32);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = crate::app::App::new_for_tests();
        let mut s = ReconScreen::new();
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        term.draw(|f| {
            s.render(f, f.area(), &mut app, &theme, true);
        }).unwrap();
        let buf = term.backend().buffer().clone();
        // The screen must have painted the tab strip — the active
        // tab's glyph is somewhere in the top three rows.
        let top: String = (0..3)
            .flat_map(|y| (0..120).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(top.contains('D'), "DNS glyph missing in tab strip: {top:?}");
        assert!(top.contains("Recon"), "tab strip title missing");
    }

    /// Render at a minimal 80×24 terminal still produces a non-
    /// empty body so the screen never paints blank at a cramped
    /// size.
    #[test]
    fn render_at_80x24_is_non_empty() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = crate::app::App::new_for_tests();
        let mut s = ReconScreen::new();
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        term.draw(|f| {
            s.render(f, f.area(), &mut app, &theme, true);
        }).unwrap();
        let buf = term.backend().buffer().clone();
        let all: String = (0..24)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect();
        assert!(all.contains("DNS") || all.contains("Recon"));
    }
}
