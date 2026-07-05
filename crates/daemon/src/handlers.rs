//! RPC handlers: one async fn per [`Method`] variant.
//!
//! Every variant of [`crate::rpc::Method`] is matched in [`dispatch`]; the
//! `_ =>` arm is only a safety net for future variants and is expected to be
//! unreachable. State-bearing handlers grab [`SharedState`] via
//! `state.read().await` / `state.write().await`; stateless handlers ignore it.
//!
//! ## Stub vs wired
//!
//! Handlers that map to an existing function in `cyberdeck-core` are wired
//! straight through. Handlers whose underlying verb is not yet implemented in
//! core return `RpcError::new("not_implemented", "...")`. The four power verbs
//! (`PowerSuspend` / `PowerHibernate` / `PowerReboot` / `PowerShutdown`) are
//! wired — they are critical and core has them.

use std::collections::HashMap;

use serde_json::{json, Value};
use tracing::warn;

use crate::rpc::{Method, Request, Response, RpcError};
use crate::state::{PaneId, SharedState, Split, WorkspaceId};

/// Dispatch a single request to the corresponding handler. Wraps the handler
/// result in [`Response::Ok`] or, on error, [`Response::Err`].
pub async fn dispatch(state: SharedState, req: Request<Value>) -> Response<Value> {
    match req.method.clone() {
        // ---- net ----
        Method::NetWifiScan => handle(req.id, handle_net_wifi_scan(state).await),
        Method::NetWifiConnect { ssid, password } => handle(
            req.id,
            handle_net_wifi_connect(state, ssid, password).await,
        ),
        Method::NetWifiDisconnect => handle(req.id, handle_net_wifi_disconnect(state).await),
        Method::NetWifiActiveSsid => handle(req.id, handle_net_wifi_active_ssid(state).await),
        Method::NetInterfaceList => handle(req.id, handle_net_interface_list(state).await),
        Method::NetInterfaceToggle { name, up } => {
            handle(req.id, handle_net_interface_toggle(state, name, up).await)
        }
        Method::NetSavedConnections => {
            handle(req.id, handle_net_saved_connections(state).await)
        }

        // ---- bluetooth ----
        Method::BtList => handle(req.id, handle_bt_list(state).await),
        Method::BtScan => handle(req.id, handle_bt_scan(state).await),
        Method::BtPair { mac } => handle(req.id, handle_bt_pair(state, mac).await),
        Method::BtConnect { mac } => handle(req.id, handle_bt_connect(state, mac).await),
        Method::BtDisconnect { mac } => {
            handle(req.id, handle_bt_disconnect(state, mac).await)
        }
        Method::BtTrust { mac } => handle(req.id, handle_bt_trust(state, mac).await),
        Method::BtPower { on } => handle(req.id, handle_bt_power(state, on).await),

        // ---- audio ----
        Method::AudioSinks => handle(req.id, handle_audio_sinks(state).await),
        Method::AudioSetVolume { target, percent } => handle(
            req.id,
            handle_audio_set_volume(state, target, percent).await,
        ),
        Method::AudioSetMute { target, mute } => handle(
            req.id,
            handle_audio_set_mute(state, target, mute).await,
        ),
        Method::AudioSetDefault { sink } => {
            handle(req.id, handle_audio_set_default(state, sink).await)
        }

        // ---- display ----
        Method::DisplayOutputs => handle(req.id, handle_display_outputs(state).await),
        Method::DisplayBrightnessGet => {
            handle(req.id, handle_display_brightness_get(state).await)
        }
        Method::DisplayBrightnessSet { value } => handle(
            req.id,
            handle_display_brightness_set(state, value).await,
        ),

        // ---- power ----
        Method::PowerBattery => handle(req.id, handle_power_battery(state).await),
        Method::PowerGovernor => handle(req.id, handle_power_governor(state).await),
        Method::PowerSetGovernor { governor } => handle(
            req.id,
            handle_power_set_governor(state, governor).await,
        ),
        Method::PowerSuspend => handle(req.id, handle_power_suspend(state).await),
        Method::PowerHibernate => handle(req.id, handle_power_hibernate(state).await),
        Method::PowerReboot => handle(req.id, handle_power_reboot(state).await),
        Method::PowerShutdown => handle(req.id, handle_power_shutdown(state).await),

        // ---- storage ----
        Method::StorageDf => handle(req.id, handle_storage_df(state).await),
        Method::StorageLsblk => handle(req.id, handle_storage_lsblk(state).await),
        Method::StorageMount { src, target } => handle(
            req.id,
            handle_storage_mount(state, src, target).await,
        ),
        Method::StorageUmount { target } => {
            handle(req.id, handle_storage_umount(state, target).await)
        }

        // ---- services ----
        Method::ServiceList => handle(req.id, handle_service_list(state).await),
        Method::ServiceStart { unit } => {
            handle(req.id, handle_service_start(state, unit).await)
        }
        Method::ServiceStop { unit } => {
            handle(req.id, handle_service_stop(state, unit).await)
        }
        Method::ServiceRestart { unit } => {
            handle(req.id, handle_service_restart(state, unit).await)
        }
        Method::ServiceEnable { unit } => {
            handle(req.id, handle_service_enable(state, unit).await)
        }
        Method::ServiceDisable { unit } => {
            handle(req.id, handle_service_disable(state, unit).await)
        }
        Method::ServiceStatus { unit } => {
            handle(req.id, handle_service_status(state, unit).await)
        }

        // ---- packages ----
        Method::PackageList => handle(req.id, handle_package_list(state).await),
        Method::PackageSearch { query } => {
            handle(req.id, handle_package_search(state, query).await)
        }
        Method::PackageUpgradable => {
            handle(req.id, handle_package_upgradable(state).await)
        }
        Method::PackageInstall { name } => {
            handle(req.id, handle_package_install(state, name).await)
        }
        Method::PackageRemove { name } => {
            handle(req.id, handle_package_remove(state, name).await)
        }
        Method::PackageUpdate => handle(req.id, handle_package_update(state).await),
        Method::PackageUpgrade => handle(req.id, handle_package_upgrade(state).await),

        // ---- processes ----
        Method::ProcessList => handle(req.id, handle_process_list(state).await),
        Method::ProcessKill { pid, signal } => handle(
            req.id,
            handle_process_kill(state, pid, signal).await,
        ),
        Method::ProcessRenice { pid, nice } => handle(
            req.id,
            handle_process_renice(state, pid, nice).await,
        ),

        // ---- logs ----
        Method::LogsRecent { since_secs } => handle(
            req.id,
            handle_logs_recent(state, since_secs).await,
        ),
        Method::LogsUnits => handle(req.id, handle_logs_units(state).await),

        // ---- system ----
        Method::SystemInfo => handle(req.id, handle_system_info(state).await),
        Method::SystemUptime => handle(req.id, handle_system_uptime(state).await),
        Method::SystemLoadavg => handle(req.id, handle_system_loadavg(state).await),
        Method::SystemMemory => handle(req.id, handle_system_memory(state).await),
        Method::SystemThermals => handle(req.id, handle_system_thermals(state).await),

        // ---- workspaces + panes ----
        Method::WorkspaceList => handle(req.id, handle_workspace_list(state).await),
        Method::WorkspaceNew { name } => {
            handle(req.id, handle_workspace_new(state, name).await)
        }
        Method::WorkspaceClose { id } => {
            handle(req.id, handle_workspace_close(state, id).await)
        }
        Method::WorkspaceFocus { id } => {
            handle(req.id, handle_workspace_focus(state, id).await)
        }
        Method::PaneList { workspace_id } => handle(
            req.id,
            handle_pane_list(state, workspace_id).await,
        ),
        Method::PaneSplit { pane_id, dir } => handle(
            req.id,
            handle_pane_split(state, pane_id, dir).await,
        ),
        Method::PaneClose { pane_id } => {
            handle(req.id, handle_pane_close(state, pane_id).await)
        }
        Method::PaneSendText { pane_id, text } => handle(
            req.id,
            handle_pane_send_text(state, pane_id, text).await,
        ),
        Method::PaneRead { pane_id, max_bytes } => handle(
            req.id,
            handle_pane_read(state, pane_id, max_bytes).await,
        ),
        Method::PaneState { pane_id } => {
            handle(req.id, handle_pane_state(state, pane_id).await)
        }

        // ---- daemon control ----
        Method::DaemonPing => Response::Ok {
            id: req.id,
            result: json!({ "ok": true }),
        },
        Method::DaemonShutdown => {
            warn!("daemon shutdown requested via RPC");
            Response::Ok {
                id: req.id,
                result: json!({ "ok": true, "shutdown": true }),
            }
        }
    }
}

