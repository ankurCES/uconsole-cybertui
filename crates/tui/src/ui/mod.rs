//! Cross-cutting widgets: header (live values), sidebar (screen list),
//! status bar (keymap hints + clock), toast overlay.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::screen::ScreenId;
use crate::app::{App, Region};
use crate::theme::{glyphs, Theme};

pub fn header_lines(app: &App) -> Vec<Line<'static>> {
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

pub fn draw_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let p = Paragraph::new(header_lines(app))
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(theme.border(false)),
        );
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
pub fn draw_sidebar(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = matches!(app.region, Region::Sidebar);
    let narrow = area.width < 28;
    if narrow {
        draw_sidebar_narrow(f, area, app, theme, focused);
    } else {
        draw_sidebar_grid(f, area, app, theme, focused);
    }
}

fn draw_sidebar_narrow(f: &mut Frame, area: Rect, app: &App, theme: &Theme, focused: bool) {
    // One row per screen. Falls back to the pre-redesign list so users
    // on narrow terminals still get a working menu.
    let items: Vec<ListItem> = ScreenId::ALL
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let active = *id == app.current;
            let cursor = i == app.sidebar_idx;
            let prefix = if active { g().arrow } else { " " };
            let num = format!("{:>2}", i + 1);
            let (prefix_style, label_style) = sidebar_item_styles(active, cursor, theme);
            Line::from(vec![
                Span::styled(format!("{prefix} "), prefix_style),
                Span::styled(format!("{num} "), theme.dim()),
                Span::styled(format!("{} ", id.glyph()), theme.accent),
                Span::styled(id.label().to_string(), label_style),
            ])
            .into()
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .title(Span::styled(
                    if focused { " ▶ screens " } else { " screens " },
                    theme.title(),
                ))
                .borders(Borders::ALL)
                .border_style(theme.border(focused)),
        )
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
    f.render_widget(list, area);

    // Focus gutter on the inner right edge, mirroring the grid variant
    // above. Filled cyan when the sidebar owns the region, dim accent
    // when content is focused. The user should always be able to glance
    // at the sidebar and read "focus is here" from the gutter alone.
    if focused {
        let inner = Rect::new(area.x + area.width - 1, area.y + 1, 1, area.height - 2);
        if inner.height > 0 {
            let marker = Paragraph::new("".repeat(inner.height as usize))
                .style(
                    ratatui::style::Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg),
                );
            f.render_widget(marker, inner);
        }
    }
}

