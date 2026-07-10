//! Cross-cutting widgets: header (live values), sidebar (screen list),
//! status bar (keymap hints + clock), toast overlay.

// herd-style palette — one struct, many named looks (Catppuccin Mocha
//! default, plus Gruvbox, Nord, and a legacy alias for the existing
//! dark theme). See `palette.rs` for the struct definition and named
//! lookups; the renderer will consume a `Palette` from `Settings`.
// M2 — the Phase-1 `menu_bar` module is deleted; Ctrl+M toggles the
// Overworld tile grid via `app.menu_active` instead.
pub mod palette;
pub mod tab_strip;
pub mod top_menu;

// Module 5.4 — sparkline for the header chip. Maps each sample to one of
// eight block glyphs `▁▂▃▄▅▆▇█`, scaled by the per-interface max so a
// quiet link still produces a visible ribbon. Returns `""` on empty
// input — the caller renders a dashed placeholder in that case so the
// chip is always the same width.
fn sparkline(samples: &[u64]) -> String {
    if samples.is_empty() {
        return String::new();
    }
    const RAMP: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    // `.max(&1)` keeps the divisor at least 1 — an all-zero history
    // would otherwise panic (or silently map every sample to ∞).
    let max = *samples.iter().max().unwrap_or(&1).max(&1);
    samples
        .iter()
        .map(|s| RAMP[((s * 7) / max).min(7) as usize])
        .collect()
}

/// Which interface the header sparkline tracks. Defaults to the first
/// interface with a non-empty IPv4 (matches the existing header pill),
/// falling back to `"lo"` if `app.live.interfaces` is locked or empty.
fn pick_active_iface_name(app: &App) -> String {
    if let Ok(ifaces) = app.live.interfaces.try_read() {
        if let Some(primary) = ifaces.iter().find(|i| !i.ipv4.is_empty()) {
            return primary.name.clone();
        }
        // No IPv4 — fall back to the first interface by name (often
        // `lo` when the system has no wired/wireless link up).
        if let Some(first) = ifaces.first() {
            return first.name.clone();
        }
    }
    "lo".to_string()
}

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, Region};
use crate::theme::{glyphs, Theme};

pub fn header_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let g = glyphs();
    let mut spans: Vec<Span<'static>> = vec![
        " cyberdeck ".into(),
        Span::styled(
            "▸ ",
            ratatui::style::Style::default().fg(Theme::by_name(crate::theme::ThemeName::Dark).dim),
        ),
    ];
    if let Ok(info) = app.live.info.try_read() {
        spans.push(format!("{} ", info.hostname).into());
        spans.push(Span::styled(
            "· ",
            ratatui::style::Style::default().fg(Theme::by_name(crate::theme::ThemeName::Dark).dim),
        ));
        spans.push(format!("{} ", info.os).into());
        spans.push(Span::styled(
            "· ",
            ratatui::style::Style::default().fg(Theme::by_name(crate::theme::ThemeName::Dark).dim),
        ));
        if let Ok(ssid) = app.live.active_ssid.try_read() {
            spans.push(format!("{} {} ", g.wifi, ssid.as_deref().unwrap_or("—")).into());
        }
        if let Ok(ifaces) = app.live.interfaces.try_read() {
            if let Some(primary) = ifaces.iter().find(|i| !i.ipv4.is_empty()) {
                spans.push(
                    format!(
                        "{} {} {} ",
                        g.net,
                        primary.name,
                        primary.ipv4.first().cloned().unwrap_or_default()
                    )
                    .into(),
                );
            }
        }
        // Module 5.4 — header sparkline chip. Pulls the last 8 RX
        // samples for the active interface and renders them as a
        // 8-glyph ribbon (`▁▂▃▄▅▆▇█`). All-zero history falls back to
        // a dashed placeholder so the chip is always 8 cells wide and
        // the line doesn't reflow on first paint. The sparkline is
        // appended regardless of whether `live.info` succeeded above
        // so the chip is visible even when sysinfo fetch fails (the
        // header is the user's primary glance — it should never go
        // blank just because a single source blipped).
        {
            let iface = pick_active_iface_name(app);
            let samples: Vec<u64> = app
                .net_history
                .get(&iface)
                .map(|(rx_ring, _)| {
                    rx_ring
                        .as_slice_chrono()
                        .into_iter()
                        .rev()
                        .take(8)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect()
                })
                .unwrap_or_default();
            let ribbon = if samples.is_empty() {
                "────────".to_string()
            } else {
                sparkline(&samples)
            };
            let label = format!(" ↓{} ", ribbon);
            spans.push(Span::styled(
                label,
                ratatui::style::Style::default().fg(theme.accent),
            ));
        }
        if let Ok(b) = app.live.battery.try_read() {
            if let Some(bat) = b.as_ref() {
                spans.push(format!("{} {}% ", g.bat, bat.capacity).into());
            }
        }
        if let Ok(th) = app.live.thermals.try_read() {
            if let Some(t) = th.first() {
                spans.push(format!("{} {:.0}°C ", g.temp, t.temp_c).into());
            }
        }
    }
    if let Ok(enabled) = app.live.web_enabled.try_read() {
        if *enabled {
            if let Ok(url) = app.live.web_url.try_read() {
                if let Some(u) = url.as_ref() {
                    spans.push(Span::styled(
                        format!(" web: {u} "),
                        ratatui::style::Style::default()
                            .fg(Theme::by_name(crate::theme::ThemeName::Dark).accent_2),
                    ));
                }
            }
        }
    }
    vec![Line::from(spans)]
}