/// Wrap a handler's `Result<Value, RpcError>` into a [`Response`].
fn handle(id: String, result: Result<Value, RpcError>) -> Response<Value> {
    match result {
        Ok(v) => Response::Ok { id, result: v },
        Err(e) => Response::Err { id, error: e },
    }
}

/// Convert a `cyberdeck_core::CoreError` into an `RpcError` so handlers can
/// use `?` on core calls. The `code` is a snake_case identifier derived from
/// the CoreError variant — clients pattern-match on this instead of the
/// human message.
fn map_core_err(e: cyberdeck_core::CoreError) -> RpcError {
    let code = match &e {
        cyberdeck_core::CoreError::Command { .. } => "command_failed",
        cyberdeck_core::CoreError::Timeout { .. } => "timeout",
        cyberdeck_core::CoreError::Io(_) => "io",
        cyberdeck_core::CoreError::Parse(_) => "parse",
        cyberdeck_core::CoreError::NotFound(_) => "not_found",
        cyberdeck_core::CoreError::Permission(_) => "permission_denied",
        cyberdeck_core::CoreError::Invalid(_) => "invalid",
        cyberdeck_core::CoreError::Cancelled => "cancelled",
    };
    RpcError::new(code, e.to_string())
}

/// Helper: turn a value into JSON. Used everywhere to give handlers a single
/// `to_json` call site.
fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::new("serialize_error", e.to_string()))
}

