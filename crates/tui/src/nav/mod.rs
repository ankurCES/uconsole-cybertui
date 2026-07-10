pub mod dispatch;
pub mod event;
pub mod menu_stack;

use tokio::sync::mpsc;

pub use event::{key_to_nav, key_to_nav_opt, Consumed, NavEvent};
pub use menu_stack::{MenuStack, PopResult};

use crate::app::action::Action;
use crate::app::live_data::LiveData;
use crate::app::nav_state::NavigationState;
use crate::app::screen::ScreenId;
use crate::app::toast::ToastKind;
use crate::app::ui_state::UiState;
use crate::modal::Modal;
use crate::prefs::Prefs;

/// Narrow view passed to on_nav() and render(). Mutable for
/// toast/modal push and navigation mutations.
pub struct UiContext<'a> {
    pub live:  &'a LiveData,
    pub prefs: &'a Prefs,
    pub ui:    &'a mut UiState,
    pub nav:   &'a mut NavigationState,
    pub tx:    &'a mpsc::Sender<Action>,
}

impl<'a> UiContext<'a> {
    pub fn push_toast(&mut self, kind: ToastKind, msg: impl Into<String>) {
        self.ui.push_toast(kind, msg);
    }

    pub fn open_modal(&mut self, m: Box<dyn Modal>) {
        self.ui.open_modal(m);
    }

    pub fn navigate_to(&mut self, id: ScreenId) {
        self.nav.stack.push(id);
        self.nav.focus_zone = 0;
    }

    pub fn go_back(&mut self) -> PopResult {
        self.nav.stack.pop()
    }

    pub fn queue_action(&self, a: Action) {
        self.tx.try_send(a).ok();
    }
}
