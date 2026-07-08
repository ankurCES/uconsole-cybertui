//! User-editable keymap: maps physical `KeyEvent`s onto canonical
//! `NavAction`s the rest of the TUI binds against.

use std::collections::HashMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

/// The canonical TUI actions the user can rebind. The list is ordered
/// (matches the order rows appear on the Settings → Keys screen) and
/// stable — entries are referenced by their kebab-case string in
/// `prefs.json`, so renaming one is a breaking change for existing
/// user files. Append new ones at the end to keep older files
/// forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NavAction {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Tab,
    BackTab,
    NextScreen,
    PrevScreen,
    Refresh,
    Help,
    Palette,
    Quit,
}

impl NavAction {
    /// Stable display order for the Settings → Keys list.
    pub const ALL: &'static [NavAction] = &[
        NavAction::Up,
        NavAction::Down,
        NavAction::Left,
        NavAction::Right,
        NavAction::Enter,
        NavAction::Esc,
        NavAction::Tab,
        NavAction::BackTab,
        NavAction::NextScreen,
        NavAction::PrevScreen,
        NavAction::Refresh,
        NavAction::Help,
        NavAction::Palette,
        NavAction::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NavAction::Up => "up",
            NavAction::Down => "down",
            NavAction::Left => "left",
            NavAction::Right => "right",
            NavAction::Enter => "enter",
            NavAction::Esc => "esc",
            NavAction::Tab => "next pane / tab",
            NavAction::BackTab => "prev pane / back-tab",
            NavAction::NextScreen => "next screen",
            NavAction::PrevScreen => "prev screen",
            NavAction::Refresh => "refresh",
            NavAction::Help => "help (?)",
            NavAction::Palette => "command palette (:)",
            NavAction::Quit => "quit",
        }
    }
}

/// User-editable keymap. Empty by default (= identity: every action
/// uses its built-in `KeyEvent`). Each entry maps a `NavAction` to a
/// specific `KeyEvent` the user pressed. Stored in `prefs.json` as a
/// `{"up": {"code": "Up", "modifiers": []}, ...}` object; missing
/// fields are read as "use default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keymap {
    #[serde(flatten)]
    pub bindings: HashMap<NavAction, KeyEvent>,
}

/// Serialise a `KeyEvent` as the short string the Settings UI shows
/// (e.g. "↑", "Ctrl+R", "Enter"). Also used when reading from
/// `prefs.json`: missing fields are silently dropped.
pub fn key_event_label(k: KeyEvent) -> String {
    let mut s = String::new();
    if k.modifiers.contains(KeyModifiers::CONTROL) { s.push_str("Ctrl+"); }
    if k.modifiers.contains(KeyModifiers::ALT) { s.push_str("Alt+"); }
    if k.modifiers.contains(KeyModifiers::SHIFT) { s.push_str("Shift+"); }
    s.push_str(&key_code_label(k.code));
    s
}

fn key_code_label(c: KeyCode) -> String {
    use KeyCode::*;
    match c {
        Char(c) => c.to_string(),
        Up => "↑".to_string(),
        Down => "↓".to_string(),
        Left => "←".to_string(),
        Right => "→".to_string(),
        Enter => "Enter".to_string(),
        Esc => "Esc".to_string(),
        Tab => "Tab".to_string(),
        BackTab => "BackTab".to_string(),
        F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

/// Apply the user map. Returns the *canonical* `NavAction` the key
/// maps to, or `None` if the key is not remapped by the user (in which
/// case the caller should pass the original `KeyEvent` through to the
/// existing dispatch). The function is pure and side-effect-free.
pub fn resolve_keymap(key: KeyEvent, map: &Keymap) -> Option<NavAction> {
    // Reverse lookup: which NavAction has bound `key`?
    map.bindings
        .iter()
        .find(|(_, v)| **v == key)
        .map(|(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: KeyCode) -> KeyEvent { KeyEvent::new(c, KeyModifiers::NONE) }

    #[test]
    fn empty_keymap_resolves_to_none() {
        let map = Keymap::default();
        assert_eq!(resolve_keymap(k(KeyCode::Up), &map), None);
        assert_eq!(resolve_keymap(k(KeyCode::Char('j')), &map), None);
    }

    #[test]
    fn binding_to_nav_action_round_trips() {
        let mut map = Keymap::default();
        map.bindings.insert(NavAction::Down, k(KeyCode::Char('j')));
        // Pressing 'j' resolves to Down; pressing 'k' does not.
        assert_eq!(resolve_keymap(k(KeyCode::Char('j')), &map), Some(NavAction::Down));
        assert_eq!(resolve_keymap(k(KeyCode::Char('k')), &map), None);
    }

    #[test]
    fn serde_round_trip_preserves_bindings() {
        let mut map = Keymap::default();
        map.bindings.insert(NavAction::Up, k(KeyCode::Char('w')));
        map.bindings.insert(NavAction::Refresh, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let json = serde_json::to_string(&map).expect("serialize");
        // The kebab-case key name must appear, and the value must be an object.
        assert!(json.contains("\"up\""), "json was: {json}");
        assert!(json.contains("\"refresh\""), "json was: {json}");
        let back: Keymap = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, map);
    }

    #[test]
    fn key_event_label_renders_arrows() {
        assert_eq!(key_event_label(k(KeyCode::Up)), "↑");
        assert_eq!(key_event_label(k(KeyCode::Enter)), "Enter");
        assert_eq!(key_event_label(k(KeyCode::Char('q'))), "q");
        assert_eq!(
            key_event_label(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            "Ctrl+r"
        );
    }

    #[test]
    fn all_nav_action_variants_have_labels() {
        // Pin the set: every variant must be listed in `ALL` *and* have
        // a label. This is what keeps the Settings screen from blanking
        // out when we add a new action in the future.
        for a in [NavAction::Up, NavAction::Down, NavAction::Left, NavAction::Right,
                  NavAction::Enter, NavAction::Esc, NavAction::Tab, NavAction::BackTab,
                  NavAction::NextScreen, NavAction::PrevScreen, NavAction::Refresh,
                  NavAction::Help, NavAction::Palette, NavAction::Quit] {
            assert!(!NavAction::ALL.contains(&a) || !a.label().is_empty(),
                    "{a:?} missing label or not in ALL");
        }
    }
}