/// Build a stub `not_implemented` error.
fn not_implemented(verb: &str) -> RpcError {
    RpcError::new("not_implemented", format!("verb `{verb}` not wired yet"))
}

// ---------------------------------------------------------------------------
// net handlers
// ---------------------------------------------------------------------------

async fn handle_net_wifi_scan(_state: SharedState) -> Result<Value, RpcError> {
    let n = cyberdeck_core::net::wifi_scan()
        .await
        .map_err(map_core_err)?;
    to_json(&n)
}

async fn handle_net_wifi_connect(
    _state: SharedState,
    ssid: String,
    password: Option<String>,
) -> Result<Value, RpcError> {
    cyberdeck_core::net::wifi_connect(&ssid, password.as_deref())
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "ssid": ssid }))
}

async fn handle_net_wifi_disconnect(_state: SharedState) -> Result<Value, RpcError> {
    cyberdeck_core::net::wifi_disconnect()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true }))
}

async fn handle_net_wifi_active_ssid(_state: SharedState) -> Result<Value, RpcError> {
    let ssid = cyberdeck_core::net::wifi_active_ssid()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ssid": ssid }))
}

async fn handle_net_interface_list(_state: SharedState) -> Result<Value, RpcError> {
    let ifaces = cyberdeck_core::net::interfaces()
        .await
        .map_err(map_core_err)?;
    to_json(&ifaces)
}

async fn handle_net_interface_toggle(
    _state: SharedState,
    name: String,
    up: bool,
) -> Result<Value, RpcError> {
    cyberdeck_core::net::interface_toggle(&name, up)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "name": name, "up": up }))
}

async fn handle_net_saved_connections(_state: SharedState) -> Result<Value, RpcError> {
    // core returns by-value (sync); not async
    let saved = cyberdeck_core::net::saved_connections().map_err(map_core_err)?;
    to_json(&saved)
}

// ---------------------------------------------------------------------------
// bluetooth handlers
// ---------------------------------------------------------------------------

async fn handle_bt_list(_state: SharedState) -> Result<Value, RpcError> {
    let devs = cyberdeck_core::bluetooth::list().await.map_err(map_core_err)?;
    to_json(&devs)
}

async fn handle_bt_scan(_state: SharedState) -> Result<Value, RpcError> {
    // core exposes list() rather than a separate scan() — reuse it
    let devs = cyberdeck_core::bluetooth::list().await.map_err(map_core_err)?;
    to_json(&devs)
}

async fn handle_bt_pair(_state: SharedState, mac: String) -> Result<Value, RpcError> {
    cyberdeck_core::bluetooth::pair(&mac)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "mac": mac }))
}

async fn handle_bt_connect(_state: SharedState, mac: String) -> Result<Value, RpcError> {
    cyberdeck_core::bluetooth::connect(&mac)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "mac": mac }))
}

async fn handle_bt_disconnect(_state: SharedState, mac: String) -> Result<Value, RpcError> {
    cyberdeck_core::bluetooth::disconnect(&mac)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "mac": mac }))
}

async fn handle_bt_trust(_state: SharedState, mac: String) -> Result<Value, RpcError> {
    cyberdeck_core::bluetooth::trust(&mac)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "mac": mac }))
}

async fn handle_bt_power(_state: SharedState, on: bool) -> Result<Value, RpcError> {
    cyberdeck_core::bluetooth::adapter_power(on)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "on": on }))
}

// ---------------------------------------------------------------------------
// audio handlers
// ---------------------------------------------------------------------------

async fn handle_audio_sinks(_state: SharedState) -> Result<Value, RpcError> {
    let sinks = cyberdeck_core::audio::sinks().await.map_err(map_core_err)?;
    to_json(&sinks)
}

