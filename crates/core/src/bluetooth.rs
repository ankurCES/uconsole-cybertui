//! Bluetooth via `bluetoothctl`.
//!
//! Kept deliberately simple — list, scan, pair, connect, trust, disconnect.
//! All writes are routed through [`Privilege::Sudo`] because the bluetooth
//! service usually runs as root.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtDevice {
    pub mac: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
    pub trusted: bool,
    pub rssi: Option<i16>,
}

pub async fn list() -> CoreResult<Vec<BtDevice>> {
    let out = run(["bluetoothctl", "devices"], Privilege::User).await?;
    let mut v = Vec::new();
    for line in out.stdout.lines() {
        // "Device AA:BB:CC:DD:EE:FF name"
        let mut it = line.split_whitespace();
        if it.next() != Some("Device") {
            continue;
        }
        let mac = match it.next() {
            Some(m) => m.to_string(),
            None => continue,
        };
        let name = it.collect::<Vec<_>>().join(" ");
        let info = run(["bluetoothctl", "info", &mac], Privilege::User)
            .await
            .ok();
        let (paired, connected, trusted, rssi) = info
            .map(|o| parse_info(&o.stdout))
            .unwrap_or((false, false, false, None));
        v.push(BtDevice {
            mac,
            name,
            paired,
            connected,
            trusted,
            rssi,
        });
    }
    Ok(v)
}

fn parse_info(s: &str) -> (bool, bool, bool, Option<i16>) {
    let paired = s.lines().any(|l| l.trim() == "Paired: yes");
    let connected = s.lines().any(|l| l.trim() == "Connected: yes");
    let trusted = s.lines().any(|l| l.trim() == "Trusted: yes");
    let rssi = s
        .lines()
        .find(|l| l.trim().starts_with("RSSI:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|n| n.trim().split_whitespace().next())
        .and_then(|n| n.parse().ok());
    (paired, connected, trusted, rssi)
}

pub async fn pair(mac: &str) -> CoreResult<()> {
    validate_mac(mac)?;
    run(["bluetoothctl", "pair", mac], Privilege::Sudo).await?;
    Ok(())
}

pub async fn connect(mac: &str) -> CoreResult<()> {
    validate_mac(mac)?;
    run(["bluetoothctl", "connect", mac], Privilege::Sudo).await?;
    Ok(())
}

pub async fn disconnect(mac: &str) -> CoreResult<()> {
    validate_mac(mac)?;
    run(["bluetoothctl", "disconnect", mac], Privilege::Sudo).await?;
    Ok(())
}

pub async fn trust(mac: &str) -> CoreResult<()> {
    validate_mac(mac)?;
    run(["bluetoothctl", "trust", mac], Privilege::Sudo).await?;
    Ok(())
}

pub async fn adapter_power(on: bool) -> CoreResult<()> {
    let cmd = if on { "power on" } else { "power off" };
    run(["bluetoothctl", cmd], Privilege::Sudo).await?;
    Ok(())
}

fn validate_mac(mac: &str) -> CoreResult<()> {
    if mac.len() != 17 {
        return Err(CoreError::Invalid("mac".into()));
    }
    if !mac.chars().enumerate().all(|(i, c)| match i % 3 {
        2 => c == ':',
        _ => c.is_ascii_hexdigit(),
    }) {
        return Err(CoreError::Invalid("mac".into()));
    }
    Ok(())
}
