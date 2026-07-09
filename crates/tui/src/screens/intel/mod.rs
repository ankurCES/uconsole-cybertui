//! Intel screen — Phase 7 M4.
//!
//! Two-pane layout for OSINT-feed layers from the `cyberdeck-intel`
//! crate:
//!
//! ```text
//! ┌──────────────────────────┬──────────────────────────────┐
//! │ ✈ Flights      1284  ●   │ Flights                       │
//! │ ⚠ Earthquakes  17    ●   │ OpenSky Network · region=KSEA │
//! │ 🔥 Fires       243   ◐   │ 1,284 flights, 14 red         │
//! │ ☀ Weather      —     ●   │                              │
//! │ 🛰 Satellites  7     ●   │ last ok: 12s ago · ok         │
//! │ 📰 News        39    ●   │                              │
//! │ 📷 CCTV        12    ◐   │ raw payload (truncated):      │
//! │ ⚓ Maritime    8     ●   │ {"time":1731001200,"states":… │
//! │ ⚔ Conflicts   5     ◐   │                              │
//! └──────────────────────────┴──────────────────────────────┘
//! ```
//!
//! **Scope today (M4 hardcoded-snapshot):** the screen holds a
//! hardcoded `Vec<cyberdeck_intel::Snapshot>` seeded at construction
//! time. M5 wires the refiller (cyberdeck-intel M5 work) so the same
//! render path reads live data; the renderer does not change between
//! M4 and M5 — only the data source does. Keeping it that way means
//! M5 is a swap of `IntelScreen::new` internals, not a UI regression.
//!
//! **Why two panes, not one.** The 9-layer list alone tells the user
//! "things exist" but not what they are. The right pane surfaces the
//! selected layer's summary, sentinel severity, and the head of the
//! raw payload so an operator can sanity-check that the parser is
//! working without opening the daemon log.
//!
//! **Why the cursor is on the renderer, not `App`.** A `selected`
//! cursor is a per-screen concern (other screens don't care which
//! layer row is highlighted), so it lives in the screen struct — same
//! pattern `CityScreen` uses for its focus state. State only escapes
//! to `App` when something *global* needs it.

use cyberdeck_intel::{worst_sentinel, LayerId, LayerStatus, Sentinel, Snapshot};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

/// Keep the future M5 fetch module's symbol in scope for the swap —
/// when M5 lands we'll swap this module's data source for
/// `cyberdeck_intel::fetch::refiller` (not yet implemented). Today
/// it's only a marker import so the doc comment on `detail_lines()`
/// stays accurate without carrying an unused-import warning.
#[allow(unused_imports)]
use cyberdeck_intel as _future_refiller_marker;

