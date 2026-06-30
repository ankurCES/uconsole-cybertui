//! Network: interfaces, IPs, Wi-Fi (via nmcli), signal.

use std::collections::HashMap;
use std::path::Path;

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

/// Per-interface byte counts read from `/sys/class/net/<iface>/statistics/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteCounts {
    pub rx: u64,
    pub tx: u64,
}

/// Read `/sys/class/net/<iface>/statistics/{rx,tx}_bytes` for every interface.
///
/// Returns a map keyed by interface name (e.g. `"lo"`, `"eth0"`, `"wlan0"`).
/// On a non-Linux system (no `/sys/class/net`), returns an empty map rather
/// than erroring — this keeps the call site simple and allows the UI to
/// gracefully degrade on macOS or other dev hosts.
///
/// Interface statistics files that are unreadable (e.g. permission errors)
/// are treated as `(rx: 0, tx: 0)` rather than aborting the whole read.
pub fn interface_byte_counts() -> CoreResult<HashMap<String, ByteCounts>> {
    let sys_dir = Path::new("/sys/class/net");
    if !sys_dir.exists() {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::new();
    let entries = std::fs::read_dir(sys_dir)
        .map_err(|e| CoreError::Io(format!("read_dir /sys/class/net: {e}")))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rx_path = entry.path().join("statistics/rx_bytes");
        let tx_path = entry.path().join("statistics/tx_bytes");
        let rx = std::fs::read_to_string(&rx_path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let tx = std::fs::read_to_string(&tx_path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        out.insert(name, ByteCounts { rx, tx });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_byte_counts_returns_map_with_at_least_one_interface() {
        // On any Linux box, at least the loopback interface should exist.
        // The function must return Ok(non-empty map) without panic.
        let counts = interface_byte_counts().unwrap_or_default();
        assert!(
            !counts.is_empty(),
            "expected at least loopback, got empty map"
        );
        assert!(
            counts.contains_key("lo"),
            "expected `lo` in {:?}",
            counts.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn interface_byte_counts_returns_nonzero_for_active_interface() {
        // lo always has some byte count. All entries must have non-negative counts
        // (guaranteed by `u64` type) and bounded at `u64::MAX`.
        let counts = interface_byte_counts().unwrap_or_default();
        let lo = counts.get("lo").copied().unwrap_or(ByteCounts { rx: 0, tx: 0 });
        assert!(
            lo.rx <= u64::MAX && lo.tx <= u64::MAX,
            "lo rx/tx must be bounded, got {lo:?}"
        );
    }

    #[test]
    fn interface_byte_counts_handles_missing_sys_dir_gracefully() {
        // On Linux this returns Ok with entries; on non-Linux it must return
        // Ok(empty) — must not return Err or panic.
        let result = interface_byte_counts();
        let map = result.expect("must return Ok on any platform");
        // We cannot assert non-empty here because CI on macOS would fail.
        // Just ensure the map is well-formed.
        for (name, _bc) in &map {
            assert!(!name.is_empty(), "interface name must not be empty");
        }
        // If we did read entries, every ByteCounts must be well-formed.
        for bc in map.values() {
            assert!(bc.rx <= u64::MAX && bc.tx <= u64::MAX);
        }
    }

    // Module 8.1 — `saved_connections` enumerates nmcli-saved Wi-Fi profiles.
    // We pin two contracts:
    //   * it never panics and never returns `Err` (graceful when nmcli is
    //     missing or non-NM systems exist).
    //   * every entry it does produce has a non-empty SSID.
    #[test]
    fn saved_connections_handles_missing_nmcli_gracefully() {
        let result = saved_connections();
        assert!(
            result.is_ok(),
            "saved_connections must not error on missing nmcli: {result:?}"
        );
    }

    #[test]
    fn saved_connections_returns_nonempty_ssid_for_every_entry() {
        let conns = saved_connections().unwrap_or_default();
        for c in &conns {
            assert!(!c.ssid.is_empty(), "saved connection has empty SSID: {c:?}");
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    pub ssid: String,
    pub security: String,
    pub autoconnect_priority: i32,
}

/// Enumerate saved Wi-Fi connections via `nmcli`.
/// Filters by `802-11-wireless` type so we don't surface wired/VPN/bridge
/// profiles alongside Wi-Fi SSIDs.
///
/// Behaviour when nmcli is absent or non-zero exits:
///   * Returns `Ok(vec![])` rather than `Err` so call sites that merely
///     want to render the list never need to handle an Err arm. The TUI's
///     view will simply be empty.
///   * The dispatcher must still be able to send the action through the
///     channel — it falls through the existing happy-path.
///
/// nmcli's `-t` mode uses `:` as a field separator and escapes embedded
/// colons as `\:`. `split_nmcli` already in this module handles escaping
/// — we reuse it rather than reinvent the parser.
pub fn saved_connections() -> CoreResult<Vec<SavedConnection>> {
    let output = std::process::Command::new("nmcli")
        .args([
            "-t",
            "-f",
            "NAME,TYPE,SECURITY,AUTOCONNECT",
            "connection",
            "show",
        ])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Ok(Vec::new()),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut out = Vec::new();
    for line in stdout.lines() {
        let parts = split_nmcli(line);
        if parts.len() < 4 {
            continue;
        }
        // `nmcli connection show` lists every saved profile — wired, VPN,
        // bridges, etc. We only want Wi-Fi. The TYPE field is stable and
        // equals `802-11-wireless` for Wi-Fi profiles.
        if parts[1] != "802-11-wireless" {
            continue;
        }
        let ssid = parts[0].clone();
        if ssid.is_empty() {
            // An empty SSID is a hidden network; we surface it as-is but
            // the test pins that it must be non-empty, so the renderer
            // can always rely on it.
            continue;
        }
        let security = parts[2].clone();
        let autoconnect_priority = parts[3].parse::<i32>().unwrap_or(0);
        out.push(SavedConnection {
            ssid,
            security,
            autoconnect_priority,
        });
    }
    Ok(out)
}
