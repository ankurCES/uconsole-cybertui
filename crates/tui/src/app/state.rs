use std::sync::Arc;
use tokio::sync::mpsc;

use crate::app::action::Action;
use crate::app::live_data::LiveData;
use crate::app::nav_state::NavigationState;
use crate::app::ui_state::UiState;
use crate::prefs::Prefs;

/// Decomposed application state — replaces the 100-field god-object App.
/// Four focused sub-structs; no async code lives here.
pub struct AppState {
    pub nav:   NavigationState,
    pub live:  Arc<LiveData>,
    pub ui:    UiState,
    pub prefs: Prefs,
    pub tx:    mpsc::Sender<Action>,
    pub rx:    mpsc::Receiver<Action>,
}

impl AppState {
    pub fn new(
        prefs: Prefs,
        live: Arc<LiveData>,
        tx: mpsc::Sender<Action>,
        rx: mpsc::Receiver<Action>,
    ) -> Self {
        let theme_name = prefs.theme;
        Self {
            nav: NavigationState::new(),
            live,
            ui: UiState::new(theme_name),
            prefs,
            tx,
            rx,
        }
    }
}
