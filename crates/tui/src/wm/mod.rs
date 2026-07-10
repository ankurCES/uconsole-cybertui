//! TUI window manager: layout tree, PTY panes, ANSI rendering.
//!
//! Modules:
//! - `ansi`         — VT100 byte stream → ratatui cell grid.
//! - `broadcaster`  — broadcast output + mpsc input for a pane.
//! - `manager`      — owns the split tree + per-pane state.
//! - `popup`        — centered floating popup with a shadow band.
//! - `pty`          — child PTY per external pane, lifecycle + I/O.
//! - `render`       — tree-walk renderer for the manager.
//! - `tree`         — binary split tree, layout, focus neighbours.
//! - `window`       — `Window` + `WindowKind` (Builtin | Terminal).

pub mod ansi;
pub mod broadcaster;
pub mod input;
pub mod manager;
pub mod popup;
pub mod pty;
pub mod render;
pub mod tree;
pub mod window;
