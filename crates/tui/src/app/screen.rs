//! One entry per screen, plus the global registry the command palette uses.

use crate::theme::ThemeName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenId {
    System,
    Network,
    Bluetooth,
    Power,
    Display,
    Audio,
    Storage,
    Services,
    Packages,
    Processes,
    Files,
    Logs,
    Settings,
}

impl ScreenId {
    pub const ALL: &'static [ScreenId] = &[
        ScreenId::System,
        ScreenId::Network,
        ScreenId::Bluetooth,
        ScreenId::Power,
        ScreenId::Display,
        ScreenId::Audio,
        ScreenId::Storage,
        ScreenId::Services,
        ScreenId::Packages,
        ScreenId::Processes,
        ScreenId::Files,
        ScreenId::Logs,
        ScreenId::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScreenId::System => "System",
            ScreenId::Network => "Network",
            ScreenId::Bluetooth => "Bluetooth",
            ScreenId::Power => "Power",
            ScreenId::Display => "Display",
            ScreenId::Audio => "Audio",
            ScreenId::Storage => "Storage",
            ScreenId::Services => "Services",
            ScreenId::Packages => "Packages",
            ScreenId::Processes => "Processes",
            ScreenId::Files => "Files",
            ScreenId::Logs => "Logs",
            ScreenId::Settings => "Settings",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            ScreenId::System => "◉",
            ScreenId::Network => "≋",
            ScreenId::Bluetooth => "⛁",
            ScreenId::Power => "🜲",
            ScreenId::Display => "▣",
            ScreenId::Audio => "♪",
            ScreenId::Storage => "▤",
            ScreenId::Services => "⚙",
            ScreenId::Packages => "▦",
            ScreenId::Processes => "≡",
            ScreenId::Files => "▢",
            ScreenId::Logs => "▥",
            ScreenId::Settings => "✱",
        }
    }
}

/// Re-export so the App state doesn't depend on the theme module directly.
pub type ThemeNameReexport = ThemeName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsKey {
    Theme,
    Mouse,
    NerdFont,
    WebServer,
}

/// A trait every screen implements. Screens are stateless functions of App
/// + the live data; they don't hold their own state beyond what App exposes.
pub trait Screen {
    fn id(&self) -> ScreenId;
    #[allow(dead_code)] // exposed for future per-screen titles
    fn title(&self) -> &'static str {
        self.id().label()
    }
    /// Render the screen into the given area of the frame. The `focus` flag
    /// indicates whether the content pane is the focused one (for borders).
    fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        app: &mut crate::app::App,
        theme: &crate::theme::Theme,
        focus: bool,
    );
    /// Handle a key event while the screen is focused. Returns true if the
    /// event was consumed. Modal handling lives in the main loop, not here.
    fn on_key(&mut self, _key: crossterm::event::KeyEvent, _app: &mut crate::app::App) -> bool {
        false
    }
}
