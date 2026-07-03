//! Public entry point for the wifi-radar binary.
//!
//! `run_with(bind, dev_mode)` is the same pattern `cyberdeck-web` uses:
//! the lib exposes the long-lived async function, the `bin` calls it.
//! This keeps tests pure (they construct the router directly) while
//! letting `main` stay short.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use askama::Template;
use tokio::sync::{broadcast, mpsc};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::api::{sse_router, AppState};
use crate::devices::DeviceStore;
use crate::frames::DeviceEvent;
use crate::scanner::{spawn, ScannerSource};
use crate::shell::IndexTemplate;
use crate::tags::TagDb;

/// Default tag DB path. Resolved relative to the cwd.
pub const DEFAULT_TAGS_PATH: &str = "data/tags.json";

/// Default static-asset directory, relative to the workspace root.
pub const DEFAULT_STATIC_DIR: &str = "crates/wifi-radar/web";

/// Where `run_with` puts the live `DeviceEvent` stream.
const SSE_CHANNEL_CAPACITY: usize = 256;

/// What `main` passes to `run_with`.
pub struct RunConfig {
    pub bind: SocketAddr,
    pub dev_mode: bool,
    pub tags_path: PathBuf,
    pub static_dir: PathBuf,
    pub pcap_path: Option<PathBuf>,
}

/// Long-lived handle returned by `run_with`. Drop it (or call [`shutdown`])
/// to stop the server.
pub struct StandaloneLive {
    pub addr: SocketAddr,
    pub shutdown: tokio::sync::oneshot::Sender<()>,
}

impl StandaloneLive {
    pub async fn stop(self) {
        let _ = self.shutdown.send(());
    }
}

/// Build the full axum app: HTML shell + static assets + API + SSE.
pub fn build_router(state: Arc<AppState>, static_dir: PathBuf) -> Router {
    let api = crate::api::router(state.clone());
    let sse = sse_router(state.clone());
    let shell = Router::new().route("/", get(shell_handler));

    api.merge(sse)
        .merge(shell)
        .nest_service("/static", ServeDir::new(static_dir))
        .layer(TraceLayer::new_for_http())
}

async fn shell_handler() -> Result<impl IntoResponse, axum::http::StatusCode> {
    let t = IndexTemplate::default();
    let body = t.render().map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    ))
}

/// Start the server and return a handle. Blocks until shutdown is signalled.
pub async fn run_with(cfg: RunConfig) -> anyhow::Result<StandaloneLive> {
    let tags = Arc::new(TagDb::load(&cfg.tags_path)?);
    let store = Arc::new(DeviceStore::new());

    let (events_tx, _) = broadcast::channel::<DeviceEvent>(SSE_CHANNEL_CAPACITY);
    let (scanner_tx, mut scanner_rx) =
        mpsc::channel::<DeviceEvent>(SSE_CHANNEL_CAPACITY);

    let state = Arc::new(AppState {
        store: store.clone(),
        tags: tags.clone(),
        events_tx: events_tx.clone(),
        scanner_tx: scanner_tx.clone(),
    });

    // Scanner → broadcast fan-out.
    let fanout_tx = events_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = scanner_rx.recv().await {
            let _ = fanout_tx.send(ev);
        }
    });

    // Pick a scanner source: explicit pcap wins, then dev_mode, then dev default.
    let source = match (cfg.pcap_path.clone(), cfg.dev_mode) {
        (Some(p), _) => ScannerSource::PcapFile(p),
        (None, true) => ScannerSource::Dev,
        (None, false) => ScannerSource::Dev, // safe default — no live iface to read from anyway
    };
    let scanner_handle = spawn(store.clone(), scanner_tx, source);

    let app = build_router(state, cfg.static_dir);

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    let addr = listener.local_addr()?;

    tracing::info!(%addr, dev_mode = cfg.dev_mode, "wifi-radar listening");

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = rx.await;
        tracing::info!("wifi-radar: graceful shutdown");
    });

    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!(error = %e, "wifi-radar server error");
        }
    });

    // Wait for ctrl-c OR for the explicit shutdown signal.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("wifi-radar: ctrl-c received");
        }
        _ = tokio::time::sleep(Duration::from_secs(u64::MAX / 2)) => {
            // No signal path; rely on the explicit shutdown channel below.
        }
    }

    let _ = tx.send(());
    let _ = server_handle.await;
    scanner_handle.stop().await;

    Ok(StandaloneLive {
        addr,
        shutdown: tokio::sync::oneshot::channel().0,
    })
}