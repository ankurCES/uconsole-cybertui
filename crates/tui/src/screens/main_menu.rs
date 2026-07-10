use std::cell::Cell;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::nav_state::SubMenuItem;
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::modal::QuitConfirmModal;
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;
use crate::widgets::menu_list::{MenuEntry, MenuList, MenuListState};

struct Category {
    glyph: &'static str,
    label: &'static str,
    items: &'static [ScreenId],
}

static CATEGORIES: &[Category] = &[
    Category {
        glyph: "◉",
        label: "System",
        items: &[
            ScreenId::System,
            ScreenId::Power,
            ScreenId::Display,
            ScreenId::Audio,
            ScreenId::Storage,
            ScreenId::Bluetooth,
        ],
    },
    Category {
        glyph: "≋",
        label: "Network",
        items: &[ScreenId::Network, ScreenId::LoRa, ScreenId::City],
    },
    Category {
        glyph: "⌕",
        label: "Security",
        items: &[ScreenId::Intel, ScreenId::Recon],
    },
    Category {
        glyph: "▢",
        label: "Tools",
        items: &[
            ScreenId::Files,
            ScreenId::Processes,
            ScreenId::Services,
            ScreenId::Packages,
        ],
    },
    Category {
        glyph: "✱",
        label: "Settings",
        items: &[ScreenId::Settings, ScreenId::Logs],
    },
];

pub struct MainMenuScreen {
    // Cell allows scroll-offset write-back from &self render.
    state: Cell<MenuListState>,
}

impl Default for MainMenuScreen {
    fn default() -> Self {
        Self { state: Cell::new(MenuListState::default()) }
    }
}

impl ScreenV2 for MainMenuScreen {
    fn id(&self) -> ScreenId {
        ScreenId::MainMenu
    }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let mut s = self.state.get();
        match event {
            NavEvent::Up => {
                s.move_up();
                self.state.set(s);
                Consumed::Yes
            }
            NavEvent::Down => {
                s.move_down(CATEGORIES.len());
                self.state.set(s);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let cat = &CATEGORIES[s.selected];
                ctx.nav.submenu_category = cat.label.to_owned();
                ctx.nav.submenu_items = cat
                    .items
                    .iter()
                    .map(|&id| SubMenuItem { screen_id: id })
                    .collect();
                ctx.navigate_to(ScreenId::Submenu);
                Consumed::Yes
            }
            NavEvent::Back => {
                ctx.open_modal(Box::new(QuitConfirmModal));
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        // Breadcrumb line
        let crumb: Vec<&str> = ctx.nav.stack.breadcrumb().map(|id| id.label()).collect();
        frame.render_widget(
            Paragraph::new(crumb.join(" > ")).alignment(Alignment::Left),
            chunks[0],
        );

        // Category list
        let entries: Vec<MenuEntry<'_>> =
            CATEGORIES.iter().map(|c| MenuEntry::new(c.glyph, c.label)).collect();

        let block = Block::default().borders(Borders::ALL).title(" ▦ MENU ");

        let mut s = self.state.get();
        frame.render_stateful_widget(MenuList::new(&entries).block(block), chunks[1], &mut s);
        self.state.set(s); // write back updated scroll offset
    }

    fn focusable_zones(&self) -> &[Zone] {
        &[Zone::Main]
    }

    fn hint(&self) -> &str {
        "▲▼ Navigate  A Select  B Back"
    }
}
