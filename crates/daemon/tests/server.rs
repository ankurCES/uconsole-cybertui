//! Integration tests for the daemon's local socket server.
//!
//! Each test binds to a `tempfile::tempdir()` so concurrent test runs don't
//! collide on `/tmp/cyberdeck.sock`. The server future is run on a tokio
//! runtime spawned with `#[tokio::test]`; cancellation drops the future and
//! the socket file is removed on tear-down by `tempdir`'s Drop.

use std::sync::Arc;
use std::time::Duration;

use cyberdeck_daemon::rpc::{Request, Response};
use cyberdeck_daemon::server;
use cyberdeck_daemon::state::DaemonState;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::RwLock;

type SharedState = Arc<RwLock<DaemonState>>;

fn fresh_state() -> SharedState {
    Arc::new(RwLock::new(DaemonState::new()))
}

/// Helper: open a connection, write `request_json` + `\n`, read one
/// response line, return it as `Response`.
async fn round_trip(path: &std::path::Path, request_json: &str) -> Response {
    let mut client = UnixStream::connect(path).await.expect("connect");
    client
        .write_all(request_json.as_bytes())
        .await
        .expect("write request");
    client.write_all(b"\n").await.expect("write newline");
    client.shutdown().await.expect("shutdown write half");

    let (read, _write) = client.into_split();
    let mut lines = BufReader::new(read).lines();
    let line = lines
        .next_line()
        .await
        .expect("read line")
        .expect("non-empty response");
    serde_json::from_str(&line).expect("parse response JSON")
}

#[tokio::test]
async fn server_accepts_request_sends_response() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("daemon.sock");
    let state = fresh_state();

    // Spawn the server.
    let server_path = path.clone();
    let server_state = state.clone();
    let handle = tokio::spawn(async move {
        server::serve_at(server_state, server_path).await
    });
    // Give it a tick to bind.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = round_trip(&path, r#"{"id":"x","method":"daemon_ping","params":{}}"#).await;
    match resp {
        Response::Ok { result, .. } => assert_eq!(result["ok"], true),
        Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn server_handles_concurrent_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("daemon.sock");
    let state = fresh_state();

    let server_path = path.clone();
    let server_state = state.clone();
    let handle = tokio::spawn(async move {
        server::serve_at(server_state, server_path).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Two clients simultaneously: one pings, one asks for the workspace list.
    let path_a = path.clone();
    let path_b = path.clone();
    let client_a = tokio::spawn(async move {
        round_trip(&path_a, r#"{"id":"a","method":"daemon_ping","params":{}}"#).await
    });
    let client_b = tokio::spawn(async move {
        round_trip(&path_b, r#"{"id":"b","method":"workspace_list","params":{}}"#).await
    });

    let (a, b) = tokio::join!(client_a, client_b);
    let resp_a = a.unwrap();
    let resp_b = b.unwrap();
    match resp_a {
        Response::Ok { result, .. } => assert_eq!(result["ok"], true),
        Response::Err { error, .. } => panic!("client_a error: {error:?}"),
    }
    match resp_b {
        Response::Ok { result, .. } => {
            // DaemonState::new() seeds one workspace named "cyberdeck".
            let arr = result.as_array().expect("result is array");
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["name"], "cyberdeck");
        }
        Response::Err { error, .. } => panic!("client_b error: {error:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn server_garbage_request_returns_parse_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("daemon.sock");
    let state = fresh_state();

    let server_path = path.clone();
    let server_state = state.clone();
    let handle = tokio::spawn(async move {
        server::serve_at(server_state, server_path).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = round_trip(&path, "this is definitely not json").await;
    match resp {
        Response::Err { error, .. } => assert_eq!(error.code, "parse_error"),
        Response::Ok { result, .. } => panic!("expected Err, got Ok: {result:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn server_shutdown_method_doesnt_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("daemon.sock");
    let state = fresh_state();

    let server_path = path.clone();
    let server_state = state.clone();
    let handle = tokio::spawn(async move {
        server::serve_at(server_state, server_path).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = round_trip(&path, r#"{"id":"s","method":"daemon_shutdown","params":{}}"#).await;
    match resp {
        Response::Ok { result, .. } => {
            assert_eq!(result["ok"], true);
            assert_eq!(result["shutdown"], true);
        }
        Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
    }

    // We don't actually exit the process — just confirm the loop is still
    // serving. Send another ping right after to prove it.
    let resp2 = round_trip(&path, r#"{"id":"s2","method":"daemon_ping","params":{}}"#).await;
    match resp2 {
        Response::Ok { result, .. } => assert_eq!(result["ok"], true),
        Response::Err { error, .. } => panic!("unexpected error after shutdown: {error:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn server_rejects_unknown_method_field() {
    // A request that has the right shape but an unrecognised method name.
    // Serde should reject it as an "unknown variant" parse error.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("daemon.sock");
    let state = fresh_state();

    let server_path = path.clone();
    let server_state = state.clone();
    let handle = tokio::spawn(async move {
        server::serve_at(server_state, server_path).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = round_trip(
        &path,
        r#"{"id":"u","method":"never_heard_of_it","params":{}}"#,
    )
    .await;
    match resp {
        Response::Err { error, .. } => {
            // Either a serde-level parse error or a NotImplemented-style error.
            // The implementation surfaces this as `parse_error` because the
            // Request struct fails to deserialise at the serde layer.
            assert!(
                error.code == "parse_error"
                    || error.code == "not_implemented"
                    || error.code == "unknown_variant",
                "unexpected error code: {}",
                error.code
            );
        }
        Response::Ok { result, .. } => panic!("expected Err, got Ok: {result:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn request_struct_with_struct_variant_round_trips() {
    // Confirm that the wire format works for struct-bearing methods too.
    // e.g. `WorkspaceNew { name: "alpha" }` becomes
    // `{"method":"workspace_new","name":"alpha"}` at the wire level.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("daemon.sock");
    let state = fresh_state();

    let server_path = path.clone();
    let server_state = state.clone();
    let handle = tokio::spawn(async move {
        server::serve_at(server_state, server_path).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = round_trip(
        &path,
        r#"{"id":"w","method":"workspace_new","name":"alpha","params":{}}"#,
    )
    .await;
    match resp {
        Response::Ok { result, .. } => {
            assert!(result["id"].as_u64().is_some(), "expected numeric id");
        }
        Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
    }

    handle.abort();
}

/// Smoke test: build a Request from the typed API and verify it matches the
/// shape the server expects (no nested `method` field). This is the
/// regression guard for the `#[serde(flatten)]` fix on `Request.method`.
#[test]
fn request_struct_serialises_flat() {
    let r = Request {
        id: "x".into(),
        method: cyberdeck_daemon::rpc::Method::DaemonPing,
        params: serde_json::json!({}),
    };
    let s = serde_json::to_string(&r).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["id"], "x");
    assert_eq!(v["method"], "daemon_ping");
    // No nested object under `method`.
    assert!(v["method"].is_string(), "method must be a string, got: {}", v["method"]);
}