//! wifi-radar: a browser-accessible Wi-Fi radar. See README + design doc.

pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the package name + version, e.g. `"wifi-radar 0.1.0"`.
pub fn version() -> String {
    format!("wifi-radar {PKG_VERSION}")
}