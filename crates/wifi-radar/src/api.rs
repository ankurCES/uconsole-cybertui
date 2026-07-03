//! HTTP API for the radar.
//!
//! Routes:
//!   - `GET  /api/devices`     — JSON snapshot of the device store
//!   - `GET  /api/tags`        — JSON snapshot of the tag overlay
//!   - `POST /api/tags`        — upsert a tag
//!   - `DELETE /api/tags/:mac` — remove a tag
//!   - `GET  /api/events`      — SSE stream of `DeviceEvent`s
//!   - `GET  /api/health`      — `{"ok": true}`
//!
//! The SSE endpoint is built separately from the rest of the API because
//! it needs to subscribe to a `broadcast::Receiver<DeviceEvent>` per
//! connection. The two routers are nested in [`crate::run`] so the
//! final app has a single state type.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;

use crate::csi::{VitalReading, VitalsStore};
use crate::devices::{DeviceState, DeviceStore};
use crate::frames::DeviceEvent;
use crate::tags::{Tag, TagDb};

/// Shared state injected into every handler.
pub struct AppState {
    pub store: Arc<DeviceStore>,
    pub tags: Arc<TagDb>,
    /// Latest CSI-derived vitals reading (breathing/heart/presence).
    pub vitals: Arc<VitalsStore>,
    /// Live `DeviceEvent` channel — new events are broadcast here for SSE.
    pub events_tx: broadcast::Sender<DeviceEvent>,
    /// Scanner writes here; we forward into `events_tx` for SSE clients.
    pub scanner_tx: mpsc::Sender<DeviceEvent>,
}

/// What `GET /api/devices` returns.
#[derive(Debug, Serialize)]
pub struct DevicesResponse {
    pub devices: Vec<DeviceState>,
    pub tags: HashMap<String, Tag>,
}

/// `POST /api/tags` body.
#[derive(Debug, Serialize, Deserialize)]
pub struct UpsertTagRequest {
    pub mac: String,
    pub label: String,
    pub icon: String,
    pub color: String,
}

/// `POST /api/tags` response.
#[derive(Debug, Serialize)]
pub struct UpsertTagResponse {
    pub ok: bool,
    pub replaced: bool,
}

/// Build the non-SSE API router (used for tests and as a sub-router of the
/// full app).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/vitals", get(get_vitals))
        .route("/api/devices", get(get_devices))
        .route("/api/tags", get(get_tags))
        .route("/api/tags", post(post_tag))
        .route("/api/tags/:mac", delete(delete_tag))
        .with_state(state)
}

/// Build the SSE-only router. Merged with the main router in `run.rs`.
pub fn sse_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/events", get(get_events))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

/// `GET /api/vitals` — the latest CSI-derived vitals reading. Returns the
/// "empty" reading (all zero, `presence:false`) when no nexmon CSI is flowing.
async fn get_vitals(State(state): State<Arc<AppState>>) -> Json<VitalReading> {
    Json(state.vitals.get())
}

async fn get_devices(State(state): State<Arc<AppState>>) -> Json<DevicesResponse> {
    let devices = state.store.snapshot();
    let tags = state
        .tags
        .overlay(devices.iter().map(|d| d.mac.as_str()));
    Json(DevicesResponse { devices, tags })
}

async fn get_tags(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let snap = state.tags.snapshot();
    Json(serde_json::json!({ "tags": snap.tags }))
}

async fn post_tag(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpsertTagRequest>,
) -> Result<Json<UpsertTagResponse>, (StatusCode, String)> {
    let prev = state
        .tags
        .upsert(
            &req.mac,
            Tag {
                label: req.label,
                icon: req.icon,
                color: req.color,
            },
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(UpsertTagResponse {
        ok: true,
        replaced: prev.is_some(),
    }))
}

async fn delete_tag(
    State(state): State<Arc<AppState>>,
    Path(mac): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let removed = state
        .tags
        .delete(&mac)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "removed": removed })))
}

/// SSE endpoint — each connection subscribes to the broadcast channel.
async fn get_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|ev| async move {
        match ev {
            Ok(ev) => match serde_json::to_string(&ev) {
                Ok(s) => Some(Ok(Event::default().data(s))),
                Err(_) => None,
            },
            // Lagged or closed: drop and let the stream continue — the
            // browser will reconnect if the connection itself dies.
            Err(_) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}