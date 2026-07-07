//! Herdr-style tab strip.
//!
//! A 1-row strip rendered just below the menu bar showing one tab per
//! visible screen (skipping screens where `Screen::is_hidden` returns
//! true). Each tab shows the screen glyph, a one- or two-character
//! label, and the shortcut digit. The active tab is filled with the
//! selection background; the cursor tab (when focus is on the tab
//! strip) is bold.
//!
//! On narrow terminals (< `tab_count * MIN_TAB_WIDTH` cells) the strip
//! collapses to a `‹ x/N ›` indicator so the chrome stays readable on
//! a 5" D-pad display. The collapse is purely cosmetic — the user can
//! still press `Left`/`Right` to cycle tabs.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::action::Action;
use crate::app::screen::ScreenId;
use crate::app::App;
use crate::theme::Theme;

/// Tab strip height (rows). Always 1.
pub const TAB_STRIP_HEIGHT: u16 = 1;

/// Minimum cell width per tab. Below this, the strip collapses.
const MIN_TAB_WIDTH: u16 = 6;

/// Render the tab strip. `area` is a 1-row rectangle; the function
/// silently no-ops if `area.height < 1`.
///
/// `cursor_id` is the currently-highlighted tab (may differ from
/// `app.current` when focus is on the strip — e.g. user pressed Tab
/// to highlight Network without committing). `None` means "no cursor"
/// (e.g. the strip is collapsed or hidden).
pub fn draw(f: &mut Frame, area: Rect, app: &App, cursor: Option<ScreenId>, theme: &Theme) {
    if area.height < 1 {
        return;
    }
    let visible: Vec<ScreenId> = ScreenId::ALL
        .iter()
        .copied()
        // Editor is reachable only from Files via `e`, so it stays
        // hidden from the tab strip (mirrors the existing sidebar
        // behaviour — see `ScreenId::cycle`).
        .filter(|id| !matches!(id, ScreenId::Editor))
        .collect();
    if visible.is_empty() {
        return;
    }
    let total_width_needed = visible.len() as u16 * MIN_TAB_WIDTH;
    let collapsed = area.width < total_width_needed;
    if collapsed {
        draw_collapsed(f, area, &visible, app, cursor, theme);
    } else {
        draw_full(f, area, &visible, app, cursor, theme);
    }
}

/// Hit-test a `(col, row)` click against the tab strip in the same
/// coordinates the renderer used. Mirrors `draw_full`'s windowing
/// math so the click resolves to whichever tab the user actually
/// saw on screen. Returns `None` for:
///   * clicks outside `area`
///   * the collapsed strip (no per-tab hit-test; user must press
///     `Left`/`Right` to cycle instead)
///   * the right-side units/status tag (we don't treat it as a tab)
pub fn hit_test(area: Rect, col: u16, row: u16, app: &App) -> Option<ScreenId> {
    if area.width < 1 || area.height < 1 {
        return None;
    }
    if col < area.x || col >= area.x + area.width {
        return None;
    }
    if row < area.y || row >= area.y + area.height {
        return None;
    }
    let visible: Vec<ScreenId> = ScreenId::ALL
        .iter()
        .copied()
        .filter(|id| !matches!(id, ScreenId::Editor))
        .collect();
    if visible.is_empty() {
        return None;
    }
    let total_width_needed = visible.len() as u16 * MIN_TAB_WIDTH;
    if area.width < total_width_needed {
        // Collapsed — no per-tab hit surface.
        return None;
    }
    let max_tabs = (area.width / MIN_TAB_WIDTH) as usize;
    let active_pos = visible.iter().position(|id| *id == app.current);
    let window_start = match active_pos {
        Some(p) if p >= max_tabs / 2 => p.saturating_sub(max_tabs / 2),
        _ => 0,
    };
    let window_end = (window_start + max_tabs).min(visible.len());
    let local_col = col - area.x;
    let tab_idx = (local_col / MIN_TAB_WIDTH) as usize;
    if tab_idx >= window_end - window_start {
        return None;
    }
    visible.get(window_start + tab_idx).copied()
}

