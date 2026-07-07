//! Herdr-style top menu bar.
//!
//! Renders a single 1-row strip with the fixed menus `File · View · Tools · Help`.
//! Clicking a menu (Enter on the highlighted item, or Left/Right then Enter)
//! opens a dropdown of items. Dropdown is rendered as an overlay above the
//! content area; Esc closes the dropdown without firing the item.
//!
//! Items dispatch `Action`s. The menu owns no state beyond the dropdown
//! stack — the active menu id (if any) and the highlighted item index —
//! and that lives in `App::menu` so the renderer can read it across frames
//! without any borrow gymnastics.
//!
//! Menu structure is intentionally small (4 menus × 2-4 items each). Herdr's
//! menu bar is a *navigation accelerator*, not a settings UI — the command
//! palette (`:`) is for fuzzy searching. Anything bigger than ~12 items
//! total belongs in Settings or the palette.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::action::Action;
use crate::app::App;
use crate::theme::{glyphs, Theme};

/// Menu bar height (rows). Always 1 to keep the screen chrome minimal.
pub const MENU_BAR_HEIGHT: u16 = 1;

/// All four menus in display order. Each menu has a label and a list of
/// `(id, label, action)` items. The id is used as the discriminant in
/// `App::menu` so the renderer can pattern-match without string compares.
pub const MENUS: &[Menu] = &[
    Menu {
        id: MenuId::File,
        label: "File",
        items: &[
            MenuItem {
                id: "refresh",
                label: "Refresh all",
                action: MenuAction::ActionFn(act_run_wifi_scan),
                // Refresh all is its own fan-out action — see below.
                fanout: Some(MenuFanout::RefreshAll),
            },
            MenuItem {
                id: "palette",
                label: "Command palette…",
                action: MenuAction::ActionFn(act_toggle_theme),
                fanout: Some(MenuFanout::OpenPalette),
            },
            MenuItem {
                id: "quit",
                label: "Quit",
                action: MenuAction::Quit,
                fanout: None,
            },
        ],
    },
    Menu {
        id: MenuId::View,
        label: "View",
        items: &[
            MenuItem {
                id: "units-metric",
                label: "Units: Metric (°C, km/h)",
                action: MenuAction::ActionFn(act_toggle_units),
                fanout: Some(MenuFanout::UnitsMetric),
            },
            MenuItem {
                id: "units-imperial",
                label: "Units: Imperial (°F, mph)",
                action: MenuAction::ActionFn(act_toggle_units),
                fanout: Some(MenuFanout::UnitsImperial),
            },
            MenuItem {
                id: "traffic",
                label: "Toggle traffic overlay",
                action: MenuAction::ActionFn(act_toggle_traffic),
                fanout: None,
            },
            MenuItem {
                id: "weather-panel",
                label: "Toggle weather panel",
                action: MenuAction::ActionFn(act_toggle_weather),
                fanout: None,
            },
        ],
    },
    Menu {
        id: MenuId::Tools,
        label: "Tools",
        items: &[
            MenuItem {
                id: "wlan-scan",
                label: "Rescan Wi-Fi",
                action: MenuAction::ActionFn(act_run_wifi_scan),
                fanout: None,
            },
            MenuItem {
                id: "bt-scan",
                label: "Rescan Bluetooth",
                action: MenuAction::ActionFn(act_run_bluetooth_scan),
                fanout: None,
            },
            MenuItem {
                id: "web-toggle",
                label: "Toggle web server",
                action: MenuAction::ActionFn(act_toggle_web),
                fanout: None,
            },
        ],
    },
    Menu {
        id: MenuId::Help,
        label: "Help",
        items: &[
            MenuItem {
                id: "help",
                label: "Show help (?)",
                action: MenuAction::OpenModal(open_help),
                fanout: None,
            },
            MenuItem {
                id: "palette",
                label: "Command palette (:)",
                action: MenuAction::OpenModal(open_palette),
                fanout: None,
            },
            MenuItem {
                id: "toast-log",
                label: "Toast log (T)",
                action: MenuAction::OpenModal(open_toast_log),
                fanout: None,
            },
        ],
    },
];

/// Builders for menu items. These are free `fn`s returning either an
/// `Option<Action>` (None = no-op, used by conditional items) or a
/// fresh `Modal` value. `fn` pointers are `Copy`, which lets the
/// `MenuItem` enum stay `Copy` and live in a `static` table.
fn act_run_wifi_scan(_app: &App) -> Option<Action> {
    Some(Action::Run(crate::app::action::RunAction::WifiScan))
}

