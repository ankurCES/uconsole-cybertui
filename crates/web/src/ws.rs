//! WebSocket that streams live status snapshots to the browser.
//!
//! Browser-side usage: `new WebSocket("ws://host/api/ws?token=...")`. The
//! server pushes a `StatusSnapshot` (JSON object) every second. The browser
//! just renders — no client logic besides diff-and-paint.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

use crate::api::{ApiState, LiveRead};

#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    pub hostname: String,
    pub kernel: String,
    pub uptime_secs: u64,
    pub loadavg: (f64, f64, f64),
    pub mem_total_kb: u64,
    pub mem_avail_kb: u64,
    pub battery: Option<cyberdeck_core::power::Battery>,
    pub thermals: Vec<cyberdeck_core::sys::ThermalReading>,
    pub interfaces: Vec<cyberdeck_core::net::Interface>,
    pub active_ssid: Option<String>,
}

impl StatusSnapshot {
    pub async fn from_live(live: Arc<dyn LiveRead + Send + Sync>) -> Self {
        let info = live.info().await;
        Self {
            hostname: info.hostname,
            kernel: info.kernel,
            uptime_secs: info.uptime_secs,
            loadavg: info.loadavg,
            mem_total_kb: info.memory.total_kb,
            mem_avail_kb: info.memory.available_kb,
            battery: live.battery().await,
            thermals: live.thermals().await,
            interfaces: live.interfaces().await,
            active_ssid: live.active_ssid().await,
        }
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/ws", get(ws_handler))
        .with_state(state)
}
async fn ws_handler(ws: WebSocketUpgrade, State(s): State<ApiState>) -> impl IntoResponse {
    let live = s.live.clone();
    ws.on_upgrade(move |socket| ws_loop(socket, live))
}

async fn ws_loop(socket: WebSocket, live: Arc<dyn LiveRead + Send + Sync>) {
    let (mut tx, mut rx) = socket.split();
    // Send a snapshot every second; ignore client messages (we don't accept
    // any — actions go through the JSON API to keep concerns separate).
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let snap = StatusSnapshot::from_live(live.clone()).await;
                let s = match serde_json::to_string(&snap) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if tx.send(Message::Text(s)).await.is_err() {
                    break;
                }
            }
            maybe = rx.next() => {
                if maybe.is_none() {
                    break;
                }
            }
        }
    }
}
