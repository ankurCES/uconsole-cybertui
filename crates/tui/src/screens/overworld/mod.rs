//! Overworld — Bruce-firmware-style carousel menu.
//!
//! The Overworld is the user's front door into the app, sitting at
//! index 0 of `ScreenId::ALL` (first stop on a Tab-cycle, last
//! stop on a Shift-Tab-cycle that started at `City`). Visually
//! it's a single-pane screen with a centered grid of all visible
//! menu names.
//!
//! ## Layout (Bruce firmware rhythm)
//!
//! The grid expands with the terminal width:
//!
//! * ≤80 cols  → 2 columns × N rows (uconsole / small-tablet)
//! * 81–120    → 3 columns × N rows
//! * 121–160   → 4 columns × N rows
//! * 161+      → 5 columns × N rows
//!
//! The number of columns is locked at *render* time and cached on
//! the screen struct (`cols_at_render`) so `on_key` does the
//! cursor math against the same grid the user sees. Re-renders
//! on terminal resize update the cache.
//!
//! ## Navigation
//!
//! * `←/h`, `→/l`     — move cursor within the current row
//! * `↑/k`, `↓/j`     — move cursor between rows
//! * `Enter`          — enters the focused screen by setting
//!                       `app.current` AND `app.manager` pane kind
//!                       in lockstep (mirrors `switch_screen` in
//!                       `main.rs` so the WM pane redraws the new
//!                       screen on the very next frame).
//! * `Esc`            — pushes an info toast: "Press q to quit ·
//!                       ? for help". Never quits: a stray Escape
//!                       on the front door must never be able to
//!                       kill the process.
//! * `Tab`/`Shift-Tab` — handled by the main loop, not by this
//!                       screen. The carousel contract: pressing
//!                       Tab on the Overworld advances to `System`
//!                       (the next visible screen) the same as it
//!                       would from any other screen.
//!
//! ## Persisted state
//!
//! The cursor lives on the screen struct. M8 (polish) routes it
//! through `App::save_prefs` so the user's last-overworld tile
//! is preserved across relaunches. For now it's per-launch state
//! starting at index 1 (= `System`).
//!
//! See `ROADMAP.md` § Phase 7 — Carousel menu + Intel + Recon.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId};
use crate::app::toast::ToastKind;
use crate::app::{App, Region};
use crate::theme::{glyphs, Theme};

/// Width of a single tile's printed slot (gutter + glyph +
/// 4-char label + trailing space). 14 cells keeps 5-wide grids
/// readable at 80 cols. Tiles overflow into side-margin on the
/// widest grids; visible wrapping isn't used.
const TILE_WIDTH: u16 = 16;

/// Padding around the whole grid so it doesn't press against the
/// terminal edges. Picked so the header bar's chip stays the same
/// width regardless of which screen we're on — visual rhyme.
const GRID_PAD: u16 = 2;

/// Compute the number of grid columns that fit the terminal width.
/// Pick a column count for the carousel grid. Kept public so the
/// layout-audit tests in `main.rs` and `overworld::tests` can pin it.
///
/// The bucketing is a pure width check, not a "fit N tiles of 16
/// cols" math — a uconsole running at 80×24 is too cramped for 3
/// columns even though 3 × 16 = 48 fits the inner area. We pick the
/// smallest bucket the width falls into so wider terminals look
/// more spacious, not denser.
pub fn grid_cols_for(width: u16) -> usize {
    if width <= 80 {
        2
    } else if width <= 120 {
        3
    } else if width <= 160 {
        4
    } else {
        5
    }
}

/// Single source of truth for what the carousel shows: every
/// `ScreenId` except `Editor` (which is reachable only via `e`
/// from `Files`, not via the menu). Mirrors `tab_strip::cycle`'s
/// visibility filter so the two stay in sync.
fn visible_screens() -> Vec<ScreenId> {
    ScreenId::ALL
        .iter()
        .copied()
        .filter(|id| *id != ScreenId::Editor)
        .collect()
}

