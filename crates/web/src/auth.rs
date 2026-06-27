//! Auth: optional bearer token. Generated on first run and printed to the
//! console; browsers must send it in the `Authorization` header (or as the
//! `?token=` query param for the WebSocket, since EventSource can't set
//! headers). The TUI's `--web` flag is off by default — when enabled, the
//! Settings toggle turns this on too.

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Token(pub String);

impl Default for Token {
    fn default() -> Self {
        Self::new()
    }
}

impl Token {
    pub fn new() -> Self {
        use rand::Rng;
        let raw: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
        Self(format!("cdk_{raw}"))
    }

    /// Read a token from a file. Empty lines and surrounding whitespace are
    /// ignored. Used by `cyberdeck-web`'s `--token-file` flag so the
    /// installer can pin a persistent token in `/etc/cyberdeck/token`.
    pub fn from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "token file is empty",
            ));
        }
        Ok(Self(trimmed))
    }
}

#[derive(Debug, Deserialize)]
pub struct Q {
    pub token: Option<String>,
}

/// Axum middleware that demands a valid bearer token, with a query-string
/// fallback for the WebSocket upgrade. Open if `token` is `None` — that's the
/// "off" mode for the auth toggle.
pub async fn require_bearer(
    axum::extract::State(token): axum::extract::State<Arc<Option<Token>>>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let required = match token.as_ref() {
        Some(t) => t,
        None => return Ok(next.run(req).await),
    };
    if let Some(h) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(s) = h.to_str() {
            if let Some(rest) = s.strip_prefix("Bearer ") {
                if rest == required.0.as_str() {
                    return Ok(next.run(req).await);
                }
            }
        }
    }
    // Query string fallback.
    let q = req.uri().query().unwrap_or("");
    if let Some(pos) = q.find("token=") {
        let after = &q[pos + "token=".len()..];
        let value = after.split('&').next().unwrap_or("");
        if value == required.0.as_str() {
            return Ok(next.run(req).await);
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

#[allow(dead_code)]
pub async fn require_ws_token(
    axum::extract::State(token): axum::extract::State<Arc<Option<Token>>>,
    Query(q): Query<Q>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let required = match token.as_ref() {
        Some(t) => t,
        None => return Ok(next.run(req).await),
    };
    if let Some(t) = q.token {
        if t == required.0 {
            return Ok(next.run(req).await);
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}
