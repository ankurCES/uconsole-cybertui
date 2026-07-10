use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::action::Action;
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;
use crate::theme::{ThemeName, ALL_THEME_NAMES};

pub struct SettingsScreenV2 {
    selected: usize,
}

impl Default for SettingsScreenV2 {
    fn default() -> Self {
        Self { selected: 0 }
    }
}

impl ScreenV2 for SettingsScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Settings }
    fn title(&self) -> &str { "Settings" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   A apply theme   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let n = ALL_THEME_NAMES.len();
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down => {
                self.selected = (self.selected + 1).min(n - 1);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let name = ALL_THEME_NAMES[self.selected];
                ctx.queue_action(Action::SetTheme(name));
                Consumed::Yes
            }
            NavEvent::Back => {
                ctx.go_back();
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let current = ctx.prefs.theme;

        let block = Block::default()
            .title(Span::styled(" Settings — Theme ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(true));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(inner);

        let items: Vec<ListItem> = ALL_THEME_NAMES.iter().map(|&name| {
            let marker = if name == current { "● " } else { "  " };
            let label = format!("{}{}", marker, theme_label(name));
            let style = if name == current {
                Style::default().fg(theme.ok)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        }).collect();

        let mut list_state = ListState::default().with_selected(Some(self.selected));
        let list = List::new(items)
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, chunks[0], &mut list_state);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "A/Enter applies theme immediately",
                theme.dim(),
            ))).alignment(Alignment::Left),
            chunks[1],
        );
    }
}

fn theme_label(name: ThemeName) -> &'static str {
    match name {
        ThemeName::Dark           => "Dark",
        ThemeName::Light          => "Light",
        ThemeName::HighContrast   => "High Contrast",
        ThemeName::Cyberpunk      => "Cyberpunk",
        ThemeName::VsCodeDark     => "VS Code Dark",
        ThemeName::VsCodeLight    => "VS Code Light",
        ThemeName::CatppuccinMocha => "Catppuccin Mocha",
        ThemeName::Nord           => "Nord",
        ThemeName::GruvboxDark    => "Gruvbox Dark",
        ThemeName::SolarizedDark  => "Solarized Dark",
        ThemeName::CyberDeckNative => "CyberDeck Native",
    }
}
