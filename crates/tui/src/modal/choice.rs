use crate::app::action::Action;
use crate::modal::{Modal, ModalResult};
use crate::nav::event::NavEvent;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

pub struct ChoiceModal {
    pub title: String,
    pub options: Vec<String>,
    cursor: usize,
}

impl ChoiceModal {
    pub fn new(title: impl Into<String>, options: Vec<String>) -> Self {
        Self { title: title.into(), options, cursor: 0 }
    }
}

impl Modal for ChoiceModal {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult {
        match event {
            NavEvent::Up => {
                if self.cursor > 0 { self.cursor -= 1; }
                ModalResult::Consumed
            }
            NavEvent::Down => {
                if self.cursor + 1 < self.options.len() { self.cursor += 1; }
                ModalResult::Consumed
            }
            NavEvent::Confirm => ModalResult::Submitted(self.cursor.to_string()),
            NavEvent::Back    => ModalResult::Dismissed,
            _                 => ModalResult::Consumed,
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

        // Clamp list height to inner area
        let list_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(inner)[0];

        let items: Vec<ListItem> = self
            .options
            .iter()
            .map(|o| ListItem::new(format!("  {o}  ")))
            .collect();

        let list = List::new(items)
            .style(Style::default().fg(theme.fg).bg(theme.bg))
            .highlight_style(
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD),
            );

        let mut state = ListState::default();
        state.select(Some(self.cursor));
        frame.render_stateful_widget(list, list_area, &mut state);
    }

    fn commit_action(&self, value: String) -> Action {
        Action::SubmitInput(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal() -> ChoiceModal {
        ChoiceModal::new("Pick", vec!["Alpha".into(), "Beta".into(), "Gamma".into()])
    }

    #[test]
    fn confirm_returns_index_zero_by_default() {
        let mut m = modal();
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "0"));
    }

    #[test]
    fn down_advances_cursor() {
        let mut m = modal();
        m.on_nav(NavEvent::Down);
        m.on_nav(NavEvent::Down);
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "2"));
    }

    #[test]
    fn up_clamps_at_zero() {
        let mut m = modal();
        m.on_nav(NavEvent::Up);
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "0"));
    }

    #[test]
    fn down_clamps_at_last() {
        let mut m = modal();
        for _ in 0..10 { m.on_nav(NavEvent::Down); }
        let r = m.on_nav(NavEvent::Confirm);
        assert!(matches!(r, ModalResult::Submitted(s) if s == "2"));
    }

    #[test]
    fn back_dismisses() {
        let mut m = modal();
        assert!(matches!(m.on_nav(NavEvent::Back), ModalResult::Dismissed));
    }
}
