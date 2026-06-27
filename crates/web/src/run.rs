//! The public entry point used by both the `cyberdeck-web` binary and the
//! `cyberdeck-tui --web` flag.
//!
//! The TUI version builds its own `LiveRead` adapter that reads from the
//! shared `cyberdeck_core::...` types in `Arc<RwLock<...>>`. The standalone
//! binary builds an in-process refresher that calls the core functions
//! directly.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::middleware;
use tokio::sync::mpsc;

use crate::api::{ApiState, LiveRead};
use crate::auth::Token;
use crate::shell;

/// The action type the API sends back into the TUI. The TUI re-defines its
/// own `Action` enum, so we re-export a minimal compatible shape here and
/// the TUI converts it. The `toast_compat` shim handles the type bridging.
pub mod toast_compat {
    use serde::{Deserialize, Serialize};
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Action {
        pub kind: &'static str,
        pub message: String,
    }
}

/// Default bind address. Configurable via the `bind` argument.
pub const DEFAULT_BIND: &str = "0.0.0.0:7878";

/// Run the web server. `live` is whatever implements `LiveRead` (a trait
/// object so we don't have to thread the TUI's concrete `Live` through here).
/// `tx` is the TUI's action channel, used to push toasts back when an action
/// is taken over the web.
/// `token` is the optional bearer token to require; if `None`, a fresh one
/// is generated for this run. The installer passes a pinned token so it
/// survives `systemctl restart`.
pub async fn run_with(
    bind: &str,
    live: Arc<dyn LiveRead + Send + Sync>,
    tx: Option<mpsc::Sender<toast_compat::Action>>,
    token: Option<Token>,
) -> anyhow::Result<()> {
    let token = Arc::new(Some(token.unwrap_or_else(Token::new)));
    if let Some(t) = token.as_ref() {
        tracing::info!("web auth token: {}", t.0);
        eprintln!("cyberdeck-web: bearer token = {}", t.0);
        eprintln!("cyberdeck-web: open http://{bind}/?token={}", t.0);
    }

    let api_state = ApiState {
        token: token.clone(),
        live: live.clone(),
        tx,
    };
    let app = axum::Router::new()
        .merge(crate::api::router(api_state.clone()))
        .merge(crate::ws::router(api_state.clone()))
        .merge(shell::router(Arc::new(api_state)))
        .layer(middleware::from_fn_with_state(
            token.clone(),
            crate::auth::require_bearer,
        ));

    let addr: SocketAddr = bind.parse().context("parse bind address")?;
    tracing::info!("cyberdeck-web listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    axum::serve(listener, app).await.context("axum::serve")?;
    Ok(())
}

/// Standalone `LiveRead` impl for the `cyberdeck-web` binary: it doesn't
/// share state with a TUI, so it just calls the core functions on demand.
/// The web binary spawns one refresher task per resource.
pub mod standalone {
    use super::*;
    use cyberdeck_core::{
        audio, bluetooth, display, net, packages, power, process, services, storage, sys,
    };
    use std::sync::Arc;
    use tokio::sync::RwLock;

    pub struct StandaloneLive {
        pub info: Arc<RwLock<sys::SystemInfo>>,
        pub battery: Arc<RwLock<Option<power::Battery>>>,
        pub thermals: Arc<RwLock<Vec<sys::ThermalReading>>>,
        pub interfaces: Arc<RwLock<Vec<net::Interface>>>,
        pub active_ssid: Arc<RwLock<Option<String>>>,
        pub services: Arc<RwLock<Vec<services::Service>>>,
        pub filesystems: Arc<RwLock<Vec<storage::Filesystem>>>,
        pub upgradable: Arc<RwLock<Vec<packages::Package>>>,
        pub processes: Arc<RwLock<Vec<process::Process>>>,
        pub displays: Arc<RwLock<Vec<display::DisplayOutput>>>,
        pub sinks: Arc<RwLock<Vec<audio::Sink>>>,
        pub bluetooth: Arc<RwLock<Vec<bluetooth::BtDevice>>>,
    }

    impl Default for StandaloneLive {
        fn default() -> Self {
            Self {
                info: Arc::new(RwLock::new(sys::SystemInfo {
                    hostname: "?".into(),
                    kernel: "?".into(),
                    os: "Linux".into(),
                    arch: "?".into(),
                    uptime_secs: 0,
                    loadavg: (0.0, 0.0, 0.0),
                    memory: sys::Memory {
                        total_kb: 0,
                        available_kb: 0,
                        used_pct: 0.0,
                    },
                    cpu_count: 1,
                    cpu_model: "?".into(),
                })),
                battery: Arc::new(RwLock::new(None)),
                thermals: Arc::new(RwLock::new(Vec::new())),
                interfaces: Arc::new(RwLock::new(Vec::new())),
                active_ssid: Arc::new(RwLock::new(None)),
                services: Arc::new(RwLock::new(Vec::new())),
                filesystems: Arc::new(RwLock::new(Vec::new())),
                upgradable: Arc::new(RwLock::new(Vec::new())),
                processes: Arc::new(RwLock::new(Vec::new())),
                displays: Arc::new(RwLock::new(Vec::new())),
                sinks: Arc::new(RwLock::new(Vec::new())),
                bluetooth: Arc::new(RwLock::new(Vec::new())),
            }
        }
    }

    impl StandaloneLive {
        pub fn spawn_refreshers(self: &Arc<Self>) {
            let me = self.clone();
            tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(1));
                loop {
                    t.tick().await;
                    if let Ok(i) = sys::info().await {
                        *me.info.write().await = i;
                    }
                    if let Ok(b) = power::battery().await {
                        *me.battery.write().await = Some(b);
                    }
                    if let Ok(th) = sys::thermals().await {
                        *me.thermals.write().await = th;
                    }
                    if let Ok(i) = net::interfaces().await {
                        *me.interfaces.write().await = i;
                    }
                    if let Ok(s) = net::wifi_active_ssid().await {
                        *me.active_ssid.write().await = s;
                    }
                }
            });
            let me = self.clone();
            tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(5));
                loop {
                    t.tick().await;
                    if let Ok(v) = services::list_all().await {
                        *me.services.write().await = v;
                    }
                    if let Ok(v) = storage::df().await {
                        *me.filesystems.write().await = v;
                    }
                    if let Ok(v) = process::list().await {
                        *me.processes.write().await = v;
                    }
                    if let Ok(v) = display::outputs().await {
                        *me.displays.write().await = v;
                    }
                    if let Ok(v) = audio::sinks().await {
                        *me.sinks.write().await = v;
                    }
                    if let Ok(v) = bluetooth::list().await {
                        *me.bluetooth.write().await = v;
                    }
                }
            });
        }
    }

    #[axum::async_trait]
    impl LiveRead for StandaloneLive {
        async fn info(&self) -> sys::SystemInfo {
            self.info.read().await.clone()
        }
        async fn battery(&self) -> Option<power::Battery> {
            self.battery.read().await.clone()
        }
        async fn thermals(&self) -> Vec<sys::ThermalReading> {
            self.thermals.read().await.clone()
        }
        async fn interfaces(&self) -> Vec<net::Interface> {
            self.interfaces.read().await.clone()
        }
        async fn active_ssid(&self) -> Option<String> {
            self.active_ssid.read().await.clone()
        }
        async fn services(&self) -> Vec<services::Service> {
            self.services.read().await.clone()
        }
        async fn filesystems(&self) -> Vec<storage::Filesystem> {
            self.filesystems.read().await.clone()
        }
        async fn upgradable(&self) -> Vec<packages::Package> {
            self.upgradable.read().await.clone()
        }
        async fn processes(&self) -> Vec<process::Process> {
            self.processes.read().await.clone()
        }
        async fn displays(&self) -> Vec<display::DisplayOutput> {
            self.displays.read().await.clone()
        }
        async fn sinks(&self) -> Vec<audio::Sink> {
            self.sinks.read().await.clone()
        }
        async fn bluetooth(&self) -> Vec<bluetooth::BtDevice> {
            self.bluetooth.read().await.clone()
        }
    }
}