/// Wide-mode sidebar: two columns of numbered tiles. Each tile shows the
/// key number, the screen glyph, and the label. The focused cursor tile
/// gets the cyan selection block; the active screen tile gets a bold
/// accent border so what's open is unmistakable. Anything else is dim
/// so the eye lands on the cursor first, then the active marker.
fn draw_sidebar_grid(f: &mut Frame, area: Rect, app: &App, theme: &Theme, focused: bool) {
    let block = Block::default()
        .title(Span::styled(
            if focused { " ▶ screens " } else { " screens " },
            theme.title(),
        ))
        .borders(Borders::ALL)
        .border_style(theme.border(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // 13 screens → 7 rows on the left, 6 rows on the right. Two-column
    // grid keeps the cursor within thumb-reach for D-pad use: at most 7
    // `↓` presses from top to the last row, never a long scroll.
    let rows = ScreenId::ALL.len().div_ceil(2);
    let row_constraints: Vec<Constraint> =
        (0..rows).map(|_| Constraint::Length(1)).collect();
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(inner);

    for (i, id) in ScreenId::ALL.iter().enumerate() {
        let col = i / rows;
        let row = i % rows;
        let row_area = row_areas.get(row).copied();
        let Some(row_area) = row_area else { continue; };
        // First column half / second column half. Half-rows could be
        // off-by-one when 13 isn't even — clamp `mid` to the inner width.
        let mid = inner.width / 2;
        let cell_area = if col == 0 {
            Rect::new(inner.x, row_area.y, mid, 1)
        } else {
            Rect::new(inner.x + mid, row_area.y, inner.width - mid, 1)
        };
        let active = *id == app.current;
        let cursor = i == app.sidebar_idx;
        render_sidebar_cell(f, cell_area, i + 1, id, active, cursor, theme);
    }

    // Focus gutter: a 1-cell-wide vertical bar along the sidebar's right
    // border. Lit cyan when the sidebar owns the region focus (so the
    // cursor is *here*), dim accent when content is focused (so the user
    // can see at a glance "focus is on the right"). This is the single
    // most important D-pad affordance on a 5" display where the cursor
    // itself is small: the gutter is always visible regardless of which
    // row the cursor sits on.
    if inner.width >= 2 && rows >= 1 {
        let gutter_x = area.x + area.width.saturating_sub(2);
        let gutter_style = if focused {
            ratatui::style::Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
        } else {
            ratatui::style::Style::default().fg(theme.dim)
        };
        for row_area in row_areas.iter() {
            let gutter = Rect::new(gutter_x, row_area.y, 1, 1);
            let marker = Paragraph::new(Line::from(Span::styled("│", gutter_style)));
            f.render_widget(marker, gutter);
        }
    }
}

/// Paint a single sidebar tile. Cursor wins over active wins over dim.
fn render_sidebar_cell(
    f: &mut Frame,
    area: Rect,
    n: usize,
    id: &ScreenId,
    active: bool,
    cursor: bool,
    theme: &Theme,
) {
    if area.width < 6 {
        return;
    }
    let num = format!("{:>2}", n);
    let glyph = id.glyph();
    let label = id.label();
    let style = if cursor {
        ratatui::style::Style::default()
            .fg(theme.selection_fg)
            .bg(theme.selection_bg)
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else if active {
        ratatui::style::Style::default()
            .fg(theme.accent)
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        ratatui::style::Style::default().fg(theme.dim)
    };
    let glyph_style = if cursor {
        ratatui::style::Style::default().fg(theme.selection_fg).bg(theme.selection_bg)
    } else {
        ratatui::style::Style::default().fg(theme.accent)
    };
    let label_style = if cursor {
        ratatui::style::Style::default().fg(theme.selection_fg).bg(theme.selection_bg)
    } else if active {
        ratatui::style::Style::default().fg(theme.fg).add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        ratatui::style::Style::default().fg(theme.fg)
    };
    let cursor_marker = if cursor { g().arrow } else { " " };
    let line = Line::from(vec![
        Span::styled(format!("{cursor_marker}"), style),
        Span::styled(format!(" {num} "), style),
        Span::styled(format!("{glyph}"), glyph_style),
        Span::styled(format!(" {label}"), label_style),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn sidebar_item_styles(active: bool, cursor: bool, theme: &Theme) -> (ratatui::style::Style, ratatui::style::Style) {
    if cursor {
        (
            ratatui::style::Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(ratatui::style::Modifier::BOLD),
            ratatui::style::Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg),
        )
    } else if active {
        (
            ratatui::style::Style::default()
                .fg(theme.accent)
                .add_modifier(ratatui::style::Modifier::BOLD),
            ratatui::style::Style::default()
                .fg(theme.fg)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )
    } else {
        (
            ratatui::style::Style::default().fg(theme.dim),
            ratatui::style::Style::default().fg(theme.fg),
        )
    }
}

fn g() -> &'static crate::theme::Glyphs {
    glyphs()
}

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

pub fn chunks(area: Rect) -> (Rect, Rect, Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(20)])
        .split(outer[1]);
    (outer[0], body[0], body[1])
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

    #[test]
    fn sidebar_clamps_offset_when_cursor_exits_top_window() {
        let (tx, rx) = tokio::sync::mpsc::channel::<crate::app::Action>(8);
        let mut app = crate::app::App::new(tx, rx);
        let total = crate::app::screen::ScreenId::ALL.len();
        app.sidebar_idx = 5;
        app.sidebar_offset = 0;
        let new_idx = (app.sidebar_idx + 1) % total;
        app.sidebar_idx = new_idx;
        app.clamp_sidebar_offset(total, 4);
        let expected_off = (new_idx + 1).saturating_sub(4); // derive from formula
        assert_eq!(app.sidebar_idx, (5 + 1) % total);
        assert_eq!(app.sidebar_offset, expected_off);
    }
}
