//! TUI window manager: layout tree, PTY panes, ANSI rendering.
//!
//! This module is the home for the WM that replaces the original single-
//! screen TUI. Submodules land phase-wise:
//! - `ansi` — VT100 byte stream → ratatui cell grid. (Task 1.2)
//! - `pty`  — child PTY per external pane, broadcast output. (Task 1.3-1.5)
//! - `tree` — binary split tree. (Task 2)
//! - `window` — `Window` + `WindowKind`. (Task 3)

pub mod ansi;
pub mod broadcaster;
pub mod pty;