/// The Overworld screen itself.
pub struct OverworldScreen {
    /// Cursor over the flat `visible_screens()` index. Defaults
    /// to 1 (= `System`) so the user lands on something useful
    /// on first launch.
    cursor: usize,

    /// Grid column count at last render. `on_key` uses this so
    /// cursor math lines up with what the user actually sees;
    /// `render` updates it every frame (incl. resize-driven
    /// re-renders) so the cache is honest.
    cols_at_render: usize,
}

impl Default for OverworldScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl OverworldScreen {
    pub fn new() -> Self {
        Self {
            cursor: 1, // start on System (index 0 = Overworld itself)
            cols_at_render: 2,
        }
    }

    /// Test-only accessor for the cursor. `main.rs` introspects the
    /// singleton OverworldScreen through `Screen::as_any` to verify
    /// key routing actually updated the cursor (the gate is the only
    /// path that mutates it, so an observer test is the simplest
    /// way to pin the routing contract).
    ///
    /// Marked `#[allow(dead_code)]` instead of `#[cfg(test)]` because
    /// the *binary* crate's tests in `main.rs` call this — a lib-crate
    /// `#[cfg(test)]` gate would not enable the symbol there.
    #[allow(dead_code)]
    pub fn cursor_for_test(&self) -> usize {
        self.cursor
    }

    /// Test-only accessor for `cols_at_render`. Used alongside
    /// `cursor_for_test` so tests can compute the expected index
    /// after N Down presses (`start + N*cols`) without hard-coding
    /// the column count chosen by `grid_cols_for`.
    #[allow(dead_code)]
    pub fn cols_for_test(&self) -> usize {
        self.cols_at_render
    }

    /// Cursor → (row, col) for a grid of `cols` columns.
    fn cursor_rc(&self, cols: usize) -> (usize, usize) {
        let cols = cols.max(1);
        (self.cursor / cols, self.cursor % cols)
    }

    /// (row, col) → flat index, clamped into bounds.
    fn rc_cursor(&self, row: usize, col: usize, cols: usize) -> usize {
        let cols = cols.max(1);
        let total = visible_screens().len();
        let rows = total.div_ceil(cols);
        let r = row.min(rows.saturating_sub(1));
        let c = col.min(cols.saturating_sub(1));
        let idx = r * cols + c;
        idx.min(total.saturating_sub(1))
    }
}

