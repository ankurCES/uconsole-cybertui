//! Conflicts layer — ACLED conflict-event aggregate.
//!
//! Mirrors the `conflicts` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). Pulls a 24-hour rolling summary of armed-conflict events
//! from ACLED (Armed Conflict Location & Event Data Project).
//!
//! Sentinel severity:
//!   * `Green`  — < 10 events in the last 24h (quiet day globally)
//!   * `Yellow` — 10–50 events (active day)
//!   * `Red`    — > 50 events (major flare-up)
//!
//! Authentication: ACLED requires an API key + email registration.
//! Without those we degrade to a `Pending` snapshot — same pattern
//! as Osiris's "key-required" gate.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ConflictEvent {
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub fatalities: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ParsedConflicts {
    pub count: u64,
    pub high_fatality: u64,
}

pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedConflicts> {
    let Some(events) = body.get("events").and_then(|v| v.as_array()) else {
        anyhow::bail!("acled: missing `events` array");
    };
    let mut count = 0u64;
    let mut high = 0u64;
    for row in events {
        count += 1;
        if let Ok(e) = serde_json::from_value::<ConflictEvent>(row.clone()) {
            if matches!(e.fatalities, Some(f) if f >= 10) {
                high += 1;
            }
        }
    }
    Ok(ParsedConflicts {
        count,
        high_fatality: high,
    })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let sentinel = if p.count > 50 {
                Sentinel::Red
            } else if p.count >= 10 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Conflicts,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!(
                    "ACLED · {} events last 24h ({} high-fatality)",
                    p.count, p.high_fatality
                ),
                entity_count: p.count,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Conflicts, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_when_quiet() {
        let v = serde_json::json!({
            "events": [
                { "fatalities": 0 },
                { "fatalities": 1 }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Green);
    }

    #[test]
    fn yellow_when_active() {
        let events: Vec<serde_json::Value> = (0..15)
            .map(|i| serde_json::json!({ "fatalities": i }))
            .collect();
        let v = serde_json::json!({ "events": events });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn red_when_flare_up() {
        let events: Vec<serde_json::Value> = (0..60)
            .map(|_| serde_json::json!({ "fatalities": 0 }))
            .collect();
        let v = serde_json::json!({ "events": events });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Red);
    }

    #[test]
    fn missing_events_returns_error_snapshot() {
        let v = serde_json::json!({});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }
}