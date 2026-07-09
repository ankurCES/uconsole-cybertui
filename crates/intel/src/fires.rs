//! Fires layer — NASA FIRMS hotspots (MODIS / VIIRS).
//!
//! Mirrors the `fires` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris) (MIT).
//! We don't carry FIRMS's CSV mirror (it needs a MAP_KEY) — instead
//! we proxy a small subset of public FIRMS data via a keyless
//! GeoJSON feed. For the kill screen's purposes, we just count
//! hotspots and report the worst "confidence" value.
//!
//! Sentinel severity:
//!   * `Green`  — no high-confidence hotspots
//!   * `Yellow` — at least one high-confidence (≥ 80%) hotspot
//!   * `Red`    — at least one extreme-intensity (≥ 4) hotspot
//!
//! Note: real FIRMS requires a MAP_KEY. Without one, the upstream
//! returns 401. We fall back to a `Pending` snapshot with an error
//! reason so the refiller keeps ticking.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct FireFeature {
    #[serde(default)]
    pub intensity: Option<f64>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ParsedFires {
    pub count: u64,
    pub worst_intensity: f64,
    pub high_confidence: u64,
}

pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedFires> {
    let Some(features) = body.get("features").and_then(|v| v.as_array()) else {
        anyhow::bail!("firms: missing `features` array");
    };
    let mut count = 0u64;
    let mut worst_i: f64 = 0.0;
    let mut high_conf: u64 = 0;
    for row in features {
        count += 1;
        // FIRMS rows have `properties` as either a Map or a flat
        // array of scalars. We tolerate both via per-row decoding.
        if let Some(props) = row.get("properties") {
            if let Ok(f) = serde_json::from_value::<FireFeature>(props.clone()) {
                if let Some(i) = f.intensity {
                    if i > worst_i {
                        worst_i = i;
                    }
                }
                if matches!(f.confidence, Some(c) if c >= 80.0) {
                    high_conf += 1;
                }
            }
        }
    }
    Ok(ParsedFires {
        count,
        worst_intensity: worst_i,
        high_confidence: high_conf,
    })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let sentinel = if p.worst_intensity >= 4.0 {
                Sentinel::Red
            } else if p.high_confidence > 0 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Fires,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!(
                    "FIRMS · {} hotspots, {} high-confidence",
                    p.count, p.high_confidence
                ),
                entity_count: p.count,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Fires, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_when_intensity_4() {
        let v = serde_json::json!({
            "features": [
                { "properties": { "intensity": 4.5, "confidence": 95.0 } },
                { "properties": { "intensity": 2.0, "confidence": 50.0 } }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Red);
        assert_eq!(s.entity_count, 2);
    }

    #[test]
    fn yellow_when_high_confidence() {
        let v = serde_json::json!({
            "features": [
                { "properties": { "intensity": 2.0, "confidence": 85.0 } }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn green_when_only_low_confidence() {
        let v = serde_json::json!({
            "features": [
                { "properties": { "intensity": 1.5, "confidence": 30.0 } }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Green);
    }

    #[test]
    fn missing_features_returns_error_snapshot() {
        let v = serde_json::json!({});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }
}