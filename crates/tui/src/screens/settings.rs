//! Settings screen: theme, mouse, Nerd Font, web server toggle, refresh rate.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId, SettingsKey};
use crate::app::App;
use crate::keymap::{key_event_label, KeymapCmd, NavAction};
use crate::theme::Theme;

pub struct SettingsScreen;

impl Screen for SettingsScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Settings
    }
    fn title(&self) -> &'static str {
        "Settings"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        // Keys sub-mode: when editing, route all keys through the
        // dedicated handler and stop — the parent Settings list and
        // shortcut letters (t/m/n/w/u/T/K) must NOT fire while the
        // user is in the binding editor.
        if app.keymap_editing {
            return on_key_keymap_mode(key, app);
        }
        // Up/Down/j/k move the row highlight so the user can see which
        // key each row corresponds to before pressing the action letter.
        // Row count grew from 4 → 8 when City preferences (units, traffic,
        // weather, city override) landed in Step 2, then to 9 with the
        // user-editable keymap (Keys) sub-screen. City override has
        // no direct key — it's edited via the City screen's `c` modal
        // and surfaced here read-only.
        let total: usize = 9;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                app.settings_selected = (app.settings_selected + 1) % total;
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.settings_selected = app
                    .settings_selected
                    .checked_sub(1)
                    .unwrap_or(total - 1);
                return true;
            }
            KeyCode::Home | KeyCode::Char('g') => {
                app.settings_selected = 0;
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                app.settings_selected = total - 1;
                return true;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                // Enter on the highlighted row triggers its action.
                // City override (row 7) has no action — Enter is a
                // no-op so the highlight still moves but nothing
                // changes. Editing happens via `c` on the City screen.
                let key = match app.settings_selected {
                    0 => SettingsKey::Theme,
                    1 => SettingsKey::Mouse,
                    2 => SettingsKey::NerdFont,
                    3 => SettingsKey::WebServer,
                    4 => SettingsKey::Units,
                    5 => SettingsKey::TrafficOverlay,
                    6 => SettingsKey::WeatherPanel,
                    7 => SettingsKey::Keymap,
                    _ => return false,
                };
                let _ = app.tx.try_send(Action::Toggle(key));
                return true;
            }
            _ => {}
        }
        match key.code {
            KeyCode::Char('t') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::Theme));
                return true;
            }
            KeyCode::Char('m') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::Mouse));
                return true;
            }
            KeyCode::Char('n') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::NerdFont));
                return true;
            }
            KeyCode::Char('w') => {
                // `w` is overloaded on the Settings screen: it also
                // toggles the City screen's weather panel. The
                // dispatcher distinguishes by `app.current` — see
                // the SettingsKey::WebServer / WeatherPanel arms in
                // main.rs.
                let key = if app.current == ScreenId::Settings {
                    SettingsKey::WebServer
                } else {
                    SettingsKey::WeatherPanel
                };
                let _ = app.tx.try_send(Action::Toggle(key));
                return true;
            }
            // City preferences also exposed here so the user can
            // change units without navigating to the City screen.
            KeyCode::Char('u') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::Units));
                return true;
            }
            KeyCode::Char('T') => {
                // Shift-T: traffic overlay. Lowercase `t` is theme.
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::TrafficOverlay));
                return true;
            }
            KeyCode::Char('K') => {
                // Shift-K: enter the user-keymap editing sub-mode.
                // The render-side checks `app.keymap_editing` and
                // shows the Keys table; `Esc` / `q` (handled in the
                // sub-mode) flips the flag back off.
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::Keymap));
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        if app.keymap_editing {
            render_keymap_mode(f, area, app, theme, focus);
        } else {
            render_settings_list(f, area, app, theme, focus);
        }
    }
}

fn render_settings_list(f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
    let block = Block::default()
        .title(Span::styled(" Settings ", theme.title()))
        .borders(Borders::ALL)
        .border_style(theme.border(focus));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve bottom row for hints.
    let list_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(1),
    );

    let theme_name = app.theme_name.as_str();
    let web_state = app.live.web_enabled.try_read().map(|b| *b).unwrap_or(false);
    let units_str = match app.units {
        crate::prefs::Units::Metric => "metric (°C, km/h)",
        crate::prefs::Units::Imperial => "imperial (°F, mph)",
    };
    let city_str = app
        .city_override
        .as_deref()
        .unwrap_or("(ip-detected)");

    let items: Vec<ListItem> = vec![
        row("theme", theme_name, "t", theme),
        row("mouse", bool_str(app.mouse), "m", theme),
        row("nerd font", bool_str(app.nerd_font), "n", theme),
        row("web server", bool_str(web_state), "w", theme),
        row("units", units_str, "u", theme),
        row("traffic overlay", bool_str(app.traffic_overlay), "T", theme),
        row("weather panel", bool_str(app.show_weather_panel), "w", theme),
        row("city", city_str, "—", theme),
        row("keys",
            &format!("{} override{}",
                     app.keymap.bindings.len(),
                     if app.keymap.bindings.len() == 1 { "" } else { "s" }),
            "K", theme),
    ];
    if app.settings_selected >= items.len() {
        app.settings_selected = items.len() - 1;
    }
    let visible_h = list_area.height as usize;
    let offset = compute_offset(app.settings_selected, items.len(), visible_h);
    let mut state = ListState::default().with_selected(Some(app.settings_selected));
    *state.offset_mut() = offset;
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, list_area, &mut state);

    let hints = Paragraph::new(Line::from(Span::styled(
        "  j/k scroll · ⏎ toggle row · t theme · m mouse · n nerd · w web|weather · u units · T traffic · K keys",
        theme.dim(),
    )));
    let hint_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    f.render_widget(hints, hint_area);
}

