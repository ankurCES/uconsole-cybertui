//! Package management via apt (Debian-family).

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub status: String, // installed / upgradable / available
    pub description: String,
}

pub async fn list_installed() -> CoreResult<Vec<Package>> {
    let out = run(
        [
            "dpkg-query",
            "-W",
            "-f=${Package}\t${Version}\t${Architecture}\t${Status}\t${Description}\n",
        ],
        Privilege::User,
    )
    .await?;
    Ok(parse_dpkg(&out.stdout, "installed"))
}

pub async fn search(query: &str) -> CoreResult<Vec<Package>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let out = run(["apt-cache", "search", query], Privilege::User).await?;
    let mut v = Vec::new();
    for line in out.stdout.lines() {
        // "name - description"
        if let Some((head, desc)) = line.split_once(" - ") {
            let mut parts = head.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let version = parts.collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                continue;
            }
            v.push(Package {
                name,
                version,
                arch: String::new(),
                status: "available".into(),
                description: desc.to_string(),
            });
        }
    }
    Ok(v)
}

pub async fn upgradable() -> CoreResult<Vec<Package>> {
    let out = run(["apt", "list", "--upgradable"], Privilege::User).await?;
    let mut v = Vec::new();
    for line in out.stdout.lines().skip(1) {
        // "name/repo version arch [upgradable from: x]"
        if let Some((head, _rest)) = line.split_once(' ') {
            let head = head.trim_end_matches('/');
            let mut it = head.split('/');
            let name = it.next().unwrap_or("").to_string();
            if name.is_empty() || name == "Listing" {
                continue;
            }
            v.push(Package {
                name,
                version: line.to_string(),
                arch: String::new(),
                status: "upgradable".into(),
                description: String::new(),
            });
        }
    }
    Ok(v)
}

fn parse_dpkg(s: &str, status: &str) -> Vec<Package> {
    s.lines()
        .filter_map(|line| {
            let mut it = line.split('\t');
            let name = it.next()?.to_string();
            let version = it.next()?.to_string();
            let arch = it.next()?.to_string();
            let dpkg_status = it.next()?.to_string();
            let description = it.next().unwrap_or("").to_string();
            if !dpkg_status.contains("installed") {
                return None;
            }
            Some(Package {
                name,
                version,
                arch,
                status: status.into(),
                description,
            })
        })
        .collect()
}

pub async fn install(name: &str) -> CoreResult<()> {
    validate(name)?;
    run(["apt-get", "install", "-y", name], Privilege::Sudo).await?;
    Ok(())
}

pub async fn remove(name: &str) -> CoreResult<()> {
    validate(name)?;
    run(["apt-get", "remove", "-y", name], Privilege::Sudo).await?;
    Ok(())
}

pub async fn update() -> CoreResult<String> {
    let out = run(["apt-get", "update"], Privilege::Sudo).await?;
    Ok(out.stdout)
}

pub async fn upgrade() -> CoreResult<String> {
    let out = run(["apt-get", "upgrade", "-y"], Privilege::Sudo).await?;
    Ok(out.stdout)
}

fn validate(name: &str) -> CoreResult<()> {
    if name.is_empty()
        || name.contains(' ')
        || name.contains('\n')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ':' | '~'))
    {
        return Err(CoreError::Invalid(format!("bad package name: {name}")));
    }
    Ok(())
}
