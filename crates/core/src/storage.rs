//! Storage: df, lsblk, mount/unmount.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filesystem {
    pub source: String,
    pub fstype: String,
    pub size: String,
    pub used: String,
    pub avail: String,
    pub use_pct: u8,
    pub mounted_on: String,
}

pub async fn df() -> CoreResult<Vec<Filesystem>> {
    let out = run(
        [
            "df",
            "-h",
            "--output=source,fstype,size,used,avail,pcent,target",
            "-x",
            "tmpfs",
            "-x",
            "devtmpfs",
        ],
        Privilege::User,
    )
    .await?;
    let mut v = Vec::new();
    for (i, line) in out.stdout.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 7 {
            continue;
        }
        let use_pct = cols[5].trim_end_matches('%').parse().unwrap_or(0);
        v.push(Filesystem {
            source: cols[0].into(),
            fstype: cols[1].into(),
            size: cols[2].into(),
            used: cols[3].into(),
            avail: cols[4].into(),
            use_pct,
            mounted_on: cols[6].into(),
        });
    }
    Ok(v)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDevice {
    pub name: String,
    pub size: String,
    pub fstype: String,
    pub mountpoint: String,
    pub label: String,
    pub model: String,
}

pub async fn lsblk() -> CoreResult<Vec<BlockDevice>> {
    let out = run(
        [
            "lsblk",
            "-J",
            "-b",
            "-o",
            "NAME,SIZE,FSTYPE,MOUNTPOINT,LABEL,MODEL",
        ],
        Privilege::User,
    )
    .await?;
    let v: serde_json::Value = serde_json::from_str(&out.stdout)?;
    let mut result = Vec::new();
    flatten_lsblk(&v, "", &mut result);
    Ok(result)
}

fn flatten_lsblk(v: &serde_json::Value, prefix: &str, out: &mut Vec<BlockDevice>) {
    if let Some(arr) = v.get("blockdevices").and_then(|v| v.as_array()) {
        for d in arr {
            let name = d
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let size_bytes = d.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let size = format_size(size_bytes);
            let fstype = d
                .get("fstype")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mountpoint = d
                .get("mountpoint")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if prefix.is_empty() {
                        "—".into()
                    } else {
                        prefix.to_string()
                    }
                });
            let label = d
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let model = d
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push(BlockDevice {
                name,
                size,
                fstype,
                mountpoint,
                label,
                model,
            });
            if let Some(children) = d.get("children").and_then(|v| v.as_array()) {
                for c in children {
                    flatten_lsblk(&serde_json::Value::Array(vec![c.clone()]), "", out);
                }
            }
        }
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T", "P"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1}{}", size, UNITS[unit])
    }
}

pub async fn mount(src: &str, target: &str) -> CoreResult<()> {
    if src.is_empty() || target.is_empty() {
        return Err(CoreError::Invalid("mount source or target empty".into()));
    }
    run(["mount", src, target], Privilege::Sudo).await?;
    Ok(())
}

pub async fn umount(target: &str) -> CoreResult<()> {
    run(["umount", target], Privilege::Sudo).await?;
    Ok(())
}