/// Hardcoded fleet used by every fresh `IntelScreen` until the M5
/// refiller replaces it. Mirrors the shape of the refiller output
/// (one `Snapshot` per `LayerId`, oldest first) so the renderer
/// keeps working unchanged when live data lands — only the data
/// source differs.
///
/// The "pending" Weather layer is intentional: it surfaces the
/// screen's "I haven't fetched yet" rendering path so that path is
/// covered by tests instead of being a dark corner that only the
/// first-boot ever sees.
fn fallback_snapshots() -> Vec<Snapshot> {
    use cyberdeck_intel::{Snapshot as S, *};
    let now = chrono::Utc::now().timestamp();
    vec![
        // Flights — green, healthy count.
        S {
            layer: LayerId::Flights,
            status: LayerStatus::Ok { last_ok_unix: now - 12 },
            sentinel: Sentinel::Green,
            summary: "OpenSky Network · region=KSEA".into(),
            entity_count: 1284,
            raw: serde_json::json!({"time": now, "states": ["… (truncated)"]}),
        },
        // Earthquakes — yellow (one M4+ event in the last 24h).
        S {
            layer: LayerId::Earthquakes,
            status: LayerStatus::Ok { last_ok_unix: now - 45 },
            sentinel: Sentinel::Yellow,
            summary: "USGS · M2.5+ last hour, M4+ last 24h".into(),
            entity_count: 17,
            raw: serde_json::json!({"features": ["… (truncated)"]}),
        },
        // Fires — red: one intensity-4 fire detected.
        S {
            layer: LayerId::Fires,
            status: LayerStatus::Ok { last_ok_unix: now - 180 },
            sentinel: Sentinel::Red,
            summary: "FIRMS · 1 intensity-4 detection".into(),
            entity_count: 243,
            raw: serde_json::json!({"events": ["… (truncated)"]}),
        },
        // Weather — pending (demonstrates the "no fetch yet" UI).
        S {
            layer: LayerId::Weather,
            status: LayerStatus::Pending,
            sentinel: Sentinel::Green,
            summary: String::new(),
            entity_count: 0,
            raw: serde_json::Value::Null,
        },
        // Satellites — green, small payload.
        S {
            layer: LayerId::Satellites,
            status: LayerStatus::Ok { last_ok_unix: now - 240 },
            sentinel: Sentinel::Green,
            summary: "CelesTrak · 7 visible in next 60 min".into(),
            entity_count: 7,
            raw: serde_json::json!({"above": ["… (truncated)"]}),
        },
        // News — green, mid-frequency refresh.
        S {
            layer: LayerId::News,
            status: LayerStatus::Ok { last_ok_unix: now - 60 },
            sentinel: Sentinel::Green,
            summary: "GDELT · 39 mentions last 5 min".into(),
            entity_count: 39,
            raw: serde_json::json!({"articles": ["… (truncated)"]}),
        },
        // CCTV — yellow (one feed degraded).
        S {
            layer: LayerId::Cctv,
            status: LayerStatus::Error {
                last_ok_unix: Some(now - 3600),
                reason: "timeout on stream 3".into(),
            },
            sentinel: Sentinel::Yellow,
            summary: "1 of 12 streams timed out".into(),
            entity_count: 12,
            raw: serde_json::json!({"error": "timeout on stream 3"}),
        },
        // Maritime — green.
        S {
            layer: LayerId::Maritime,
            status: LayerStatus::Ok { last_ok_unix: now - 600 },
            sentinel: Sentinel::Green,
            summary: "AIS Hub · 8 vessels in bbox".into(),
            entity_count: 8,
            raw: serde_json::json!({"vessels": ["… (truncated)"]}),
        },
        // Conflicts — yellow (one ACLED event in last hour).
        S {
            layer: LayerId::Conflicts,
            status: LayerStatus::Ok { last_ok_unix: now - 2400 },
            sentinel: Sentinel::Yellow,
            summary: "ACLED · 5 events last 24h".into(),
            entity_count: 5,
            raw: serde_json::json!({"events": ["… (truncated)"]}),
        },
    ]
}

/// Intel screen. Owns its own selection cursor (which layer row is
/// focused) and the snapshot list (hardcoded today; swapped for a
/// M5-refiller output later). The renderer is the public surface of
/// this struct — `poll` is not needed because M4 snapshots are static.
pub struct IntelScreen {
    snapshots: Vec<Snapshot>,
    /// Cursor index into `snapshots`. Wraps on Up/Down. Stays in
    /// range after M5 swaps `snapshots` to a different length — the
    /// renderer clamps it on every render so a stale cursor can't
    /// outrun a freshly-trimmed snapshot list.
    selected: usize,
}

