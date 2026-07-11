//! Minimal AI agent tool harness for device control.
//!
//! Exposes a small set of read-only + low-impact system tools to the
//! llama-server sidecar via OpenAI-compatible function calling. Designed
//! for MiniCPM5-1B on a ClockworkPi uConsole CM4 — keeps tool schemas
//! simple and execution cheap.

use serde_json::{json, Value};
use std::process::Command;

pub fn tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "get_system_info",
                "description": "Get CPU, memory, temperature, battery, and uptime",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_network_info",
                "description": "Get network interfaces, IP addresses, and active WiFi SSID",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_bluetooth_devices",
                "description": "List paired and nearby Bluetooth devices",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": "Run a read-only shell command (ip, df, free, uptime, date, ping, curl, cat, ls, ps, top, uname, hostname, iwconfig, nmcli, bluetoothctl, sensors, who, journalctl). No destructive commands.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute"
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "set_brightness",
                "description": "Set screen brightness (0-100%)",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "percent": { "type": "integer", "description": "Brightness 0-100" }
                    },
                    "required": ["percent"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "set_volume",
                "description": "Set audio volume (0-100%)",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "percent": { "type": "integer", "description": "Volume 0-100" }
                    },
                    "required": ["percent"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "wifi_connect",
                "description": "Connect to a WiFi network",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "ssid": { "type": "string", "description": "Network name" },
                        "password": { "type": "string", "description": "Network password (omit for open)" }
                    },
                    "required": ["ssid"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "wifi_disconnect",
                "description": "Disconnect from current WiFi network",
                "parameters": { "type": "object", "properties": {} }
            }
        }
    ])
}

/// Execute a tool call. Returns the result string to feed back to the LLM.
pub fn execute_tool(name: &str, args: &Value) -> String {
    match name {
        "get_system_info" => get_system_info(),
        "get_network_info" => get_network_info(),
        "get_bluetooth_devices" => get_bluetooth_devices(),
        "run_shell" => {
            let cmd = args["command"].as_str().unwrap_or("");
            run_shell(cmd)
        }
        "set_brightness" => {
            let pct = args["percent"].as_u64().unwrap_or(50) as u8;
            set_brightness(pct)
        }
        "set_volume" => {
            let pct = args["percent"].as_u64().unwrap_or(50) as u8;
            set_volume(pct)
        }
        "wifi_connect" => {
            let ssid = args["ssid"].as_str().unwrap_or("");
            let pw = args["password"].as_str();
            wifi_connect(ssid, pw)
        }
        "wifi_disconnect" => wifi_disconnect(),
        _ => format!("unknown tool: {name}"),
    }
}

/// One-line summary for the UI tool log.
pub fn tool_log_line(name: &str, args: &Value, result: &str) -> String {
    let short_result = if result.len() > 80 {
        format!("{}…", &result[..77])
    } else {
        result.to_string()
    };
    match name {
        "run_shell" => {
            let cmd = args["command"].as_str().unwrap_or("?");
            format!("$ {cmd} → {short_result}")
        }
        _ => format!("{name}() → {short_result}"),
    }
}

fn sh(args: &[&str]) -> String {
    Command::new(args[0])
        .args(&args[1..])
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if o.status.success() {
                out
            } else {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                format!("error: {err}")
            }
        })
        .unwrap_or_else(|e| format!("exec failed: {e}"))
}

fn get_system_info() -> String {
    let uptime = sh(&["uptime", "-p"]);
    let mem = sh(&["free", "-h", "--si"]);
    let temp = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|t| format!("{:.1}°C", t / 1000.0))
        .unwrap_or_else(|| "n/a".into());
    let batt = sh(&["cat", "/sys/class/power_supply/battery/capacity"]);
    let load = sh(&["cat", "/proc/loadavg"]);
    format!("uptime: {uptime}\nload: {load}\ntemp: {temp}\nbattery: {batt}%\n{mem}")
}

fn get_network_info() -> String {
    let ifaces = sh(&["ip", "-br", "addr"]);
    let ssid = sh(&["nmcli", "-t", "-f", "active,ssid", "dev", "wifi"]);
    let route = sh(&["ip", "route", "show", "default"]);
    format!("interfaces:\n{ifaces}\n\nwifi: {ssid}\ndefault route: {route}")
}

fn get_bluetooth_devices() -> String {
    sh(&["bluetoothctl", "devices"])
}

const SHELL_ALLOWLIST: &[&str] = &[
    "ip", "df", "free", "uptime", "date", "ping", "curl", "cat", "ls",
    "ps", "top", "uname", "hostname", "iwconfig", "nmcli", "bluetoothctl",
    "sensors", "who", "journalctl", "lsblk", "lsusb", "lscpu", "ss",
    "dig", "nslookup", "traceroute", "head", "tail", "wc", "grep",
    "find", "file", "stat", "mount", "id", "groups", "env", "printenv",
    "timedatectl", "hostnamectl", "systemctl",
];

fn run_shell(cmd: &str) -> String {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return "empty command".into();
    }
    let bin = parts[0].rsplit('/').next().unwrap_or(parts[0]);
    if !SHELL_ALLOWLIST.contains(&bin) {
        return format!("blocked: '{bin}' not in allowlist. Allowed: {}", SHELL_ALLOWLIST.join(", "));
    }
    // Block obviously destructive flags
    if parts.iter().any(|p| *p == "rm" || *p == "mkfs" || *p == "--force" || p.starts_with(">/")) {
        return "blocked: destructive operation".into();
    }
    sh(&parts)
}

fn set_brightness(pct: u8) -> String {
    let pct = pct.min(100);
    let bl = "/sys/class/backlight";
    let Ok(rd) = std::fs::read_dir(bl) else {
        return "no backlight device found".into();
    };
    for entry in rd.flatten() {
        let max_path = entry.path().join("max_brightness");
        if let Ok(s) = std::fs::read_to_string(&max_path) {
            if let Ok(max) = s.trim().parse::<u64>() {
                let val = (max * pct as u64 / 100).min(max);
                if std::fs::write(entry.path().join("brightness"), val.to_string()).is_ok() {
                    return format!("brightness set to {pct}%");
                }
            }
        }
    }
    "failed to set brightness".into()
}

fn set_volume(pct: u8) -> String {
    let pct = pct.min(100);
    sh(&["pactl", "set-sink-volume", "@DEFAULT_SINK@", &format!("{pct}%")])
}

fn wifi_connect(ssid: &str, password: Option<&str>) -> String {
    if ssid.is_empty() { return "ssid required".into(); }
    match password {
        Some(pw) => sh(&["nmcli", "dev", "wifi", "connect", ssid, "password", pw]),
        None => sh(&["nmcli", "dev", "wifi", "connect", ssid]),
    }
}

fn wifi_disconnect() -> String {
    sh(&["nmcli", "dev", "disconnect", "wlan0"])
}
