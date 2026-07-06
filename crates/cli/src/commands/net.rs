//! `cyberdeck net` — wifi + interface control.

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::output::OutputMode;

#[derive(Debug, Subcommand)]
pub enum NetCmd {
    /// Scan for wifi networks.
    WifiScan,
    /// Connect to a wifi network.
    WifiConnect {
        /// SSID to connect to.
        ssid: String,
        /// Passphrase (omit for open networks).
        #[arg(long)]
        password: Option<String>,
    },
    /// Disconnect the active wifi connection.
    WifiDisconnect,
    /// Print the SSID of the current wifi network.
    WifiActive,
    /// List network interfaces.
    Ifaces,
    /// Bring an interface up or down.
    IfUp {
        /// Interface name (e.g. `eth0`).
        name: String,
        /// Whether to bring the interface up (else down).
        #[arg(long, default_value_t = true)]
        up: bool,
    },
    /// List saved wifi connections.
    Saved,
}

pub fn run(cmd: NetCmd, mode: OutputMode) -> Result<i32> {
    // In direct mode (no daemon), every verb immediately returns a structured
    // value. The real implementations (Tasks 18-21) wire these to cyberdeck-core
    // and to the daemon's RPC handlers.
    match cmd {
        NetCmd::WifiScan => {
            crate::output::print(mode, &json!({ "ssids": ["stub-network-A", "stub-network-B"] }))?;
            Ok(0)
        }
        NetCmd::WifiConnect { ssid, password } => {
            let has_password = password.is_some();
            crate::output::print(mode, &json!({
                "ssid": ssid,
                "password_provided": has_password,
                "status": "connected (stub)",
            }))?;
            Ok(0)
        }
        NetCmd::WifiDisconnect => crate::output::print_ok(mode, true).map(|_| 0),
        NetCmd::WifiActive => {
            crate::output::print(mode, &json!({ "ssid": "stub-network-A" })).map(|_| 0)
        }
        NetCmd::Ifaces => {
            crate::output::print(mode, &json!({ "ifaces": ["lo", "eth0", "wlan0"] })).map(|_| 0)
        }
        NetCmd::IfUp { name, up } => crate::output::print(
            mode,
            &json!({ "iface": name, "up": up, "ok": true }),
        )
        .map(|_| 0),
        NetCmd::Saved => {
            crate::output::print(mode, &json!({ "saved": ["home", "office"] })).map(|_| 0)
        }
    }
}
