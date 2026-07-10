//! Intel screen v2 — OSINT layer grid (left) + snapshot detail (right).
//! Reads live snapshots from ctx.live.intel_snapshots; falls back to
//! hardcoded fixture for any missing layer.
use cyberdeck_intel::{worst_sentinel, LayerId, LayerStatus, Sentinel, Snapshot};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

const ZONES: &[Zone] = &[Zone::Left, Zone::Right];

pub struct IntelScreenV2 {
    pub selected: usize,
}

impl Default for IntelScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for IntelScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Intel }
    fn title(&self) -> &str { "Intel" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼ layer   ◀▶ pane   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let n = LayerId::ALL.len();
        match event {
            NavEvent::Left  => { ctx.nav.focus_zone = 0; Consumed::Yes }
            NavEvent::Right => { ctx.nav.focus_zone = 1; Consumed::Yes }
            NavEvent::Tab   => { ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % ZONES.len(); Consumed::Yes }
            NavEvent::BackTab => {
                let zones = ZONES.len();
                ctx.nav.focus_zone = (ctx.nav.focus_zone + zones - 1) % zones;
                Consumed::Yes
            }
            NavEvent::Down => {
                self.selected = (self.selected + 1) % n;
                Consumed::Yes
            }
            NavEvent::Up => {
                self.selected = if self.selected == 0 { n - 1 } else { self.selected - 1 };
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let left_focused  = ctx.nav.focus_zone == 0;
        let right_focused = ctx.nav.focus_zone == 1;

        // Collect snapshots: live first, fallback fixture for gaps.
        let snapshots = collect_snapshots(ctx);
        let selected = self.selected.min(snapshots.len().saturating_sub(1));

        // Rollup sentinel for the outer title.
        let worst = worst_sentinel(snapshots.iter().map(|s| s.sentinel));
        let outer_title = format!(" Intel · {} ", sentinel_chip(worst));

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(20)])
            .split(area);

        // ── Left: layer grid ─────────────────────────────────────────────────
        let rows: Vec<ListItem<'static>> = snapshots.iter().enumerate().map(|(i, s)| {
            let marker = if i == selected { "▶ " } else { "  " };
            let glyph  = s.layer.glyph();
            let label  = format!("{:<10}", s.layer.label());
            let count  = format!("{:>5}", format_count(s.entity_count));
            let dot    = status_dot_char(&s.status);
            let style = if i == selected {
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(
                format!("{marker}{glyph} {label} {count} {dot}"),
                style,
            )))
        }).collect();

        let left = List::new(rows)
            .block(Block::default()
                .title(Span::styled(outer_title, theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(left_focused)));
        frame.render_widget(left, cols[0]);

        // ── Right: detail for selected layer ─────────────────────────────────
        let detail_lines = detail_lines_for(snapshots.get(selected), theme);
        let right = Paragraph::new(detail_lines)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(right_focused)))
            .wrap(Wrap { trim: false });
        frame.render_widget(right, cols[1]);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn collect_snapshots(ctx: &UiContext<'_>) -> Vec<Snapshot> {
    let live = ctx.live.intel_snapshots.try_read().ok();
    let fixture = fallback_snapshots();
    LayerId::ALL.iter().map(|id| {
        live.as_ref()
            .and_then(|m| m.get(id).cloned())
            .unwrap_or_else(|| fixture_for(&fixture, *id))
    }).collect()
}

fn detail_lines_for(snap: Option<&Snapshot>, theme: &crate::theme::Theme) -> Vec<Line<'static>> {
    let Some(snap) = snap else {
        return vec![Line::from("no layer selected")];
    };
    let header = format!("{} {}  · {}", snap.layer.glyph(), snap.layer.label(), sentinel_chip(snap.sentinel));
    let last_ok = match &snap.status {
        LayerStatus::Pending => "—".to_string(),
        LayerStatus::Ok { last_ok_unix } => format_ago(*last_ok_unix),
        LayerStatus::Error { last_ok_unix: Some(t), .. } => format!("{} (last ok)", format_ago(*t)),
        LayerStatus::Error { last_ok_unix: None,    .. } => "never".to_string(),
    };
    let status_label = match &snap.status {
        LayerStatus::Pending      => "PENDING".to_string(),
        LayerStatus::Ok { .. }    => "OK".to_string(),
        LayerStatus::Error { .. } => "ERROR".to_string(),
    };
    let summary = if snap.summary.is_empty() { "(no summary yet)".to_string() } else { snap.summary.clone() };
    let mut lines = vec![
        Line::from(header),
        Line::from(format!("{status_label} · {last_ok}")),
        Line::from(""),
        Line::from(summary),
    ];
    if !snap.raw.is_null() {
        let raw = serde_json::to_string(&snap.raw)
            .unwrap_or_else(|_| "<unprintable>".into())
            .chars().take(160).collect::<String>();
        lines.push(Line::from(""));
        lines.push(Line::from(format!("raw: {raw}")));
    }
    if let LayerStatus::Error { reason, .. } = &snap.status {
        lines.push(Line::from(Span::styled(format!("error: {reason}"), theme.warn())));
    }
    lines
}

