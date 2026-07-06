//! Local socket server: newline-framed JSON-RPC over a Unix domain socket.
//!
//! Each accepted connection is handled in a tokio task; requests are read
//! one line at a time, dispatched through [`crate::handlers::dispatch`], and
//! the response is written back as a single line followed by `\n`.
//!
//! Parse errors are converted into a `Response::Err { code: "parse_error", .. }`
//! so the caller can always parse the response (no connection-level errors
//! escape as raw text).
//!
//! The server loop reads `Method::DaemonShutdown` as a no-op for now — the
//! `serve` future just keeps running. Process-level shutdown is wired by the
//! binary entrypoint (a later task) which awaits a shutdown signal.

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, warn};

use crate::handlers;
use crate::rpc::{Request, Response, RpcError};
use crate::state::SharedState;

/// Spawn the daemon server bound to [`crate::socket::socket_path`].
///
/// Returns immediately; the future runs until the process is signalled.
/// Each accepted connection is handled in its own tokio task so multiple
/// clients can connect concurrently.
pub async fn serve(state: SharedState) -> anyhow::Result<()> {
    serve_at(state, crate::socket::socket_path()).await
}

/// Same as [`serve`] but binds to an explicit path. Used by tests so they
/// can use `tempfile::tempdir()` and avoid colliding with a real daemon.
pub async fn serve_at(state: SharedState, path: std::path::PathBuf) -> anyhow::Result<()> {
    // Remove any stale socket file from a previous run.
    let _ = tokio::fs::remove_file(&path).await;

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind {}", path.display()))?;
    info!("daemon listening at {}", path.display());

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                warn!("accept error: {e}");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(state, stream).await {
                warn!("connection error: {e}");
            }
        });
    }
}

/// Handle a single client connection: read lines, dispatch each, write the
/// response line back. Loop ends when the client closes the socket.
pub async fn handle_conn(state: SharedState, stream: UnixStream) -> anyhow::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Request<serde_json::Value> = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::Err {
                    id: "?".into(),
                    error: RpcError::new("parse_error", e.to_string()),
                };
                write_response(&mut write, &resp).await?;
                continue;
            }
        };
        let resp = handlers::dispatch(state.clone(), req).await;
        write_response(&mut write, &resp).await?;
    }
    Ok(())
}

/// Serialise a response to one line and write it (plus `\n`) to the writer.
async fn write_response<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    resp: &Response<serde_json::Value>,
) -> anyhow::Result<()> {
    let s = serde_json::to_string(resp).context("serialise response")?;
    w.write_all(s.as_bytes()).await?;
    w.write_all(b"\n").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DaemonState;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn fresh_state() -> SharedState {
        Arc::new(tokio::sync::RwLock::new(DaemonState::new()))
    }

    /// Unit test for the per-connection read loop. Binds to a temp socket,
    /// writes one request, reads one response, closes.
    #[tokio::test]
    async fn handle_conn_round_trips_one_request() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");
        let state = fresh_state();

        // Bind listener directly (so we can stop it after the one client).
        let listener = UnixListener::bind(&path).unwrap();
        let server_path = path.clone();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            // accept once, then handle
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(server_state, stream).await;
            }
            let _ = tokio::fs::remove_file(&server_path).await;
        });

        let mut client = UnixStream::connect(&path).await.unwrap();
        let req = serde_json::json!({
            "id": "x",
            "method": "daemon_ping",
            "params": {}
        });
        let s = serde_json::to_string(&req).unwrap();
        client.write_all(s.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap(); // signal EOF after the one request

        let (read, _write) = client.into_split();
        let mut lines = BufReader::new(read).lines();
        let line = lines.next_line().await.unwrap().unwrap();
        let resp: Response = serde_json::from_str(&line).unwrap();
        match resp {
            Response::Ok { result, .. } => assert_eq!(result["ok"], true),
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }

        let _ = server.await;
    }

    /// Parse-error path: garbage JSON on the wire must come back as a
    /// `Response::Err { code: "parse_error", .. }`, not as a closed socket.
    #[tokio::test]
    async fn handle_conn_garbage_returns_parse_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");
        let state = fresh_state();

        let listener = UnixListener::bind(&path).unwrap();
        let server_path = path.clone();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(server_state, stream).await;
            }
            let _ = tokio::fs::remove_file(&server_path).await;
        });

        let mut client = UnixStream::connect(&path).await.unwrap();
        client.write_all(b"this is not json\n").await.unwrap();
        client.shutdown().await.unwrap();

        let (read, _write) = client.into_split();
        let mut lines = BufReader::new(read).lines();
        let line = lines.next_line().await.unwrap().unwrap();
        let resp: Response = serde_json::from_str(&line).unwrap();
        match resp {
            Response::Err { error, .. } => assert_eq!(error.code, "parse_error"),
            Response::Ok { result, .. } => panic!("expected Err, got Ok: {result:?}"),
        }

        let _ = server.await;
    }
}
