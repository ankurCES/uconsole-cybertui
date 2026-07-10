use std::collections::VecDeque;

use chrono::{DateTime, Local};

use crate::app::toast::{Toast, ToastKind};
use crate::modal::Modal;
use crate::theme::{Theme, ThemeName};
use crate::ui::top_menu::TopMenuBar;

// re-use cap from parent module
use super::{ToastEntry, TOAST_HISTORY_CAP};

pub struct UiState {
    pub theme:         Theme,
    pub toasts:        VecDeque<Toast>,
    pub toast_history: VecDeque<ToastEntry>,
    pub modal:         Option<Box<dyn Modal>>,
    pub status_msg:    Option<String>,
    pub clock:         DateTime<Local>,
    pub top_menu:      TopMenuBar,
}

impl UiState {
    pub fn new(theme_name: ThemeName) -> Self {
        Self {
            theme:         Theme::by_name(theme_name),
            toasts:        VecDeque::new(),
            toast_history: VecDeque::new(),
            modal:         None,
            status_msg:    None,
            clock:         Local::now(),
            top_menu:      TopMenuBar::default(),
        }
    }

    pub fn push_toast(&mut self, kind: ToastKind, msg: impl Into<String>) {
        let text = msg.into();
        self.toasts.push_back(Toast::new(kind, text.clone()));
        if self.toast_history.len() >= TOAST_HISTORY_CAP {
            self.toast_history.pop_front();
        }
        self.toast_history.push_back(ToastEntry { ts: Local::now(), kind, message: text });
    }

    pub fn open_modal(&mut self, m: Box<dyn Modal>) {
        self.modal = Some(m);
    }

    pub fn modal_accepts_text_input(&self) -> bool {
        self.modal.as_ref().map_or(false, |m| m.accepts_text_input())
    }
}