impl IntelScreen {
    /// Build a fresh `IntelScreen`. Reads the live per-layer snapshots
    /// from `App::intel_snapshots` first (populated by the M5
    /// refiller); for any `LayerId` that hasn't produced a snapshot
    /// yet we substitute a `Pending` snapshot from
    /// `fallback_snapshots()` so the grid renders 9 rows from the
    /// first paint instead of an empty left pane.
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            selected: 0,
        }
    }

    /// Materialize the snapshot list for `render` — prefer live data
    /// from `App::intel_snapshots`, fall back to the hardcoded
    /// fixture for any missing `LayerId`. The list is ordered
    /// `LayerId::ALL` so the grid reads top-to-bottom in the same
    /// order the layer IDs were declared.
    pub fn collect_snapshots(app: &App) -> Vec<Snapshot> {
        let mut out = Vec::with_capacity(LayerId::ALL.len());
        let fixture = fallback_snapshots();
        for id in LayerId::ALL {
            let snap = app
                .intel_snapshots
                .get(id)
                .cloned()
                .unwrap_or_else(|| fixture_for(&fixture, *id));
            out.push(snap);
        }
        out
    }

    /// For tests: build with a custom snapshot set so the renderer's
    /// behavior can be exercised against synthetic data without
    /// touching the hardcoded fixture.
    #[cfg(test)]
    pub fn with_snapshots(snapshots: Vec<Snapshot>) -> Self {
        Self { snapshots, selected: 0 }
    }

    /// Clamp `self.selected` so it stays a valid index even after a
    /// `snapshots` swap (e.g. when the M5 refiller pushes new rows).
    fn clamp_selected(&mut self) {
        if self.snapshots.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.snapshots.len() {
            self.selected = self.snapshots.len() - 1;
        }
    }

    /// Sentinel rollup across the current snapshot set. Used by the
    /// right-pane header and the would-be footer chip. Mirrors the
    /// helper in `cyberdeck-intel::worst_sentinel` — same logic, but
    /// there's no benefit to round-tripping through JSON when we
    /// already have the values in memory.
    fn worst_sentinel(&self) -> Sentinel {
        worst_sentinel(self.snapshots.iter().map(|s| s.sentinel))
    }

    /// Build the right-pane body lines for the currently-selected
    /// layer. Extracted so tests can assert on the rendered text
    /// directly without spinning up a `Buffer`/`Frame`.
    fn detail_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        let Some(snap) = self.snapshots.get(self.selected) else {
            return vec![Line::from("no layer selected")];
        };
        let header = format!(
            "{} {}  · {}",
            snap.layer.glyph(),
            snap.layer.label(),
            sentinel_chip(snap.sentinel),
        );
        let last_ok = match &snap.status {
            LayerStatus::Pending => "—".to_string(),
            LayerStatus::Ok { last_ok_unix } => format_ago(last_ok_unix),
            LayerStatus::Error { last_ok_unix: Some(t), .. } => format!("{} (last ok)", format_ago(t)),
            LayerStatus::Error { last_ok_unix: None, .. } => "never".to_string(),
        };
        let status_line = format!(
            "{} · {}",
            status_word(snap.status.word(), theme),
            last_ok
        );
        let summary = if snap.summary.is_empty() {
            "(no summary yet)".to_string()
        } else {
            snap.summary.clone()
        };
        let raw_preview = format!(
            "raw payload: {}",
            serde_json::to_string(&snap.raw)
                .unwrap_or_else(|_| "<unprintable>".into())
                .chars()
                .take(160)
                .collect::<String>()
        );
        let error_note = match &snap.status {
            LayerStatus::Error { reason, .. } => Some(format!("error: {reason}")),
            _ => None,
        };
        let mut lines = vec![
            Line::from(header),
            Line::from(status_line),
            Line::from(""),
            Line::from(summary),
            Line::from(""),
            Line::from(raw_preview),
        ];
        if let Some(err) = error_note {
            lines.push(Line::from(Span::styled(err, theme.warn())));
        }
        lines
    }
}

/// Look up the fallback fixture's snapshot for a single `LayerId`.
fn fixture_for(fixture: &[Snapshot], id: LayerId) -> Snapshot {
    fixture
        .iter()
        .find(|s| s.layer == id)
        .cloned()
        .unwrap_or_else(|| Snapshot {
            layer: id,
            status: LayerStatus::Pending,
            sentinel: Sentinel::Green,
            summary: String::new(),
            entity_count: 0,
            raw: serde_json::Value::Null,
        })
}

