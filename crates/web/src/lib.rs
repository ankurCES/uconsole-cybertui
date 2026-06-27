//! cyberdeck-web: a small axum server that exposes `cyberdeck-core` over HTTP
//! and a WebSocket. Can run standalone (`cyberdeck-web` binary) or be embedded
//! in the TUI via the `--web` flag.
//!
//! Layout:
//!   * `api`  — JSON routes (one per resource) + POST handlers for actions
//!   * `ws`   — WebSocket that streams live status snapshots to the browser
//!   * `shell`— askama-rendered HTML shell + static JS bundle
//!   * `auth` — optional bearer-token middleware (generated on first run)
//!   * `run`  — the public `run_with(bind, live, tx)` entry point

pub mod api;
pub mod auth;
pub mod run;
pub mod shell;
pub mod ws;

pub use run::run_with;
