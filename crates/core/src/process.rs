//! Process management: ps, kill, renice.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub pid: i32,
    pub user: String,
    pub cpu: f32,
    pub mem: f32,
    pub vsz_kb: u64,
    pub rss_kb: u64,
    pub stat: String,
    pub start: String,
    pub time: String,
    pub command: String,
}

pub async fn list() -> CoreResult<Vec<Process>> {
    let out = run(
        [
            "ps",
            "-eo",
            "pid,user,pcpu,pmem,vsz,rss,stat,start,time,comm,args",
            "--sort=-pcpu",
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
        if cols.len() < 11 {
            continue;
        }
        let pid: i32 = cols[0].parse().unwrap_or(0);
        if pid == 0 {
            continue;
        }
        v.push(Process {
            pid,
            user: cols[1].into(),
            cpu: cols[2].parse().unwrap_or(0.0),
            mem: cols[3].parse().unwrap_or(0.0),
            vsz_kb: cols[4].parse().unwrap_or(0),
            rss_kb: cols[5].parse().unwrap_or(0),
            stat: cols[6].into(),
            start: cols[7].into(),
            time: cols[8].into(),
            command: cols[10..].join(" "),
        });
    }
    Ok(v)
}

pub async fn kill(pid: i32, signal: &str) -> CoreResult<()> {
    if pid <= 0 {
        return Err(CoreError::Invalid("pid".into()));
    }
    let sig = if signal.is_empty() { "TERM" } else { signal };
    run(
        ["kill", &format!("-{sig}"), &pid.to_string()],
        Privilege::User,
    )
    .await?;
    Ok(())
}

pub async fn renice(pid: i32, nice: i32) -> CoreResult<()> {
    if pid <= 0 {
        return Err(CoreError::Invalid("pid".into()));
    }
    if !(-20..=19).contains(&nice) {
        return Err(CoreError::Invalid("nice must be in -20..=19".into()));
    }
    run(
        ["renice", &nice.to_string(), "-p", &pid.to_string()],
        Privilege::User,
    )
    .await?;
    Ok(())
}
