//! wifi-radar: a browser-accessible Wi-Fi radar. See README + design doc.

pub mod api;
pub mod devices;
pub mod frames;
pub mod run;
pub mod scanner;
pub mod shell;
pub mod tags;

pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the package name + version, e.g. `"wifi-radar 0.1.0"`.
pub fn version() -> String {
    format!("wifi-radar {PKG_VERSION}")
}