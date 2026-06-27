//! HTML shell + a tiny static JS bundle. Askama renders the shell; the JS
//! is served as a single file. No build step, no npm — keeps the binary
//! self-contained and easy to audit on a cyberdeck.

use askama::Template;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

use crate::api::ApiState;

#[derive(Template)]
#[template(path = "index.html")]
struct Index;

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(js))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    let t = Index;
    match t.render() {
        Ok(html) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn js() -> Response {
    let body = include_str!("../static/app.js");
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        body.to_string(),
    )
        .into_response()
}
