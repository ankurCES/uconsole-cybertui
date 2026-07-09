//! Flights layer — live OpenSky `/states/all`.
//!
//! Mirrors the `flights` layer in [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT): one bbox-bounded fetch returns the live state vectors as a
//! 2-element JSON array `[now, [states]]`. We re-shape to
//! `{count, items: [...]}`. State arrays are positional per OpenSky
//! docs, so we explicitly name fields rather than rely on order.
//!
//! Authentication: optional OAuth2 (client id/secret) since the
//! March 2025 change. Both env keys are honoured; absent = anonymous
//! tier (lower rate limit but keyless).
//!
//! This file is M1's minimum-viable shape — the parse + snapshot
//! contract only. The HTTP fetch is wired in M5. Keeping parse
//! separate from fetch is the same pattern Osiris uses
//! (`src/lib/osint-utils.ts`).

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

/// State-vector positional fields per
/// <https://opensky-network.org/apidoc/rest.html#all-aircraft-states>.
/// `index 0 = icao24`, the rest as listed there. We don't use all 17
/// columns — only the ones the TUI summary needs.
#[derive(Debug, Clone, Deserialize)]
pub struct State {
    pub icao24: String,
    pub callsign: Option<String>,
    pub origin_country: String,
    pub time_position: Option<i64>,
    pub longitude: Option<f64>,
    pub latitude: Option<f64>,
    pub baro_altitude: Option<f64>,
    pub on_ground: bool,
    pub velocity: Option<f64>,
    pub true_track: Option<f64>,
    pub vertical_rate: Option<f64>,
}

/// What our layer module returns after parsing the upstream body.
#[derive(Debug, Clone)]
pub struct ParsedFlights {
    pub count: u64,
    pub nearest: Vec<State>,
}

/// Parse the OpenSky `/states/all` response. The shape is:
/// `{ "time": <unix>, "states": [[...], ...] }` — the inner arrays
/// are positional, so we decode into `Vec<Vec<serde_json::Value>>`
/// first and then map each row into our `State` struct by column
/// index. This keeps us robust to OpenSky adding columns at the tail.
///
/// No upstream call — this is M1's pure parser, called directly
/// from tests and from the M5 refiller.
pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedFlights> {
    let Some(arr) = body.get("states").and_then(|v| v.as_array()) else {
        anyhow::bail!("opensky: missing `states` array");
    };

    let mut nearest: Vec<State> = Vec::with_capacity(arr.len().min(5));
    let mut count = 0u64;
    for row in arr {
        count += 1;
        if nearest.len() < 5 {
            // Don't bail on a malformed row — skip and count the
            // rest. OpenSky has been known to insert `null` rows for
            // aircraft that briefly lose their transponder.
            if let Ok(s) = decode_state_row(row) {
                if !s.on_ground {
                    nearest.push(s);
                }
            }
        }
    }
    // Keep the closest 5 — without a user bbox we just take the
    // first 5 we encounter; the refiller passes a bbox-filtered body
    // in M5 so this only kicks in for the "no bbox" fallback.
    nearest.truncate(5);
    Ok(ParsedFlights { count, nearest })
}

fn decode_state_row(row: &serde_json::Value) -> anyhow::Result<State> {
    // Positional columns per OpenSky docs. We tolerate extra columns
    // past index 16 by ignoring them.
    //
    // Decoding rules:
    //   * `icao24` (column 0) must be a non-empty string. Rows with
    //     a missing icao24 (e.g., `["bad-row"]`) are intentionally
    //     rejected so a truncated upstream payload doesn't show up as
    //     an anonymous phantom aircraft in the UI.
    //   * `on_ground` (column 8) is treated as "unknown = on ground"
    //     when null/missing. Defaulting to `false` (airborne) made a
    //     truncated row look like an aircraft on final approach; the
    //     M1 test caught it.
    let a = row
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("state row is not an array"))?;
    let icao24 = a
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("state row missing icao24"))?
        .to_string();
    if icao24.is_empty() {
        anyhow::bail!("state row has empty icao24");
    }
    let get = |i: usize| a.get(i).cloned().unwrap_or(serde_json::Value::Null);
    Ok(State {
        icao24,
        callsign: get(1).as_str().map(|s| s.trim().to_string()),
        origin_country: get(2).as_str().unwrap_or("").to_string(),
        time_position: get(3).as_i64(),
        longitude: get(5).as_f64(),
        latitude: get(6).as_f64(),
        baro_altitude: get(7).as_f64(),
        on_ground: get(8).as_bool().unwrap_or(true),
        velocity: get(9).as_f64(),
        true_track: get(10).as_f64(),
        vertical_rate: get(11).as_f64(),
    })
}

