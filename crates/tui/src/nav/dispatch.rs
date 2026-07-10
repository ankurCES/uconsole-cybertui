use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::app::action::{Action, RunAction};
use crate::app::screen::ScreenRegistry;
use crate::app::state::AppState;
use crate::keymap::{resolve_keymap, NavAction};
use crate::modal::{AboutModal, ModalResult, QuitConfirmModal, RunActionModal};
use crate::nav::event::{key_to_nav, key_to_nav_opt, Consumed};
use crate::nav::menu_stack::PopResult;
use crate::nav::UiContext;
use crate::ui::top_menu::MenuAction;

/// Full key dispatch pipeline. Returns true if the key was consumed.
/// Screens receive NavEvent; raw KeyEvent never crosses the screen boundary.
pub fn dispatch_key(
    raw: KeyEvent,
    state: &mut AppState,
    screens: &mut ScreenRegistry,
    tx: &mpsc::Sender<Action>,
) -> bool {
    // 1. Hardware remap: uConsole A-button→Enter, B-button→Esc
    //    Gate: skip when a text-input modal is active so 'a'/'b' reach the buffer.
    let key = if !state.ui.modal_accepts_text_input() {
        hardware_remap(raw)
    } else {
        raw
    };

    // 2. User keymap remap (same gate)
    let key = if !state.ui.modal_accepts_text_input() {
        apply_keymap(key, &state.prefs.keymap)
    } else {
        key
    };

    // 3. Ctrl-C: hard quit (unconditional)
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        tx.try_send(Action::Quit).ok();
        return true;
    }

    // 3.5. F10 toggles the top menu bar (only when no modal is active).
    if key.code == KeyCode::F(10) && state.ui.modal.is_none() {
        if state.ui.top_menu.active {
            state.ui.top_menu.close();
        } else {
            state.ui.top_menu.active = true;
        }
        return true;
    }

    // 4. Modal routing — modal absorbs every key
    if state.ui.modal.is_some() {
        let ev = key_to_nav(key);
        let result = state.ui.modal.as_mut().unwrap().on_nav(ev);
        match result {
            ModalResult::Consumed => return true,
            ModalResult::Dismissed => {
                state.ui.modal = None;
                return true;
            }
            ModalResult::Submitted(v) => {
                let act = {
                    let m = state.ui.modal.as_ref().unwrap();
                    m.commit_action(v)
                };
                state.ui.modal = None;
                tx.try_send(act).ok();
                return true;
            }
        }
    }

    // 4.5. Top menu routing — active menu absorbs keys before the screen.
    if state.ui.top_menu.active {
        let ev = key_to_nav(key);
        return handle_top_menu(ev, state, tx);
    }

    // 5. Convert to NavEvent — unrecognised keys are ignored
    let Some(event) = key_to_nav_opt(key) else {
        return false;
    };

    // 6. Deliver to current screen
    let id = state.nav.stack.current();
    if let Some(screen) = screens.get_mut(id) {
        let mut ctx = UiContext {
            live:  &state.live,
            prefs: &state.prefs,
            ui:    &mut state.ui,
            nav:   &mut state.nav,
            tx,
        };
        if screen.on_nav(event, &mut ctx) == Consumed::Yes {
            return true;
        }
    }

    // 7. Global NavEvent fallbacks
    match event {
        crate::nav::event::NavEvent::Back => match state.nav.stack.pop() {
            PopResult::Ok(_) => true,
            PopResult::WouldExit => {
                state.ui.modal = Some(Box::new(QuitConfirmModal));
                true
            }
        },
        crate::nav::event::NavEvent::Tab => {
            let next = screens.next_visible(state.nav.stack.current());
            state.nav.stack.push(next);
            state.nav.focus_zone = 0;
            true
        }
        crate::nav::event::NavEvent::BackTab => {
            let prev = screens.prev_visible(state.nav.stack.current());
            state.nav.stack.push(prev);
            state.nav.focus_zone = 0;
            true
        }
        _ => false,
    }
}

