use crate::app::action::Action;
use crate::nav::event::NavEvent;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::Frame;

pub mod choice;
pub mod confirm;
pub mod input;
pub mod overlay;
pub mod progress;
pub mod secret;

pub use choice::ChoiceModal;
pub use confirm::ConfirmModal;
pub use input::InputModal;
pub use overlay::render_modal_overlay;
pub use progress::ProgressModal;
pub use secret::SecretModal;

pub enum ModalResult {
    /// Key handled; caller state unchanged.
    Consumed,
    /// Modal dismissed without output (Esc / cancel).
    Dismissed,
    /// User submitted a value; modal is done.
    Submitted(String),
}

pub trait Modal: Send {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult;
    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme);
    /// True when Char/Backspace should reach this modal directly,
    /// bypassing hardware remap and user keymap remap.
    fn accepts_text_input(&self) -> bool {
        false
    }
    fn commit_action(&self, value: String) -> Action;
}

/// Placeholder quit-confirm modal opened when Back is pressed at the root.
pub struct QuitConfirmModal;

impl Modal for QuitConfirmModal {
    fn on_nav(&mut self, event: NavEvent) -> ModalResult {
        match event {
            NavEvent::Confirm => ModalResult::Submitted("quit".to_owned()),
            NavEvent::Back    => ModalResult::Dismissed,
            _                 => ModalResult::Consumed,
        }
    }

    fn render(&self, _frame: &mut Frame, _area: Rect, _theme: &Theme) {}

    fn commit_action(&self, _value: String) -> Action {
        Action::Quit
    }
}