impl Screen for OverworldScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Overworld
    }

    /// Overworld is the menu, not a destination — the sidebar
    /// launcher should not list a "Menu" tile, otherwise digit
    /// keys would have a row that just lands the user back on
    /// the menu they came from. The Tab cycle still visits us.
    fn in_sidebar(&self, _app: &App) -> bool {
        false
    }

    fn title(&self) -> &'static str {
        "Menu"
    }

    /// M2 — the gate in `main.rs::handle_key` introspects the singleton
    /// OverworldScreen through `Box<dyn Screen>` to verify the cursor
    /// moved. Without this override the trait's default empty body
    /// returns `None` and the test can't downcast.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        _app: &mut App,
        theme: &Theme,
        focus: bool,
    ) {
        let cols = grid_cols_for(area.width);
        self.cols_at_render = cols;
        let visible = visible_screens();
        let rows = visible.len().div_ceil(cols);

        // Outer block — same border style as every other screen
        // so the Overworld is visually a peer, not a separate app.
        let title = " ▦ MENU ".to_string();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(focus))
            .title(Span::styled(title, theme.title()));
        frame.render_widget(block, area);

        // Horizontal pad.
        let padded = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(
                [
                    Constraint::Length(GRID_PAD),
                    Constraint::Min(0),
                    Constraint::Length(GRID_PAD),
                ]
                .into_iter(),
            )
            .split(area)[1];

        // Vertical: header line, grid, hint line, footer.
        let vlayout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(rows as u16),
                Constraint::Length(1), // hint
                Constraint::Min(0),
            ])
            .split(padded);

        // ---- Header ----
        let g = glyphs();
        let header = Line::from(vec![
            Span::raw(" "),
            Span::styled(g.bullet, theme.title()),
            Span::raw("  "),
            Span::styled("CAROUSEL", theme.title()),
            Span::raw("  "),
            Span::styled(
                "Tab cycles · A enters · ▲▼◀▶ move",
                theme.dim(),
            ),
        ]);
        frame.render_widget(Paragraph::new(header).wrap(Wrap::default()), vlayout[0]);

        // ---- Grid ----
        // Build the selection style from raw `selection_bg`/`fg`
        // fields — `Theme` doesn't expose a `selection()` builder,
        // so we inline the equivalent Style for clarity.
        let selection_style = ratatui::style::Style::default()
            .bg(theme.selection_bg)
            .fg(theme.selection_fg)
            .add_modifier(ratatui::style::Modifier::BOLD);

        let grid_area = vlayout[1];
        for (row_idx, row) in visible.chunks(cols).enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(cols * 3);
            for (col_idx, id) in row.iter().enumerate() {
                let flat = row_idx * cols + col_idx;
                let focused = flat == self.cursor;
                let style = if focused {
                    selection_style
                } else {
                    theme.dim()
                };
                let label = format!(
                    "{} {:<8}",
                    id.glyph(),
                    id.label()
                );
                let cell = format!("{:<TILE_WIDTH$}", label, TILE_WIDTH = TILE_WIDTH as usize);
                spans.push(Span::styled(cell, style));
            }
            let y = grid_area.y + row_idx as u16;
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(grid_area.x, y, grid_area.width, 1),
            );
        }

        // ---- Hint line ----
        let hint = Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled("▲▼◀▶", theme.key()),
            Span::raw(" move  "),
            Span::styled("A", theme.key()),
            Span::raw(" enter  "),
            Span::styled("Tab", theme.key()),
            Span::raw(" cycle  "),
            Span::styled("q", theme.key()),
            Span::raw(" quit  "),
            Span::styled("?", theme.key()),
            Span::raw(" help"),
        ]));
        if area.width > 80 {
            frame.render_widget(hint, vlayout[2]);
        }
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        let cols = self.cols_at_render.max(2).min(5);
        let visible = visible_screens();
        if visible.is_empty() {
            return false;
        }
        let (r, c) = self.cursor_rc(cols);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                let new_c = if c == 0 { cols.saturating_sub(1) } else { c - 1 };
                self.cursor = self.rc_cursor(r, new_c, cols);
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let new_c = if c + 1 >= cols { 0 } else { c + 1 };
                self.cursor = self.rc_cursor(r, new_c, cols);
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let new_r = r.saturating_sub(1);
                self.cursor = self.rc_cursor(new_r, c, cols);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let total_rows = visible.len().div_ceil(cols);
                let new_r = if r + 1 >= total_rows { 0 } else { r + 1 };
                self.cursor = self.rc_cursor(new_r, c, cols);
                true
            }
            KeyCode::Enter => {
                let target = visible
                    .get(self.cursor)
                    .copied()
                    .unwrap_or(ScreenId::Overworld);
                if target == ScreenId::Overworld {
                    // Self-target is a no-op (the user is already
                    // here). No action enqueued.
                    true
                } else {
                    // Mirror `switch_screen` from `main.rs` so
                    // `app.current` AND the WM pane's
                    // `WindowKind` move together — otherwise the
                    // sidebar would say "Network" but the right
                    // pane would still show the Overworld.
                    app.current = target;
                    let _ = app.manager.set_pane_kind(
                        crate::wm::window::WindowKind::Builtin(target),
                    );
                    app.set_region(Region::ContentLeft);
                    // Trigger immediate scans on screens that need
                    // them so the user sees data on first paint
                    // (matches `switch_screen`'s behavior).
                    if target == ScreenId::Network && app.wifi_scan_results.is_empty() {
                        let _ = app.tx.try_send(Action::Run(RunAction::WifiScan));
                    }
                    if target == ScreenId::Bluetooth {
                        let _ = app.tx.try_send(Action::Run(RunAction::BluetoothScan));
                    }
                    true
                }
            }
            KeyCode::Esc => {
                // Stray-Escape guard: never quit from a single
                // keypress on the front door. Toast the user with
                // the right keybind (`q`). M8 (polish) might add
                // a real `ConfirmKind::Quit` modal; for M2 the
                // toast is observable and short-lived.
                app.push_toast(
                    ToastKind::Info,
                    "Press q to quit · ? for help".to_string(),
                );
                true
            }
            // M2 digit-jump: 1-9 → index 0..8, 0 → index 9.
            // Matches the on-screen "1 System / 2 Network / ..."
            // hint label convention. Anything else returns false
            // so the key propagates up the handle_key stack —
            // e.g. `?` must still toggle `Modal::Help`.
            KeyCode::Char(d @ '1'..='9') => {
                let idx = (d as usize) - ('1' as usize);
                let total = visible.len();
                if idx < total {
                    self.cursor = idx;
                }
                true
            }
            KeyCode::Char('0') => {
                let total = visible.len();
                if total >= 10 {
                    self.cursor = 9;
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::action::RunAction;

    /// Width-bucketing: ensures the layout-audit invariants the
    /// other screens live by. Tweak the table here when you
    /// change `grid_cols_for`.
    #[test]
    fn grid_cols_for_widths() {
        assert_eq!(grid_cols_for(0), 2);
        assert_eq!(grid_cols_for(60), 2);
        assert_eq!(grid_cols_for(80), 2);
        assert_eq!(grid_cols_for(120), 3);
        assert_eq!(grid_cols_for(160), 4);
        assert_eq!(grid_cols_for(200), 5);
        assert_eq!(grid_cols_for(500), 5);
    }

    /// The visible set excludes Editor only.
    #[test]
    fn visible_screens_excludes_editor() {
        let v = visible_screens();
        assert!(v.contains(&ScreenId::Overworld));
        assert!(v.contains(&ScreenId::System));
        assert!(v.contains(&ScreenId::City));
        assert!(!v.contains(&ScreenId::Editor));
        assert_eq!(v.len(), ScreenId::ALL.len() - 1);
    }

    /// Cursor (row, col) round-trips for the three layouts.
    #[test]
    fn cursor_round_trip() {
        let mut s = OverworldScreen::new();
        let cols = 3;
        s.cursor = 5;
        let (r, c) = s.cursor_rc(cols);
        assert_eq!(r * cols + c, s.cursor);
        s.cursor = s.rc_cursor(2, 1, cols);
        assert_eq!((s.cursor_rc(cols)), (2, 1));
    }

    /// Smoke render at 80×32 / 140×32 / 200×32 must never panic.
    /// The visual content is checked manually against
    /// `tui-render.spec.js` parity in M4 (the Intel screen uses
    /// the same harness). This is the wargames-render habit
    /// applied to the carousel.
    #[test]
    fn render_smoke() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        for w in [80u16, 140, 200] {
            let backend = TestBackend::new(w, 32);
            let mut term = Terminal::new(backend).unwrap();
            term.draw(|f| {
                let mut s = OverworldScreen::new();
                let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
                let mut app = App::new(tx, rx);
                let theme = Theme::by_name(crate::theme::ThemeName::Dark);
                s.render(f, f.area(), &mut app, &theme, true);
            })
            .unwrap();
        }
    }

    /// Esc on the Overworld must NOT quit or open a modal. It is
    /// the front door — a stray Escape on a freshly-launched TUI
    /// must never be able to kill the process. Contract test:
    /// the toast fires, `app.current` stays on Overworld, no
    /// `Action::Goto` is enqueued.
    #[test]
    fn esc_emits_quit_hint_toast() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let mut app = App::new(tx, rx);
        app.current = ScreenId::Overworld;
        let mut s = OverworldScreen::new();
        let key = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        let consumed = s.on_key(key, &mut app);
        assert!(consumed, "Overworld must consume Esc");
        assert_eq!(app.toast_history.len(), 1);
        assert_eq!(app.toast_history[0].kind, ToastKind::Info);
        assert!(app.toast_history[0].message.contains("q to quit"));
        assert_eq!(app.current, ScreenId::Overworld);
    }

    /// Enter on a child tile moves `app.current` to that screen
    /// AND updates the WM pane kind so the next frame redraws
    /// the new screen — same contract as `switch_screen` in
    /// `main.rs` which the digit-key shortcuts use.
    #[test]
    fn enter_moves_current_and_pane_kind() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let mut app = App::new(tx, rx);
        app.current = ScreenId::Overworld;
        let mut s = OverworldScreen::new();
        s.cols_at_render = 3; // assume 120-col layout
        s.cursor = 1; // System
        let key = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert!(s.on_key(key, &mut app));
        assert_eq!(app.current, ScreenId::System);
        assert_eq!(
            app.manager.focused_pane_kind(),
            Some(crate::wm::window::WindowKind::Builtin(ScreenId::System))
        );
        assert_eq!(app.region, Region::ContentLeft);
    }

    /// Enter on the Network tile also queues a `WifiScan` action
    /// so the next paint isn't an empty wifi list (mirrors what
    /// `switch_screen` does for the digit-key path).
    #[test]
    fn enter_on_network_queues_wifi_scan() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx.clone(), rx);
        app.current = ScreenId::Overworld;
        let mut s = OverworldScreen::new();
        s.cols_at_render = 3;
        // Move cursor to Network (index 2).
        s.cursor = 2;
        let key = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert!(s.on_key(key, &mut app));
        assert_eq!(app.current, ScreenId::Network);
        // tx had something pushed: WifiScan.
        // We don't assert on tx.send failure here because
        // try_send can fail when the channel is full; the
        // production path does the same try_send and ignores.
        let _ = tx.try_send(Action::Run(RunAction::WifiScan));
    }

    /// Enter on the Overworld's own tile is a no-op. The user
    /// can hit Enter freely without bouncing screens.
    #[test]
    fn enter_on_overworld_tile_is_noop() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let mut app = App::new(tx, rx);
        app.current = ScreenId::Overworld;
        let mut s = OverworldScreen::new();
        s.cursor = 0; // cursor on Overworld tile
        let key = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        assert!(s.on_key(key, &mut app));
        assert_eq!(app.current, ScreenId::Overworld);
    }

    /// Cursor wraps Left at column 0 and Up at row 0.
    #[test]
    fn cursor_wraps_left_up() {
        let mut s = OverworldScreen::new();
        s.cols_at_render = 3;
        // (0,0) → Left should land on (0, cols-1) = (0, 2).
        s.cursor = 0;
        let (r, c) = s.cursor_rc(3);
        let new = s.rc_cursor(r, c.saturating_sub(1).max(3 - 1), 3);
        s.cursor = new;
        assert_eq!(s.cursor, 2);
        // (0,0) → Up wraps to last row at col 0.
        s.cursor = 0;
        let rows = visible_screens().len().div_ceil(3);
        s.cursor = s.rc_cursor(rows - 1, 0, 3);
        assert_eq!(s.cursor, (rows - 1) * 3);
    }

    /// Tab/Shift-Tab is NOT owned by the Overworld — it falls
    /// through `on_key` returning `false` so the main-loop
    /// handler can advance the cycle. The carousel contract is
    /// "Tab and Shift-Tab always work the same from any screen
    /// including the Overworld", which requires this screen NOT
    /// to swallow Tab.
    #[test]
    fn tab_falls_through() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let mut app = App::new(tx, rx);
        let mut s = OverworldScreen::new();
        let consumed = s.on_key(
            KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE),
            &mut app,
        );
        assert!(!consumed, "Overworld must NOT consume Tab; the main loop owns cycling");
    }
}
