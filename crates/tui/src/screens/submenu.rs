use std::cell::Cell;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;
use crate::widgets::menu_list::{MenuEntry, MenuList, MenuListState};

pub struct SubMenuScreen {
    state: Cell<MenuListState>,
}

impl Default for SubMenuScreen {
    fn default() -> Self {
        Self { state: Cell::new(MenuListState::default()) }
    }
}

impl ScreenV2 for SubMenuScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Submenu
    }

    fn on_focus(&mut self, _ctx: &mut UiContext<'_>) {
        // Reset cursor each time we enter the submenu.
        self.state.set(MenuListState::default());
    }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let items_len = ctx.nav.submenu_items.len();
        let mut s = self.state.get();
        match event {
            NavEvent::Up => {
                s.move_up();
                self.state.set(s);
                Consumed::Yes
            }
            NavEvent::Down => {
                s.move_down(items_len);
                self.state.set(s);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                if let Some(item) = ctx.nav.submenu_items.get(s.selected) {
                    ctx.navigate_to(item.screen_id);
                }
                Consumed::Yes
            }
            // B / Esc: let dispatch handle stack pop (returns to MainMenu).
            NavEvent::Back => Consumed::No,
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        // Breadcrumb: replace ScreenId::Submenu label with the actual category name.
        let crumb: Vec<&str> = ctx
            .nav
            .stack
            .breadcrumb()
            .map(|id| {
                if id == ScreenId::Submenu {
                    ctx.nav.submenu_category.as_str()
                } else {
                    id.label()
                }
            })
            .collect();
        frame.render_widget(
            Paragraph::new(crumb.join(" > ")).alignment(Alignment::Left),
            chunks[0],
        );

        // Item list sourced from NavigationState.
        let entries: Vec<MenuEntry<'_>> = ctx
            .nav
            .submenu_items
            .iter()
            .map(|item| MenuEntry::new(item.screen_id.glyph(), item.screen_id.label()))
            .collect();

        let title = format!(" {} ", ctx.nav.submenu_category);
        let block = Block::default().borders(Borders::ALL).title(title.as_str());

        let mut s = self.state.get();
        frame.render_stateful_widget(MenuList::new(&entries).block(block), chunks[1], &mut s);
        self.state.set(s);
    }

    fn focusable_zones(&self) -> &[Zone] {
        &[Zone::Main]
    }

    fn hint(&self) -> &str {
        "▲▼ Navigate  A Select  B Back"
    }
}
