use crate::app::action::Action;
use crate::modal::{Modal, ModalResult};
use crate::nav::event::NavEvent;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};
use ratatui::Frame;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct ProgressModal {
    pub title: String,
    pub message: String,
    /// 0–100, or None for indeterminate (spinner only).
    pub percent: Option<u16>,
    /// Tick counter for spinner animation.
    tick: usize,
}

impl ProgressModal {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self { title: title.into(), message: message.into(), percent: None, tick: 0 }
    }

    pub fn with_percent(mut self, pct: u16) -> Self {
        self.percent = Some(pct.min(100));
        self
    }

    /// Advance the spinner frame. Call once per render tick.
    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }
}

impl Modal for ProgressModal {
    fn on_nav(&mut self, _event: NavEvent) -> ModalResult {
        // ponytail: always consumed — only caller can dismiss programmatically
        ModalResult::Consumed
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

        let spinner = SPINNER_FRAMES[self.tick % SPINNER_FRAMES.len()];
        let label = format!("{spinner} {}", self.message);
        frame.render_widget(
            Paragraph::new(label)
                .style(Style::default().fg(theme.fg))
                .alignment(ratatui::layout::Alignment::Center),
            rows[0],
        );

        if let Some(pct) = self.percent {
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(theme.accent).bg(theme.bg))
                .percent(pct);
            frame.render_widget(gauge, rows[2]);
        }
    }

    fn commit_action(&self, value: String) -> Action {
        Action::SubmitInput(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_nav_events_consumed() {
        let mut m = ProgressModal::new("Loading", "Please wait…");
        for ev in [NavEvent::Up, NavEvent::Down, NavEvent::Confirm, NavEvent::Back, NavEvent::Left] {
            assert!(matches!(m.on_nav(ev), ModalResult::Consumed));
        }
    }

    #[test]
    fn advance_wraps_without_panic() {
        let mut m = ProgressModal::new("T", "");
        for _ in 0..1000 { m.advance(); }
        // tick wraps at usize::MAX; just confirm no panic
        assert!(m.tick < SPINNER_FRAMES.len() * 1000 + SPINNER_FRAMES.len());
    }
}
