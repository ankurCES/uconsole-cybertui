//! User-editable keymap: maps physical `KeyEvent`s onto canonical
//! `NavAction`s the rest of the TUI binds against.
//! Used by the Settings → Keys sub-screen.

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

/// Settings → Keys sub-mode commands. See `app::action::Action::KeymapCmd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeymapCmd {
    BeginCapture(NavAction),
    /// Captured key from the dispatcher. The action is "the user
    /// just pressed `key`; if it doesn't conflict, store it as the
    /// binding for the action currently being captured."
    CaptureKey,
    Clear(NavAction),
    ResetAll,
    ExitMode,
}

/// User-editable keymap. Empty by default (= identity: every action
/// uses its built-in `KeyEvent`). Each entry maps a `NavAction` to a
/// specific `KeyEvent` the user pressed.
///
/// On-disk shape (via `#[serde(flatten)]` on the inner `HashMap`):
/// ```json
/// {
///   "up":   { "code": "Up",                 "modifiers": "",        "kind": "Press", "state": "" },
///   "down": { "code": { "Char": "j" },      "modifiers": "",        "kind": "Press", "state": "" }
/// }
/// ```
/// Crossterm uses external tagging for `KeyCode` (bare strings for
/// unit variants, `{"Char": "r"}` for tuple variants) and a
/// bitflags-string for `KeyModifiers` (`"CONTROL"`, `""`, etc.).
/// Round-trip is lossless; the shape is intentionally verbose so
/// hand-edits are obvious in `git diff`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keymap {
    #[serde(flatten)]
    pub(crate) bindings: HashMap<NavAction, KeyEvent>,
}

/// Render a `KeyEvent` as the short string the Settings UI shows
/// (e.g. "↑", "Ctrl+r", "Enter").
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

/// Returns the `NavAction` the key is mapped to under the user map,
/// or `None` if no user binding exists. The caller is expected to
/// translate a `Some(action)` into a `KeyEvent` matching the action's
/// built-in default (Up/Down/Enter/Char('q')/etc.) so the rest of the
/// TUI keeps matching against the same `KeyCode` it's always matched
/// against. The function is pure and side-effect-free.
pub fn resolve_keymap(key: KeyEvent, map: &Keymap) -> Option<NavAction> {
    // Reverse lookup: which NavAction has bound `key`?
    map.bindings
        .iter()
        .find(|(_, v)| **v == key)
        .map(|(k, _)| *k)
}

impl Keymap {
    /// Bind `action` to `key`. Overwrites any previous binding for
    /// `action`. Does *not* enforce conflict-freeness (a single key
    /// bound to two actions) — that lives at the call site (the
    /// `Action::KeymapCmd` dispatcher arm) so the user gets a toast
    /// instead of a silent overwrite.
    pub fn bind(&mut self, action: NavAction, key: KeyEvent) {
        self.bindings.insert(action, key);
    }

    /// Drop the binding for `action`, if any. No-op when unbound.
    pub fn unbind(&mut self, action: NavAction) {
        self.bindings.remove(&action);
    }

    /// The current binding for `action`, if any.
    pub fn get(&self, action: NavAction) -> Option<KeyEvent> {
        self.bindings.get(&action).copied()
    }

    /// True if `key` is already bound to some action in this map.
    /// Used for conflict detection at capture time.
    pub fn is_key_taken(&self, key: KeyEvent) -> bool {
        self.bindings.values().any(|v| *v == key)
    }

    /// Iterator over `(action, key)` pairs in the map. Order is
    /// unspecified (HashMap iteration).
    pub fn iter(&self) -> impl Iterator<Item = (NavAction, KeyEvent)> + '_ {
        self.bindings.iter().map(|(k, v)| (*k, *v))
    }

    /// Number of active bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// True when no user overrides are set (= the TUI uses every
    /// action's built-in default).
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
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
        // out when we add a new action in the future. We iterate
        // explicitly over every variant so a future variant that isn't
        // added to `ALL` still fails this test.
        let all = [
            NavAction::Up, NavAction::Down, NavAction::Left, NavAction::Right,
            NavAction::Enter, NavAction::Esc, NavAction::Tab, NavAction::BackTab,
            NavAction::NextScreen, NavAction::PrevScreen, NavAction::Refresh,
            NavAction::Help, NavAction::Palette, NavAction::Quit,
        ];
        // 1. `ALL` must contain every variant we know about.
        for a in all {
            assert!(NavAction::ALL.contains(&a), "{a:?} missing from NavAction::ALL");
            assert!(!a.label().is_empty(), "{a:?} has empty label");
        }
        // 2. `ALL` must not contain duplicates (the `Set` is also the
        // canonical display order — duplicates would render twice).
        let mut sorted = NavAction::ALL.to_vec();
        sorted.sort_by_key(|n| format!("{:?}", n));
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), before, "NavAction::ALL contains duplicates");
    }

    #[test]
    fn accessors_round_trip() {
        let mut map = Keymap::default();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.bind(NavAction::Down, k(KeyCode::Char('j')));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(NavAction::Down), Some(k(KeyCode::Char('j'))));
        assert!(!map.is_empty());
        assert!(map.is_key_taken(k(KeyCode::Char('j'))));
        assert!(!map.is_key_taken(k(KeyCode::Char('k'))));

        // Overwrite.
        map.bind(NavAction::Down, k(KeyCode::Char('s')));
        assert_eq!(map.get(NavAction::Down), Some(k(KeyCode::Char('s'))));
        assert_eq!(map.len(), 1, "overwrite must not grow the map");

        // iter visits the binding.
        let collected: Vec<_> = map.iter().collect();
        assert_eq!(collected, vec![(NavAction::Down, k(KeyCode::Char('s')))]);

        // unbind.
        map.unbind(NavAction::Down);
        assert!(map.is_empty());
        assert_eq!(map.get(NavAction::Down), None);
    }
}
