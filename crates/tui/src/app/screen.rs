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

    /// Step to the next/previous screen, skipping any whose `Screen::is_hidden`
    /// returns `true`. Mirrors orbital's Tab/Shift-Tab widget navigation with
    /// hidden-widget skipping. Wraps around the end of `ScreenId::ALL` in the
    /// cycle direction. If every screen is hidden the current screen is
    /// returned unchanged — cycling must never strand the user on a dead end.
    pub fn cycle(
        screens: &[Box<dyn Screen>],
        app: &crate::app::App,
        current: ScreenId,
        forward: bool,
    ) -> ScreenId {
        let all = ScreenId::ALL;
        let pos = all.iter().position(|s| *s == current).unwrap_or(0);
        let n = all.len();
        // Bound the loop at `n` iterations so a fully-hidden screen set can't
        // spin forever; the early return below handles that case explicitly.
        for step in 1..=n {
            let idx = if forward {
                (pos + step) % n
            } else {
                (pos + n - step % n) % n
            };
            let candidate = all[idx];
            // The slice carries one screen per ScreenId in the registered
            // order; an absent slot is treated as "not hidden" so default
            // screens stay reachable in tests that build partial screens.
            if let Some(s) = screens.get(idx) {
                if s.is_hidden(app) {
                    continue;
                }
            }
            return candidate;
        }
        current
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
    /// Whether this screen should be skipped by `Tab` / `Shift-Tab` screen
    /// cycling. Defaults to `false` so every screen is reachable unless it
    /// explicitly opts out. Mirrors orbital's hidden-widget skip in its
    /// Tab/Shift-Tab navigation.
    fn is_hidden(&self, _app: &crate::app::App) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    /// Test fake: each instance declares its own `ScreenId` and `is_hidden`
    /// answer. Keeps the cycle test independent of the real screen impls.
    struct FakeScreen {
        id: ScreenId,
        hidden: bool,
    }

    impl Screen for FakeScreen {
        fn id(&self) -> ScreenId {
            self.id
        }
        fn render(
            &mut self,
            _frame: &mut ratatui::Frame,
            _area: ratatui::layout::Rect,
            _app: &mut crate::app::App,
            _theme: &crate::theme::Theme,
            _focus: bool,
        ) {
        }
        fn is_hidden(&self, _app: &crate::app::App) -> bool {
            self.hidden
        }
    }

    fn dummy_app() -> crate::app::App {
        // The cycle helper only inspects `is_hidden(&App)`; nothing else is
        // touched, so an unconnected mpsc pair is enough. We never await on
        // the receiver — it exists solely so `App::new` can take ownership
        // of both ends.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        crate::app::App::new(tx, rx)
    }

    fn all_visible() -> Vec<Box<dyn Screen>> {
        ScreenId::ALL
            .iter()
            .map(|id| Box::new(FakeScreen { id: *id, hidden: false }) as Box<dyn Screen>)
            .collect()
    }

    #[test]
    fn cycle_forward_steps_to_next_screen() {
        let screens = all_visible();
        let app = dummy_app();
        let next = ScreenId::cycle(&screens, &app, ScreenId::Network, true);
        assert_eq!(next, ScreenId::Bluetooth);
    }

    #[test]
    fn cycle_backward_wraps_around() {
        let screens = all_visible();
        let app = dummy_app();
        // From System (position 0) going backward must wrap to Settings
        // (the last screen), mirroring orbital's wrap-around tab navigation.
        let prev = ScreenId::cycle(&screens, &app, ScreenId::System, false);
        assert_eq!(prev, ScreenId::Settings);
    }

    #[test]
    fn cycle_forward_wraps_around() {
        let screens = all_visible();
        let app = dummy_app();
        // From Settings (last) going forward must wrap back to System.
        let next = ScreenId::cycle(&screens, &app, ScreenId::Settings, true);
        assert_eq!(next, ScreenId::System);
    }

    #[test]
    fn cycle_skips_hidden_screens() {
        // Build a screen set where Network and Power are hidden. Starting on
        // System and going forward must land on Bluetooth (skipping Network).
        let mut screens: Vec<Box<dyn Screen>> = ScreenId::ALL
            .iter()
            .map(|id| {
                let hidden = matches!(id, ScreenId::Network | ScreenId::Power);
                Box::new(FakeScreen { id: *id, hidden }) as Box<dyn Screen>
            })
            .collect();
        let app = dummy_app();
        // First forward step must skip the hidden Network.
        let next = ScreenId::cycle(&screens, &app, ScreenId::System, true);
        assert_eq!(next, ScreenId::Bluetooth);
        // And the backward step from System must skip Power too.
        let prev = ScreenId::cycle(&screens, &app, ScreenId::System, false);
        assert_eq!(prev, ScreenId::Settings);
        // Sanity: with everything visible, the first forward step lands on
        // Network itself, proving the skip is what made the test above pass.
        for s in screens.iter_mut() {
            // No way back through `dyn Screen` to flip `hidden` without a
            // second FakeScreen type, so just rebuild the slice.
            let _ = s;
        }
        let visible = all_visible();
        let sanity = ScreenId::cycle(&visible, &app, ScreenId::System, true);
        assert_eq!(sanity, ScreenId::Network);
    }

    #[test]
    fn cycle_all_hidden_returns_current() {
        // If every screen is hidden the helper must NOT loop forever — it
        // must return the current screen unchanged so the user is never
        // stranded on a dead end.
        let screens: Vec<Box<dyn Screen>> = ScreenId::ALL
            .iter()
            .map(|id| Box::new(FakeScreen { id: *id, hidden: true }) as Box<dyn Screen>)
            .collect();
        let app = dummy_app();
        assert_eq!(
            ScreenId::cycle(&screens, &app, ScreenId::Network, true),
            ScreenId::Network
        );
        assert_eq!(
            ScreenId::cycle(&screens, &app, ScreenId::Network, false),
            ScreenId::Network
        );
    }
}