/// Cyberdeck console header — single row of live status icons + clock.
/// Replaces the previous 2-row header that crammed every value in.
/// The icons are always glyphs so a glance reads as a console HUD, not
/// a wall of text. Right side shows the clock.
pub fn draw_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let g = glyphs();
    let dark = Theme::by_name(crate::theme::ThemeName::Dark);
    let mut spans: Vec<Span<'static>> = Vec::new();
    // Brand mark on the far left.
    spans.push(Span::styled(
        " ▸ CYBERDECK ",
        ratatui::style::Style::default()
            .fg(theme.accent)
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    // Live status icons. Each glyph = "this is wired up and healthy";
    // a dim placeholder = "no data yet".
    if let Ok(ssid) = app.live.active_ssid.try_read() {
        spans.push(Span::styled(
            format!(" {} ", ssid.as_deref().unwrap_or("—")),
            theme.fg,
        ));
    } else {
        spans.push(Span::styled(format!(" {} — ", g.wifi), theme.dim));
    }
    if let Ok(ifaces) = app.live.interfaces.try_read() {
        if let Some(primary) = ifaces.iter().find(|i| !i.ipv4.is_empty()) {
            spans.push(Span::styled(
                format!(" {} {} ", g.net, primary.ipv4.first().cloned().unwrap_or_default()),
                theme.fg,
            ));
        } else {
            spans.push(Span::styled(format!(" {} — ", g.net), theme.dim));
        }
    }
    if let Ok(b) = app.live.battery.try_read() {
        if let Some(bat) = b.as_ref() {
            spans.push(Span::styled(
                format!(" {} {}% ", g.bat, bat.capacity),
                theme.fg,
            ));
        }
    } else {
        spans.push(Span::styled(format!(" {} — ", g.bat), theme.dim));
    }
    if let Ok(th) = app.live.thermals.try_read() {
        if let Some(t) = th.first() {
            spans.push(Span::styled(
                format!(" {} {:.0}°C ", g.temp, t.temp_c),
                theme.fg,
            ));
        }
    }
    // Sparkline chip — keeps the header lively with a 1-character of
    // net-history info. The same Module 5.4 sparkline math; just
    // single-glyph so it fits the 1-row header.
    {
        let iface = pick_active_iface_name(app);
        let samples: Vec<u64> = app
            .net_history
            .get(&iface)
            .map(|(rx_ring, _)| {
                rx_ring
                    .as_slice_chrono()
                    .into_iter()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let ribbon = if samples.is_empty() {
            "────────".to_string()
        } else {
            sparkline(&samples)
        };
        spans.push(Span::styled(
            format!(" ↓{} ", ribbon),
            ratatui::style::Style::default().fg(theme.accent),
        ));
    }
    if let Ok(enabled) = app.live.web_enabled.try_read() {
        if *enabled {
            if let Ok(url) = app.live.web_url.try_read() {
                if let Some(u) = url.as_ref() {
                    spans.push(Span::styled(
                        format!(" web:{u} "),
                        ratatui::style::Style::default().fg(dark.accent_2),
                    ));
                }
            }
        }
    }
    // Clock on the far right.
    spans.push(Span::styled("  ", theme.dim));
    spans.push(Span::styled(
        app.clock.format("%H:%M:%S").to_string(),
        ratatui::style::Style::default()
            .fg(theme.accent)
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    let line = Line::from(spans);
    let p = Paragraph::new(line)
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
    f.render_widget(p, area);
}

/// Region indicator chip rendered on the right edge of the header.
/// Mirrors the sidebar focus gutter and the status bar label so all
/// three places tell the same story: "focus is here." On a 5" D-pad
/// display this is the single most-glanced indicator — the user looks
/// at the header to see *which* screen they're on AND *where* focus
/// sits inside it.
pub fn draw_region_chip(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // Region pill: bright filled block on the focused region, dim
    // outline for the unfocused ones. Three chips side by side so the
    // user sees the whole region topology at a glance.
    let labels = ["sidebar", "left", "right"];
    let active = match app.region {
        Region::Sidebar => 0,
        Region::ContentLeft => 1,
        Region::ContentRight => 2,
    };
    let n = labels.len() as u16;
    if area.width < n * 6 {
        return; // not enough room — header is too narrow to host the chip
    }
    let cell_w = area.width / n;
    for (i, label) in labels.iter().enumerate() {
        let x = area.x + i as u16 * cell_w;
        let cell = Rect::new(x, area.y, cell_w, area.height);
        let is_active = i == active;
        let style = if is_active {
            ratatui::style::Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            ratatui::style::Style::default().fg(theme.dim)
        };
        let text = if is_active {
            format!(" ▶ {} ", label)
        } else {
            format!("   {} ", label)
        };
        let p = Paragraph::new(Line::from(Span::styled(text, style)))
            .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
        f.render_widget(p, cell);
    }
}

/// Draw the redesigned sidebar: a numbered grid of 13 tiles. Replaces the
/// old cramped 24-col list strip with a layout that suits a 5" D-pad
/// display — every screen gets its own row, with the cursor row in a
/// filled highlight that survives glances and the *active* screen
/// ringed in the accent colour so the user always sees what's open.
/// Falls back gracefully on narrow terminals (≤ 28 cols) by collapsing
/// to a one-column list so a uconsole in landscape still works.

pub fn draw_status(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // Region label tells the user where focus is. With a 5" D-pad display
    // the focus cursor on the screen itself is small, so the status bar
    // has to spell out the active region in plain English. Uses the
    // same ▶ vocabulary as the header chip so header / sidebar / status
    // bar all read in the same visual language.
    let region_label = match app.region {
        Region::Sidebar => Span::styled(" ▶ sidebar ", theme.title()),
        Region::ContentLeft => Span::styled(" content ▶ left ", theme.title()),
        Region::ContentRight => Span::styled(" content ▶ right ", theme.title()),
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(region_label);

    // Region-conditional hints. These are the only navigation verbs
    // available at this region; everything else is screen-specific and
    // already shown inside the screen body.
    spans.push(Span::styled(" │ ", theme.dim()));
    match app.region {
        Region::Sidebar => {
            spans.push(Span::styled(" ↑↓ ", theme.key()));
            spans.push(Span::styled("move ", theme.dim()));
            spans.push(Span::styled(" 1-9 ", theme.key()));
            spans.push(Span::styled("jump ", theme.dim()));
            spans.push(Span::styled(" →/l ", theme.key()));
            spans.push(Span::styled("enter ", theme.dim()));
        }
        Region::ContentLeft | Region::ContentRight => {
            spans.push(Span::styled(" ←/h ", theme.key()));
            spans.push(Span::styled("sidebar ", theme.dim()));
            spans.push(Span::styled(" →/l ", theme.key()));
            if app.current.has_right_pane() {
                spans.push(Span::styled("other pane ", theme.dim()));
            } else {
                spans.push(Span::styled("forward ", theme.dim()));
            }
            spans.push(Span::styled(" tab ", theme.key()));
            spans.push(Span::styled("switch screen ", theme.dim()));
        }
    }
    spans.push(Span::styled(" │ ", theme.dim()));
    spans.push(Span::styled(" : ", theme.key()));
    spans.push(Span::styled("palette ", theme.dim()));
    spans.push(Span::styled(" ? ", theme.key()));
    spans.push(Span::styled("help ", theme.dim()));
    spans.push(Span::styled(" q ", theme.key()));
    spans.push(Span::styled("quit ", theme.dim()));

    // M5 — Intel footer chip. Shows `intel: N/M layers live · K
    // {SENTINEL}` so the user can glance at the status bar and
    // know whether the refiller is healthy without opening the
    // Intel screen. The chip is right-aligned to the clock and
    // dimmed when no live data has landed yet (everything still
    // `Pending`). The sentinel color comes from the theme so a
    // RED rollup pops even from the corner of the eye.
    {
        let total = cyberdeck_intel::LayerId::ALL.len() as u64;
        let live = app
            .intel_snapshots
            .values()
            .filter(|s| matches!(s.status, cyberdeck_intel::LayerStatus::Ok { .. }))
            .count() as u64;
        let sentinel_style = match app.intel_worst {
            cyberdeck_intel::Sentinel::Red => theme.error(),
            cyberdeck_intel::Sentinel::Yellow => theme.warn(),
            cyberdeck_intel::Sentinel::Green => theme.ok(),
        };
        let chip = format!(
            " intel: {}/{} live · {} ",
            live,
            total,
            app.intel_worst.short(),
        );
        spans.push(Span::styled("  ", theme.dim()));
        spans.push(Span::styled(chip, sentinel_style));
    }

    // Right side: clock.
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        app.clock.format("%H:%M:%S").to_string(),
        theme.accent,
    ));

    let line = Line::from(spans);
    let p = Paragraph::new(line)
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(theme.border(false)),
        );
    f.render_widget(p, area);
}

// M1 (menu revamp) — `draw_launcher` deleted. The launcher pane was a
// 4×4 (or 2×8 on narrow terminals) grid of glyph+number+label tiles
// the user could `B`/Esc into and arrow-key around. The menu-revamp
// redesign replaces it with the Overworld screen as the single global
// menu: `main.rs::draw` now renders `OverworldScreen` whenever
// `app.region == Region::Sidebar`. M3 deletes the `Region::Sidebar`
// variant entirely, at which point this comment goes away too.


/// Button-legend strip — single row at the very bottom of the screen.
/// Shows the canonical X / Y / A / B button labels. Reads the focused
/// region so the hint is contextual.
pub fn draw_button_legend(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.region;
    let (xb, xa, yr, ya, sb, sa): (&str, &str, &str, &str, &str, &str) = match focused {
        Region::Sidebar => (
            "navigate", "[↑↓←→]",
            "help", "[Y]",
            "settings", "[Tab]",
        ),
        Region::ContentLeft | Region::ContentRight => {
            let screen_hint = app.current_button_hint.as_deref().unwrap_or("");
            if !screen_hint.is_empty() {
                ("select", "[A]", "primary", "[B]", "menu", screen_hint)
            } else {
                ("select", "[A]", "back", "[B]", "menu", "[Tab]")
            }
        }
    };
    let line = Line::from(vec![
        Span::styled(format!(" ◀ {xb} "), theme.dim),
        Span::styled(xa, ratatui::style::Style::default().fg(theme.accent).add_modifier(ratatui::style::Modifier::BOLD)),
        Span::styled(format!("   △ {yr} "), theme.dim),
        Span::styled(ya, ratatui::style::Style::default().fg(theme.accent).add_modifier(ratatui::style::Modifier::BOLD)),
        Span::styled(format!("   ○ {sb} "), theme.dim),
        Span::styled(sa, ratatui::style::Style::default().fg(theme.accent).add_modifier(ratatui::style::Modifier::BOLD)),
        Span::styled("   ◍ start ", theme.dim),
        Span::styled("[?]", ratatui::style::Style::default().fg(theme.dim).add_modifier(ratatui::style::Modifier::BOLD)),
        Span::styled("   ⌃M menu ", theme.dim),
    ]);
    let p = Paragraph::new(line)
        .alignment(ratatui::layout::Alignment::Center)
        .style(ratatui::style::Style::default().bg(theme.bg).fg(theme.fg));
    f.render_widget(p, area);
}

pub fn draw_toasts(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.toasts.is_empty() {
        return;
    }
    let w = (area.width as i32 - 4).max(8) as u16;
    let h = app.toasts.len() as u16;
    let x = area.x + (area.width.saturating_sub(w + 2)) / 2;
    let y = area.y + area.height.saturating_sub(h + 2);
    let rect = Rect::new(x, y, w + 2, h + 2);
    let items: Vec<ListItem> = app
        .toasts
        .iter()
        .map(|t| {
            let style = match t.kind {
                crate::app::toast::ToastKind::Info => theme.dim(),
                crate::app::toast::ToastKind::Ok => theme.ok(),
                crate::app::toast::ToastKind::Warn => theme.warn(),
                crate::app::toast::ToastKind::Error => theme.error(),
            };
            ListItem::new(Line::from(Span::styled(t.text.clone(), style)))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(false))
            .title(Span::styled(" notifications ", theme.title())),
    );
    f.render_widget(list, rect);
}

// Fix #2a — Cyberdeck console layout. Three rows:
//   * header   (1 row) — live status icons + clock
//   * body     (flex)  — the launcher grid OR the focused screen
//   * legend   (1 row) — on-screen button legend (X/Y/A/B + Start/Select)
// The previous five-row chrome (header/menu_bar/tab_strip/body/status)
// consumed ~6 rows on a 32-row terminal and made the screen list feel
// cramped. The new layout gives the body 30+ rows on the same terminal.
// Four rows, top-to-bottom:
//   * header   (1 row)             — live status icons + clock
//   * tab_strip (1 row, optional)  — Bruce-firmware menu-name strip with
//                                    preview-cursor highlight (Tab moves
//                                    the cursor, Enter commits, Esc cancels)
//   * body     (flex)              — the launcher grid OR the focused screen
//   * legend   (1 row)             — on-screen button legend
// The tab_strip reuses one of the body's flex rows when the terminal is
// too short (height < 10) so the focused screen never gets crushed.
// Anything that needs to know "did the strip actually render this frame"
// reads `tab_strip_rect` on the returned tuple.
pub fn chunks(area: Rect) -> (Rect, Option<Rect>, Rect, Rect) {
    let strip_visible = area.height >= 10 && area.width > 80;
    let outer = if strip_visible {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // tab_strip
                Constraint::Min(8),    // body
                Constraint::Length(1), // legend
            ])
            .split(area)
    } else {
        // Header / body / legend only — the strip row collapses away.
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(8),
                Constraint::Length(1),
            ])
            .split(area)
    };
    if strip_visible {
        let header = outer[0];
        let strip = outer[1];
        let body = outer[2];
        let legend = outer[3];
        (header, Some(strip), body, legend)
    } else {
        let header = outer[0];
        let body = outer[1];
        let legend = outer[2];
        (header, None, body, legend)
    }
}

