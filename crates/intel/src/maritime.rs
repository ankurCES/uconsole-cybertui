//! Maritime layer — AIS vessel positions in the user's bounding box.
//!
//! Mirrors the `maritime` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). Pulls a position list from AIS Hub (or, in the offline
//! fixture, from a bundled JSON) and reports the count.
//!
//! Sentinel severity:
//!   * `Green`  — vessels present, no flags
//!   * `Yellow` — at least one flagged vessel (sanctioned, dark, etc.)
//!   * `Red`    — 5+ flagged vessels (likely an active incident)
//!
//! Authentication: AIS Hub requires an API key. We degrade gracefully
//! to `Pending` snapshot with an "API key required" reason — same
//! pattern as Osiris's "key-required" gate.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Vessel {
    #[serde(default)]
    pub mmsi: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub flagged: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ParsedMaritime {
    pub count: u64,
    pub flagged: u64,
}

pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedMaritime> {
    let Some(vessels) = body.get("vessels").and_then(|v| v.as_array()) else {
        anyhow::bail!("ais: missing `vessels` array");
    };
    let mut count = 0u64;
    let mut flagged = 0u64;
    for row in vessels {
        count += 1;
        if let Ok(v) = serde_json::from_value::<Vessel>(row.clone()) {
            if matches!(v.flagged, Some(true)) {
                flagged += 1;
            }
        }
    }
    Ok(ParsedMaritime { count, flagged })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let sentinel = if p.flagged >= 5 {
                Sentinel::Red
            } else if p.flagged > 0 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Maritime,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!(
                    "AIS · {} vessels, {} flagged",
                    p.count, p.flagged
                ),
                entity_count: p.count,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Maritime, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_when_no_flags() {
        let v = serde_json::json!({
            "vessels": [
                { "mmsi": "1", "flagged": false },
                { "mmsi": "2", "flagged": false }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Green);
    }

    #[test]
    fn yellow_when_one_flag() {
        let v = serde_json::json!({
            "vessels": [
                { "mmsi": "1", "flagged": true },
                { "mmsi": "2", "flagged": false }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn red_when_five_plus_flags() {
        let flagged: Vec<serde_json::Value> = (0..6)
            .map(|i| serde_json::json!({ "mmsi": format!("{i}"), "flagged": true }))
            .collect();
        let v = serde_json::json!({ "vessels": flagged });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Red);
    }

    #[test]
    fn missing_vessels_returns_error_snapshot() {
        let v = serde_json::json!({});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }
}