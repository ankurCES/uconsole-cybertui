//! cyberdeck-core: OS-level control primitives shared by the TUI and the web UI.
//!
//! Every module is async, returns `Result<T, CoreError>`, and runs external
//! commands through [`shell::run`] so that timeouts, sudo handling, and error
//! mapping are done in one place.

pub mod audio;
pub mod bluetooth;
pub mod display;
pub mod net;
pub mod packages;
pub mod power;
pub mod process;
pub mod services;
pub mod shell;
pub mod storage;
pub mod sys;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// All errors produced by core. Serializable so the web layer can return it
/// as JSON without an extra conversion.
#[derive(Debug, Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoreError {
    #[error("command `{cmd}` failed: {detail}")]
    Command { cmd: String, detail: String },

    #[error("command `{cmd}` timed out after {secs}s")]
    Timeout { cmd: String, secs: u64 },

    #[error("io: {0}")]
    Io(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    Permission(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("cancelled by user")]
    Cancelled,
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Parse(e.to_string())
    }
}

/// A short human-readable reason — used in toasts and HTTP error bodies.
pub type CoreResult<T> = std::result::Result<T, CoreError>;
