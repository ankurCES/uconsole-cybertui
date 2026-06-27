//! Systemd services: list, inspect, start/stop/restart, enable/disable.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub unit: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

pub async fn list_all() -> CoreResult<Vec<Service>> {
    let out = match run(
        [
            "systemctl",
            "list-units",
            "--type=service",
            "--all",
            "--no-pager",
            "--no-legend",
            "--output=json",
        ],
        Privilege::User,
    )
    .await
    {
        Ok(o) => o,
        Err(_) => {
            // Fallback to the older tabular form on older systemd.
            run(
                [
                    "systemctl",
                    "list-units",
                    "--type=service",
                    "--all",
                    "--no-pager",
                    "--no-legend",
                ],
                Privilege::User,
            )
            .await?
        }
    };
    if out.stdout.trim_start().starts_with('[') {
        let raw: Vec<serde_json::Value> = serde_json::from_str(&out.stdout)?;
        let mut v = Vec::new();
        for entry in raw {
            v.push(Service {
                unit: entry
                    .get("unit")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
                load: entry
                    .get("load")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
                active: entry
                    .get("active")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
                sub: entry
                    .get("sub")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
                description: entry
                    .get("description")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
            });
        }
        return Ok(v);
    }
    Ok(parse_table(&out.stdout))
}

fn parse_table(s: &str) -> Vec<Service> {
    s.lines()
        .filter_map(|line| {
            // 0..2 leading spaces, then UNIT LOAD ACTIVE SUB DESCRIPTION
            let line = line.trim_start();
            let mut it = line.split_whitespace();
            let unit = it.next()?.to_string();
            let load = it.next()?.to_string();
            let active = it.next()?.to_string();
            let sub = it.next()?.to_string();
            let description = it.collect::<Vec<_>>().join(" ");
            Some(Service {
                unit,
                load,
                active,
                sub,
                description,
            })
        })
        .collect()
}

pub async fn start(unit: &str) -> CoreResult<()> {
    run(["systemctl", "start", unit], Privilege::Sudo).await?;
    Ok(())
}
pub async fn stop(unit: &str) -> CoreResult<()> {
    run(["systemctl", "stop", unit], Privilege::Sudo).await?;
    Ok(())
}
pub async fn restart(unit: &str) -> CoreResult<()> {
    run(["systemctl", "restart", unit], Privilege::Sudo).await?;
    Ok(())
}
pub async fn enable(unit: &str) -> CoreResult<()> {
    run(["systemctl", "enable", unit], Privilege::Sudo).await?;
    Ok(())
}
pub async fn disable(unit: &str) -> CoreResult<()> {
    run(["systemctl", "disable", unit], Privilege::Sudo).await?;
    Ok(())
}
pub async fn status(unit: &str) -> CoreResult<String> {
    let out = run(
        ["systemctl", "status", unit, "--no-pager", "-n", "20"],
        Privilege::User,
    )
    .await?;
    Ok(out.stdout)
}

pub fn unit_basename(unit: &str) -> &str {
    unit.strip_suffix(".service").unwrap_or(unit)
}

pub async fn list_unit_files(filter: &str) -> CoreResult<Vec<(String, String, String)>> {
    // (unit, state, preset)
    let out = run(
        [
            "systemctl",
            "list-unit-files",
            "--type=service",
            "--no-pager",
            "--no-legend",
        ],
        Privilege::User,
    )
    .await?;
    let f = filter.to_lowercase();
    Ok(out
        .stdout
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let unit = it.next()?.to_string();
            let state = it.next()?.to_string();
            let preset = it.next().unwrap_or("").to_string();
            if !f.is_empty() && !unit.to_lowercase().contains(&f) {
                return None;
            }
            Some((unit, state, preset))
        })
        .collect())
}

pub fn require_unit(unit: &str) -> CoreResult<()> {
    if unit.is_empty() || unit.contains(' ') || unit.contains('\n') {
        return Err(CoreError::Invalid("invalid unit name".into()));
    }
    Ok(())
}
