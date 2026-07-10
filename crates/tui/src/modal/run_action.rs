use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::action::Action;
use crate::modal::{Modal, ModalResult};
use crate::nav::event::NavEvent;
use crate::theme::Theme;

/// Confirm dialog that, on "Yes", dispatches a pre-built `Action` directly.
/// This lets v2 screens do destructive operations (kill, stop, reboot) without
/// needing a custom Modal per action type.
pub struct RunActionModal {
    message: String,
    action: Action,
    yes_selected: bool,
}

impl RunActionModal {
    pub fn new(message: impl Into<String>, action: Action) -> Self {
        Self { message: message.into(), action, yes_selected: false }
    }
}

impl Modal for RunActionModal {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult {
        match event {
            NavEvent::Left | NavEvent::Right => {
                self.yes_selected = !self.yes_selected;
                ModalResult::Consumed
            }
            NavEvent::Confirm => {
                if self.yes_selected {
                    ModalResult::Submitted("yes".into())
                } else {
                    ModalResult::Dismissed
                }
            }
            NavEvent::Back => ModalResult::Dismissed,
            _ => ModalResult::Consumed,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .title(" confirm ")
            .borders(Borders::ALL)
            .style(Style::default().fg(theme.border_focus).bg(theme.bg));

        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
            .split(inner);

        frame.render_widget(
            Paragraph::new(self.message.as_str())
                .style(Style::default().fg(theme.fg))
                .alignment(ratatui::layout::Alignment::Center),
            rows[0],
        );

        let (yes_style, no_style) = if self.yes_selected {
            (
                Style::default().fg(theme.selection_fg).bg(theme.selection_bg).add_modifier(Modifier::BOLD),
                Style::default().fg(theme.fg),
            )
        } else {
            (
                Style::default().fg(theme.fg),
                Style::default().fg(theme.selection_fg).bg(theme.selection_bg).add_modifier(Modifier::BOLD),
            )
        };

        let btn_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[2]);

        frame.render_widget(
            Paragraph::new(" [ Yes ] ").style(yes_style).alignment(ratatui::layout::Alignment::Center),
            btn_row[0],
        );
        frame.render_widget(
            Paragraph::new(" [ No ] ").style(no_style).alignment(ratatui::layout::Alignment::Center),
            btn_row[1],
        );
    }

    fn commit_action(&self, _value: String) -> Action {
        self.action.clone()
    }
}
