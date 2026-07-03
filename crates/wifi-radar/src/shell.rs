//! HTML shell for the radar UI.
//!
//! Uses askama templates so the page is rendered server-side once (no
//! per-request cost) and we avoid any runtime templating. Static assets
//! (`style.css`, `app.js`, `radar.js`) are mounted at `/static/*` by
//! `run.rs`.

use askama::Template;

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub title: String,
    pub version: String,
}

impl Default for IndexTemplate {
    fn default() -> Self {
        Self {
            title: "wifi-radar".to_string(),
            version: crate::version(),
        }
    }
}