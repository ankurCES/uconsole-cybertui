//! Display: outputs, modes, brightness, scaling.
//!
//! Tries `wlr-randr` first (wlroots-based compositors — Sway, Hyprland, labwc
//! on a uconsole), then `xrandr` for X11. If neither is on PATH we return
//! [`CoreError::NotFound`] and the UI shows a hint instead of crashing.

use serde::{Deserialize, Serialize};

use crate::shell::{read_sysfs, run, Privilege};
use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayOutput {
    pub name: String,
    pub enabled: bool,
    pub mode: String,
    pub position: String,
    pub scale: f32,
    pub adaptive_sync: bool,
}

pub async fn outputs() -> CoreResult<Vec<DisplayOutput>> {
    if let Ok(out) = run(["wlr-randr", "--json"], Privilege::User).await {
        return parse_wlr(&out.stdout);
    }
    if let Ok(out) = run(["xrandr", "--listmonitors"], Privilege::User).await {
        return parse_xrandr(&out.stdout);
    }
    Err(CoreError::NotFound(
        "no wlr-randr or xrandr on PATH — install wlr-randr or x11-xserver-utils".into(),
    ))
}

fn parse_wlr(s: &str) -> CoreResult<Vec<DisplayOutput>> {
    // wlr-randr --json wraps each output as an object whose keys are the
    // output names and values are the modes. We accept both shapes:
    //   { "eDP-1": { "modes":[...], "current_mode":"...", ... } }
    // and the older array form: [ { "name":"eDP-1", ... } ]
    let v: serde_json::Value = serde_json::from_str(s)?;
    let mut out = Vec::new();
    if let Some(arr) = v.as_array() {
        for entry in arr {
            out.push(DisplayOutput {
                name: entry
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?")
                    .into(),
                enabled: entry
                    .get("enabled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                mode: entry
                    .pointer("/current_mode/width")
                    .and_then(|x| x.as_u64())
                    .zip(
                        entry
                            .pointer("/current_mode/height")
                            .and_then(|x| x.as_u64()),
                    )
                    .map(|(w, h)| format!("{w}x{h}"))
                    .unwrap_or_default(),
                position: entry
                    .pointer("/position/x")
                    .and_then(|x| x.as_i64())
                    .zip(entry.pointer("/position/y").and_then(|x| x.as_i64()))
                    .map(|(x, y)| format!("{x},{y}"))
                    .unwrap_or_default(),
                scale: entry.get("scale").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
                adaptive_sync: entry
                    .get("adaptive_sync_enabled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
            });
        }
    } else if let Some(obj) = v.as_object() {
        for (name, val) in obj {
            out.push(DisplayOutput {
                name: name.clone(),
                enabled: val
                    .get("enabled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                mode: val
                    .get("current_mode")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                position: String::new(),
                scale: val.get("scale").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
                adaptive_sync: val
                    .get("adaptive_sync_enabled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
            });
        }
    }
    Ok(out)
}

fn parse_xrandr(s: &str) -> CoreResult<Vec<DisplayOutput>> {
    let mut out = Vec::new();
    let mut current: Option<DisplayOutput> = None;
    for line in s.lines() {
        let trimmed = line.trim_start();
        // "eDP-1 connected 1920x1080+0+0 ..."
        if !line.starts_with(' ') {
            if let Some(o) = current.take() {
                out.push(o);
            }
            let mut it = line.split_whitespace();
            let name = it.next().unwrap_or("").to_string();
            let state = it.next().unwrap_or("");
            current = Some(DisplayOutput {
                name,
                enabled: state == "connected",
                mode: String::new(),
                position: String::new(),
                scale: 1.0,
                adaptive_sync: false,
            });
        } else if let Some(o) = current.as_mut() {
            // trim "+0+0" or "1920x1080" hints off the same line
            for tok in trimmed.split_whitespace() {
                if tok.contains('x') && tok.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    o.mode = tok.to_string();
                } else if tok.starts_with('+') {
                    o.position = tok.to_string();
                }
            }
        }
    }
    if let Some(o) = current.take() {
        out.push(o);
    }
    Ok(out)
}

pub async fn set_brightness(value: u8) -> CoreResult<()> {
    if value > 100 {
        return Err(CoreError::Invalid("brightness must be 0..=100".into()));
    }
    // Prefer brightnessctl (handles all backlight types).
    if which("brightnessctl").await {
        run(
            ["brightnessctl", "set", &format!("{value}%")],
            Privilege::Sudo,
        )
        .await?;
        return Ok(());
    }
    // Fall back to the first intel_backlight/aml-bl/acpi_video0 device.
    let mut dir = tokio::fs::read_dir("/sys/class/backlight").await?;
    if let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        let max = read_sysfs(&format!("{}/max_brightness", path.display()))
            .await
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(100);
        let target = (max as f64 * (value as f64 / 100.0)) as u64;
        tokio::fs::write(format!("{}/brightness", path.display()), target.to_string())
            .await
            .map_err(CoreError::from)?;
        return Ok(());
    }
    Err(CoreError::NotFound("no backlight device".into()))
}

pub async fn brightness() -> CoreResult<u8> {
    if which("brightnessctl").await {
        let out = run(["brightnessctl", "get"], Privilege::User).await?;
        let cur: u64 =
            out.stdout.trim().parse().map_err(|_| {
                CoreError::Parse(format!("brightnessctl returned `{}`", out.stdout))
            })?;
        let max_out = run(["brightnessctl", "max"], Privilege::User).await?;
        let max: u64 = max_out.stdout.trim().parse().unwrap_or(100);
        return Ok(((cur as f64 / max as f64) * 100.0) as u8);
    }
    Err(CoreError::NotFound("no backlight tooling".into()))
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
