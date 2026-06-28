//! End-to-end smoke tests for the LAN web server: spawn `run_with` on an
//! ephemeral port, then drive it with `reqwest` (HTTP) and
//! `tokio-tungstenite` (WebSocket). Verifies:
//!   - the bearer-token middleware blocks unauthenticated requests,
//!   - the query-string fallback works for the WS upgrade,
//!   - the JSON API serves the system-info route,
//!   - the WebSocket streams a `StatusSnapshot` on connect.
//!
//! These tests are deliberately small — they exercise the LAN *plumbing*
//! (routing, auth, middleware, WS upgrade), not the underlying
//! `cyberdeck_core` calls, which require real hardware. The `StandaloneLive`
//! fixture returns canned defaults so the test doesn't need nmcli/pactl/etc.
//!
//! Run with: `cargo test -p cyberdeck-web --test lan_smoke -- --test-threads=1`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cyberdeck_web::auth::Token;
use cyberdeck_web::run::standalone::StandaloneLive;

use futures::StreamExt;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

fn build_router(
    live: Arc<StandaloneLive>,
    token: Option<Token>,
) -> axum::Router {
    use axum::middleware;
    use cyberdeck_web::api::router as api_router;
    use cyberdeck_web::auth::require_bearer;
    use cyberdeck_web::shell::router as shell_router;
    use cyberdeck_web::ws::router as ws_router;

    let token_arc = Arc::new(token);
    let api_state = cyberdeck_web::api::ApiState {
        token: token_arc.clone(),
        live: live.clone(),
        tx: None,
    };
    axum::Router::new()
        .merge(api_router(api_state.clone()))
        .merge(ws_router(api_state.clone()))
        .merge(shell_router(Arc::new(api_state)))
        .layer(middleware::from_fn_with_state(token_arc, require_bearer))
}

async fn spawn_server(token: Option<Token>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let live = Arc::new(StandaloneLive::default());
    tokio::spawn(async move {
        let serve = axum::serve(listener, build_router(live, token));
        let _ = serve.await;
    });
    // Give the server a tick to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

#[tokio::test]
async fn open_server_serves_system_info() {
    let addr = spawn_server(None).await;
    let resp = reqwest::get(format!("http://{addr}/api/system"))
        .await
        .expect("GET /api/system");
    assert!(resp.status().is_success(), "expected 2xx, got {}", resp.status());
}

#[tokio::test]
async fn token_required_returns_401_without_bearer() {
    let token = Token::new();
    let addr = spawn_server(Some(token.clone())).await;
    let resp = reqwest::get(format!("http://{addr}/api/system"))
        .await
        .expect("GET /api/system");
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn token_required_returns_200_with_bearer() {
    let token = Token::new();
    let addr = spawn_server(Some(token.clone())).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/api/system"))
        .bearer_auth(token.0.clone())
        .send()
        .await
        .expect("GET /api/system");
    assert!(resp.status().is_success(), "expected 2xx, got {}", resp.status());
}

#[tokio::test]
async fn token_in_query_string_authorizes_ws_upgrade() {
    let token = Token::new();
    let addr = spawn_server(Some(token.clone())).await;
    // The WS upgrade requires the token in the query string (EventSource
    // can't set headers). Build the upgrade request manually.
    let url = format!("ws://{addr}/api/ws?token={}", token.0);
    let req = url.into_client_request().expect("request");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws connect");
    // First frame should be a Text snapshot. Drain one, then close.
    let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("snapshot within 3s")
        .expect("ws stream alive")
        .expect("ws message");
    let text = match msg {
        tokio_tungstenite::tungstenite::Message::Text(s) => s,
        other => panic!("expected Text, got {other:?}"),
    };
    // The snapshot is JSON. We don't care about the exact shape — that's
    // covered by the `StatusSnapshot` serialization — but it must contain
    // the `hostname` field.
    assert!(text.contains("hostname"), "snapshot text: {text}");
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn ws_rejects_without_token() {
    let token = Token::new();
    let addr = spawn_server(Some(token.clone())).await;
    let url = format!("ws://{addr}/api/ws");
    let req = url.into_client_request().expect("request");
    let res = tokio_tungstenite::connect_async(req).await;
    assert!(res.is_err(), "expected ws to reject without token");
    let err = res.err().unwrap();
    let s = err.to_string();
    assert!(
        s.contains("401") || s.to_lowercase().contains("unauthorized"),
        "expected 401/unauthorized, got: {s}"
    );
}

#[tokio::test]
async fn shell_serves_index_html() {
    let addr = spawn_server(None).await;
    let resp = reqwest::get(format!("http://{addr}/"))
        .await
        .expect("GET /");
    assert!(resp.status().is_success(), "expected 2xx, got {}", resp.status());
    let body = resp.text().await.expect("body");
    assert!(body.contains("cyberdeck"), "shell html: {body}");
}

#[tokio::test]
async fn shell_serves_static_app_js() {
    let addr = spawn_server(None).await;
    let resp = reqwest::get(format!("http://{addr}/static/app.js"))
        .await
        .expect("GET /static/app.js");
    assert!(resp.status().is_success(), "expected 2xx, got {}", resp.status());
    let body = resp.text().await.expect("body");
    assert!(body.contains("cyberdeck"), "app.js: {body}");
}