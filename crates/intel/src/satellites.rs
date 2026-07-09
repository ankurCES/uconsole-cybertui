//! Satellites layer — CelesTrak "above" pass predictions.
//!
//! Mirrors the `satellites` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). CelesTrak publishes a TLE catalogue; we just count the
//! objects currently above the user's horizon (default lat/lon
//! matching weather.rs).
//!
//! Sentinel severity:
//!   * `Green` — at least one visible pass in the next 60 minutes
//!   * `Yellow` — none in the next hour but at least one in the next 24h
//!   * `Red`    — none in the next 24h (silent sky — likely a tracker bug)
//!
//! Authentication: none (CelesTrak is public).

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SatPass {
    #[serde(default)]
    pub norad_id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub rise_time: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ParsedSats {
    pub count: u64,
    pub nearest_minutes: i64,
}

pub fn parse(body: &serde_json::Value, now_unix: i64) -> anyhow::Result<ParsedSats> {
    let Some(arr) = body.get("above").and_then(|v| v.as_array()) else {
        anyhow::bail!("celestrak: missing `above` array");
    };
    let mut count = 0u64;
    let mut nearest: i64 = i64::MAX;
    for row in arr {
        count += 1;
        if let Ok(p) = serde_json::from_value::<SatPass>(row.clone()) {
            if let Some(t) = p.rise_time {
                let dt_min = (t - now_unix) / 60;
                if dt_min >= 0 && dt_min < nearest {
                    nearest = dt_min;
                }
            }
        }
    }
    Ok(ParsedSats {
        count,
        nearest_minutes: if nearest == i64::MAX { -1 } else { nearest },
    })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body, last_ok_unix) {
        Ok(p) => {
            let sentinel = if p.nearest_minutes < 0 {
                Sentinel::Red
            } else if p.nearest_minutes > 60 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Satellites,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!(
                    "CelesTrak · {} visible (next pass {}m)",
                    p.count, p.nearest_minutes
                ),
                entity_count: p.count,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Satellites, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_when_pass_in_next_hour() {
        let v = serde_json::json!({
            "above": [
                { "norad_id": 1, "name": "ISS", "rise_time": 1000 + 600 },
                { "norad_id": 2, "name": "STARLINK-1", "rise_time": 1000 + 3000 }
            ]
        });
        let s = snapshot_from(&v, 1000);
        assert_eq!(s.sentinel, Sentinel::Green);
        assert_eq!(s.entity_count, 2);
    }

    #[test]
    fn yellow_when_only_far_passes() {
        let v = serde_json::json!({
            "above": [
                { "rise_time": 1000 + 7200 }
            ]
        });
        let s = snapshot_from(&v, 1000);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn red_when_no_future_passes() {
        let v = serde_json::json!({
            "above": [
                { "rise_time": 1000 - 100 }
            ]
        });
        let s = snapshot_from(&v, 1000);
        assert_eq!(s.sentinel, Sentinel::Red);
    }

    #[test]
    fn missing_above_returns_error_snapshot() {
        let v = serde_json::json!({"members": []});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }
}