fn act_run_bluetooth_scan(_app: &App) -> Option<Action> {
    Some(Action::Run(crate::app::action::RunAction::BluetoothScan))
}

fn act_toggle_theme(_app: &App) -> Option<Action> {
    Some(Action::Toggle(crate::app::screen::SettingsKey::Theme))
}

fn act_toggle_units(_app: &App) -> Option<Action> {
    Some(Action::Toggle(crate::app::screen::SettingsKey::Units))
}

fn act_toggle_traffic(_app: &App) -> Option<Action> {
    Some(Action::Toggle(crate::app::screen::SettingsKey::TrafficOverlay))
}

fn act_toggle_weather(_app: &App) -> Option<Action> {
    Some(Action::Toggle(crate::app::screen::SettingsKey::WeatherPanel))
}

fn act_toggle_web(_app: &App) -> Option<Action> {
    Some(Action::Toggle(crate::app::screen::SettingsKey::WebServer))
}

fn open_help(_app: &mut App) -> crate::app::Modal {
    crate::app::Modal::Help
}

fn open_toast_log(_app: &mut App) -> crate::app::Modal {
    crate::app::Modal::ToastLog
}

/// Open the command palette. Mirrors the `:` global key handler so
/// the menu is a single source of truth: pressing `:` OR clicking
/// Help → Command palette lands in the same place with the same
/// reset state (cleared buffer, cursor at 0).
pub fn open_palette(app: &mut App) -> crate::app::Modal {
    // Reset palette buffer/cursor on every open so a stale query from
    // a previous session doesn't leak in. The `:` key handler calls
    // this same builder — keeping both paths in sync here avoids a
    // "menu-opened palette has leftover text" footgun.
    app.palette_buf.clear();
    app.palette_idx = 0;
    crate::app::Modal::CommandPalette
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuId {
    File,
    View,
    Tools,
    Help,
}

#[derive(Debug, Clone, Copy)]
pub struct Menu {
    pub id: MenuId,
    pub label: &'static str,
    pub items: &'static [MenuItem],
}

#[derive(Debug, Clone, Copy)]
pub struct MenuItem {
    pub id: &'static str,
    pub label: &'static str,
    pub action: MenuAction,
    /// Some(items) means "after firing the action, fan out to these other
    /// actions". Used by Refresh All (which sends one Action::Refresh per
    /// screen) and by Units-Metric/Imperial (which only fires if the
    /// current units don't already match — i.e. the metric item is a
    /// no-op when already metric).
    pub fanout: Option<MenuFanout>,
}

/// Discriminant for the menu items' payloads. We don't store the
/// `Action` directly because (a) `Action` is not `Copy`, so a `Copy`
/// enum is impossible; (b) some menu actions need to fire *multiple*
/// actions in sequence (Refresh All) or need access to runtime state
/// (the Units menu items look at `app.units` before firing).
///
/// `OpenModal` and `ActionFn` carry `fn` pointers so `MenuItem` can
/// live in `static` tables (everything is `Copy`).
#[derive(Debug, Clone, Copy)]
pub enum MenuAction {
    /// Build the Action fresh on each invocation. Returns `None` to
    /// mean "do nothing" — used by conditional items that decide at
    /// dispatch time whether the action should fire.
    ActionFn(fn(&App) -> Option<Action>),
    /// Build the modal fresh on each invocation so we can copy out of
    /// the static table without taking ownership of a `Modal`.
    OpenModal(fn(&mut App) -> crate::app::Modal),
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub enum MenuFanout {
    RefreshAll,
    OpenPalette,
    UnitsMetric,
    UnitsImperial,
}

/// State for the menu bar. Held on `App` so `main.rs` can drive both
/// the renderer and the key handler from the same source of truth.
///
/// `open` = `None` → menu bar is closed (the default).
/// `open = Some(menu_id)` → that menu's dropdown is open and `cursor`
/// points at the highlighted item.
#[derive(Debug, Clone, Default)]
pub struct MenuState {
    pub open: Option<MenuId>,
    pub cursor: usize,
}

impl MenuState {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn open(&mut self, id: MenuId) {
        self.open = Some(id);
        self.cursor = 0;
    }

    pub fn close(&mut self) {
        self.open = None;
        self.cursor = 0;
    }

    /// Move the cursor within the currently-open menu. Wraps.
    pub fn move_cursor(&mut self, delta: i32) {
        let Some(id) = self.open else { return };
        let Some(menu) = MENUS.iter().find(|m| m.id == id) else {
            return;
        };
        let n = menu.items.len();
        if n == 0 {
            return;
        }
        let cur = self.cursor as i32;
        let next = ((cur + delta) % n as i32 + n as i32) % n as i32;
        self.cursor = next as usize;
    }

    /// Step the open menu left/right (Left from File wraps to Help).
    pub fn step_menu(&mut self, delta: i32) {
        if let Some(id) = self.open {
            let n = MENUS.len() as i32;
            let pos = MENUS.iter().position(|m| m.id == id).unwrap_or(0) as i32;
            let next = ((pos + delta) % n + n) % n;
            self.open = Some(MENUS[next as usize].id);
            self.cursor = 0;
        }
    }
}

/// Render the menu bar (always 1 row) and, if a dropdown is open, render
/// the dropdown as an overlay above the content. The overlay is drawn
/// last so it sits on top of any other widget in the same row.
pub fn draw(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // The menu bar is always rendered, even when no dropdown is open —
    // it's a permanent affordance at the top of the chrome.
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(" ", theme.dim()));
    for menu in MENUS {
        let is_open = app.menu.open == Some(menu.id);
        let style = if is_open {
            ratatui::style::Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            ratatui::style::Style::default().fg(theme.fg)
        };
        let label = if is_open {
            format!(" ▶ {} ", menu.label)
        } else {
            format!(" {} ", menu.label)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" ", theme.dim()));
    }
    // Right side: units indicator. Cheap and useful.
    let units_str = match app.units {
        crate::prefs::Units::Metric => "metric",
        crate::prefs::Units::Imperial => "imperial",
    };
    spans.push(Span::styled(
        format!(" {}{}", glyphs().sep, units_str),
        theme.dim(),
    ));
    let line = Line::from(spans);
    let p = Paragraph::new(line).style(
        ratatui::style::Style::default().fg(theme.fg).bg(theme.bg),
    );
    f.render_widget(p, area);

    // Dropdown overlay.
    if let Some(menu_id) = app.menu.open {
        if let Some(menu) = MENUS.iter().find(|m| m.id == menu_id) {
            draw_dropdown(f, area, menu, app.menu.cursor, app, theme);
        }
    }
}

