//! Audio: sinks, sources, default, volume. PipeWire (wpctl) first, then
//! PulseAudio (pactl). Most Debian 13 desktops ship PipeWire's pulse shim, so
//! pactl always works as a fallback.

use serde::{Deserialize, Serialize};

use crate::shell::{run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sink {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub volume: u8, // 0..=150 (PipeWire allows >100)
    pub muted: bool,
    pub default: bool,
}

pub async fn sinks() -> CoreResult<Vec<Sink>> {
    if which("wpctl").await {
        return wpctl_sinks().await;
    }
    if which("pactl").await {
        return pactl_sinks().await;
    }
    Err(CoreError::NotFound(
        "install pipewire-audio or pulseaudio-utils".into(),
    ))
}

async fn wpctl_sinks() -> CoreResult<Vec<Sink>> {
    let status = run(["wpctl", "status"], Privilege::User).await?;
    let default_id = parse_wpctl_default(&status.stdout, "Sinks");
    let out = run(
        ["wpctl", "inspect", "@DEFAULT_AUDIO_SINK@"],
        Privilege::User,
    )
    .await?;
    let _ = out; // for completeness, we list via status parsing
    let mut v = Vec::new();
    let mut in_sinks = false;
    for line in status.stdout.lines() {
        let t = line.trim_start();
        if t.starts_with("Audio") {
            // section header in the Sinks block
        }
        if line.trim().ends_with("Sinks:") {
            in_sinks = true;
            continue;
        }
        if line.trim().ends_with("Sources:") {
            in_sinks = false;
            continue;
        }
        if !in_sinks || !t.starts_with('*') && !t.starts_with('│') {
            continue;
        }
        // e.g. " *   54. alsa_output.pci-0000_00_1f.3.hdmi-stereo   [vol: 0.75]"
        if let Some(parsed) = parse_wpctl_line(t) {
            let id = parsed.0;
            let name = parsed.1;
            let vol = parsed.2;
            let desc = parsed.3;
            v.push(Sink {
                id,
                name,
                description: desc,
                volume: (vol * 100.0) as u8,
                muted: false,
                default: Some(id) == default_id,
            });
        }
    }
    Ok(v)
}

fn parse_wpctl_default(s: &str, section: &str) -> Option<u32> {
    let mut in_section = false;
    for line in s.lines() {
        if line.trim().ends_with(&format!("{section}:")) {
            in_section = true;
            continue;
        }
        if in_section && line.trim().ends_with(':') {
            break;
        }
        if in_section && line.contains("Default") {
            // e.g. "Default: alsa_output.pci-0000_00_1f.3.hdmi-stereo (44)"
            if let Some(id_part) = line.rsplit('(').next() {
                if let Some(id) = id_part.trim_end_matches(')').trim().parse::<u32>().ok() {
                    return Some(id);
                }
            }
        }
    }
    None
}

fn parse_wpctl_line(line: &str) -> Option<(u32, String, f32, String)> {
    // "*  54. name  [vol: 0.75]"
    let s = line.trim_start_matches(|c: char| !c.is_ascii_digit() && c != ' ');
    let mut it = s.splitn(2, '.');
    let id_str = it.next()?.trim();
    let id: u32 = id_str.parse().ok()?;
    let rest = it.next()?.trim();
    let mut parts = rest.split_whitespace();
    let name = parts.next()?.to_string();
    let vol_line = rest.split("[vol: ").nth(1).unwrap_or("");
    let vol_str = vol_line.trim_end_matches(']');
    let vol: f32 = vol_str.parse().unwrap_or(1.0);
    let name2 = name.clone();
    Some((id, name, vol, name2))
}

async fn pactl_sinks() -> CoreResult<Vec<Sink>> {
    let short = run(
        ["pactl", "-f", "json", "list", "short", "sinks"],
        Privilege::User,
    )
    .await?;
    let info = run(["pactl", "-f", "json", "list", "sinks"], Privilege::User).await?;
    let default_out = run(["pactl", "get-default-sink"], Privilege::User)
        .await
        .ok();
    let default_name = default_out
        .as_ref()
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default();
    let _ = short; // we have everything in the full dump
    let arr: Vec<serde_json::Value> = serde_json::from_str(&info.stdout)?;
    let mut v = Vec::new();
    for entry in arr {
        let id: u32 = entry
            .pointer("/index")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32;
        let name = entry
            .pointer("/name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let desc = entry
            .pointer("/description")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let vol_raw = entry
            .pointer("/volume/front-left/value")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let muted = entry
            .pointer("/mute")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        v.push(Sink {
            id,
            name,
            description: desc,
            volume: ((vol_raw * 100.0).clamp(0.0, 150.0)) as u8,
            muted,
            default: !default_name.is_empty()
                && default_name
                    == entry
                        .pointer("/name")
                        .and_then(|x| x.as_str())
                        .unwrap_or(""),
        });
    }
    Ok(v)
}

pub async fn set_default_sink(name: &str) -> CoreResult<()> {
    if name.is_empty() {
        return Err(CoreError::Invalid("sink name".into()));
    }
    if which("wpctl").await {
        run(["wpctl", "set-default", name], Privilege::User).await?;
    } else {
        run(["pactl", "set-default-sink", name], Privilege::User).await?;
    }
    Ok(())
}

pub async fn set_volume(target: &str, percent: u8) -> CoreResult<()> {
    if percent > 150 {
        return Err(CoreError::Invalid("volume must be 0..=150".into()));
    }
    if which("wpctl").await {
        run(
            ["wpctl", "set-volume", target, &format!("{percent}%")],
            Privilege::User,
        )
        .await?;
    } else {
        run(
            ["pactl", "set-sink-volume", target, &format!("{percent}%")],
            Privilege::User,
        )
        .await?;
    }
    Ok(())
}

pub async fn set_mute(target: &str, mute: bool) -> CoreResult<()> {
    let state = if mute { "1" } else { "0" };
    if which("wpctl").await {
        run(["wpctl", "set-mute", target, state], Privilege::User).await?;
    } else {
        run(["pactl", "set-sink-mute", target, state], Privilege::User).await?;
    }
    Ok(())
}

async fn which(prog: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if tokio::fs::metadata(dir.join(prog)).await.is_ok() {
            return true;
        }
    }
    false
}
