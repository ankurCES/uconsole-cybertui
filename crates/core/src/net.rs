//! Network: interfaces, IPs, Wi-Fi (via nmcli), signal.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interface {
    pub name: String,
    pub state: String,
    pub mac: String,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
}

pub async fn interfaces() -> CoreResult<Vec<Interface>> {
    // `ip -j addr` is the modern, parseable form (iproute2 >= 4).
    let out = run(["ip", "-j", "addr", "show"], Privilege::User).await?;
    let v: Vec<serde_json::Value> = serde_json::from_str(&out.stdout)?;
    let mut result = Vec::new();
    for entry in v {
        let name = entry
            .get("ifname")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let state = entry
            .pointer("/operstate")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();
        let mac = entry
            .pointer("/address")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut ipv4 = Vec::new();
        let mut ipv6 = Vec::new();
        if let Some(arr) = entry.get("addr_info").and_then(|v| v.as_array()) {
            for a in arr {
                let local = a.get("local").and_then(|v| v.as_str()).unwrap_or("");
                let family = a.get("family").and_then(|v| v.as_str()).unwrap_or("");
                if local.is_empty() {
                    continue;
                }
                match family {
                    "inet" => ipv4.push(local.to_string()),
                    "inet6" => ipv6.push(local.to_string()),
                    _ => {}
                }
            }
        }
        result.push(Interface {
            name,
            state,
            mac,
            ipv4,
            ipv6,
        });
    }
    Ok(result)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8, // 0..100
    pub security: String,
    pub in_use: bool,
}

pub async fn wifi_scan() -> CoreResult<Vec<WifiNetwork>> {
    // Trigger a rescan, then list visible networks.
    let _ = run(["nmcli", "device", "wifi", "rescan"], Privilege::User).await;
    let out = run(
        [
            "nmcli",
            "-t",
            "-f",
            "SSID,SIGNAL,SECURITY,IN-USE",
            "device",
            "wifi",
            "list",
            "--rescan",
            "no",
        ],
        Privilege::User,
    )
    .await?;
    Ok(parse_wifi(&out.stdout))
}

pub async fn wifi_active_ssid() -> CoreResult<Option<String>> {
    let out = run(
        [
            "nmcli",
            "-t",
            "-f",
            "NAME,TYPE,DEVICE",
            "connection",
            "show",
            "--active",
        ],
        Privilege::User,
    )
    .await?;
    for line in out.stdout.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[1] == "802-11-wireless" {
            return Ok(Some(parts[0].to_string()));
        }
    }
    Ok(None)
}

fn parse_wifi(s: &str) -> Vec<WifiNetwork> {
    let mut out = Vec::new();
    for line in s.lines() {
        // nmcli -t escapes ':' inside fields as '\:'
        let parts = split_nmcli(line);
        if parts.is_empty() || parts[0].is_empty() {
            continue;
        }
        let ssid = parts[0].to_string();
        let signal = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let security = parts.get(2).cloned().unwrap_or_default();
        let in_use = parts.get(3).map(|s| s.trim() == "*").unwrap_or(false);
        out.push(WifiNetwork {
            ssid,
            signal,
            security,
            in_use,
        });
    }
    out
}

fn split_nmcli(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                cur.push(next);
                chars.next();
            }
        } else if c == ':' {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    parts
}

pub async fn wifi_connect(ssid: &str, password: Option<&str>) -> CoreResult<()> {
    if ssid.is_empty() {
        return Err(CoreError::Invalid("ssid is empty".into()));
    }
    let mut argv: Vec<String> = vec![
        "nmcli".into(),
        "device".into(),
        "wifi".into(),
        "connect".into(),
        ssid.into(),
    ];
    if let Some(p) = password {
        argv.push("password".into());
        argv.push(p.into());
    }
    run(argv, Privilege::User).await?;
    Ok(())
}

pub async fn wifi_disconnect() -> CoreResult<()> {
    run(["nmcli", "radio", "wifi", "off"], Privilege::User).await?;
    Ok(())
}

pub async fn interface_toggle(name: &str, up: bool) -> CoreResult<()> {
    let state = if up { "up" } else { "down" };
    run(["ip", "link", "set", name, state], Privilege::Sudo).await?;
    Ok(())
}