/// Compute the rect for the dropdown. Anchored to the menu header that
/// was opened, falling back to (0, 1) if the menu is unknown (defensive —
/// shouldn't happen because `open` is only ever set to a real `MenuId`).
fn dropdown_rect(menu: &Menu, area: Rect) -> Rect {
    // Find the x-offset of this menu's header in the bar.
    let mut x = area.x + 1; // leading space
    for m in MENUS {
        if m.id == menu.id {
            break;
        }
        // Each menu takes its label width + 3 chars padding (" X " + space).
        x += m.label.len() as u16 + 4;
    }
    let width = menu
        .items
        .iter()
        .map(|i| (i.label.chars().count() as u16) + 4)
        .max()
        .unwrap_or(20)
        .max(20);
    let height = (menu.items.len() as u16) + 2; // borders
    // Anchor just below the menu bar.
    let y = area.y + area.height;
    Rect::new(x, y, width.min(area.width.saturating_sub(x)), height)
}

fn draw_dropdown(f: &mut Frame, bar_area: Rect, menu: &Menu, cursor: usize, _app: &App, theme: &Theme) {
    let rect = dropdown_rect(menu, bar_area);
    if rect.width < 4 || rect.height < 2 {
        return;
    }
    f.render_widget(Clear, rect);
    let lines: Vec<Line> = menu
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let is_cursor = i == cursor;
            let style = if is_cursor {
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
            } else {
                ratatui::style::Style::default().fg(theme.fg)
            };
            let prefix = if is_cursor { " ▶ " } else { "   " };
            Line::from(Span::styled(
                format!("{prefix}{}", item.label),
                style,
            ))
        })
        .collect();
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(true))
            .title(Span::styled(
                format!(" {} ", menu.label),
                theme.title(),
            )),
    );
    f.render_widget(p, rect);
}

