use crate::app::action::Action;
use crate::modal::{Modal, ModalResult};
use crate::nav::event::NavEvent;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub struct InputModal {
    pub title: String,
    pub prompt: String,
    pub buf: String,
}

impl InputModal {
    pub fn new(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self { title: title.into(), prompt: prompt.into(), buf: String::new() }
    }
}

impl Modal for InputModal {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult {
        match event {
            NavEvent::Char(c)   => { self.buf.push(c); ModalResult::Consumed }
            NavEvent::Backspace => { self.buf.pop(); ModalResult::Consumed }
            NavEvent::Confirm   => ModalResult::Submitted(self.buf.clone()),
            NavEvent::Back      => ModalResult::Dismissed,
            _                   => ModalResult::Consumed,
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
            Paragraph::new(self.prompt.as_str()).style(Style::default().fg(theme.fg)),
            rows[0],
        );

        let display = format!("{}_", self.buf);
        frame.render_widget(
            Paragraph::new(display).style(Style::default().fg(theme.accent)),
            rows[2],
        );
    }

    fn accepts_text_input(&self) -> bool {
        true
    }

    fn commit_action(&self, value: String) -> Action {
        Action::SubmitInput(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal() -> InputModal {
        InputModal::new("Test", "Enter value:")
    }

    #[test]
    fn chars_accumulate_and_confirm_returns_submitted() {
        let mut m = modal();
        m.on_nav(NavEvent::Char('h'));
        m.on_nav(NavEvent::Char('i'));
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "hi"));
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut m = modal();
        m.on_nav(NavEvent::Char('a'));
        m.on_nav(NavEvent::Char('b'));
        m.on_nav(NavEvent::Backspace);
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "a"));
    }

    #[test]
    fn back_dismisses() {
        let mut m = modal();
        assert!(matches!(m.on_nav(NavEvent::Back), ModalResult::Dismissed));
    }

    #[test]
    fn nav_keys_consumed_without_side_effects() {
        let mut m = modal();
        assert!(matches!(m.on_nav(NavEvent::Up), ModalResult::Consumed));
        assert!(matches!(m.on_nav(NavEvent::Down), ModalResult::Consumed));
    }
}