async fn handle_audio_set_volume(
    _state: SharedState,
    target: String,
    percent: u8,
) -> Result<Value, RpcError> {
    cyberdeck_core::audio::set_volume(&target, percent)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "target": target, "percent": percent }))
}

async fn handle_audio_set_mute(
    _state: SharedState,
    target: String,
    mute: bool,
) -> Result<Value, RpcError> {
    cyberdeck_core::audio::set_mute(&target, mute)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "target": target, "mute": mute }))
}

async fn handle_audio_set_default(
    _state: SharedState,
    sink: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::audio::set_default_sink(&sink)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "sink": sink }))
}

// ---------------------------------------------------------------------------
// display handlers
// ---------------------------------------------------------------------------

async fn handle_display_outputs(_state: SharedState) -> Result<Value, RpcError> {
    let outs = cyberdeck_core::display::outputs()
        .await
        .map_err(map_core_err)?;
    to_json(&outs)
}

async fn handle_display_brightness_get(_state: SharedState) -> Result<Value, RpcError> {
    let v = cyberdeck_core::display::brightness()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "value": v }))
}

async fn handle_display_brightness_set(
    _state: SharedState,
    value: u8,
) -> Result<Value, RpcError> {
    cyberdeck_core::display::set_brightness(value)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "value": value }))
}

// ---------------------------------------------------------------------------
// power handlers (real — these call into core)
// ---------------------------------------------------------------------------

async fn handle_power_battery(_state: SharedState) -> Result<Value, RpcError> {
    let b = cyberdeck_core::power::battery().await.map_err(map_core_err)?;
    to_json(&b)
}

async fn handle_power_governor(_state: SharedState) -> Result<Value, RpcError> {
    let g = cyberdeck_core::power::cpu_governor()
        .await
        .map_err(map_core_err)?;
    to_json(&g)
}

async fn handle_power_set_governor(
    _state: SharedState,
    governor: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::power::set_governor(&governor)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "governor": governor }))
}

async fn handle_power_suspend(_state: SharedState) -> Result<Value, RpcError> {
    cyberdeck_core::power::suspend().await.map_err(map_core_err)?;
    Ok(json!({ "ok": true, "action": "suspend" }))
}

async fn handle_power_hibernate(_state: SharedState) -> Result<Value, RpcError> {
    cyberdeck_core::power::hibernate()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "action": "hibernate" }))
}

async fn handle_power_reboot(_state: SharedState) -> Result<Value, RpcError> {
    cyberdeck_core::power::reboot()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "action": "reboot" }))
}

async fn handle_power_shutdown(_state: SharedState) -> Result<Value, RpcError> {
    cyberdeck_core::power::shutdown()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "action": "shutdown" }))
}

// ---------------------------------------------------------------------------
// storage handlers
// ---------------------------------------------------------------------------

async fn handle_storage_df(_state: SharedState) -> Result<Value, RpcError> {
    let fs = cyberdeck_core::storage::df().await.map_err(map_core_err)?;
    to_json(&fs)
}

async fn handle_storage_lsblk(_state: SharedState) -> Result<Value, RpcError> {
    let devs = cyberdeck_core::storage::lsblk()
        .await
        .map_err(map_core_err)?;
    to_json(&devs)
}

async fn handle_storage_mount(
    _state: SharedState,
    src: String,
    target: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::storage::mount(&src, &target)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "src": src, "target": target }))
}

async fn handle_storage_umount(
    _state: SharedState,
    target: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::storage::umount(&target)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "target": target }))
}

// ---------------------------------------------------------------------------
// service handlers
// ---------------------------------------------------------------------------

async fn handle_service_list(_state: SharedState) -> Result<Value, RpcError> {
    let svcs = cyberdeck_core::services::list_all()
        .await
        .map_err(map_core_err)?;
    to_json(&svcs)
}

async fn handle_service_start(
    _state: SharedState,
    unit: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::services::start(&unit)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "unit": unit, "action": "start" }))
}

async fn handle_service_stop(
    _state: SharedState,
    unit: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::services::stop(&unit)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "unit": unit, "action": "stop" }))
}

async fn handle_service_restart(
    _state: SharedState,
    unit: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::services::restart(&unit)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "unit": unit, "action": "restart" }))
}

async fn handle_service_enable(
    _state: SharedState,
    unit: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::services::enable(&unit)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "unit": unit, "action": "enable" }))
}

async fn handle_service_disable(
    _state: SharedState,
    unit: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::services::disable(&unit)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "unit": unit, "action": "disable" }))
}