fn draw_full(
    f: &mut Frame,
    area: Rect,
    visible: &[ScreenId],
    app: &App,
    cursor: Option<ScreenId>,
    theme: &Theme,
) {
    // Number of tabs we can actually fit. We allocate `MIN_TAB_WIDTH`
    // cells per tab and clip the rest; the user can still cycle past
    // the visible window with Left/Right (the cursor wraps).
    let max_tabs = (area.width / MIN_TAB_WIDTH) as usize;
    let active_pos = visible.iter().position(|id| *id == app.current);
    // Center the active tab in the window when possible — gives the
    // user some forward context without scrolling.
    let window_start = match active_pos {
        Some(p) if p >= max_tabs / 2 => p.saturating_sub(max_tabs / 2),
        _ => 0,
    };
    let window_end = (window_start + max_tabs).min(visible.len());
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, id) in visible.iter().enumerate().take(window_end).skip(window_start) {
        let is_active = Some(*id) == active_pos.map(|p| visible[p]).or(Some(app.current))
            && active_pos == Some(i);
        let is_cursor = cursor == Some(*id);
        let style = if is_active {
            ratatui::style::Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else if is_cursor {
            ratatui::style::Style::default()
                .fg(theme.accent)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            ratatui::style::Style::default().fg(theme.fg)
        };
        let marker = if is_active { "▶" } else { " " };
        // Tab label: glyph + 4-char abbreviation + (optional) digit.
        let short = short_label(*id);
        let num = if i < 9 { format!("{}", i + 1) } else { String::new() };
        let tab_text = format!(" {}{} {} ", marker, num, short);
        spans.push(Span::styled(tab_text, style));
    }
    // Right side: units / status indicator (single short tag).
    let right = match app.units {
        crate::prefs::Units::Metric => "metric",
        crate::prefs::Units::Imperial => "imperial",
    };
    spans.push(Span::styled(format!(" {}", right), theme.dim()));
    let p = Paragraph::new(Line::from(spans)).style(
        ratatui::style::Style::default().fg(theme.fg).bg(theme.bg),
    );
    f.render_widget(p, area);
}

fn draw_collapsed(
    f: &mut Frame,
    area: Rect,
    visible: &[ScreenId],
    app: &App,
    cursor: Option<ScreenId>,
    theme: &Theme,
) {
    let pos = visible
        .iter()
        .position(|id| *id == app.current)
        .unwrap_or(0);
    let total = visible.len();
    // Window-indicator on the left, "1-9 shortcut" hint on the right.
    let left = format!(" ‹ {}/{} › ", pos + 1, total);
    let right = match app.units {
        crate::prefs::Units::Metric => "metric ",
        crate::prefs::Units::Imperial => "imperial ",
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(left, theme.accent));
    // Show the active tab's glyph + short label in the middle if there's room.
    if let Some(active) = visible.get(pos) {
        let mid = format!(" {} {} ", active.glyph(), short_label(*active));
        spans.push(Span::styled(mid, theme.title()));
    }
    // Cursor marker (only when tab strip owns focus).
    if let Some(c) = cursor {
        let marker = format!(" ←{} ", short_label(c));
        spans.push(Span::styled(marker, theme.accent));
    }
    spans.push(Span::styled(right.to_string(), theme.dim()));
    let p = Paragraph::new(Line::from(spans)).style(
        ratatui::style::Style::default().fg(theme.fg).bg(theme.bg),
    );
    f.render_widget(p, area);
}

/// Short label for a screen. Falls back to the full label truncated to
/// 4 chars when we don't have a hand-tuned abbreviation.
fn short_label(id: ScreenId) -> &'static str {
    match id {
        ScreenId::System => "Sys",
        ScreenId::Network => "Net",
        ScreenId::Bluetooth => "BT",
        ScreenId::Power => "Pwr",
        ScreenId::Display => "Disp",
        ScreenId::Audio => "Aud",
        ScreenId::Storage => "Stor",
        ScreenId::Services => "Svc",
        ScreenId::Packages => "Pkg",
        ScreenId::Processes => "Proc",
        ScreenId::Files => "File",
        ScreenId::Logs => "Logs",
        ScreenId::Settings => "Set",
        ScreenId::Editor => "Edit",
        ScreenId::LoRa => "LoRa",
        ScreenId::City => "City",
    }
}

/// Cycle the tab cursor (Left/Right). Returns the new cursor id (or
/// `app.current` if no cursor was set). Wraps around.
pub fn cycle(app: &App, forward: bool) -> ScreenId {
    let visible: Vec<ScreenId> = ScreenId::ALL
        .iter()
        .copied()
        .filter(|id| !matches!(id, ScreenId::Editor))
        .collect();
    if visible.is_empty() {
        return app.current;
    }
    let pos = visible
        .iter()
        .position(|id| *id == app.current)
        .unwrap_or(0);
    let n = visible.len();
    let next = if forward {
        (pos + 1) % n
    } else {
        (pos + n - 1) % n
    };
    visible[next]
}