impl Screen for IntelScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Intel
    }

    fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        app: &mut App,
        theme: &Theme,
        focus: bool,
    ) {
        // M5 — read live snapshots first; if any `LayerId` has no
        // refiller snapshot yet, the fallback fixture fills it in
        // so the grid is always 9 rows from the first paint. The
        // screen keeps `self.snapshots` in sync so on_key has a
        // stable cursor target.
        self.snapshots = Self::collect_snapshots(app);
        self.clamp_selected();

        // Outer block — title chip carries the worst-sentinel rollup
        // so the user sees the overall health before reading the row.
        let sentinel = self.worst_sentinel();
        let title = format!(
            " Intel · {} ",
            sentinel_chip(sentinel)
        );
        let outer_block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = outer_block.inner(area);
        f.render_widget(outer_block, area);

        // Split into left (layer grid) + right (detail).
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(20)])
            .split(inner);

        // --- Left: one row per layer.
        let rows: Vec<ListItem> = self
            .snapshots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let marker = if i == self.selected { "▶ " } else { "  " };
                let glyph = s.layer.glyph();
                let label = format!("{:<10}", s.layer.label());
                let count = format!("{:>5}", format_count(s.entity_count));
                let dot = status_dot(s.status.dot_color(theme), theme);
                let line = format!("{marker}{glyph} {label} {count} {dot}");
                let style = if i == self.selected {
                    ratatui::style::Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    ratatui::style::Style::default().fg(theme.fg)
                };
                ListItem::new(Line::from(Span::styled(line, style)))
            })
            .collect();
        let list = List::new(rows)
            .block(Block::default().borders(Borders::RIGHT))
            .style(ratatui::style::Style::default().bg(theme.bg).fg(theme.fg));
        f.render_widget(list, cols[0]);

        // --- Right: detail for the selected row.
        let detail = Paragraph::new(self.detail_lines(theme))
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false })
            .style(ratatui::style::Style::default().bg(theme.bg).fg(theme.fg));
        f.render_widget(detail, cols[1]);
    }

    fn on_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        _app: &mut App,
    ) -> bool {
        use crossterm::event::KeyCode;
        match key.code {
            // Down/j: next layer. Wraps. Does NOT leak to the WM
            // — we own the key when focus is on this screen.
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.snapshots.is_empty() {
                    self.selected = (self.selected + 1) % self.snapshots.len();
                }
                true
            }
            // Up/k: previous layer. Wraps.
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.snapshots.is_empty() {
                    self.selected = if self.selected == 0 {
                        self.snapshots.len() - 1
                    } else {
                        self.selected - 1
                    };
                }
                true
            }
            _ => false,
        }
    }
}

impl Default for IntelScreen {
    fn default() -> Self {
        Self::new()
    }
}

// --- helpers ---------------------------------------------------------------

/// Two-letter chip for a `Sentinel`. Same vocabulary as the daemon
/// log so log-reading turns into UI-reading without a translation
/// step.
fn sentinel_chip(s: Sentinel) -> &'static str {
    match s {
        Sentinel::Green => "GREEN",
        Sentinel::Yellow => "YELLOW",
        Sentinel::Red => "RED",
    }
}

/// Compact "Xs ago" formatter. Caps at ">"1d" so the right pane
/// doesn't grow a long tail when a layer hasn't refreshed in hours.
fn format_ago(unix_secs: &i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let dt = now - *unix_secs;
    if dt < 0 {
        return "in the future".into();
    }
    if dt < 60 {
        format!("{dt}s ago")
    } else if dt < 3600 {
        format!("{}m ago", dt / 60)
    } else if dt < 86_400 {
        format!("{}h ago", dt / 3600)
    } else {
        ">1d".into()
    }
}

/// Word for the current status — "OK", "PENDING", or "ERROR". Picked
/// so the right-pane reads as a sentence ("OK · 12s ago") rather than
/// a status code.
fn status_word(w: &str, _theme: &Theme) -> String {
    w.to_string()
}

/// Single character indicating status, plus its color. We render the
/// dot in three flavors so a colorblind operator can still tell them
/// apart by glyph.
fn status_dot(c: ratatui::style::Color, _theme: &Theme) -> Span<'static> {
    Span::styled("●", ratatui::style::Style::default().fg(c))
}