#[cfg(test)]
mod status_region_vocabulary {
    //! Pin the status-bar region-label vocabulary to ▶ so it always
    //! matches the header chip introduced in `ee1b197`. The header chip
    //! and the status-bar `region_label` arm must read in the same
    //! visual language; if a future revert reintroduces `← content · left`
    //! or `content · right →`, these tests fail.
    //!
    //! Tests use `TestBackend` + buffer assertion, the same pattern as
    //! `crates/tui/src/screens/services.rs::offset_tests::render_clips_to_offset`.
    use super::*;
    use crate::app::{App, Region};
    use crate::theme::{Theme, ThemeName};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tokio::sync::mpsc;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer().clone();
        let mut rows: Vec<String> = Vec::new();
        for y in 0..buffer.area.height {
            let mut row = String::new();
            for x in 0..buffer.area.width {
                row.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            rows.push(row);
        }
        rows.join("\n")
    }

    fn fresh_app() -> App {
        let (tx, rx) = mpsc::channel::<crate::app::Action>(8);
        App::new(tx, rx)
    }

    fn render_status_with(region: Region) -> (String, String) {
        let backend = TestBackend::new(80, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.region = region;
        let area = terminal.backend().buffer().area;
        let theme = Theme::by_name(ThemeName::Dark);
        terminal
            .draw(|f| draw_status(f, area, &app, &theme))
            .unwrap();
        let full = buffer_text(&terminal);
        // The first non-empty row of the status bar starts with the
        // `region_label` Span. Slice off the trailing hint strip so the
        // assertion only governs the label, not legitimate `←/h` / `→/l`
        // hint keys in the hint strip.
        let label = full
            .lines()
            .filter(|r| !r.trim().is_empty())
            .next_back()
            .unwrap_or("")
            .split('│')
            .next()
            .unwrap_or("")
            .to_string();
        (full, label)
    }

    #[test]
    fn sidebar_uses_triangle_vocabulary() {
        let (_, label) = render_status_with(Region::Sidebar);
        assert!(
            label.contains('▶'),
            "sidebar region_label must contain ▶; got label slice: {:?}",
            label
        );
        assert!(
            label.contains("sidebar"),
            "sidebar region_label must contain 'sidebar'; got: {:?}",
            label
        );
        assert!(!label.contains("←"), "old ← form must not appear in label; got: {:?}", label);
        assert!(!label.contains("→"), "old → form must not appear in label; got: {:?}", label);
    }

    #[test]
    fn content_left_uses_triangle_vocabulary() {
        let (_, label) = render_status_with(Region::ContentLeft);
        assert!(label.contains('▶'), "left region_label must contain ▶; got: {:?}", label);
        assert!(label.contains("left"), "left region_label must contain 'left'; got: {:?}", label);
        assert!(!label.contains("←"), "old ← form must not appear in label; got: {:?}", label);
        assert!(!label.contains("→"), "old → form must not appear in label; got: {:?}", label);
    }

    #[test]
    fn content_right_uses_triangle_vocabulary() {
        let (_, label) = render_status_with(Region::ContentRight);
        assert!(label.contains('▶'), "right region_label must contain ▶; got: {:?}", label);
        assert!(label.contains("right"), "right region_label must contain 'right'; got: {:?}", label);
        assert!(!label.contains("←"), "old ← form must not appear in label; got: {:?}", label);
        assert!(!label.contains("→"), "old → form must not appear in label; got: {:?}", label);
    }
}

#[cfg(test)]
mod sparkline_tests {
    //! Module 5.4 — pin the sparkline math so a future rework (e.g.
    //! switching to a base64-encoded buffer glyph, or adding a
    //! logarithmic ramp) doesn't silently shift the chip's appearance.
    use super::sparkline;

