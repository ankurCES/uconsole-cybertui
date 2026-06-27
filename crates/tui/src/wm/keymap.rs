//! Hardware button → arrow/enter/esc mapping for keyboards that don't emit
//! standard arrow codes (e.g. the ClockworkPi uconsole's top X/Y/A/B row).
//!
//! `map_key` is a pure function: same input, same output, no I/O. It runs
//! at the very top of `main::handle_key` so every code path downstream
//! sees a normal `KeyEvent`. The desktop profile is identity — the
//! remap is a no-op unless the binary is built with `--features
//! uconsole-keymap` and the env var `CYBERDECK_KEYMAP=uconsole` is set.

use crossterm::event::KeyEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapProfile {
    /// Standard x86 laptop. Pass-through.
    Desktop,
    /// ClockworkPi uconsole: X/Y/A/B → Up/Down/Enter/Esc.
    Uconsole,
}

impl KeymapProfile {
    /// Resolved at runtime from the env var, with a sensible default
    /// (Desktop) so x86 development builds Just Work.
    pub fn detect() -> Self {
        match std::env::var("CYBERDECK_KEYMAP").as_deref() {
            Ok("uconsole") => Self::Uconsole,
            _ => Self::Desktop,
        }
    }
}

pub fn map_key(key: KeyEvent, profile: KeymapProfile) -> Option<KeyEvent> {
    use crossterm::event::KeyCode;
    match profile {
        KeymapProfile::Desktop => Some(key),
        KeymapProfile::Uconsole => match key.code {
            KeyCode::Char('x') => Some(KeyEvent::new(KeyCode::Up, key.modifiers)),
            KeyCode::Char('y') => Some(KeyEvent::new(KeyCode::Down, key.modifiers)),
            KeyCode::Char('a') => Some(KeyEvent::new(KeyCode::Enter, key.modifiers)),
            KeyCode::Char('b') => Some(KeyEvent::new(KeyCode::Esc, key.modifiers)),
            _ => Some(key),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn uconsole_buttons_map_to_nav_keys() {
        let p = KeymapProfile::Uconsole;
        assert_eq!(map_key(k(KeyCode::Char('x')), p), Some(k(KeyCode::Up)));
        assert_eq!(map_key(k(KeyCode::Char('y')), p), Some(k(KeyCode::Down)));
        assert_eq!(map_key(k(KeyCode::Char('a')), p), Some(k(KeyCode::Enter)));
        assert_eq!(map_key(k(KeyCode::Char('b')), p), Some(k(KeyCode::Esc)));
    }

    #[test]
    fn uconsole_mapping_is_a_passthrough_for_other_keys() {
        let p = KeymapProfile::Uconsole;
        assert_eq!(map_key(k(KeyCode::Char('q')), p), Some(k(KeyCode::Char('q'))));
        assert_eq!(map_key(k(KeyCode::Tab), p), Some(k(KeyCode::Tab)));
    }

    #[test]
    fn desktop_profile_is_identity() {
        let p = KeymapProfile::Desktop;
        assert_eq!(map_key(k(KeyCode::Up), p), Some(k(KeyCode::Up)));
        assert_eq!(map_key(k(KeyCode::Char('x')), p), Some(k(KeyCode::Char('x'))));
    }
}
