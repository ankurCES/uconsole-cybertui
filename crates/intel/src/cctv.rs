//! CCTV layer — traffic-cam / public webcam aggregate.
//!
//! Mirrors the `cctv` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). We pull from a small curated list of public traffic-cam
//! directories (Caltrans, NYC DOT, Transport for London open data).
//! The "feed" we're tracking is a per-camera health check — does
//! the camera respond within a 2s timeout?
//!
//! Sentinel severity:
//!   * `Green` — every checked stream is responsive
//!   * `Yellow` — at least one stream timed out but > 50% are responsive
//!   * `Red`    — majority of streams timed out
//!
//! Authentication: none. The feeds we touch are all public.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct CctvFeed {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub responsive: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ParsedCctv {
    pub total: u64,
    pub responsive: u64,
}

pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedCctv> {
    let Some(feeds) = body.get("feeds").and_then(|v| v.as_array()) else {
        anyhow::bail!("cctv: missing `feeds` array");
    };
    let mut total = 0u64;
    let mut ok = 0u64;
    for row in feeds {
        total += 1;
        if let Ok(f) = serde_json::from_value::<CctvFeed>(row.clone()) {
            if matches!(f.responsive, Some(true)) {
                ok += 1;
            }
        }
    }
    Ok(ParsedCctv {
        total,
        responsive: ok,
    })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let degraded = p.total.saturating_sub(p.responsive);
            let half = p.total / 2;
            let sentinel = if p.total == 0 {
                Sentinel::Green
            } else if degraded > half {
                Sentinel::Red
            } else if degraded > 0 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Cctv,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!(
                    "{} of {} streams responsive",
                    p.responsive, p.total
                ),
                entity_count: p.total,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Cctv, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_when_all_responsive() {
        let v = serde_json::json!({
            "feeds": [
                { "id": "a", "responsive": true },
                { "id": "b", "responsive": true }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Green);
    }

    #[test]
    fn yellow_when_one_timeout() {
        let v = serde_json::json!({
            "feeds": [
                { "id": "a", "responsive": true },
                { "id": "b", "responsive": false },
                { "id": "c", "responsive": true }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn red_when_majority_timeout() {
        let v = serde_json::json!({
            "feeds": [
                { "id": "a", "responsive": false },
                { "id": "b", "responsive": false },
                { "id": "c", "responsive": true }
            ]
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Red);
    }

    #[test]
    fn missing_feeds_returns_error_snapshot() {
        let v = serde_json::json!({});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }
}