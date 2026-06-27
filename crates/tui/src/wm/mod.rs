//! TUI window manager: layout tree, PTY panes, ANSI rendering.
//!
//! Modules:
//! - `ansi`         — VT100 byte stream → ratatui cell grid.
//! - `broadcaster`  — broadcast output + mpsc input for a pane.
//! - `keymap`       — hardware button remap (uconsole X/Y/A/B → arrows).
//! - `manager`      — owns the split tree + per-pane state.
//! - `pty`          — child PTY per external pane, lifecycle + I/O.
//! - `render`       — tree-walk renderer for the manager.
//! - `tree`         — binary split tree, layout, focus neighbours.
//! - `window`       — `Window` + `WindowKind` (Builtin | Terminal).

pub mod ansi;
pub mod broadcaster;
pub mod keymap;
pub mod pty;
pub mod tree;
pub mod window;
