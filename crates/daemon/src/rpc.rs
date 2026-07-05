//! JSON-RPC envelope shared by the CLI and the daemon.
//!
//! Wire format: one JSON object per line, framed by `\n`. A request is
//! `{"id": "<client-tag>", "method": "<Method>", "params": {...}}` and a
//! response is `{"id": "...", "result": ...}` or `{"id": "...", "error": {...}}`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request<P = serde_json::Value> {
    pub id: String,
    /// `#[serde(flatten)]` is critical: it lets the `Method` enum's internal
    /// tag (`{"method": "daemon_ping", ...}` for unit variants,
    /// `{"method": "workspace_new", "name": "..."}` for struct variants)
    /// appear at the top level of the request object instead of being
    /// wrapped in a nested `method` field. Without this, the wire format
    /// would be `{"id": "x", "method": {"method": "daemon_ping"}, "params": {}}`
    /// which no JSON-RPC client would naturally send.
    #[serde(flatten)]
    pub method: Method,
    pub params: P,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response<R = serde_json::Value> {
    Ok { id: String, result: R },
    Err { id: String, error: RpcError },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

impl RpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

/// One variant per verb. The flat list mirrors the CLI verb tree so adding
/// a CLI verb always means adding an RPC method (and vice versa).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Method {
    // net
    NetWifiScan,
    NetWifiConnect { ssid: String, password: Option<String> },
    NetWifiDisconnect,
    NetWifiActiveSsid,
    NetInterfaceList,
    NetInterfaceToggle { name: String, up: bool },
    NetSavedConnections,

    // bluetooth
    BtList,
    BtScan,
    BtPair { mac: String },
    BtConnect { mac: String },
    BtDisconnect { mac: String },
    BtTrust { mac: String },
    BtPower { on: bool },

    // audio
    AudioSinks,
    AudioSetVolume { target: String, percent: u8 },
    AudioSetMute { target: String, mute: bool },
    AudioSetDefault { sink: String },

    // display
    DisplayOutputs,
    DisplayBrightnessGet,
    DisplayBrightnessSet { value: u8 },

    // power
    PowerBattery,
    PowerGovernor,
    PowerSetGovernor { governor: String },
    PowerSuspend,
    PowerHibernate,
    PowerReboot,
    PowerShutdown,

    // storage
    StorageDf,
    StorageLsblk,
    StorageMount { src: String, target: String },
    StorageUmount { target: String },

    // services
    ServiceList,
    ServiceStart { unit: String },
    ServiceStop { unit: String },
    ServiceRestart { unit: String },
    ServiceEnable { unit: String },
    ServiceDisable { unit: String },
    ServiceStatus { unit: String },

    // packages
    PackageList,
    PackageSearch { query: String },
    PackageUpgradable,
    PackageInstall { name: String },
    PackageRemove { name: String },
    PackageUpdate,
    PackageUpgrade,

    // processes
    ProcessList,
    ProcessKill { pid: i32, signal: String },
    ProcessRenice { pid: i32, nice: i32 },

    // logs
    LogsRecent { since_secs: u64 },
    LogsUnits,

    // system
    SystemInfo,
    SystemUptime,
    SystemLoadavg,
    SystemMemory,
    SystemThermals,

    // workspaces + panes (the herd model)
    WorkspaceList,
    WorkspaceNew { name: String },
    WorkspaceClose { id: u64 },
    WorkspaceFocus { id: u64 },
    PaneList { workspace_id: Option<u64> },
    PaneSplit { pane_id: u64, dir: String },
    PaneClose { pane_id: u64 },
    PaneSendText { pane_id: u64, text: String },
    PaneRead { pane_id: u64, max_bytes: usize },
    PaneState { pane_id: u64 },

    // daemon control
    DaemonPing,
    DaemonShutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let r = Request {
            id: "cli:net:wifi_scan".into(),
            method: Method::NetWifiScan,
            params: serde_json::json!({}),
        };
        let line = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, r.id);
        assert!(matches!(back.method, Method::NetWifiScan));
    }

    #[test]
    fn response_ok_round_trips() {
        let resp: Response = Response::Ok {
            id: "1".into(),
            result: serde_json::json!({ "ssids": ["a", "b"] }),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        match back {
            Response::Ok { id, result } => {
                assert_eq!(id, "1");
                assert_eq!(result["ssids"][0], "a");
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn response_err_round_trips() {
        let resp: Response = Response::Err {
            id: "2".into(),
            error: RpcError::new("permission_denied", "needs sudo"),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        match back {
            Response::Err { id, error } => {
                assert_eq!(id, "2");
                assert_eq!(error.code, "permission_denied");
            }
            _ => panic!("expected Err"),
        }
    }

    #[test]
    fn method_serializes_with_tag() {
        let m = Method::WorkspaceNew { name: "repo-x".into() };
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["method"], "workspace_new");
        assert_eq!(v["name"], "repo-x");
    }
}
