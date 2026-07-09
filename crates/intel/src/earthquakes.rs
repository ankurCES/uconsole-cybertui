//! Earthquakes layer — USGS earthquake GeoJSON feed.
//!
//! Mirrors the `earthquakes` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris) (MIT):
//! USGS publishes a rolling 24h GeoJSON feed of every M2.5+ quake
//! worldwide. We pull the same feed and report counts + the worst
//! magnitude in the last 24h. Sentinel severity:
//!
//!   * `Green`  — last hour saw nothing ≥ M2.5
//!   * `Yellow` — at least one M2.5+ in the last 24h
//!   * `Red`    — at least one M5.0+ in the last 24h
//!
//! No upstream authentication. Free, keyless, no documented rate limit.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

/// One feature row in the USGS GeoJSON `features` array. We only decode
/// the fields the snapshot actually needs (`mag`, `place`, `time`);
/// `geometry.coordinates` is positional but we drop the geometry here —
/// the screen doesn't render maps, just counts and severity.
#[derive(Debug, Clone, Deserialize)]
pub struct QuakeFeature {
    pub properties: QuakeProperties,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuakeProperties {
    #[serde(default)]
    pub mag: Option<f64>,
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub time: Option<i64>,
}

/// What our layer module returns after parsing the upstream body.
#[derive(Debug, Clone)]
pub struct ParsedQuakes {
    pub count: u64,
    pub worst_mag: f64,
}

/// Parse the USGS GeoJSON `FeatureCollection` body. The shape is:
/// `{ "type": "FeatureCollection", "features": [ {...}, ... ] }`.
///
/// Each feature carries its `mag` in `properties.mag`. Missing mag is
/// treated as M0 (so it counts but doesn't trip the sentinel). USGS has
/// been known to drop `properties.mag` to `null` for "deleted event"
/// placeholders, so we tolerate it.
pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedQuakes> {
    let Some(features) = body.get("features").and_then(|v| v.as_array()) else {
        anyhow::bail!("usgs: missing `features` array");
    };
    let mut count = 0u64;
    let mut worst: f64 = 0.0;
    for row in features {
        count += 1;
        if let Ok(f) = serde_json::from_value::<QuakeFeature>(row.clone()) {
            if let Some(m) = f.properties.mag {
                if m > worst {
                    worst = m;
                }
            }
        }
    }
    Ok(ParsedQuakes {
        count,
        worst_mag: worst,
    })
}

/// Build a `Snapshot` from an upstream body — used by the refiller and
/// by the M4 hardcoded fixture path. `last_ok_unix` is wall-clock at
/// fetch time.
pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let sentinel = if p.worst_mag >= 5.0 {
                Sentinel::Red
            } else if p.worst_mag >= 2.5 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Earthquakes,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!("USGS · M{:.1}+ in last 24h", p.worst_mag),
                entity_count: p.count,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Earthquakes, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "type": "FeatureCollection",
      "features": [
        { "properties": { "mag": 4.7, "place": "10km S of Eureka", "time": 1700000000000 } },
        { "properties": { "mag": 5.3, "place": "100km W of Tokyo",  "time": 1700003600000 } },
        { "properties": { "mag": 2.9, "place": "50km N of Lima",    "time": 1700007200000 } }
      ]
    }"#;

    #[test]
    fn parses_count_and_worst_mag() {
        let v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let p = parse(&v).unwrap();
        assert_eq!(p.count, 3);
        assert!((p.worst_mag - 5.3).abs() < 1e-6);
    }

    #[test]
    fn red_when_m5_present() {
        let v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let s = snapshot_from(&v, 1);
        assert_eq!(s.layer, LayerId::Earthquakes);
        assert_eq!(s.sentinel, Sentinel::Red);
        assert_eq!(s.entity_count, 3);
    }

    #[test]
    fn yellow_when_only_m2_to_m5() {
        let v = serde_json::json!({
            "features": [
                { "properties": { "mag": 2.7 } },
                { "properties": { "mag": 4.2 } }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn green_when_all_below_m2() {
        let v = serde_json::json!({
            "features": [
                { "properties": { "mag": 1.4 } },
                { "properties": { "mag": null } }
            ]
        });
        let s = snapshot_from(&v, 1);
        // worst_mag defaults to 0 since both entries are <2.5 / null.
        assert_eq!(s.sentinel, Sentinel::Green);
    }

    #[test]
    fn missing_features_returns_error_snapshot() {
        let v = serde_json::json!({"type": "FeatureCollection"});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }

    #[test]
    fn tolerates_missing_properties() {
        // A row that's not an object — should still bump the count
        // without panicking or downgrading the worst_mag.
        let v = serde_json::json!({
            "features": [
                { "properties": { "mag": 3.0 } },
                "garbage row",
                null
            ]
        });
        let p = parse(&v).unwrap();
        assert_eq!(p.count, 3);
        assert!((p.worst_mag - 3.0).abs() < 1e-6);
    }
}