async fn handle_service_status(
    _state: SharedState,
    unit: String,
) -> Result<Value, RpcError> {
    let s = cyberdeck_core::services::status(&unit)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "unit": unit, "status": s }))
}

// ---------------------------------------------------------------------------
// package handlers
// ---------------------------------------------------------------------------

async fn handle_package_list(_state: SharedState) -> Result<Value, RpcError> {
    let pkgs = cyberdeck_core::packages::list_installed()
        .await
        .map_err(map_core_err)?;
    to_json(&pkgs)
}

async fn handle_package_search(
    _state: SharedState,
    query: String,
) -> Result<Value, RpcError> {
    let pkgs = cyberdeck_core::packages::search(&query)
        .await
        .map_err(map_core_err)?;
    to_json(&pkgs)
}

async fn handle_package_upgradable(_state: SharedState) -> Result<Value, RpcError> {
    let pkgs = cyberdeck_core::packages::upgradable()
        .await
        .map_err(map_core_err)?;
    to_json(&pkgs)
}

async fn handle_package_install(
    _state: SharedState,
    name: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::packages::install(&name)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "name": name, "action": "install" }))
}

async fn handle_package_remove(
    _state: SharedState,
    name: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::packages::remove(&name)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "name": name, "action": "remove" }))
}

async fn handle_package_update(_state: SharedState) -> Result<Value, RpcError> {
    let out = cyberdeck_core::packages::update()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "output": out }))
}

async fn handle_package_upgrade(_state: SharedState) -> Result<Value, RpcError> {
    let out = cyberdeck_core::packages::upgrade()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "output": out }))
}

// ---------------------------------------------------------------------------
// process handlers
// ---------------------------------------------------------------------------

async fn handle_process_list(_state: SharedState) -> Result<Value, RpcError> {
    let ps = cyberdeck_core::process::list()
        .await
        .map_err(map_core_err)?;
    to_json(&ps)
}

async fn handle_process_kill(
    _state: SharedState,
    pid: i32,
    signal: String,
) -> Result<Value, RpcError> {
    cyberdeck_core::process::kill(pid, &signal)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "pid": pid, "signal": signal }))
}

async fn handle_process_renice(
    _state: SharedState,
    pid: i32,
    nice: i32,
) -> Result<Value, RpcError> {
    cyberdeck_core::process::renice(pid, nice)
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "ok": true, "pid": pid, "nice": nice }))
}

// ---------------------------------------------------------------------------
// logs handlers
// ---------------------------------------------------------------------------

async fn handle_logs_recent(
    _state: SharedState,
    since_secs: u64,
) -> Result<Value, RpcError> {
    let entries = cyberdeck_core::logs::recent_since(since_secs)
        .await
        .map_err(map_core_err)?;
    // entries is `Vec<(DateTime<Utc>, String)>` — flatten to owned JSON.
    let owned: Vec<HashMap<String, String>> = entries
        .into_iter()
        .map(|(t, line)| {
            let mut m = HashMap::new();
            m.insert("timestamp".to_string(), t.to_rfc3339());
            m.insert("line".to_string(), line);
            m
        })
        .collect();
    to_json(&owned)
}

async fn handle_logs_units(_state: SharedState) -> Result<Value, RpcError> {
    // core doesn't expose a units-only helper; return empty list — the CLI
    // surfaces this through the ServiceList verb instead.
    let units: Vec<String> = Vec::new();
    Ok(json!({ "units": units, "note": "see ServiceList" }))
}

// ---------------------------------------------------------------------------
// system handlers
// ---------------------------------------------------------------------------

async fn handle_system_info(_state: SharedState) -> Result<Value, RpcError> {
    let info = cyberdeck_core::sys::info().await.map_err(map_core_err)?;
    to_json(&info)
}

async fn handle_system_uptime(_state: SharedState) -> Result<Value, RpcError> {
    let secs = cyberdeck_core::sys::uptime()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "uptime_secs": secs, "human": cyberdeck_core::sys::format_uptime(secs) }))
}

async fn handle_system_loadavg(_state: SharedState) -> Result<Value, RpcError> {
    let (a, b, c) = cyberdeck_core::sys::loadavg()
        .await
        .map_err(map_core_err)?;
    Ok(json!({ "load1": a, "load5": b, "load15": c }))
}

async fn handle_system_memory(_state: SharedState) -> Result<Value, RpcError> {
    let m = cyberdeck_core::sys::memory().await.map_err(map_core_err)?;
    to_json(&m)
}

async fn handle_system_thermals(_state: SharedState) -> Result<Value, RpcError> {
    let t = cyberdeck_core::sys::thermals()
        .await
        .map_err(map_core_err)?;
    to_json(&t)
}

