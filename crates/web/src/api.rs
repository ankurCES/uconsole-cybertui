//! JSON API: one module-level handler per resource, mirroring `cyberdeck-core`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::auth::Token;
use cyberdeck_core::{
    audio, bluetooth, display, net, packages, power, process, services, storage, sys,
};

/// Shared state for the API. `live` is the same `Arc<Live>` the TUI uses; we
/// only ever read it. `tx` lets the API push a `Toast` back into the TUI
/// when an action is taken over the web (e.g. a friend reboots the deck).
#[derive(Clone)]
pub struct ApiState {
    pub token: Arc<Option<Token>>,
    pub live: Arc<dyn LiveRead + Send + Sync>,
    pub tx: Option<mpsc::Sender<crate::run::toast_compat::Action>>,
}

/// Trait that abstracts the TUI's `Live` so the web crate doesn't depend on
/// `cyberdeck-tui` (which would be a circular dep). The TUI's
/// `cyberdeck_core::...` types are the source of truth.
#[axum::async_trait]
pub trait LiveRead {
    async fn info(&self) -> sys::SystemInfo;
    async fn battery(&self) -> Option<power::Battery>;
    async fn thermals(&self) -> Vec<sys::ThermalReading>;
    async fn interfaces(&self) -> Vec<net::Interface>;
    async fn active_ssid(&self) -> Option<String>;
    async fn services(&self) -> Vec<services::Service>;
    async fn filesystems(&self) -> Vec<storage::Filesystem>;
    async fn upgradable(&self) -> Vec<packages::Package>;
    async fn processes(&self) -> Vec<process::Process>;
    async fn displays(&self) -> Vec<display::DisplayOutput>;
    async fn sinks(&self) -> Vec<audio::Sink>;
    async fn bluetooth(&self) -> Vec<bluetooth::BtDevice>;
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/system", get(get_system))
        .route("/api/network/interfaces", get(get_interfaces))
        .route("/api/network/wifi/scan", post(post_wifi_scan))
        .route("/api/network/wifi/connect", post(post_wifi_connect))
        .route("/api/network/wifi/disconnect", post(post_wifi_disconnect))
        .route("/api/services", get(get_services))
        .route("/api/services/:unit/:op", post(post_service_op))
        .route("/api/power/battery", get(get_battery))
        .route("/api/power/thermals", get(get_thermals))
        .route("/api/power/governor", get(get_governor).post(post_governor))
        .route("/api/power/suspend", post(post_suspend))
        .route("/api/power/hibernate", post(post_hibernate))
        .route("/api/power/reboot", post(post_reboot))
        .route("/api/power/shutdown", post(post_shutdown))
        .route("/api/storage/df", get(get_df))
        .route("/api/packages/upgradable", get(get_upgradable))
        .route("/api/packages/search", post(post_pkg_search))
        .route("/api/packages/install", post(post_pkg_install))
        .route("/api/packages/remove", post(post_pkg_remove))
        .route("/api/packages/update", post(post_pkg_update))
        .route("/api/packages/upgrade", post(post_pkg_upgrade))
        .route("/api/processes", get(get_processes))
        .route("/api/processes/:pid/kill", post(post_proc_kill))
        .route("/api/display/outputs", get(get_displays))
        .route(
            "/api/display/brightness",
            get(get_brightness).post(post_brightness),
        )
        .route("/api/audio/sinks", get(get_sinks))
        .route("/api/audio/volume", post(post_volume))
        .route("/api/bluetooth/devices", get(get_bt))
        .route("/api/bluetooth/connect", post(post_bt_connect))
        .route("/api/bluetooth/pair", post(post_bt_pair))
        .with_state(Arc::new(state))
}

