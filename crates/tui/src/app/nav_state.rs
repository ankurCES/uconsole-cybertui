use crate::app::screen::ScreenId;
use crate::nav::menu_stack::MenuStack;

/// One item in a submenu — a leaf screen the user can navigate to.
pub struct SubMenuItem {
    pub screen_id: ScreenId,
}

pub struct NavigationState {
    pub stack:             MenuStack,
    /// Which focusable zone within the current screen is active.
    pub focus_zone:        usize,
    /// Display name of the currently open submenu category (e.g. "Network").
    pub submenu_category:  String,
    /// Items shown in the Submenu screen; populated by MainMenuScreen on Confirm.
    pub submenu_items:     Vec<SubMenuItem>,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            stack:            MenuStack::new(),
            focus_zone:       0,
            submenu_category: String::new(),
            submenu_items:    Vec::new(),
        }
    }
}
