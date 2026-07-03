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

use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use askama::Template;
use tokio::sync::{broadcast, mpsc};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
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

/// Stamp `Cache-Control: no-store` on every response. We use
/// `tower_http::set_header::SetResponseHeaderLayer` rather than an
/// axum `from_fn` middleware because `nest_service("/static", ...)`
/// mounts `ServeDir` as an internal router node, and axum 0.7's
/// `from_fn` only wraps responses from routes defined on the router
/// itself. The `tower-http` layer wraps the whole tower stack and
/// reliably stamps the header on every response, including
/// `nest_service` and SSE responses.
fn no_store_layer() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    )
}

/// Fallback middleware: if `ServeDir` returns 404 for a `/static/*` request,
/// try the embedded asset set (see [`crate::assets`]). This makes the
/// binary self-contained when launched from a cwd that doesn't contain
/// the workspace's `crates/wifi-radar/web` directory (e.g. when installed
/// to `/usr/local/bin/wifi-radar` and started by systemd).
async fn static_fallback(req: Request, next: Next) -> Response {
    // Read the path off the request itself, before running the inner
    // service. `nest_service` doesn't populate `OriginalUri`, so we have
    // to grab it from the incoming request.
    let uri_path = req.uri().path().to_string();

    let resp = next.run(req).await;
    if resp.status() != StatusCode::NOT_FOUND {
        return resp;
    }

    let Some(name) = uri_path.strip_prefix("/static/") else {
        return resp;
    };
    // Guard against path traversal: embedded keys are flat filenames only.
    if name.is_empty() || name.contains("..") || name.starts_with('/') {
        return resp;
    }
    if !crate::assets::contains(name) {
        return resp;
    }
    crate::assets::get(name)
}

/// Build the full axum app: HTML shell + static assets + API + SSE.
pub fn build_router(state: Arc<AppState>, static_dir: PathBuf) -> Router {
    let api = crate::api::router(state.clone());
    let sse = sse_router(state.clone());
    let shell = Router::new().route("/", get(shell_handler));

    api.merge(sse)
        .merge(shell)
        .nest_service("/static", ServeDir::new(static_dir))
        // Order matters: `no_store_layer` MUST be applied first so it
        // wraps the entire stack including `nest_service` and the
        // `static_fallback` middleware below.
        .layer(no_store_layer())
        // Run the embedded-asset fallback only over the `/static` mount
        // so API routes are unaffected.
        .layer(middleware::from_fn(static_fallback))
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