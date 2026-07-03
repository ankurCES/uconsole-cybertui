//! Embedded SPA assets.
//!
//! `wifi-radar`'s `--static-dir` argument resolves relative to the cwd.
//! When the binary runs as a system service (cwd = `/`) or from any
//! outside the workspace, the on-disk directory doesn't exist and every
//! `/static/*` request returns 404. The HTML shell still renders, so the
//! user sees only the topbar — no CSS, no JS, no canvas, no SSE.
//!
//! To make the binary self-contained we embed the contents of
//! `crates/wifi-radar/web/` at compile time via `rust-embed`. The router
//! in [`crate::run`] tries `ServeDir` first and falls back to
//! [`Assets`] when the file isn't on disk, so a developer can still
//! override `--static-dir` for live-reload-style iteration.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/"]
struct Assets;

/// Serve an embedded asset by path (e.g. `"app.js"`, `"style.css"`).
/// Returns 404 if the file isn't part of the embedded set.
pub fn get(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            // Embedded assets are returned directly from `static_fallback`
            // (bypassing the outer `SetResponseHeaderLayer`), so the
            // no-store header has to be stamped here at construction time.
            // Returning users with a stale disk cache would otherwise stay
            // stuck on the broken topbar-only page.
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(file.data.into_owned()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// True if `path` is one of the files we ship embedded. Lets the router
/// decide whether a 404 from `ServeDir` should fall through to us.
pub fn contains(path: &str) -> bool {
    Assets::get(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_contain_core_files() {
        // The HTML shell links exactly these three — if any of them is
        // missing from the embedded set, the page would render blank.
        assert!(contains("app.js"), "app.js must be embedded");
        assert!(contains("radar.js"), "radar.js must be embedded");
        assert!(contains("style.css"), "style.css must be embedded");
    }

    #[test]
    fn get_returns_404_for_unknown_files() {
        let resp = get("does-not-exist.js");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn get_serves_with_a_content_type() {
        let resp = get("app.js");
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // `mime_guess` reports `text/javascript` (RFC 9239) for `.js`.
        // We just want a sensible content-type, not an `application/octet-stream`
        // fallback that browsers would refuse to execute.
        assert!(
            ct.starts_with("text/") || ct.starts_with("application/javascript"),
            "got content-type {ct}"
        );
    }

    #[test]
    fn get_stamps_no_store_so_returning_users_get_fresh_assets() {
        // Regression for the "topbar-only" page: the embedded fallback is
        // returned directly from `static_fallback`, bypassing the outer
        // `SetResponseHeaderLayer`. If `no-store` isn't stamped here, a
        // returning user with a stale disk cache keeps the broken page.
        let resp = get("style.css");
        let cc = resp
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(cc, "no-store", "embedded assets must carry Cache-Control: no-store");
    }
}