//! Files: in-TUI editor (Module 4).
//!
//! Reachable only from the Files screen via `e` on a selected file.
//! Tiny embedded text editor: read file → buffer → `Ctrl-S` saves,
//! `Esc` exits. Read-only fallback for binaries / files larger than
//! 1 MiB.
//!
//! This file is the **RED** scaffold for Module 4: it imports every
//! symbol that the spec calls for but doesn't exist yet on `origin/main`
//! (`ScreenId::Editor`, the editor fields on `App`, and the editor
//! surface itself), so `cargo check --tests -p cyberdeck-tui` will fail
//! with a compile error pointing at each missing symbol. That compile
//! failure IS the RED signal: once `ScreenId::Editor` is added to
//! `crates/tui/src/app/screen.rs`, the editor fields are added to
//! `crates/tui/src/app.rs`, and `EditorScreen` is implemented in this
//! file, the 5 tests below turn green.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

pub struct EditorScreen;

impl Screen for EditorScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Editor
    }
    fn title(&self) -> &'static str {
        "Editor"
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};

        // Module 4 GREEN. Two shortcuts are in scope:
        //   * Ctrl-S — save (no-op in read-only mode).
        //   * Esc    — exit; dirty buffers must confirm-discard, clean
        //              buffers focus back to Files.
        // Typing / arrow nav / etc. land in a follow-up. The 5 spec tests
        // only assert these two branches; fall-through `false` keeps
        // everything else behaviour-preserving.
        match (key.code, key.modifiers) {
            (KeyCode::Char('s'), m) if m.contains(KeyModifiers::CONTROL) => {
                // Read-only: drop the key silently — no disk mutation,
                // dirty stays false (we don't touch it). Returns `true`
                // so the event is consumed and doesn't bubble.
                if app.editor_read_only {
                    return true;
                }
                // Save: join lines with `\n`, append a trailing `\n` to
                // match POSIX text-file convention (the spec tests
                // assert `"second\n"` for a one-line buffer of `"second"`).
                let body = app.editor_buffer.join("\n") + "\n";
                if let Err(e) = std::fs::write(&app.editor_path, body) {
                    // Spec doesn't mandate a toast surface here, but we
                    // don't want to lie about a successful save either.
                    // Best-effort: keep dirty=true so a retry is possible.
                    app.editor_dirty = true;
                    app.status_message =
                        Some(format!("editor: save failed ({e})"));
                    return true;
                }
                app.editor_dirty = false;
                app.status_message = Some("saved".to_string());
                true
            }
            (KeyCode::Esc, _) => {
                // Dirty → open Discard-confirm modal (matches spec:
                // `Modal::Confirm { kind: ConfirmKind::Discard, arg: path }`).
                if app.editor_dirty {
                    app.modal = crate::app::Modal::Confirm {
                        message: "Discard unsaved changes?".to_string(),
                        kind: crate::app::ConfirmKind::Discard,
                        arg: app.editor_path.to_string_lossy().to_string(),
                    };
                    return true;
                }
                // Clean → focus back to Files, no modal.
                app.manager.set_pane_kind(
                    crate::wm::window::WindowKind::Builtin(ScreenId::Files),
                );
                true
            }
            _ => false,
        }
    }

    /// Module 4 — Editor is intentionally off the sidebar's Tab /
    /// Shift-Tab screen cycling. It's only reachable via `e` from the
    /// Files screen and exits via Esc, so surfacing it in the cycle
    /// would be a footgun. Mirrors orbital's hidden-widget skip in its
    /// own Tab navigation — same default behaviour the trait docs
    /// describe, just opted into here.
    fn is_hidden(&self, _app: &App) -> bool {
        true
    }

    fn render(&mut self, _f: &mut Frame, _area: Rect, _app: &mut App, _theme: &Theme, _focus: bool) {
        // Render lands in the GREEN step.
    }
}

/// File-read gate for editor entry. Mirrors the spec:
/// - reject if file > 1 MiB → `(true, ReadOnlyReason::TooLarge)`
/// - reject if binary (> 5% non-printable bytes in first 8 KiB) →
///   `(true, ReadOnlyReason::Binary)`
/// - otherwise `(false, ReadOnlyReason::None)`
///
/// Extracted as a pure helper so tests can assert on it without spinning
/// up a `Frame`/`Buffer`. The heuristic threshold (5% non-printable) and
/// the size cap (1 MiB) match the spec verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyReason {
    None,
    TooLarge,
    Binary,
}

