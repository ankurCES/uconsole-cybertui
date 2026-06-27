//! Power: battery, AC, CPU governor, suspend/hibernate.

use serde::{Deserialize, Serialize};

use crate::shell::{read_sysfs, run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Battery {
    pub present: bool,
    pub capacity: u8,   // 0..100
    pub status: String, // Charging / Discharging / Full / Unknown
    pub time_to_full: Option<String>,
    pub time_to_empty: Option<String>,
    pub health: Option<String>,
    pub power_now_w: Option<f32>,
}

pub async fn battery() -> CoreResult<Battery> {
    let mut bats = Vec::new();
    let mut dir = match tokio::fs::read_dir("/sys/class/power_supply").await {
        Ok(d) => d,
        Err(_) => {
            return Ok(Battery {
                present: false,
                capacity: 0,
                status: "AC".into(),
                time_to_full: None,
                time_to_empty: None,
                health: None,
                power_now_w: None,
            });
        }
    };
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("BAT") && !name.starts_with("battery") {
            continue;
        }
        let path = entry.path();
        let cap = tokio::fs::read_to_string(path.join("capacity"))
            .await
            .ok()
            .and_then(|s| s.trim().parse().ok());
        let status = tokio::fs::read_to_string(path.join("status"))
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "Unknown".into());
        let time_to_full = tokio::fs::read_to_string(path.join("time_to_full_now"))
            .await
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
            .map(format_seconds_hms);
        let time_to_empty = tokio::fs::read_to_string(path.join("time_to_empty_now"))
            .await
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
            .map(format_seconds_hms);
        let health = tokio::fs::read_to_string(path.join("health"))
            .await
            .ok()
            .map(|s| s.trim().to_string());
        let power_now_w = tokio::fs::read_to_string(path.join("power_now"))
            .await
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|uw| uw as f32 / 1_000_000.0);
        if let Some(capacity) = cap {
            bats.push(Battery {
                present: true,
                capacity,
                status,
                time_to_full,
                time_to_empty,
                health,
                power_now_w,
            });
        }
    }
    if let Some(b) = bats.into_iter().next() {
        return Ok(b);
    }
    Err(CoreError::NotFound("no battery".into()))
}

fn format_seconds_hms(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuGovernor {
    pub driver: String,
    pub governor: String,
    pub available: Vec<String>,
}

pub async fn cpu_governor() -> CoreResult<CpuGovernor> {
    // Look at cpu0 — they're usually all the same on a uconsole, and if not
    // we surface a warning in the UI.
    let path = "/sys/devices/system/cpu/cpu0/cpufreq";
    let governor = read_sysfs(&format!("{path}/scaling_governor"))
        .await
        .unwrap_or_else(|_| "?".into());
    let driver = read_sysfs(&format!("{path}/scaling_driver"))
        .await
        .unwrap_or_else(|_| "?".into());
    let available = read_sysfs(&format!("{path}/scaling_available_governors"))
        .await
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    Ok(CpuGovernor {
        driver,
        governor,
        available,
    })
}

pub async fn set_governor(g: &str) -> CoreResult<()> {
    if g.is_empty() {
        return Err(CoreError::Invalid("governor".into()));
    }
    run(
        [
            "/bin/sh",
            "-c",
            &format!(
                "echo {g} | tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor >/dev/null"
            ),
        ],
        Privilege::Sudo,
    )
    .await?;
    Ok(())
}

pub async fn suspend() -> CoreResult<()> {
    run(["systemctl", "suspend"], Privilege::Sudo).await?;
    Ok(())
}

pub async fn hibernate() -> CoreResult<()> {
    run(["systemctl", "hibernate"], Privilege::Sudo).await?;
    Ok(())
}

pub async fn reboot() -> CoreResult<()> {
    run(["systemctl", "reboot"], Privilege::Sudo).await?;
    Ok(())
}

pub async fn shutdown() -> CoreResult<()> {
    run(["systemctl", "poweroff"], Privilege::Sudo).await?;
    Ok(())
}