fn sentinel_chip(s: Sentinel) -> &'static str {
    match s {
        Sentinel::Green  => "GREEN",
        Sentinel::Yellow => "YELLOW",
        Sentinel::Red    => "RED",
    }
}

fn status_dot_char(status: &LayerStatus) -> &'static str {
    match status {
        LayerStatus::Pending      => "◌",
        LayerStatus::Ok { .. }    => "●",
        LayerStatus::Error { .. } => "✗",
    }
}

fn format_count(n: u64) -> String {
    if n < 1_000         { format!("{n}") }
    else if n < 1_000_000 { format!("{:.1}k", n as f64 / 1_000.0) }
    else                  { format!("{:.1}M", n as f64 / 1_000_000.0) }
}

fn format_ago(unix_secs: i64) -> String {
    let dt = chrono::Utc::now().timestamp() - unix_secs;
    if dt < 0    { "in the future".into() }
    else if dt < 60     { format!("{dt}s ago") }
    else if dt < 3600   { format!("{}m ago", dt / 60) }
    else if dt < 86_400 { format!("{}h ago", dt / 3600) }
    else                { ">1d".into() }
}

fn fixture_for(fixture: &[Snapshot], id: LayerId) -> Snapshot {
    fixture.iter().find(|s| s.layer == id).cloned().unwrap_or_else(|| Snapshot {
        layer:        id,
        status:       LayerStatus::Pending,
        sentinel:     Sentinel::Green,
        summary:      String::new(),
        entity_count: 0,
        raw:          serde_json::Value::Null,
    })
}

fn fallback_snapshots() -> Vec<Snapshot> {
    use cyberdeck_intel::Snapshot as S;
    let now = chrono::Utc::now().timestamp();
    vec![
        S { layer: LayerId::Flights,     status: LayerStatus::Ok { last_ok_unix: now - 12 },  sentinel: Sentinel::Green,  summary: "OpenSky · region=KSEA".into(), entity_count: 1284, raw: serde_json::Value::Null },
        S { layer: LayerId::Earthquakes, status: LayerStatus::Ok { last_ok_unix: now - 45 },  sentinel: Sentinel::Yellow, summary: "USGS · M4+ last 24h".into(),   entity_count: 17,   raw: serde_json::Value::Null },
        S { layer: LayerId::Fires,       status: LayerStatus::Ok { last_ok_unix: now - 180 }, sentinel: Sentinel::Red,    summary: "FIRMS · 1 intensity-4".into(),  entity_count: 243,  raw: serde_json::Value::Null },
        S { layer: LayerId::Weather,     status: LayerStatus::Pending,                        sentinel: Sentinel::Green,  summary: String::new(),                  entity_count: 0,    raw: serde_json::Value::Null },
        S { layer: LayerId::Satellites,  status: LayerStatus::Ok { last_ok_unix: now - 240 }, sentinel: Sentinel::Green,  summary: "CelesTrak · 7 visible".into(),  entity_count: 7,    raw: serde_json::Value::Null },
        S { layer: LayerId::News,        status: LayerStatus::Ok { last_ok_unix: now - 60 },  sentinel: Sentinel::Green,  summary: "GDELT · 39 mentions".into(),    entity_count: 39,   raw: serde_json::Value::Null },
        S { layer: LayerId::Cctv,        status: LayerStatus::Error { last_ok_unix: Some(now - 3600), reason: "timeout on stream 3".into() }, sentinel: Sentinel::Yellow, summary: "1 of 12 streams timed out".into(), entity_count: 12, raw: serde_json::Value::Null },
        S { layer: LayerId::Maritime,    status: LayerStatus::Ok { last_ok_unix: now - 600 }, sentinel: Sentinel::Green,  summary: "AIS Hub · 8 vessels".into(),    entity_count: 8,    raw: serde_json::Value::Null },
        S { layer: LayerId::Conflicts,   status: LayerStatus::Ok { last_ok_unix: now - 2400},  sentinel: Sentinel::Yellow, summary: "ACLED · 5 events 24h".into(),   entity_count: 5,    raw: serde_json::Value::Null },
    ]
}
