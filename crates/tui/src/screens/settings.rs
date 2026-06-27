//! Settings screen: theme, mouse, Nerd Font, web server toggle, refresh rate.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::action::{Action, RunAction};
use crate::app::screen::{Screen, ScreenId, SettingsKey};
use crate::app::App;
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
        match key.code {
            KeyCode::Char('t') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::Theme));
            }
            KeyCode::Char('m') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::Mouse));
            }
            KeyCode::Char('n') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::NerdFont));
            }
            KeyCode::Char('w') => {
                let _ = app.tx.try_send(Action::Toggle(SettingsKey::WebServer));
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Settings ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let theme_name = match app.theme_name {
            crate::app::screen::ThemeNameReexport::Dark => "dark",
            crate::app::screen::ThemeNameReexport::Light => "light",
            crate::app::screen::ThemeNameReexport::HighContrast => "high-contrast",
        };
        let web_state = app.live.web_enabled.try_read().map(|b| *b).unwrap_or(false);

        let items: Vec<ListItem> = vec![
            row("theme", theme_name, "t", theme),
            row("mouse", bool_str(app.mouse), "m", theme),
            row("nerd font", bool_str(app.nerd_font), "n", theme),
            row("web server", bool_str(web_state), "w", theme),
        ];
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        let hints = Paragraph::new(Line::from(Span::styled(
            "  t theme · m mouse · n nerd font · w web server",
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

// Suppress unused warning for RunAction in case future toggles need it.
#[allow(dead_code)]
fn _r(_: RunAction) {}
