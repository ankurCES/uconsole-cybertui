//! One stub per remaining screen so the module compiles. Real
//! implementations land in milestones 3-5.
#![allow(dead_code)] // stubs are referenced by ScreenId dispatch once Phase-3 wires them up

pub mod audio;
pub mod bluetooth;
pub mod main_menu;
pub mod submenu;
pub mod city;
pub mod display;
pub mod editor;
pub mod files;
pub mod logs;
pub mod network;
pub mod overworld; // Phase 7 — Carousel front door.
pub mod packages;
pub mod power;
pub mod processes;
pub mod recon; // Phase 7 M7 — 7-tab OSINT action console.
pub mod services;
pub mod settings;
pub mod storage;
pub mod system;
pub mod lora;
pub mod intel;

// ── ScreenV2 migrations ──────────────────────────────────────────────────────
pub mod stubs_v2;
pub mod system_v2;
pub mod network_v2;
pub mod lora_v2;
pub mod intel_v2;
pub mod recon_v2;
pub mod city_v2;
pub mod settings_v2;
pub mod bluetooth_v2;
pub mod power_v2;
pub mod storage_v2;
pub mod packages_v2;
pub mod processes_v2;
pub mod services_v2;
pub mod files_v2;
pub mod logs_v2;
pub mod display_v2;
pub mod audio_v2;
pub mod editor_v2;
pub mod screensaver_v2;
pub mod ai_v2;
pub mod ai_logs_v2;

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

macro_rules! stub_screen {
    ($name:ident, $id:expr, $title:expr) => {
        pub struct $name;
        impl Screen for $name {
            fn id(&self) -> ScreenId {
                $id
            }
            fn title(&self) -> &'static str {
                $title
            }
            fn render(
                &mut self,
                f: &mut Frame,
                area: Rect,
                _app: &mut App,
                theme: &Theme,
                focus: bool,
            ) {
                let p = Paragraph::new(Line::from(format!("{} — coming up next.", $title)))
                    .style(ratatui::style::Style::default().fg(theme.dim).bg(theme.bg))
                    .block(
                        Block::default()
                            .title(format!(" {} ", $title))
                            .borders(Borders::ALL)
                            .border_style(theme.border(focus)),
                    );
                f.render_widget(p, area);
            }
        }
    };
}

stub_screen!(BluetoothScreen, ScreenId::Bluetooth, "Bluetooth");
stub_screen!(FilesScreen, ScreenId::Files, "Files");
stub_screen!(LogsScreen, ScreenId::Logs, "Logs");
stub_screen!(SettingsScreen, ScreenId::Settings, "Settings");
