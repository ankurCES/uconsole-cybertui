use crate::app::action::Action;
use crate::modal::{Modal, ModalResult};
use crate::nav::event::NavEvent;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub struct ConfirmModal {
    pub title: String,
    pub message: String,
    /// true = Yes highlighted, false = No highlighted.
    yes_selected: bool,
}

impl ConfirmModal {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self { title: title.into(), message: message.into(), yes_selected: false }
    }
}

impl Modal for ConfirmModal {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult {
        match event {
            NavEvent::Left | NavEvent::Right => {
                self.yes_selected = !self.yes_selected;
                ModalResult::Consumed
            }
            NavEvent::Confirm => {
                let val = if self.yes_selected { "yes" } else { "no" };
                ModalResult::Submitted(val.to_owned())
            }
            NavEvent::Back => ModalResult::Dismissed,
            _              => ModalResult::Consumed,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .title(format!(" {} ", self.title))
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

    fn commit_action(&self, value: String) -> Action {
        Action::SubmitInput(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal() -> ConfirmModal {
        ConfirmModal::new("Confirm", "Are you sure?")
    }

    #[test]
    fn default_selection_is_no() {
        let mut m = modal();
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "no"));
    }

    #[test]
    fn left_right_toggles_to_yes() {
        let mut m = modal();
        m.on_nav(NavEvent::Right);
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "yes"));
    }

    #[test]
    fn double_toggle_returns_to_no() {
        let mut m = modal();
        m.on_nav(NavEvent::Left);
        m.on_nav(NavEvent::Left);
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "no"));
    }

    #[test]
    fn back_dismisses() {
        let mut m = modal();
        assert!(matches!(m.on_nav(NavEvent::Back), ModalResult::Dismissed));
    }
}
