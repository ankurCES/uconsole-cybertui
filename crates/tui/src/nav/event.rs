use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavEvent {
    Up,
    Down,
    Left,
    Right,
    Confirm,   // Enter / A-button
    Back,      // Esc / B-button
    Tab,       // cycle forward through focusable zones
    BackTab,   // cycle backward
    Char(char),
    Backspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumed {
    Yes,
    No,
}

pub fn key_to_nav_opt(key: KeyEvent) -> Option<NavEvent> {
    match key.code {
        KeyCode::Up        => Some(NavEvent::Up),
        KeyCode::Down      => Some(NavEvent::Down),
        KeyCode::Left      => Some(NavEvent::Left),
        KeyCode::Right     => Some(NavEvent::Right),
        KeyCode::Enter     => Some(NavEvent::Confirm),
        KeyCode::Esc       => Some(NavEvent::Back),
        KeyCode::Tab       => Some(NavEvent::Tab),
        KeyCode::BackTab   => Some(NavEvent::BackTab),
        KeyCode::Char(c)   => Some(NavEvent::Char(c)),
        KeyCode::Backspace => Some(NavEvent::Backspace),
        _                  => None,
    }
}

/// Non-optional variant for paths where key is already confirmed nav-worthy.
pub fn key_to_nav(key: KeyEvent) -> NavEvent {
    key_to_nav_opt(key).unwrap_or(NavEvent::Char('\0'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn arrow_keys_map_correctly() {
        assert_eq!(key_to_nav_opt(k(KeyCode::Up)),    Some(NavEvent::Up));
        assert_eq!(key_to_nav_opt(k(KeyCode::Down)),  Some(NavEvent::Down));
        assert_eq!(key_to_nav_opt(k(KeyCode::Left)),  Some(NavEvent::Left));
        assert_eq!(key_to_nav_opt(k(KeyCode::Right)), Some(NavEvent::Right));
    }

    #[test]
    fn confirm_and_back() {
        assert_eq!(key_to_nav_opt(k(KeyCode::Enter)), Some(NavEvent::Confirm));
        assert_eq!(key_to_nav_opt(k(KeyCode::Esc)),   Some(NavEvent::Back));
    }

    #[test]
    fn tab_and_backtab() {
        assert_eq!(key_to_nav_opt(k(KeyCode::Tab)),    Some(NavEvent::Tab));
        assert_eq!(key_to_nav_opt(k(KeyCode::BackTab)), Some(NavEvent::BackTab));
    }

    #[test]
    fn char_and_backspace() {
        assert_eq!(key_to_nav_opt(k(KeyCode::Char('x'))), Some(NavEvent::Char('x')));
        assert_eq!(key_to_nav_opt(k(KeyCode::Backspace)), Some(NavEvent::Backspace));
    }

    #[test]
    fn f_key_returns_none() {
        assert_eq!(key_to_nav_opt(k(KeyCode::F(1))), None);
        assert_eq!(key_to_nav_opt(k(KeyCode::F(12))), None);
    }

    #[test]
    fn key_to_nav_falls_back_to_null_char() {
        assert_eq!(key_to_nav(k(KeyCode::F(5))), NavEvent::Char('\0'));
    }
}