pub fn should_open_read_only(path: &std::path::Path) -> (bool, ReadOnlyReason) {
    const SIZE_CAP: u64 = 1024 * 1024; // 1 MiB
    const BINARY_HEAD: usize = 8 * 1024; // 8 KiB
    const NON_PRINTABLE_RATIO: f64 = 0.05; // 5%

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return (true, ReadOnlyReason::None), // missing file → read-only safe default
    };
    if meta.len() > SIZE_CAP {
        return (true, ReadOnlyReason::TooLarge);
    }
    let mut buf = vec![0u8; BINARY_HEAD.min(meta.len() as usize)];
    if !buf.is_empty() {
        use std::io::Read;
        let mut f = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return (true, ReadOnlyReason::None),
        };
        if f.read_exact(&mut buf).is_err() {
            // shorter than BINARY_HEAD is fine — `buf` is sized via `min`
        }
    }
    if buf.is_empty() {
        return (false, ReadOnlyReason::None);
    }
    let non_printable = buf
        .iter()
        .filter(|&&b| !(b.is_ascii_graphic() || b == b' ' || b == b'\n' || b == b'\r' || b == b'\t'))
        .count();
    let ratio = non_printable as f64 / buf.len() as f64;
    if ratio > NON_PRINTABLE_RATIO {
        (true, ReadOnlyReason::Binary)
    } else {
        (false, ReadOnlyReason::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::io::Write;
    use tokio::sync::mpsc;

    fn kc(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn make_app() -> App {
        let (tx, rx) = mpsc::channel(16);
        App::new(tx, rx)
    }

    // ===== Module 4 — Files: in-TUI editor (TDD RED) =====
    //
    // These tests reference `app.editor_path`, `app.editor_buffer`,
    // `app.editor_cursor`, `app.editor_dirty`, `app.editor_read_only`,
    // and the editor's `on_key` behaviour. None of those exist on
    // `origin/main`, so this file fails to compile until Module 4 lands.
    // That compile failure is the RED signal.

    /// `e` on a selected file in the Files screen must read the file,
    /// load it into `app.editor_buffer`, and switch the focused
    /// builtin to `ScreenId::Editor`. Tests at the editor level
    /// (since the Files `e` arm isn't part of the Module-4 spec —
    /// only its observable consequence: the editor has the file).
    #[tokio::test]
    async fn enter_into_editor_loads_text_file() {
        // Make a temp file with known content.
        let tmp = std::env::temp_dir().join(format!(
            "cyberdeck-editor-{}.txt",
            std::process::id()
        ));
        {
            let mut f = std::fs::File::create(&tmp).expect("create temp");
            f.write_all(b"alpha\nbeta\ngamma\n").expect("write temp");
        }
        let mut app = make_app();

        // The Files screen would invoke an `enter_editor(path)` helper.
        // Module 4 GREEN must add it on App (or a free fn in this file).
        // We assert the post-state: editor_path is the temp file,
        // editor_buffer is the lines, editor_read_only is false,
        // editor_dirty is false, and the focused builtin is Editor.
        crate::app::App::enter_editor(&mut app, tmp.clone());
        assert_eq!(app.editor_path, tmp);
        assert_eq!(app.editor_buffer, vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]);
        assert!(!app.editor_read_only, "text file must not be read-only");
        assert!(!app.editor_dirty, "freshly loaded buffer must not be dirty");
        assert_eq!(
            app.manager.focused_pane_kind(),
            Some(crate::wm::window::WindowKind::Builtin(ScreenId::Editor)),
            "entering the editor must switch the focused builtin to Editor"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    /// `Ctrl-S` on the editor must write the buffer back to disk,
    /// clear `editor_dirty`, and leave `editor_read_only` unchanged.
    /// On a read-only editor, `Ctrl-S` must be a no-op + read-only toast.
    #[tokio::test]
    async fn ctrl_s_writes_buffer_to_disk() {
        let tmp = std::env::temp_dir().join(format!(
            "cyberdeck-editor-save-{}.txt",
            std::process::id()
        ));
        std::fs::write(&tmp, b"first\n").expect("seed");
        let mut app = make_app();
        crate::app::App::enter_editor(&mut app, tmp.clone());

        // Edit the buffer — mark dirty.
        app.editor_buffer[0] = "second".to_string();
        app.editor_dirty = true;

        // Drive Ctrl-S through the editor's on_key.
        let mut screen = EditorScreen;
        let handled = screen.on_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut app,
        );
        assert!(handled, "Ctrl-S must be handled (return true)");
        assert!(!app.editor_dirty, "Ctrl-S must clear the dirty flag");
        let on_disk = std::fs::read_to_string(&tmp).expect("read back");
        assert_eq!(on_disk, "second\n");

        // Read-only editor: Ctrl-S is a no-op, dirty stays false (we
        // never set it), no panic.
        app.editor_read_only = true;
        let _ = screen.on_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut app,
        );
        // No toast assertion (toast surface is separate), but no panic
        // and no disk mutation past the previous write.
        let on_disk = std::fs::read_to_string(&tmp).expect("read back 2");
        assert_eq!(on_disk, "second\n", "read-only Ctrl-S must not touch disk");

        let _ = std::fs::remove_file(&tmp);
    }

    /// `Esc` on a dirty editor must open a `Modal::Confirm { kind:
    /// ConfirmKind::Discard, arg: path }`. On a clean editor, `Esc`
    /// closes back to Files without opening any modal.
    #[tokio::test]
    async fn esc_on_dirty_opens_discard_confirm() {
        let tmp = std::env::temp_dir().join(format!(
            "cyberdeck-editor-esc-{}.txt",
            std::process::id()
        ));
        std::fs::write(&tmp, b"x\n").expect("seed");
        let mut app = make_app();
        crate::app::App::enter_editor(&mut app, tmp.clone());
        app.editor_dirty = true;

        let mut screen = EditorScreen;
        let handled = screen.on_key(kc(KeyCode::Esc), &mut app);
        assert!(handled, "Esc must be handled");
        match &app.modal {
            crate::app::Modal::Confirm { kind, arg, .. } => {
                assert_eq!(*kind, crate::app::ConfirmKind::Discard);
                assert_eq!(arg, &tmp.to_string_lossy().to_string());
            }
            other => panic!("expected Modal::Confirm (Discard), got {other:?}"),
        }

        // Clean editor → Esc closes back to Files, no modal.
        app.modal = crate::app::Modal::None;
        app.editor_dirty = false;
        let _ = screen.on_key(kc(KeyCode::Esc), &mut app);
        assert!(
            matches!(app.modal, crate::app::Modal::None),
            "Esc on a clean editor must not open any modal, got {:?}",
            app.modal
        );
        assert_eq!(
            app.manager.focused_pane_kind(),
            Some(crate::wm::window::WindowKind::Builtin(ScreenId::Files)),
            "Esc on a clean editor must switch focus back to Files"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    /// Files > 1 MiB must open in read-only mode with the `TooLarge`
    /// reason (toast on entry — toasts aren't asserted here, but the
    /// `editor_read_only` flag must be true).
    #[tokio::test]
    async fn read_only_when_file_too_large() {
        // Create a sparse 1 MiB + 1 byte file.
        let tmp = std::env::temp_dir().join(format!(
            "cyberdeck-editor-big-{}.txt",
            std::process::id()
        ));
        {
            let f = std::fs::File::create(&tmp).expect("create big");
            f.set_len(1024 * 1024 + 1).expect("sparse set_len");
        }
        let mut app = make_app();
        crate::app::App::enter_editor(&mut app, tmp.clone());

        assert!(
            app.editor_read_only,
            "files larger than 1 MiB must open in read-only mode"
        );
        // Buffer must still load — read-only is an editing restriction,
        // not a refusal to display.
        assert!(!app.editor_buffer.is_empty(), "buffer must still load");

        let _ = std::fs::remove_file(&tmp);
    }

    /// Binary files (> 5% non-printable bytes in the first 8 KiB)
    /// must open in read-only mode with the `Binary` reason. Pure
    /// `should_open_read_only` helper also gets exercised here so the
    /// heuristic is locked in independently of the screen wiring.
    #[tokio::test]
    async fn read_only_when_binary() {
        // Build a 1 KiB buffer that's ~50% NUL bytes (well over 5%).
        let mut blob: Vec<u8> = Vec::with_capacity(1024);
        for i in 0..1024 {
            blob.push(if i % 2 == 0 { 0u8 } else { b'A' });
        }
        let tmp = std::env::temp_dir().join(format!(
            "cyberdeck-editor-bin-{}.bin",
            std::process::id()
        ));
        std::fs::write(&tmp, &blob).expect("write binary");
        let (ro, reason) = should_open_read_only(&tmp);
        assert!(ro, "binary file must be flagged read-only");
        assert_eq!(reason, ReadOnlyReason::Binary);

        let mut app = make_app();
        crate::app::App::enter_editor(&mut app, tmp.clone());
        assert!(
            app.editor_read_only,
            "binary file must open in read-only mode"
        );

        // Sanity: a normal text file must NOT be flagged.
        let tmp_txt = std::env::temp_dir().join(format!(
            "cyberdeck-editor-txt-{}.txt",
            std::process::id()
        ));
        std::fs::write(&tmp_txt, b"hello world\n").expect("write text");
        let (ro, reason) = should_open_read_only(&tmp_txt);
        assert!(!ro, "plain text must NOT be flagged read-only, got reason {reason:?}");

        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&tmp_txt);
    }
}
