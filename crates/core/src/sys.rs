//! System info: hostname, kernel, OS, uptime, load avg, memory, thermal.

use serde::{Deserialize, Serialize};

use crate::shell::{read_sysfs, run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub hostname: String,
    pub kernel: String,
    pub os: String,
    pub arch: String,
    pub uptime_secs: u64,
    pub loadavg: (f64, f64, f64),
    pub memory: Memory,
    pub cpu_count: usize,
    pub cpu_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub total_kb: u64,
    pub available_kb: u64,
    pub used_pct: f32,
}

pub async fn info() -> CoreResult<SystemInfo> {
    let hostname = hostname().await?;
    let kernel = read_sysfs("/proc/sys/kernel/osrelease")
        .await
        .unwrap_or_else(|_| "unknown".into());
    let os = detect_os().await;
    let arch = std::env::consts::ARCH.to_string();
    let uptime_secs = uptime().await?;
    let loadavg = loadavg().await?;
    let memory = memory().await?;
    let (cpu_count, cpu_model) = cpu_info().await?;

    Ok(SystemInfo {
        hostname,
        kernel,
        os,
        arch,
        uptime_secs,
        loadavg,
        memory,
        cpu_count,
        cpu_model,
    })
}

pub async fn hostname() -> CoreResult<String> {
    let out = run(["hostname"], Privilege::User).await?;
    Ok(out.stdout.trim().to_string())
}

async fn detect_os() -> String {
    // Try /etc/os-release first (works on every modern distro).
    if let Ok(s) = tokio::fs::read_to_string("/etc/os-release").await {
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                return v.trim_matches('"').to_string();
            }
        }
    }
    "Linux".to_string()
}

pub async fn uptime() -> CoreResult<u64> {
    let s = read_sysfs("/proc/uptime").await?;
    let secs: f64 = s
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CoreError::Parse("uptime".into()))?;
    Ok(secs as u64)
}

pub async fn loadavg() -> CoreResult<(f64, f64, f64)> {
    let s = read_sysfs("/proc/loadavg").await?;
    let mut it = s.split_whitespace();
    let a: f64 = it
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CoreError::Parse("loadavg".into()))?;
    let b: f64 = it
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CoreError::Parse("loadavg".into()))?;
    let c: f64 = it
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CoreError::Parse("loadavg".into()))?;
    Ok((a, b, c))
}

pub async fn memory() -> CoreResult<Memory> {
    let s = read_sysfs("/proc/meminfo").await?;
    let mut total = 0u64;
    let mut avail = 0u64;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = kb_from_line(rest).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail = kb_from_line(rest).unwrap_or(0);
        }
    }
    let used_pct = if total == 0 {
        0.0
    } else {
        ((total - avail) as f32 / total as f32) * 100.0
    };
    Ok(Memory {
        total_kb: total,
        available_kb: avail,
        used_pct,
    })
}

fn kb_from_line(s: &str) -> Option<u64> {
    s.split_whitespace().next()?.parse().ok()
}

async fn cpu_info() -> CoreResult<(usize, String)> {
    let s = read_sysfs("/proc/cpuinfo").await?;
    let mut count = 0usize;
    let mut model = String::from("unknown");
    for line in s.lines() {
        if line.starts_with("processor") {
            count += 1;
        } else if model == "unknown" {
            if let Some(v) = line.strip_prefix("model name\t: ") {
                model = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Hardware\t: ") {
                model = v.trim().to_string();
            }
        }
    }
    Ok((count.max(1), model))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalReading {
    pub label: String,
    pub temp_c: f32,
}

pub async fn thermals() -> CoreResult<Vec<ThermalReading>> {
    let mut out = Vec::new();
    let mut dir = tokio::fs::read_dir("/sys/class/thermal").await?;
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let path = entry.path();
        let temp_path = path.join("temp");
        let type_path = path.join("type");
        let temp_str = match tokio::fs::read_to_string(&temp_path).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        let label = tokio::fs::read_to_string(&type_path)
            .await
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| name.to_string());
        let milli: i64 = match temp_str.trim().parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        out.push(ThermalReading {
            label,
            temp_c: milli as f32 / 1000.0,
        });
    }
    if out.is_empty() {
        return Err(CoreError::NotFound("no thermal zones".into()));
    }
    Ok(out)
}

/// Pretty "3d 4h" formatter for uptime.
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

/// Pretty "1.4G / 7.6G" for memory.
pub fn format_mem(m: &Memory) -> String {
    format!(
        "{:.1}G / {:.1}G ({:.0}%)",
        m.used_kb() as f64 / 1_048_576.0,
        m.total_kb as f64 / 1_048_576.0,
        m.used_pct
    )
}

impl Memory {
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }
}