/// Render the Settings → Keys sub-mode: one row per `NavAction`,
/// showing the user-bound key (or `[unbound]`) and highlighting the
/// row the user is currently inspecting. A row being captured shows
/// `[press a key…]` and is highlighted as a visual cue that the next
/// keypress will be consumed as the new binding.
fn render_keymap_mode(f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
    let block = Block::default()
        .title(Span::styled(" Keys ", theme.title()))
        .borders(Borders::ALL)
        .border_style(theme.border(focus));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve bottom row for hints.
    let list_area = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(1),
    );

    // Clamp the cursor in case NavAction::ALL shrank since last
    // render (defensive — should never happen at runtime).
    if app.keymap_selected >= NavAction::ALL.len() {
        app.keymap_selected = NavAction::ALL.len().saturating_sub(1);
    }

    let capture_target = app.keymap_capture;
    let items: Vec<ListItem> = NavAction::ALL
        .iter()
        .map(|&action| {
            let is_selected = action == NavAction::ALL[app.keymap_selected];
            let is_capturing = capture_target == Some(action);
            let binding_str = if is_capturing {
                "[press a key…]".to_string()
            } else {
                app.keymap
                    .get(action)
                    .map(key_event_label)
                    .unwrap_or_else(|| "[unbound]".to_string())
            };
            let label = action.label();
            // The capturing row uses a dedicated accent + bold
            // treatment so the user can tell at a glance that the
            // next keypress will be eaten (vs. a normal highlighted
            // row that just shows the current binding).
            let binding_style = if is_capturing {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent)
            };
            let line = Line::from(vec![
                Span::styled("  ", theme.dim()),
                Span::styled(format!("{:<16}", label), theme.fg),
                Span::styled(format!("{:>10}", binding_str), binding_style),
            ]);
            let mut item = ListItem::new(line);
            if is_selected {
                item = item.style(
                    Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg),
                );
            }
            item
        })
        .collect();

    let visible_h = list_area.height as usize;
    let offset = compute_offset(app.keymap_selected, items.len(), visible_h);
    let mut state = ListState::default().with_selected(Some(app.keymap_selected));
    *state.offset_mut() = offset;
    // The per-row style on the ListItem already paints the selection
    // background, so we use a blank highlight_symbol to avoid a
    // double "▸ " on top of our own "  " prefix.
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_symbol("");
    f.render_stateful_widget(list, list_area, &mut state);

    let hints = Paragraph::new(Line::from(Span::styled(
        "  j/k move · ⏎ capture · Esc clear · r reset · q exit",
        theme.dim(),
    )));
    let hint_area = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    f.render_widget(hints, hint_area);
}

/// Handle a keypress while in the Settings → Keys sub-mode.
///
/// `j`/`k` move the cursor; `Enter` (or space) arms a capture for
/// the selected row; `Esc` clears the selected row's binding; `r`
/// wipes every override; `q` exits the sub-mode. Anything else
/// returns `false` so the outer dispatcher can ignore it.
fn on_key_keymap_mode(key: KeyEvent, app: &mut App) -> bool {
    let total = NavAction::ALL.len();
    let selected = app.keymap_selected;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.keymap_selected = (selected + 1) % total;
            true
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.keymap_selected = selected.checked_sub(1).unwrap_or(total - 1);
            true
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.keymap_selected = 0;
            true
        }
        KeyCode::Char('G') | KeyCode::End => {
            app.keymap_selected = total - 1;
            true
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let action = NavAction::ALL[app.keymap_selected];
            let _ = app.tx.try_send(Action::KeymapCmd(KeymapCmd::BeginCapture(action)));
            true
        }
        KeyCode::Esc => {
            let action = NavAction::ALL[app.keymap_selected];
            let _ = app.tx.try_send(Action::KeymapCmd(KeymapCmd::Clear(action)));
            true
        }
        KeyCode::Char('r') => {
            let _ = app.tx.try_send(Action::KeymapCmd(KeymapCmd::ResetAll));
            true
        }
        KeyCode::Char('q') => {
            let _ = app.tx.try_send(Action::KeymapCmd(KeymapCmd::ExitMode));
            true
        }
        _ => false,
    }
}

fn row(label: &str, value: &str, key: &str, theme: &Theme) -> ListItem<'static> {
    let line = Line::from(vec![
        Span::styled("  ", theme.dim()),
        Span::styled(format!("{:<12}", label), theme.fg),
        Span::styled(format!("{:<16}", value), theme.accent),
        Span::styled(format!("  ({key})"), theme.dim()),
    ]);
    ListItem::new(line)
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// Compute the scroll offset that keeps `selected` visible inside a window
/// of `visible` rows drawn from a list of `total` items. Top-aligned:
/// shifts only when the cursor scrolls past the bottom (or top) edge of
/// the visible window, so the view visually tracks the cursor immediately
/// instead of waiting until the cursor reaches the middle (which is what a
/// centred offset does, and which makes long lists look frozen at the top
/// until you've already half-scrolled). PgUp/PgDn still feel symmetric
/// because each call recomputes from the current cursor.
fn compute_offset(selected: usize, total: usize, visible: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    let sel = selected.min(total - 1);
    if sel >= visible {
        sel - visible + 1
    } else {
        0
    }
}

// Suppress unused warning for RunAction in case future toggles need it.
#[allow(dead_code)]
fn _r(_: RunAction) {}