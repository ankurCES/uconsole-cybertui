use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::action::Action;
use crate::modal::{Modal, ModalResult};
use crate::nav::event::NavEvent;
use crate::theme::Theme;

pub struct AboutModal;

impl Modal for AboutModal {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult {
        match event {
            NavEvent::Confirm | NavEvent::Back => ModalResult::Dismissed,
            _ => ModalResult::Consumed,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .title(" about ")
            .borders(Borders::ALL)
            .style(Style::default().fg(theme.border_focus).bg(theme.bg));

        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // blank
                Constraint::Length(1), // built by
                Constraint::Length(1), // url
                Constraint::Min(0),    // spacer + ok button
            ])
            .split(inner);

        let centered = |s: &'static str, style: Style| {
            Paragraph::new(Line::from(Span::styled(s, style)))
                .alignment(Alignment::Center)
        };

        frame.render_widget(
            centered("CyberDeck TUI", Style::default().fg(theme.accent)),
            rows[0],
        );
        frame.render_widget(
            centered("Built with \u{2665} by Lumi", Style::default().fg(theme.fg)),
            rows[2],
        );
        frame.render_widget(
            centered("www.cesltd.com", Style::default().fg(theme.dim)),
            rows[3],
        );
        frame.render_widget(
            centered("[ OK ]", Style::default().fg(theme.selection_fg).bg(theme.selection_bg)),
            rows[4],
        );
    }

    fn commit_action(&self, _value: String) -> Action {
        Action::Tick // unreachable: on_nav only returns Dismissed
    }
}