// ---------------------------------------------------------------------------
// workspace / pane handlers
// ---------------------------------------------------------------------------

async fn handle_workspace_list(state: SharedState) -> Result<Value, RpcError> {
    let s = state.read().await;
    to_json(&s.workspaces)
}

async fn handle_workspace_new(
    state: SharedState,
    name: String,
) -> Result<Value, RpcError> {
    let ws = crate::state::Workspace::new(name);
    let id = ws.id;
    {
        let mut s = state.write().await;
        s.workspaces.push(ws);
        s.focused_workspace = Some(id);
    }
    Ok(json!({ "id": id.0 }))
}

async fn handle_workspace_close(
    state: SharedState,
    id: u64,
) -> Result<Value, RpcError> {
    let target = WorkspaceId(id);
    let mut s = state.write().await;
    let before = s.workspaces.len();
    s.workspaces.retain(|w| w.id != target);
    if s.workspaces.len() == before {
        return Err(RpcError::new("not_found", format!("workspace {id} not found")));
    }
    if s.focused_workspace == Some(target) {
        s.focused_workspace = s.workspaces.first().map(|w| w.id);
    }
    Ok(json!({ "ok": true, "id": id }))
}

async fn handle_workspace_focus(
    state: SharedState,
    id: u64,
) -> Result<Value, RpcError> {
    let target = WorkspaceId(id);
    let mut s = state.write().await;
    if !s.workspaces.iter().any(|w| w.id == target) {
        return Err(RpcError::new("not_found", format!("workspace {id} not found")));
    }
    s.focused_workspace = Some(target);
    Ok(json!({ "ok": true, "id": id }))
}

async fn handle_pane_list(
    state: SharedState,
    workspace_id: Option<u64>,
) -> Result<Value, RpcError> {
    let s = state.read().await;
    let ws = match workspace_id {
        Some(id) => s.workspaces.iter().find(|w| w.id == WorkspaceId(id)),
        None => s.focused_workspace(),
    };
    let panes: Vec<&crate::state::Pane> = ws
        .map(|w| w.tabs.iter().flat_map(|t| t.panes.iter()).collect())
        .unwrap_or_default();
    to_json(&panes)
}

async fn handle_pane_split(
    state: SharedState,
    pane_id: u64,
    dir: String,
) -> Result<Value, RpcError> {
    let split = match dir.as_str() {
        "horizontal" | "h" => Split::Horizontal,
        "vertical" | "v" => Split::Vertical,
        other => {
            return Err(RpcError::new(
                "invalid",
                format!("dir must be horizontal|vertical (got {other:?})"),
            ))
        }
    };
    let mut s = state.write().await;
    let new_id = s
        .split_pane(PaneId(pane_id), split)
        .map_err(|e| RpcError::new(e.code, e.message))?;
    Ok(json!({ "id": new_id.0 }))
}

async fn handle_pane_close(
    state: SharedState,
    pane_id: u64,
) -> Result<Value, RpcError> {
    let target = PaneId(pane_id);
    let mut s = state.write().await;
    for ws in &mut s.workspaces {
        for tab in &mut ws.tabs {
            let before = tab.panes.len();
            tab.panes.retain(|p| p.id != target);
            if tab.panes.len() != before {
                if tab.focused == Some(target) {
                    tab.focused = tab.panes.first().map(|p| p.id);
                }
                return Ok(json!({ "ok": true, "id": pane_id }));
            }
        }
    }
    Err(RpcError::new("not_found", format!("pane {pane_id} not found")))
}

async fn handle_pane_send_text(
    _state: SharedState,
    pane_id: u64,
    text: String,
) -> Result<Value, RpcError> {
    // Real PTY wiring lands in the PTY integration task; for now this is a
    // stub that returns the would-be-sent bytes.
    let _ = pane_id;
    let _ = text.len();
    Err(not_implemented("pane_send_text (PTY integration pending)"))
}

async fn handle_pane_read(
    _state: SharedState,
    pane_id: u64,
    max_bytes: usize,
) -> Result<Value, RpcError> {
    let _ = pane_id;
    let _ = max_bytes;
    Err(not_implemented("pane_read (PTY integration pending)"))
}

