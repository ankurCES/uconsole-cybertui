use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::nav::event::NavEvent;
use crate::theme::Theme;

pub enum MenuAction {
    Consumed,
    Deactivate,
    ConfirmPowerOff,
    ConfirmReboot,
    ConfirmSuspend,
    ExitTui,
    OpenAbout,
}

const MENUS: &[&str] = &["System", "About"];
const SYSTEM_ITEMS: &[&str] = &["Power Off...", "Restart...", "Suspend...", "Exit TUI..."];

pub struct TopMenuBar {
    pub active: bool,
    menu_open: bool,
    active_menu: usize,
    selected_item: usize,
}

impl Default for TopMenuBar {
    fn default() -> Self {
        Self { active: false, menu_open: false, active_menu: 0, selected_item: 0 }
    }
}

impl TopMenuBar {
    pub fn close(&mut self) {
        self.active = false;
        self.menu_open = false;
        self.active_menu = 0;
        self.selected_item = 0;
    }

    pub fn on_nav(&mut self, event: NavEvent) -> MenuAction {
        if self.menu_open { self.nav_dropdown(event) } else { self.nav_bar(event) }
    }

    fn nav_bar(&mut self, event: NavEvent) -> MenuAction {
        match event {
            NavEvent::Left => {
                if self.active_menu > 0 { self.active_menu -= 1; }
                MenuAction::Consumed
            }
            NavEvent::Right | NavEvent::Tab => {
                if self.active_menu + 1 < MENUS.len() { self.active_menu += 1; }
                MenuAction::Consumed
            }
            NavEvent::Down | NavEvent::Confirm => match self.active_menu {
                0 => { self.menu_open = true; self.selected_item = 0; MenuAction::Consumed }
                1 => MenuAction::OpenAbout,
                _ => MenuAction::Consumed,
            },
            NavEvent::Back => MenuAction::Deactivate,
            _ => MenuAction::Consumed,
        }
    }

    fn nav_dropdown(&mut self, event: NavEvent) -> MenuAction {
        let item_count = match self.active_menu {
            0 => SYSTEM_ITEMS.len(),
            _ => 0,
        };
        match event {
            NavEvent::Up => {
                if self.selected_item > 0 { self.selected_item -= 1; }
                MenuAction::Consumed
            }
            NavEvent::Down => {
                if self.selected_item + 1 < item_count { self.selected_item += 1; }
                MenuAction::Consumed
            }
            NavEvent::Confirm => {
                let idx = self.selected_item;
                self.close();
                system_action(idx)
            }
            NavEvent::Back => {
                self.menu_open = false;
                self.selected_item = 0;
                MenuAction::Consumed
            }
            NavEvent::Left => {
                self.menu_open = false;
                self.selected_item = 0;
                if self.active_menu > 0 { self.active_menu -= 1; }
                MenuAction::Consumed
            }
            NavEvent::Right => {
                self.menu_open = false;
                self.selected_item = 0;
                if self.active_menu + 1 < MENUS.len() { self.active_menu += 1; }
                MenuAction::Consumed
            }
            _ => MenuAction::Consumed,
        }
    }

    pub fn render_bar(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut spans = vec![
            Span::styled(" ▦ ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        ];
        for (i, title) in MENUS.iter().enumerate() {
            let style = if self.active && self.active_menu == i && !self.menu_open {
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else if self.active && self.active_menu == i {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim)
            };
            spans.push(Span::styled(format!(" {} ", title), style));
        }
        let hint = if self.active {
            Span::styled("  F10/Esc:close  ←→:nav  ↓/Enter:open", Style::default().fg(theme.dim))
        } else {
            Span::styled("  F10:menu", Style::default().fg(theme.dim))
        };
        spans.push(hint);
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(Style::default().fg(theme.fg).bg(theme.bg)),
            area,
        );
    }

    /// Render the open dropdown as a floating overlay. Call this last in draw_v2.
    pub fn render_dropdown(&self, frame: &mut Frame, bar_area: Rect, theme: &Theme) {
        if !self.active || !self.menu_open { return; }
        let items = match self.active_menu {
            0 => SYSTEM_ITEMS,
            _ => return,
        };

        // Compute x offset: " ▦ " (3) + preceding menus " {title} "
        let x_base: u16 = 3 + MENUS[..self.active_menu]
            .iter()
            .map(|m| m.len() as u16 + 2)
            .sum::<u16>();
        let max_item = items.iter().map(|s| s.len()).max().unwrap_or(8) as u16;
        let width = max_item + 4; // " {label} " + 2 borders
        let height = items.len() as u16 + 2;

        let full = frame.area();
        let x = (bar_area.x + x_base).min(full.width.saturating_sub(width));
        let y = bar_area.y + bar_area.height; // just below the menu bar row
        let rect = Rect {
            x,
            y,
            width: width.min(full.width.saturating_sub(x)),
            height: height.min(full.height.saturating_sub(y)),
        };

        let list_items: Vec<ListItem> = items.iter().enumerate()
            .map(|(i, label)| {
                let style = if i == self.selected_item {
                    Style::default().fg(theme.selection_fg).bg(theme.selection_bg)
                } else {
                    Style::default().fg(theme.fg)
                };
                ListItem::new(format!(" {} ", label)).style(style)
            })
            .collect();

        frame.render_widget(Clear, rect);
        frame.render_widget(
            List::new(list_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border_focus))
                    .style(Style::default().bg(theme.bg)),
            ),
            rect,
        );
    }
}

fn system_action(idx: usize) -> MenuAction {
    match idx {
        0 => MenuAction::ConfirmPowerOff,
        1 => MenuAction::ConfirmReboot,
        2 => MenuAction::ConfirmSuspend,
        3 => MenuAction::ExitTui,
        _ => MenuAction::Consumed,
    }
}
