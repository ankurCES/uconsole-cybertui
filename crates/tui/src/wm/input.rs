//! Terminal-keystroke translation: take a `KeyEvent` and produce the
//! bytes a real terminal would expect. We need this because the WM
//! hands raw keys to a child PTY (bash, vim, ssh, …) which speaks
//! VT100, not crossterm's `KeyEvent`.
//!
//! Coverage is intentionally minimal — printable chars, Enter, Tab,
//! Esc, Backspace, and the four arrows. Anything more exotic (F-keys,
//! modifiers other than Ctrl) is out of scope for v0; the user can
//! still type by running `cat` or `read` to verify the basics.
//!
//! If a key has no translation, return `None` and the caller will
//! drop it (the same as a real terminal that hasn't been configured
//! for that key).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn bytes_for_key(key: &KeyEvent) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl + letter → control code. Only handles the
                // standard 0x1F range; ignores Ctrl+Space (NUL) and
                // anything outside the printable subset.
                let lc = c.to_ascii_lowercase() as u8;
                if (b'a'..=b'z').contains(&lc) {
                    buf.push(lc - b'a' + 1);
                } else if lc == b' ' {
                    buf.push(0);
                } else {
                    return None;
                }
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                buf.push(0x1b);
                let mut s = [0u8; 4];
                let s = c.encode_utf8(&mut s);
                buf.extend_from_slice(s.as_bytes());
            } else {
                let mut s = [0u8; 4];
                let s = c.encode_utf8(&mut s);
                buf.extend_from_slice(s.as_bytes());
            }
        }
        KeyCode::Enter => buf.extend_from_slice(b"\r"),
        KeyCode::Backspace => buf.push(0x7f), // canonical "erase"
        KeyCode::Tab => buf.extend_from_slice(b"\t"),
        KeyCode::Esc => buf.extend_from_slice(b"\x1b"),
        KeyCode::Up => buf.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => buf.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => buf.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => buf.extend_from_slice(b"\x1b[D"),
        _ => return None,
    }
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn regular_chars_become_utf8() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .expect("translateable");
        assert_eq!(b, b"a");
    }

    #[test]
    fn enter_becomes_carriage_return() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("translateable");
        assert_eq!(b, b"\r");
    }

    #[test]
    fn arrow_up_becomes_csi_a() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
            .expect("translateable");
        assert_eq!(b, b"\x1b[A");
    }

    #[test]
    fn ctrl_c_becomes_etx() {
        let b = bytes_for_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .expect("translateable");
        assert_eq!(b, &[0x03]);
    }
}
