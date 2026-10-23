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

use axum::extract::{Request, State};
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
    /// UDP address to listen on for nexmon_csi frames (vitals sensing).
    /// `None` disables the UDP path. Default bind: `0.0.0.0:5500`.
    pub csi_udp: Option<SocketAddr>,
    /// Read nexmon CSI from a pcap stream (`-` = stdin). This is the reliable
    /// nexmon_csi path: `tcpdump -i wlan0 -U -w - 'udp port 5500' | ...`.
    pub csi_pcap: Option<PathBuf>,
    /// Fixed CSI sample rate override (Hz). `None` = estimate from arrivals.
    pub csi_rate_hz: Option<f32>,
    /// Presence/motion detection threshold on the chest signal.
    pub csi_motion_threshold: f32,
    /// Shared bearer token guarding `/api/*` and the SSE stream.
    /// `None` disables the gate (tests, local dev). The HTML shell and
    /// `/static` stay public either way so the page can boot; every
    /// data path requires the token.
    pub auth_token: Option<String>,
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

/// Bearer-token gate for the data plane.
///
/// The token may arrive as `Authorization: Bearer <t>`, `?token=<t>`
/// (needed by `EventSource`, which cannot set headers) or a
/// `freya_token` cookie (set by the UI after the first ?token= hit).
/// A `None` state disables the gate entirely.
async fn auth_middleware(
    State(required): State<Option<Arc<String>>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(token) = required.as_deref() else {
        return next.run(req).await;
    };
    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    let supplied = supplied.or_else(|| {
        let q = req.uri().query()?;
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == "token").then(|| v.to_owned())
        })
    });
    let supplied = supplied.or_else(|| {
        req.headers()
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').find_map(|c| {
                    let c = c.trim();
                    c.strip_prefix("freya_token=")
                        .map(|v| v.trim().to_owned())
                })
            })
    });

    match supplied {
        Some(supplied) if constant_time_eq(supplied.as_bytes(), token.as_bytes()) => {
            next.run(req).await
        }
        _ => Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Bearer")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                "{\"error\":\"unauthorized\",\"hint\":\"present the shared token as Authorization: Bearer, ?token= or freya_token cookie\"}",
            ))
            .unwrap(),
    }
}

/// Constant-time comparison so a misbehaving LAN peer cannot time the
/// token out byte by byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Build the full axum app: HTML shell + static assets + API + SSE.
/// `auth_token` guards the API + SSE merge; the shell and static
/// assets stay public so the page can boot at all.
pub fn build_router(
    state: Arc<AppState>,
    static_dir: PathBuf,
    auth_token: Option<String>,
) -> Router {
    let api = crate::api::router(state.clone());
    let sse = sse_router(state.clone());
    let shell = Router::new().route("/", get(shell_handler));

    let token_state = auth_token.map(Arc::new);
    let protected = api
        .merge(sse)
        .layer(middleware::from_fn_with_state(
            token_state.clone(),
            auth_middleware,
        ));

    protected
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
    // Phase 3 — BLE store mirrors the Wi-Fi one. Spun up here so
    // the `/api/ble_devices` endpoint always returns a snapshot
    // (empty is fine — it means no BLE adapter is reporting).
    let ble_store = Arc::new(crate::ble_devices::BleDeviceStore::new());
    let vitals = Arc::new(crate::csi::VitalsStore::new());

    let (events_tx, _) = broadcast::channel::<DeviceEvent>(SSE_CHANNEL_CAPACITY);
    let (scanner_tx, mut scanner_rx) =
        mpsc::channel::<DeviceEvent>(SSE_CHANNEL_CAPACITY);

    let state = Arc::new(AppState {
        store: store.clone(),
        ble_store: ble_store.clone(),
        tags: tags.clone(),
        vitals: vitals.clone(),
        events_tx: events_tx.clone(),
        scanner_tx: scanner_tx.clone(),
    });

    // Phase 3 — BLE scanner spawn. Tries the live BlueZ source
    // first; falls back to the Dev synthetic stream when
    // bluetoothd isn't reachable so the radar canvas isn't blank
    // on dev machines without Bluetooth hardware.
    let ble_source = if std::env::var("WIFI_RADAR_BLE_DEV").is_ok() {
        crate::ble_scanner::ScannerSource::Dev
    } else {
        crate::ble_scanner::ScannerSource::BlueZ
    };
    if let Some(handle) = crate::ble_scanner::spawn(ble_store.clone(), ble_source) {
        // Park the handle so the spawned task isn't dropped
        // (drop = immediate cancel via abort()). On real shutdown
        // the axum server's `with_graceful_shutdown` aborts the
        // runtime, which tears the task down with it — we don't
        // call `stop()` explicitly here because `run_with` doesn't
        // own the shutdown signal. A future enhancement is to wire
        // `Stop` into the `StandaloneLive` handle so callers can
        // cancel cleanly.
        tokio::spawn(async move {
            // Future: drive `handle.stop()` from the shutdown
            // signal. For now we leak the handle (the JoinHandle
            // is in the spawned task) which is fine — when axum
            // aborts the runtime the inner task is torn down.
            std::mem::forget(handle);
        });
    }

    // CSI vitals sensing (nexmon_csi → UDP). Additive: it runs alongside the
    // RSSI radar and only starts when a listen address is configured.
    if let Some(csi_bind) = cfg.csi_udp {
        let vitals = vitals.clone();
        let rate = cfg.csi_rate_hz;
        let thr = cfg.csi_motion_threshold;
        tokio::spawn(async move {
            if let Err(e) = crate::csi::run_csi_udp(csi_bind, vitals, rate, thr).await {
                tracing::warn!(error = %e, "csi: UDP listener exited");
            }
        });
    }
    if let Some(pcap) = cfg.csi_pcap.clone() {
        let vitals = vitals.clone();
        let rate = cfg.csi_rate_hz;
        let thr = cfg.csi_motion_threshold;
        tokio::spawn(async move {
            if let Err(e) = crate::csi::run_csi_pcap(pcap, vitals, rate, thr).await {
                tracing::warn!(error = %e, "csi: pcap reader exited");
            }
        });
    }

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

    let app = build_router(state, cfg.static_dir, cfg.auth_token.clone());

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