    #[test]
    fn sparkline_returns_eight_chars_for_eight_samples() {
        // The chip renders up to 8 trailing samples; the helper must
        // emit one glyph per sample.
        let s = sparkline(&[0, 100, 200, 300, 400, 500, 600, 700]);
        assert_eq!(s.chars().count(), 8);
    }

    #[test]
    fn sparkline_picks_lower_block_for_smaller_values() {
        // The ramp is monotonic: lower samples → lower glyph index.
        // Specifically the lowest glyph (`▁`) covers `[0, max/8)` so
        // the first of an ascending ramp must be `▁`, the last must
        // be `█`.
        let s = sparkline(&[0, 100, 200, 300, 400, 500, 600, 700]);
        assert!(
            s.starts_with('▁'),
            "first sample (0/max) must be the lowest glyph ▁; got {:?}",
            s
        );
        assert!(
            s.ends_with('█'),
            "last sample (max/max) must be the top glyph █; got {:?}",
            s
        );
        // Every char must come from the ramp.
        for c in s.chars() {
            assert!(
                "▁▂▃▄▅▆▇█".contains(c),
                "unexpected glyph {:?} in sparkline",
                c
            );
        }
    }

    #[test]
    fn sparkline_all_zeros_returns_lowest_glyph() {
        // All-zero history must not panic — we clamp via `.max(&1)`
        // so divisor is at least 1 and every sample lands in
        // bucket 0.
        let s = sparkline(&[0, 0, 0, 0]);
        assert_eq!(s.chars().count(), 4);
        assert!(
            s.chars().all(|c| c == '▁'),
            "all zeros must render as ▁▁▁▁; got {:?}",
            s
        );
    }

    #[test]
    fn sparkline_empty_returns_empty_string() {
        // The render code falls back to a dashed placeholder on
        // empty input; the helper itself returns `""` so callers
        // can distinguish "no data" from "all zeros".
        assert_eq!(sparkline(&[]), "");
        assert_eq!(sparkline(&[]).chars().count(), 0);
    }
}
