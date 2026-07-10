//! Stub ScreenV2 implementations — "coming soon" placeholder for each
//! screen not yet fully ported. All live in one file to minimise noise.
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::Frame;
use ratatui::widgets::Paragraph;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

macro_rules! stub_v2 {
    // visible variant (is_hidden = false)
    ($name:ident, $id:expr, $title:literal) => {
        pub struct $name;
        impl ScreenV2 for $name {
            fn id(&self) -> ScreenId { $id }
            fn title(&self) -> &str { $title }
            fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
            fn hint(&self) -> &str { "B back" }
            fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
                if event == NavEvent::Back { ctx.go_back(); Consumed::Yes } else { Consumed::No }
            }
            fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
                let y = area.y + area.height.saturating_sub(1) / 2;
                frame.render_widget(
                    Paragraph::new(format!("[ {} ] — coming soon", $title))
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(ctx.ui.theme.dim)),
                    Rect { y, height: 1, ..area },
                );
            }
        }
    };
    // hidden variant (Editor)
    ($name:ident, $id:expr, $title:literal, hidden) => {
        pub struct $name;
        impl ScreenV2 for $name {
            fn id(&self) -> ScreenId { $id }
            fn title(&self) -> &str { $title }
            fn is_hidden(&self) -> bool { true }
            fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
            fn hint(&self) -> &str { "B back" }
            fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
                if event == NavEvent::Back { ctx.go_back(); Consumed::Yes } else { Consumed::No }
            }
            fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
                let y = area.y + area.height.saturating_sub(1) / 2;
                frame.render_widget(
                    Paragraph::new(format!("[ {} ] — coming soon", $title))
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(ctx.ui.theme.dim)),
                    Rect { y, height: 1, ..area },
                );
            }
        }
    };
}

stub_v2!(BluetoothScreenV2,  ScreenId::Bluetooth,  "Bluetooth");
stub_v2!(PowerScreenV2,      ScreenId::Power,      "Power");
stub_v2!(DisplayScreenV2,    ScreenId::Display,    "Display");
stub_v2!(AudioScreenV2,      ScreenId::Audio,      "Audio");
stub_v2!(StorageScreenV2,    ScreenId::Storage,    "Storage");
stub_v2!(PackagesScreenV2,   ScreenId::Packages,   "Packages");
stub_v2!(ProcessesScreenV2,  ScreenId::Processes,  "Processes");
stub_v2!(ServicesScreenV2,   ScreenId::Services,   "Services");
stub_v2!(FilesScreenV2,      ScreenId::Files,      "Files");
stub_v2!(LogsScreenV2,       ScreenId::Logs,       "Logs");
stub_v2!(SettingsScreenV2,   ScreenId::Settings,   "Settings");
stub_v2!(EditorScreenV2,     ScreenId::Editor,     "Editor",   hidden);