/// Build a `Snapshot` from an upstream body — used by the refiller in
/// M5 and by the hardcoded snapshot in the Intel screen's M4 tests.
///
/// `last_ok_unix` is the wall-clock at the moment of fetch. We
/// compute the sentinel from the raw data: any aircraft with
/// `baro_altitude < 100 m && velocity > 50` while in the bbox is a
/// candidate `YELLOW` (low and fast = landing approach near an
/// airport). M5 will refine this against the user's bbox.
pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => Snapshot {
            layer: LayerId::Flights,
            status: LayerStatus::Ok { last_ok_unix },
            sentinel: if p.nearest.iter().any(low_and_fast) {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            },
            summary: format!("{} aircraft in bbox", p.count),
            entity_count: p.count,
            raw: body.clone(),
        },
        Err(e) => Snapshot::error(LayerId::Flights, None, e.to_string()),
    }
}

fn low_and_fast(s: &State) -> bool {
    matches!(s.baro_altitude, Some(a) if a < 100.0)
        && matches!(s.velocity, Some(v) if v > 50.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "time": 1700000000,
      "states": [
        ["abc123", "UAL123  ", "United States", 1700000000, 1700000000,
         -122.41, 37.62, 1525.0, false, 240.5, 90.0, 0.0,
         null, null, null, null, null],
        ["def456", "ACA456  ", "Canada", 1700000000, 1700000000,
         -122.41, 37.62, 50.0, false, 80.0, 90.0, -1.5,
         null, null, null, null, null],
        ["ghi789", null,        "Mexico",  1700000000, 1700000000,
         -99.13, 19.43, 11000.0, false, 250.0, 180.0, 0.0,
         null, null, null, null, null]
      ]
    }"#;

    #[test]
    fn parses_count_and_nearest() {
        let v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let p = parse(&v).unwrap();
        // SAMPLE has 3 rows, all airborne (on_ground=false), so all
        // decode to the `nearest` short-list (cap = 5).
        assert_eq!(p.count, 3);
        assert_eq!(p.nearest.len(), 3);
        assert!(p.nearest.iter().any(low_and_fast));
    }

    #[test]
    fn snapshot_is_yellow_when_lowfast_present() {
        let v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let s = snapshot_from(&v, 1);
        assert_eq!(s.layer, LayerId::Flights);
        assert_eq!(s.sentinel, Sentinel::Yellow);
        assert_eq!(s.entity_count, 3);
        assert!(s.summary.contains("3 aircraft"));
    }

    #[test]
    fn missing_states_array_returns_error_snapshot() {
        let v: serde_json::Value = serde_json::json!({"time": 1});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }

    #[test]
    fn tolerates_null_rows_and_extra_columns() {
        let v: serde_json::Value = serde_json::json!({
            "states": [
                serde_json::Value::Null,
                ["aaa111", "X", "T", 1, 1, 0.0, 0.0, 1000.0, false, 200.0, 0.0, 0.0,
                 null, null, null, null, null, "EXTRA_COLUMN"],
                ["bad-row"]
            ]
        });
        let p = parse(&v).unwrap();
        assert_eq!(p.count, 3);
        // Only one row decoded; the other two were skipped.
        assert_eq!(p.nearest.len(), 1);
    }
}
