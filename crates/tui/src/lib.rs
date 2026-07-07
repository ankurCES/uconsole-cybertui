//! cyberdeck-tui — public library surface.
//!
//! The crate ships as both a library (this file) and a binary
//! (`main.rs`). The library exists so integration tests can drive the
//! LoRa ingest pipeline end-to-end against a real HTTP server (the
//! `wiremock` test in `tests/lora_http_live.rs`) without having to
//! spin up the full ratatui event loop.
//!
//! Each module is `pub` and re-exported from its canonical home; the
//! binary just uses them.

pub mod app;
pub mod prefs;
pub mod screens;
pub mod theme;
pub mod ui;
pub mod util;
pub mod workspace;
pub mod wm;

pub use ui::palette::Palette;

#[cfg(feature = "web")]
pub mod web_bridge;