fn handle_top_menu(ev: crate::nav::event::NavEvent, state: &mut AppState, _tx: &mpsc::Sender<Action>) -> bool {
    let action = state.ui.top_menu.on_nav(ev);
    match action {
        MenuAction::Consumed => {}
        MenuAction::Deactivate => { state.ui.top_menu.close(); }
        MenuAction::ConfirmPowerOff => {
            state.ui.open_modal(Box::new(RunActionModal::new(
                "Power off the system?",
                Action::Run(RunAction::Shutdown),
            )));
            state.ui.top_menu.close();
        }
        MenuAction::ConfirmReboot => {
            state.ui.open_modal(Box::new(RunActionModal::new(
                "Restart the system?",
                Action::Run(RunAction::Reboot),
            )));
            state.ui.top_menu.close();
        }
        MenuAction::ConfirmSuspend => {
            state.ui.open_modal(Box::new(RunActionModal::new(
                "Suspend the system?",
                Action::Run(RunAction::Suspend),
            )));
            state.ui.top_menu.close();
        }
        MenuAction::ExitTui => {
            state.ui.open_modal(Box::new(QuitConfirmModal));
            state.ui.top_menu.close();
        }
        MenuAction::OpenAbout => {
            state.ui.open_modal(Box::new(AboutModal));
            state.ui.top_menu.close();
        }
    }
    true
}

/// uConsole hardware button remap: A→Enter, B→Esc.
fn hardware_remap(key: KeyEvent) -> KeyEvent {
    match key.code {
        KeyCode::Char('a') => KeyEvent::new(KeyCode::Enter, key.modifiers),
        KeyCode::Char('b') => KeyEvent::new(KeyCode::Esc,   key.modifiers),
        _                  => key,
    }
}

/// Translate a user keymap binding back to the canonical KeyEvent for that action.
fn apply_keymap(key: KeyEvent, map: &crate::keymap::Keymap) -> KeyEvent {
    if let Some(action) = resolve_keymap(key, map) {
        match action {
            NavAction::Up      => KeyEvent::new(KeyCode::Up,      KeyModifiers::NONE),
            NavAction::Down    => KeyEvent::new(KeyCode::Down,    KeyModifiers::NONE),
            NavAction::Left    => KeyEvent::new(KeyCode::Left,    KeyModifiers::NONE),
            NavAction::Right   => KeyEvent::new(KeyCode::Right,   KeyModifiers::NONE),
            NavAction::Enter   => KeyEvent::new(KeyCode::Enter,   KeyModifiers::NONE),
            NavAction::Esc     => KeyEvent::new(KeyCode::Esc,     KeyModifiers::NONE),
            NavAction::Tab     => KeyEvent::new(KeyCode::Tab,     KeyModifiers::NONE),
            NavAction::BackTab => KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            _                  => key, // non-directional actions don't remap keys
        }
    } else {
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn hardware_remap_a_to_enter() {
        assert_eq!(hardware_remap(k(KeyCode::Char('a'))).code, KeyCode::Enter);
    }

    #[test]
    fn hardware_remap_b_to_esc() {
        assert_eq!(hardware_remap(k(KeyCode::Char('b'))).code, KeyCode::Esc);
    }

    #[test]
    fn hardware_remap_passthrough_other_chars() {
        assert_eq!(hardware_remap(k(KeyCode::Char('c'))).code, KeyCode::Char('c'));
        assert_eq!(hardware_remap(k(KeyCode::Up)).code,        KeyCode::Up);
    }

    #[test]
    fn apply_keymap_empty_map_is_identity() {
        let map = crate::keymap::Keymap::default();
        let key = k(KeyCode::Char('j'));
        assert_eq!(apply_keymap(key, &map).code, KeyCode::Char('j'));
    }

    #[test]
    fn apply_keymap_translates_bound_key() {
        let mut map = crate::keymap::Keymap::default();
        map.bind(NavAction::Down, k(KeyCode::Char('j')));
        // pressing 'j' with this map → canonical Down
        let result = apply_keymap(k(KeyCode::Char('j')), &map);
        assert_eq!(result.code, KeyCode::Down);
    }
}