/// Dispatch a menu item. Called from `main.rs` after the user hits
/// Enter on the highlighted dropdown row. Implements the fanout
/// variants (Refresh All, OpenPalette, UnitsMetric/Imperial) that need
/// to fire *multiple* actions or read runtime state before firing.
pub async fn dispatch(
    item: &MenuItem,
    app: &mut App,
    tx: &tokio::sync::mpsc::Sender<Action>,
) {
    // First, run any fanout that depends on runtime state.
    let mut skip_main_action = false;
    if let Some(fanout) = item.fanout {
        match fanout {
            MenuFanout::RefreshAll => {
                for id in crate::app::screen::ScreenId::ALL {
                    let _ = tx.send(Action::Refresh(*id)).await;
                }
                // Refresh All doesn't fire the item's own `Action` —
                // it's a pure fanout — so we mark it to skip below.
                skip_main_action = true;
            }
            MenuFanout::OpenPalette => {
                app.modal = crate::app::Modal::CommandPalette;
                app.palette_buf.clear();
                app.palette_idx = 0;
                skip_main_action = true;
            }
            MenuFanout::UnitsMetric => {
                if app.units != crate::prefs::Units::Metric {
                    let _ = tx.send(Action::Toggle(crate::app::screen::SettingsKey::Units)).await;
                }
                skip_main_action = true;
            }
            MenuFanout::UnitsImperial => {
                if app.units != crate::prefs::Units::Imperial {
                    let _ = tx.send(Action::Toggle(crate::app::screen::SettingsKey::Units)).await;
                }
                skip_main_action = true;
            }
        }
    }
    if skip_main_action {
        return;
    }
    match item.action {
        MenuAction::ActionFn(build) => {
            if let Some(a) = build(app) {
                let _ = tx.send(a).await;
            }
        }
        MenuAction::OpenModal(build) => {
            app.modal = build(app);
        }
        MenuAction::Quit => {
            let _ = tx.send(Action::Quit).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::screen::ScreenId;

    /// The menu bar should render a single row containing every menu
    /// label, in order, separated by spaces.
    #[test]
    fn menu_bar_renders_every_menu_label() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let app = App::new(tx, rx);
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        terminal
            .draw(|f| draw(f, Rect::new(0, 0, 120, 1), &app, &theme))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push(buf[(x, 0)].symbol().chars().next().unwrap_or(' '));
        }
        for m in MENUS {
            assert!(
                row.contains(m.label),
                "menu bar must render {:?}; got {:?}",
                m.label,
                row
            );
        }
    }

    /// Dropdown rect must be just below the menu bar and wide enough for
    /// the longest item.
    #[test]
    fn dropdown_rect_below_menu_bar() {
        let menu = &MENUS[0]; // File
        let bar = Rect::new(0, 0, 120, 1);
        let r = dropdown_rect(menu, bar);
        assert_eq!(r.y, 1, "dropdown must start one row below the menu bar");
        assert!(r.width >= 20, "dropdown must be at least 20 cols wide");
    }

    /// MenuState::open resets the cursor to 0 so the user always starts
    /// at the top of the freshly-opened menu.
    #[test]
    fn open_resets_cursor() {
        let mut m = MenuState::default();
        m.cursor = 2;
        m.open(MenuId::View);
        assert_eq!(m.open, Some(MenuId::View));
        assert_eq!(m.cursor, 0, "open() must reset cursor");
    }

    /// MenuState::step_menu wraps around the menu list.
    #[test]
    fn step_menu_wraps() {
        let mut m = MenuState::default();
        m.open(MenuId::File);
        m.step_menu(-1);
        assert_eq!(m.open, Some(MenuId::Help), "stepping back from File wraps to Help");
        m.step_menu(1);
        assert_eq!(m.open, Some(MenuId::File), "stepping forward from Help wraps to File");
    }

    /// MenuState::move_cursor wraps within the open menu's item list.
    #[test]
    fn move_cursor_wraps_within_menu() {
        let mut m = MenuState::default();
        m.open(MenuId::View);
        // Move past the end (View has 4 items).
        for _ in 0..5 {
            m.move_cursor(1);
        }
        assert!(m.cursor < MENUS[1].items.len(), "cursor must wrap, got {}", m.cursor);
    }

    /// Refusing to compile if `ScreenId::ALL` ever empties (defensive —
    /// the menu fanout iterates it).
    #[test]
    fn all_screens_nonempty() {
        assert!(!ScreenId::ALL.is_empty());
    }

    /// `open_palette` must clear the palette buffer and reset the
    /// cursor on every call, so opening the palette via either the
    /// `:` key or the menu item lands on a clean state. Without this,
    /// a stale query from a previous session leaks into the new one.
    #[test]
    fn open_palette_resets_buffer_and_cursor() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let mut app = App::new(tx, rx);
        app.palette_buf = "stale query".to_string();
        app.palette_idx = 4;
        let modal = open_palette(&mut app);
        assert!(matches!(modal, crate::app::Modal::CommandPalette));
        assert_eq!(app.palette_buf, "");
        assert_eq!(app.palette_idx, 0);
    }
}