async fn get_system(State(s): State<Arc<ApiState>>) -> Result<Json<sys::SystemInfo>, ApiError> {
    Ok(Json(s.live.info().await))
}
async fn get_interfaces(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<net::Interface>>, ApiError> {
    Ok(Json(s.live.interfaces().await))
}
async fn get_services(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<services::Service>>, ApiError> {
    Ok(Json(s.live.services().await))
}
async fn get_battery(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Option<power::Battery>>, ApiError> {
    Ok(Json(s.live.battery().await))
}
async fn get_thermals(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<sys::ThermalReading>>, ApiError> {
    Ok(Json(s.live.thermals().await))
}
async fn get_df(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<storage::Filesystem>>, ApiError> {
    Ok(Json(s.live.filesystems().await))
}
async fn get_upgradable(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<packages::Package>>, ApiError> {
    Ok(Json(s.live.upgradable().await))
}
async fn get_processes(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<process::Process>>, ApiError> {
    Ok(Json(s.live.processes().await))
}
async fn get_displays(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<display::DisplayOutput>>, ApiError> {
    Ok(Json(s.live.displays().await))
}
async fn get_sinks(State(s): State<Arc<ApiState>>) -> Result<Json<Vec<audio::Sink>>, ApiError> {
    Ok(Json(s.live.sinks().await))
}
async fn get_bt(
    State(s): State<Arc<ApiState>>,
) -> Result<Json<Vec<bluetooth::BtDevice>>, ApiError> {
    Ok(Json(s.live.bluetooth().await))
}
async fn get_brightness() -> Result<Json<u8>, ApiError> {
    let b = display::brightness().await?;
    Ok(Json(b))
}
async fn post_brightness(Json(req): Json<BrightnessReq>) -> Result<StatusCode, ApiError> {
    display::set_brightness(req.value).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn get_governor() -> Result<Json<power::CpuGovernor>, ApiError> {
    Ok(Json(power::cpu_governor().await?))
}
async fn post_governor(Json(req): Json<GovernorReq>) -> Result<StatusCode, ApiError> {
    power::set_governor(&req.governor).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_suspend() -> Result<StatusCode, ApiError> {
    power::suspend().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_hibernate() -> Result<StatusCode, ApiError> {
    power::hibernate().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_reboot() -> Result<StatusCode, ApiError> {
    power::reboot().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_shutdown() -> Result<StatusCode, ApiError> {
    power::shutdown().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_wifi_scan() -> Result<Json<Vec<net::WifiNetwork>>, ApiError> {
    Ok(Json(net::wifi_scan().await?))
}
async fn post_wifi_connect(Json(req): Json<WifiConnectReq>) -> Result<StatusCode, ApiError> {
    net::wifi_connect(&req.ssid, req.password.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_wifi_disconnect() -> Result<StatusCode, ApiError> {
    net::wifi_disconnect().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_service_op(Path((unit, op)): Path<(String, String)>) -> Result<StatusCode, ApiError> {
    match op.as_str() {
        "start" => services::start(&unit).await?,
        "stop" => services::stop(&unit).await?,
        "restart" => services::restart(&unit).await?,
        "enable" => services::enable(&unit).await?,
        "disable" => services::disable(&unit).await?,
        _ => return Err(ApiError::invalid(format!("unknown op: {op}"))),
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn post_pkg_search(
    Json(req): Json<PkgSearchReq>,
) -> Result<Json<Vec<packages::Package>>, ApiError> {
    Ok(Json(packages::search(&req.query).await?))
}
async fn post_pkg_install(Json(req): Json<PkgNameReq>) -> Result<StatusCode, ApiError> {
    packages::install(&req.name).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_pkg_remove(Json(req): Json<PkgNameReq>) -> Result<StatusCode, ApiError> {
    packages::remove(&req.name).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_pkg_update() -> Result<StatusCode, ApiError> {
    packages::update().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_pkg_upgrade() -> Result<StatusCode, ApiError> {
    packages::upgrade().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_proc_kill(Path(pid): Path<i32>) -> Result<StatusCode, ApiError> {
    process::kill(pid, "TERM").await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_volume(Json(req): Json<VolumeReq>) -> Result<StatusCode, ApiError> {
    audio::set_volume(&req.target, req.percent).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_bt_connect(Json(req): Json<BtMacReq>) -> Result<StatusCode, ApiError> {
    bluetooth::connect(&req.mac).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn post_bt_pair(Json(req): Json<BtMacReq>) -> Result<StatusCode, ApiError> {
    bluetooth::pair(&req.mac).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct BrightnessReq {
    pub value: u8,
}
#[derive(Debug, Deserialize)]
pub struct GovernorReq {
    pub governor: String,
}
#[derive(Debug, Deserialize)]
pub struct WifiConnectReq {
    pub ssid: String,
    pub password: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct PkgSearchReq {
    pub query: String,
}
#[derive(Debug, Deserialize)]
pub struct PkgNameReq {
    pub name: String,
}
#[derive(Debug, Deserialize)]
pub struct VolumeReq {
    pub target: String,
    pub percent: u8,
}
#[derive(Debug, Deserialize)]
pub struct BtMacReq {
    pub mac: String,
}

/// Uniform API error that becomes a 4xx/5xx with a JSON body.
#[derive(Debug)]
pub struct ApiError(pub cyberdeck_core::CoreError);

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use cyberdeck_core::CoreError::*;
        let (status, msg) = match &self.0 {
            Invalid(_) | NotFound(_) => (StatusCode::BAD_REQUEST, self.0.to_string()),
            Permission(_) => (StatusCode::FORBIDDEN, self.0.to_string()),
            Timeout { .. } => (StatusCode::GATEWAY_TIMEOUT, self.0.to_string()),
            Cancelled => (StatusCode::CONFLICT, self.0.to_string()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl From<cyberdeck_core::CoreError> for ApiError {
    fn from(e: cyberdeck_core::CoreError) -> Self {
        Self(e)
    }
}
impl ApiError {
    pub fn invalid(s: impl Into<String>) -> Self {
        Self(cyberdeck_core::CoreError::Invalid(s.into()))
    }
}