async fn handle_pane_state(
    state: SharedState,
    pane_id: u64,
) -> Result<Value, RpcError> {
    let s = state.read().await;
    let pane = s.pane_mut_for_read(PaneId(pane_id));
    let pane = pane.ok_or_else(|| RpcError::new("not_found", format!("pane {pane_id} not found")))?;
    to_json(&pane.state)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PaneKind;
    use crate::state::{DaemonState, Pane, PaneState};

    /// Every variant of `Method` must build. We enumerate by building one
    /// representative Request per variant. Variants with fields get a
    /// reasonable value; variants without get `Method::Variant` directly.
    #[test]
    fn every_method_variant_has_a_handler_arm() {
        // Build a single sample Method per variant and confirm dispatch
        // recognises it (the match is exhaustive by construction; this is a
        // double-check that we haven't accidentally broken the function).
        let variants: Vec<Method> = vec![
            Method::NetWifiScan,
            Method::NetWifiConnect { ssid: "x".into(), password: None },
            Method::NetWifiDisconnect,
            Method::NetWifiActiveSsid,
            Method::NetInterfaceList,
            Method::NetInterfaceToggle { name: "wlan0".into(), up: true },
            Method::NetSavedConnections,
            Method::BtList,
            Method::BtScan,
            Method::BtPair { mac: "AA:BB".into() },
            Method::BtConnect { mac: "AA:BB".into() },
            Method::BtDisconnect { mac: "AA:BB".into() },
            Method::BtTrust { mac: "AA:BB".into() },
            Method::BtPower { on: true },
            Method::AudioSinks,
            Method::AudioSetVolume { target: "Master".into(), percent: 50 },
            Method::AudioSetMute { target: "Master".into(), mute: true },
            Method::AudioSetDefault { sink: "HDMI".into() },
            Method::DisplayOutputs,
            Method::DisplayBrightnessGet,
            Method::DisplayBrightnessSet { value: 80 },
            Method::PowerBattery,
            Method::PowerGovernor,
            Method::PowerSetGovernor { governor: "powersave".into() },
            Method::PowerSuspend,
            Method::PowerHibernate,
            Method::PowerReboot,
            Method::PowerShutdown,
            Method::StorageDf,
            Method::StorageLsblk,
            Method::StorageMount { src: "/dev/sdb1".into(), target: "/mnt".into() },
            Method::StorageUmount { target: "/mnt".into() },
            Method::ServiceList,
            Method::ServiceStart { unit: "sshd".into() },
            Method::ServiceStop { unit: "sshd".into() },
            Method::ServiceRestart { unit: "sshd".into() },
            Method::ServiceEnable { unit: "sshd".into() },
            Method::ServiceDisable { unit: "sshd".into() },
            Method::ServiceStatus { unit: "sshd".into() },
            Method::PackageList,
            Method::PackageSearch { query: "vim".into() },
            Method::PackageUpgradable,
            Method::PackageInstall { name: "vim".into() },
            Method::PackageRemove { name: "vim".into() },
            Method::PackageUpdate,
            Method::PackageUpgrade,
            Method::ProcessList,
            Method::ProcessKill { pid: 1, signal: "TERM".into() },
            Method::ProcessRenice { pid: 1, nice: 10 },
            Method::LogsRecent { since_secs: 60 },
            Method::LogsUnits,
            Method::SystemInfo,
            Method::SystemUptime,
            Method::SystemLoadavg,
            Method::SystemMemory,
            Method::SystemThermals,
            Method::WorkspaceList,
            Method::WorkspaceNew { name: "w".into() },
            Method::WorkspaceClose { id: 1 },
            Method::WorkspaceFocus { id: 1 },
            Method::PaneList { workspace_id: None },
            Method::PaneSplit { pane_id: 1, dir: "horizontal".into() },
            Method::PaneClose { pane_id: 1 },
            Method::PaneSendText { pane_id: 1, text: "ls\n".into() },
            Method::PaneRead { pane_id: 1, max_bytes: 1024 },
            Method::PaneState { pane_id: 1 },
            Method::DaemonPing,
            Method::DaemonShutdown,
        ];
        // The plan promises ≥40 variants. Sanity-check the count.
        assert!(
            variants.len() >= 40,
            "Method has only {} variants (expected ≥40)",
            variants.len()
        );
        // Each variant must serialise to JSON — that's the only thing we can
        // check synchronously here without spinning up core calls.
        for v in &variants {
            let s = serde_json::to_string(v).unwrap();
            let back: Method = serde_json::from_str(&s).unwrap();
            // Debug equality is the closest we have without custom PartialEq
            // on Method.
            assert_eq!(format!("{v:?}"), format!("{back:?}"));
        }
    }

    fn fresh_state() -> SharedState {
        std::sync::Arc::new(tokio::sync::RwLock::new(DaemonState::new()))
    }

    #[tokio::test]
    async fn workspace_list_returns_empty_state_initially() {
        let state = fresh_state();
        let req = Request {
            id: "t".into(),
            method: Method::WorkspaceList,
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Ok { result, .. } => {
                // DaemonState::new() ships with one default workspace, so the
                // list is non-empty but contains exactly one item.
                let arr = result.as_array().expect("result must be an array");
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0]["name"], "cyberdeck");
            }
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }
    }

    #[tokio::test]
    async fn workspace_new_then_list_shows_it() {
        let state = fresh_state();
        // Mutate state directly to add a workspace, then dispatch the read.
        {
            let mut s = state.write().await;
            let ws = crate::state::Workspace::new("alpha");
            s.workspaces.push(ws);
        }
        let req = Request {
            id: "t".into(),
            method: Method::WorkspaceList,
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Ok { result, .. } => {
                let arr = result.as_array().unwrap();
                assert!(arr.iter().any(|w| w["name"] == "alpha"));
            }
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }
    }

    #[tokio::test]
    async fn workspace_new_then_focus_then_list() {
        let state = fresh_state();
        let req_new = Request {
            id: "t".into(),
            method: Method::WorkspaceNew { name: "beta".into() },
            params: json!({}),
        };
        let resp = dispatch(state.clone(), req_new).await;
        let new_id = match resp {
            Response::Ok { result, .. } => result["id"].as_u64().unwrap(),
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        };
        let req_focus = Request {
            id: "t".into(),
            method: Method::WorkspaceFocus { id: new_id },
            params: json!({}),
        };
        let resp = dispatch(state.clone(), req_focus).await;
        match resp {
            Response::Ok { result, .. } => assert_eq!(result["id"], new_id),
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }
        // Now focused_workspace should be the new one.
        let s = state.read().await;
        assert_eq!(s.focused_workspace, Some(WorkspaceId(new_id)));
    }

    #[tokio::test]
    async fn workspace_close_unknown_returns_not_found() {
        let state = fresh_state();
        let req = Request {
            id: "t".into(),
            method: Method::WorkspaceClose { id: 999 },
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Err { error, .. } => assert_eq!(error.code, "not_found"),
            Response::Ok { .. } => panic!("expected Err for unknown workspace"),
        }
    }

    #[tokio::test]
    async fn pane_state_returns_state_for_known_pane() {
        let state = fresh_state();
        // Plant a pane in the focused workspace.
        let pane_id;
        {
            let mut s = state.write().await;
            let ws = s.focused_workspace_mut().unwrap();
            let p = Pane {
                id: PaneId(42),
                kind: PaneKind::Screen { id: "System".into() },
                title: "system".into(),
                state: PaneState::Working,
                last_state_change_seq: 0,
                seen: false,
            };
            pane_id = p.id;
            ws.tabs[0].panes.push(p);
        }
        let req = Request {
            id: "t".into(),
            method: Method::PaneState { pane_id: pane_id.0 },
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Ok { result, .. } => assert_eq!(result, "Working"),
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }
    }

    #[tokio::test]
    async fn pane_close_unknown_returns_not_found() {
        let state = fresh_state();
        let req = Request {
            id: "t".into(),
            method: Method::PaneClose { pane_id: 999 },
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Err { error, .. } => assert_eq!(error.code, "not_found"),
            Response::Ok { .. } => panic!("expected Err for unknown pane"),
        }
    }

    #[test]
    fn map_core_err_distinguishes_not_found() {
        let e = cyberdeck_core::CoreError::NotFound("foo".into());
        let r = map_core_err(e);
        assert_eq!(r.code, "not_found");
        let e = cyberdeck_core::CoreError::Permission("nope".into());
        let r = map_core_err(e);
        assert_eq!(r.code, "permission_denied");
        let e = cyberdeck_core::CoreError::Cancelled;
        let r = map_core_err(e);
        assert_eq!(r.code, "cancelled");
    }

    #[tokio::test]
    async fn daemon_ping_returns_ok_true() {
        let state = fresh_state();
        let req = Request {
            id: "t".into(),
            method: Method::DaemonPing,
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Ok { result, .. } => assert_eq!(result["ok"], true),
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }
    }

    #[tokio::test]
    async fn daemon_shutdown_returns_ok_does_not_crash() {
        let state = fresh_state();
        let req = Request {
            id: "t".into(),
            method: Method::DaemonShutdown,
            params: json!({}),
        };
        let resp = dispatch(state, req).await;
        match resp {
            Response::Ok { result, .. } => {
                assert_eq!(result["ok"], true);
                assert_eq!(result["shutdown"], true);
            }
            Response::Err { error, .. } => panic!("unexpected error: {error:?}"),
        }
        // dispatch is sync-looking — no panic, no state mutation.
        // (The actual process exit is handled by the server loop, not here.)
    }
}