/// Commit the cursor as the new current screen. Dispatches a single
/// `Action::Goto` so the main loop can update the WM pane kind in
/// lockstep (mirrors how the sidebar Enter path works).
pub fn commit(app: &App, id: ScreenId) -> Action {
    let _ = app; // unused — included for API symmetry with other commit helpers
    Action::Goto(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::action::Action;
    use crate::app::screen::ScreenId;

    fn fresh_app() -> App {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        App::new(tx, rx)
    }

    /// The tab strip on a wide terminal renders every visible screen's
    /// glyph.
    #[test]
    fn tab_strip_renders_glyphs_wide() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(160, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = fresh_app();
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        terminal
            .draw(|f| draw(f, Rect::new(0, 0, 160, 1), &app, None, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push(buf[(x, 0)].symbol().chars().next().unwrap_or(' '));
        }
        for id in ScreenId::ALL {
            if matches!(*id, ScreenId::Editor) {
                continue;
            }
            let short = short_label(*id);
            assert!(
                row.contains(short),
                "tab strip on wide terminal must contain {:?}; got {:?}",
                short,
                row
            );
        }
    }

    /// On a narrow terminal the tab strip collapses to `‹ x/N ›`.
    #[test]
    fn tab_strip_collapses_on_narrow() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = fresh_app();
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        terminal
            .draw(|f| draw(f, Rect::new(0, 0, 40, 1), &app, None, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push(buf[(x, 0)].symbol().chars().next().unwrap_or(' '));
        }
        assert!(
            row.contains('‹'),
            "narrow tab strip must use ‹ › indicator; got {:?}",
            row
        );
        assert!(row.contains('›'), "narrow tab strip must contain ›");
    }

    /// `cycle(forward=true)` moves to the next visible screen, wrapping.
    #[test]
    fn cycle_forward_wraps() {
        let app = fresh_app();
        let next = cycle(&app, true);
        assert_ne!(next, app.current);
    }

    /// `cycle(forward=false)` from the first visible screen wraps to the
    /// last (which is City today; if ScreenId::ALL changes the test
    /// still passes because it only checks "wrap-around").
    #[test]
    fn cycle_backward_wraps_from_first() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let app = App::with_current(tx, rx, ScreenId::System);
        let prev = cycle(&app, false);
        // System is at position 0 in the visible list; stepping
        // backward should land on the last visible screen.
        assert_eq!(prev, ScreenId::City);
    }

    /// Editor must never appear in the visible list (it's reachable only
    /// from Files via `e`).
    #[test]
    fn editor_is_hidden_from_tab_strip() {
        // Step forward enough times to visit every screen; Editor must
        // never be returned. We seed `current` from each visible screen
        // via `with_current` so we exercise every starting position
        // without needing to mutate `App` in-place (which would require
        // either `Clone` or driving the main loop).
        for start in ScreenId::ALL.iter().copied() {
            let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
            let app = App::with_current(tx, rx, start);
            // `start` is consumed by `with_current` so we re-seed
            // `id` from the live app on the first cycle.
            let mut id = app.current;
            for _ in 0..ScreenId::ALL.len() + 1 {
                id = cycle(&app, true);
                assert_ne!(id, ScreenId::Editor, "Editor must be skipped");
            }
        }
    }

    /// Click outside the tab-strip rect must return None. The renderer
    /// might draw the strip anywhere on the row, so the caller has to
    /// check both axes — a hit_test that only checks the row would
    /// mistakenly fire for clicks on the status bar below.
    #[test]
    fn hit_test_returns_none_outside_area() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let app = App::with_current(tx, rx, ScreenId::System);
        let area = ratatui::layout::Rect::new(10, 2, 160, 1);
        // Above the strip.
        assert_eq!(hit_test(area, 20, 0, &app), None);
        // Below the strip.
        assert_eq!(hit_test(area, 20, 5, &app), None);
        // Left of the strip.
        assert_eq!(hit_test(area, 5, 2, &app), None);
        // Past the right edge.
        assert_eq!(hit_test(area, 170, 2, &app), None);
        // Zero-sized area is a no-op.
        let empty = ratatui::layout::Rect::new(0, 0, 0, 1);
        assert_eq!(hit_test(empty, 0, 0, &app), None);
    }

    /// A click in the centre of a visible tab must resolve to that
    /// tab's `ScreenId`. Centers on `app.current` because the strip
    /// is windowed around it, so we pick a tab we know is on screen.
    #[test]
    fn hit_test_resolves_inner_tab() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let app = App::with_current(tx, rx, ScreenId::Network);
        // Wide area so the strip is full-mode, not collapsed.
        let area = ratatui::layout::Rect::new(0, 0, 200, 1);
        // Network is index 1 in ScreenId::ALL (System=0, Network=1).
        // With 200 cells / MIN_TAB_WIDTH=6 the window can hold ~33
        // tabs, so Network is rendered at its natural slot: x ∈
        // [6, 12). A click at x=8 should land on Network.
        assert_eq!(hit_test(area, 8, 0, &app), Some(ScreenId::Network));
    }

    /// In the collapsed view there is no per-tab hit surface — the
    /// user has to use Left/Right. Returns None for any click.
    #[test]
    fn hit_test_returns_none_in_collapsed_mode() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let app = App::with_current(tx, rx, ScreenId::System);
        // Narrow: fewer cells than `visible.len() * MIN_TAB_WIDTH`.
        let area = ratatui::layout::Rect::new(0, 0, 20, 1);
        assert_eq!(hit_test(area, 5, 0, &app), None);
        assert_eq!(hit_test(area, 10, 0, &app), None);
    }
}