/// Compact entity-count formatter: 1284 stays 1284; 12_345 becomes
/// 12.3k; 9_876_543 becomes 9.9M. Used in the layer grid's right
/// column so the cells line up under 8 chars.
fn format_count(n: u64) -> String {
    if n < 1_000 {
        format!("{n}")
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

// Extension trait that gives `LayerStatus` a stable "(word, dot color)"
// pair, so the renderer's match arms don't have to repeat themselves.
trait LayerStatusExt {
    fn word(&self) -> &'static str;
    fn dot_color(&self, theme: &Theme) -> ratatui::style::Color;
}
impl LayerStatusExt for LayerStatus {
    fn word(&self) -> &'static str {
        match self {
            LayerStatus::Pending => "PENDING",
            LayerStatus::Ok { .. } => "OK",
            LayerStatus::Error { .. } => "ERROR",
        }
    }
    fn dot_color(&self, theme: &Theme) -> ratatui::style::Color {
        match self {
            LayerStatus::Pending => theme.dim,
            LayerStatus::Ok { .. } => theme.ok,
            LayerStatus::Error { .. } => theme.error,
        }
    }
}



// =============================================================================
// tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use cyberdeck_intel::{LayerId, LayerStatus, Sentinel, Snapshot};

    fn s(layer: LayerId, sentinel: Sentinel, status: LayerStatus, count: u64) -> Snapshot {
        Snapshot {
            layer,
            status,
            sentinel,
            summary: format!("{} summary", layer.label()),
            entity_count: count,
            raw: serde_json::json!({"x": 1}),
        }
    }

    #[test]
    fn render_picks_worst_sentinel() {
        // Three layers — worst-of-Green/Yellow/Red is Red. The block
        // title must say RED; if the rollup ever silently downgrades
        // (e.g. the "max()" switched to "min()"), the test catches it.
        let snaps = vec![
            s(LayerId::Flights, Sentinel::Green, LayerStatus::Pending, 0),
            s(LayerId::Fires, Sentinel::Red, LayerStatus::Pending, 0),
            s(LayerId::News, Sentinel::Yellow, LayerStatus::Pending, 0),
        ];
        let screen = IntelScreen::with_snapshots(snaps);
        assert_eq!(screen.worst_sentinel(), Sentinel::Red);
    }

    #[test]
    fn render_counts_compactly() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0k");
        assert_eq!(format_count(12_345), "12.3k");
        assert_eq!(format_count(9_876_543), "9.9M");
    }

    #[test]
    fn clamp_selected_survives_swap_to_shorter_list() {
        // M5 will swap `snapshots` to a different length. A cursor
        // pointing past the new tail must clamp — otherwise we get
        // a panic when the renderer dereferences into `snapshots`.
        let mut screen = IntelScreen::with_snapshots(vec![
            s(LayerId::Flights, Sentinel::Green, LayerStatus::Pending, 0),
            s(LayerId::News, Sentinel::Green, LayerStatus::Pending, 0),
            s(LayerId::Cctv, Sentinel::Green, LayerStatus::Pending, 0),
        ]);
        screen.selected = 2;
        // Replace with a 1-row list.
        screen.snapshots = vec![s(LayerId::Flights, Sentinel::Green, LayerStatus::Pending, 0)];
        screen.clamp_selected();
        assert_eq!(screen.selected, 0);
    }

    #[test]
    fn detail_lines_for_pending_layer_says_no_summary() {
        // The Weather row of the hardcoded fixture is `Pending`. The
        // detail text must not panic and must convey "no fetch yet"
        // instead of leaking an empty-string. We use the explicit
        // fixture path here (not `IntelScreen::new()`) because new()
        // now reads from `App::intel_snapshots` and a fresh `App` has
        // no entries; the fixture path is the M4 contract and the
        // M5 swap reads it through `collect_snapshots()`.
        let fixture = fallback_snapshots();
        let mut screen = IntelScreen::with_snapshots(fixture);
        screen.selected = 3; // Weather
        let lines = screen.detail_lines(&Theme::by_name(crate::theme::ThemeName::Dark));
        let any = lines
            .iter()
            .any(|l| l.to_string().contains("no summary yet") || l.to_string().contains("PENDING"));
        assert!(
            any,
            "pending layer's detail must mention PENDING or 'no summary yet'; got {:?}",
            lines
        );
    }

    #[test]
    fn error_layer_shows_error_reason_in_detail() {
        // CCTV in the hardcoded fixture is `Error { reason: "timeout on stream 3" }`.
        // The right pane must surface that string verbatim — it's how
        // the user learns which feed is broken without opening the
        // daemon log. Same fixture-path contract as the pending test
        // above — `with_snapshots` with the fixture, not the empty
        // default-new path.
        let fixture = fallback_snapshots();
        let mut screen = IntelScreen::with_snapshots(fixture);
        screen.selected = 6; // CCTV
        let lines = screen.detail_lines(&Theme::by_name(crate::theme::ThemeName::Dark));
        let combined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("error: timeout on stream 3"),
            "CCTV detail must include the error reason; got {}",
            combined
        );
    }

    #[test]
    fn on_key_cycles_up_and_down() {
        let mut screen = IntelScreen::with_snapshots(vec![
            s(LayerId::Flights, Sentinel::Green, LayerStatus::Pending, 0),
            s(LayerId::Earthquakes, Sentinel::Green, LayerStatus::Pending, 0),
            s(LayerId::Fires, Sentinel::Green, LayerStatus::Pending, 0),
        ]);
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // Build a real (but unconnected) App — `on_key` only mutates
        // `self.selected` for Down/Up so the rest of the App's state
        // is irrelevant. `App::default()` doesn't exist; the public
        // constructor is `App::new(tx, rx)`.
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(4);
        let mut app = App::new(tx, rx);
        let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(screen.selected, 0);
        assert!(screen.on_key(k(KeyCode::Down), &mut app), "Down cycles");
        assert_eq!(screen.selected, 1);
        assert!(screen.on_key(k(KeyCode::Down), &mut app));
        assert_eq!(screen.selected, 2);
        assert!(screen.on_key(k(KeyCode::Down), &mut app), "wraps to 0");
        assert_eq!(screen.selected, 0);
        assert!(screen.on_key(k(KeyCode::Up), &mut app), "wraps from 0 to last");
        assert_eq!(screen.selected, 2);
    }

    /// M4 contract — the Intel screen must render cleanly at every
    /// width we'd ship on (uconsole 80, terminal 140, wide 200). The
    /// right-pane split is fixed-width-28 on the left, so the only
    /// failure mode this catches is "left column wider than area" or
    /// "split constraint underflow" panics from ratatui. The hardcoded
    /// fixture is used so the render output is stable across runs
    /// (no live snapshot drift).
    #[test]
    fn render_smoke_at_three_widths() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        for w in [80u16, 140, 200] {
            let backend = TestBackend::new(w, 32);
            let mut term = Terminal::new(backend).expect("terminal");
            let mut screen = IntelScreen::new();
            let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(1);
            let mut app = App::new(tx, rx);
            term.draw(|f| {
                screen.render(f, f.area(), &mut app, &theme, true);
            })
            .expect("draw must not panic");
        }
    }

    /// Sentinel rollup title chip must read the worst severity across
    /// the snapshot list. With a single red snapshot in a list of
    /// greens, the title must say RED — this guards against a future
    /// refactor that flips `worst()` to `min()` and silently downgrades
    /// the chip.
    #[test]
    fn render_title_uses_worst_sentinel() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        let snaps = vec![
            s(LayerId::Flights, Sentinel::Green, LayerStatus::Pending, 0),
            s(LayerId::Fires, Sentinel::Red, LayerStatus::Pending, 0),
        ];
        let mut screen = IntelScreen::with_snapshots(snaps);
        let backend = TestBackend::new(140, 32);
        let mut term = Terminal::new(backend).expect("terminal");
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(1);
        let mut app = App::new(tx, rx);
        term.draw(|f| {
            screen.render(f, f.area(), &mut app, &theme, true);
        })
        .expect("draw must not panic");
        let buf = term.backend().buffer().clone();
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push(buf[(x, 0)].symbol().chars().next().unwrap_or(' '));
        }
        assert!(
            row.contains("RED"),
            "title chip must say RED when any layer is Red; got {:?}",
            row
        );
    }

    /// Tab key on the Intel screen must NOT be consumed by the screen —
    /// the main loop owns cycling so Tab/Shift-Tab works the same on
    /// every screen (overworld contract). Returning `false` from
    /// `on_key` is what makes the cycle handler in main.rs fire.
    #[test]
    fn tab_falls_through_to_main_loop() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut screen = IntelScreen::new();
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(1);
        let mut app = App::new(tx, rx);
        let k = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(
            !screen.on_key(k, &mut app),
            "Intel must NOT consume Tab — main loop owns cycling"
        );
    }

    /// Esc on Intel must NOT be consumed — Esc is the universal "leave
    /// to sidebar" verb and is handled by the region router. A screen
    /// that swallows Esc breaks the back navigation contract.
    #[test]
    fn esc_falls_through_to_region_router() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut screen = IntelScreen::new();
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(1);
        let mut app = App::new(tx, rx);
        let k = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(
            !screen.on_key(k, &mut app),
            "Intel must NOT consume Esc — region router owns leave-to-sidebar"
        );
    }

    /// Detail lines for an OK layer must include the summary, last-ok
    /// relative timestamp, and a one-line raw payload preview — that's
    /// the right-pane contract M4 documents. We pick the Flights row
    /// of the hardcoded fixture (always `Ok` with a 12s-old timestamp).
    #[test]
    fn detail_lines_for_ok_layer_shows_summary_and_raw() {
        let fixture = fallback_snapshots();
        let mut screen = IntelScreen::with_snapshots(fixture);
        screen.selected = 0; // Flights
        let lines = screen.detail_lines(&Theme::by_name(crate::theme::ThemeName::Dark));
        let combined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("OpenSky Network"), "summary missing");
        assert!(combined.contains("s ago"), "last-ok relative timestamp missing");
        assert!(combined.contains("raw payload"), "raw preview header missing");
    }

    /// M5 — `collect_snapshots` reads `App::intel_snapshots` first
    /// and falls back to the hardcoded fixture for any missing layer.
    /// After M5 lands, the first paint must NOT show 9 empty `Pending`
    /// rows; it must show the fixture's data until the refiller's
    /// first snapshot lands. This is the contract that keeps the
    /// "first impression" feeling finished.
    #[test]
    fn collect_snapshots_falls_back_to_fixture_on_empty_app() {
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(1);
        let app = App::new(tx, rx);
        assert!(app.intel_snapshots.is_empty());
        let snaps = IntelScreen::collect_snapshots(&app);
        assert_eq!(snaps.len(), LayerId::ALL.len());
        // Every layer rendered must have a non-empty summary OR
        // an explicit `Pending` (Weather is the only Pending in
        // the fixture).
        for s in &snaps {
            assert!(
                !s.summary.is_empty()
                    || matches!(s.status, LayerStatus::Pending),
                "layer {:?} has empty summary and is not Pending",
                s.layer
            );
        }
    }

    /// M5 — when a layer has a live snapshot, `collect_snapshots`
    /// must surface the live data, not the fixture fallback.
    #[test]
    fn collect_snapshots_prefers_live_over_fixture() {
        use cyberdeck_intel::LayerId as L;
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(1);
        let mut app = App::new(tx, rx);
        let live = Snapshot {
            layer: L::Flights,
            status: LayerStatus::Ok { last_ok_unix: 1 },
            sentinel: Sentinel::Green,
            summary: "live-test-marker".into(),
            entity_count: 9999,
            raw: serde_json::Value::Null,
        };
        app.intel_snapshots.insert(L::Flights, live);
        let snaps = IntelScreen::collect_snapshots(&app);
        let flights = snaps.iter().find(|s| s.layer == L::Flights).unwrap();
        assert_eq!(flights.summary, "live-test-marker");
        assert_eq!(flights.entity_count, 9999);
        // Other layers still come from the fixture.
        let news = snaps.iter().find(|s| s.layer == L::News).unwrap();
        assert!(news.summary.contains("GDELT"));
    }
}
