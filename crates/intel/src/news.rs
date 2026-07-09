//! News layer — GDELT 5-minute rolling event count.
//!
//! Mirrors the `news` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). GDELT publishes a 5-minute update of every news event
//! their pipeline catches. We pull the count for a chosen topic
//! (default: world + tech) and surface it.
//!
//! Sentinel severity:
//!   * `Green` — < 200 mentions in the last 5 min
//!   * `Yellow` — 200–500 mentions (trending topic)
//!   * `Red`    — > 500 mentions (major event in progress)
//!
//! Authentication: none. GDELT is fully public, generous rate limits.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct GdeltResponse {
    #[serde(default)]
    pub articles: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ParsedNews {
    pub count: u64,
}

pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedNews> {
    // GDELT 2.0 doc API returns either a bare list or
    // `{ "articles": [...] }`. We tolerate both shapes.
    if let Some(arr) = body.as_array() {
        return Ok(ParsedNews { count: arr.len() as u64 });
    }
    let resp: GdeltResponse = serde_json::from_value(body.clone())?;
    Ok(ParsedNews {
        count: resp.articles.len() as u64,
    })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let sentinel = if p.count > 500 {
                Sentinel::Red
            } else if p.count > 200 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::News,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!("GDELT · {} mentions last 5 min", p.count),
                entity_count: p.count,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::News, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_when_low_volume() {
        let v = serde_json::json!({ "articles": [{}, {}, {}] });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Green);
        assert_eq!(s.entity_count, 3);
    }

    #[test]
    fn yellow_when_trending() {
        let arts: Vec<serde_json::Value> =
            (0..250).map(|_| serde_json::json!({})).collect();
        let v = serde_json::json!({ "articles": arts });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn red_when_major_event() {
        let arts: Vec<serde_json::Value> =
            (0..600).map(|_| serde_json::json!({})).collect();
        let v = serde_json::json!({ "articles": arts });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Red);
    }

    #[test]
    fn bare_array_also_works() {
        let v = serde_json::json!([{}, {}, {}, {}]);
        let s = snapshot_from(&v, 1);
        assert_eq!(s.entity_count, 4);
    }
}