//! Overpass API road fetcher.
//!
//! Fetches highway geometry for a bbox and returns it as the same
//! `Vec<Polyline>` format the braille renderer already understands.
//! No new dependencies — uses the `reqwest` client already pulled in
//! by the `http` feature.
//!
//! Rate limits: Overpass allows ~2 req/s per IP; we call this at most
//! once per geo-location resolution (startup + 10-minute tick), so we
//! stay well within the limit.

use std::time::Duration;

use serde::Deserialize;

use super::roads::{Polyline, RoadImportance};

const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
const USER_AGENT: &str = concat!("cyberdeck-tui/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(30);

/// Overpass QL response (partial — only what we consume).
#[derive(Deserialize)]
struct OverpassResp {
    elements: Vec<Element>,
}

#[derive(Deserialize)]
struct Element {
    tags: Option<Tags>,
    geometry: Option<Vec<Pt>>,
}

#[derive(Deserialize)]
struct Tags {
    highway: Option<String>,
}

#[derive(Deserialize)]
struct Pt {
    lat: f64,
    lon: f64,
}

/// Fetch road polylines for `bbox = [min_lat, min_lon, max_lat, max_lon]`.
/// Returns an empty Vec on any error (graceful degradation to bundled data).
pub async fn fetch_roads(bbox: [f64; 4]) -> Result<Vec<Polyline>, reqwest::Error> {
    let [min_lat, min_lon, max_lat, max_lon] = bbox;
    // Query: motorway → tertiary + residential so the map shows both
    // arterials (thick strokes) and local streets (thin strokes).
    let query = format!(
        "[out:json][timeout:25];\
         way[\"highway\"~\"^(motorway|trunk|primary|secondary|tertiary|residential)$\"]\
         ({min_lat},{min_lon},{max_lat},{max_lon});\
         out geom;"
    );
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .build()?;
    let resp: OverpassResp = client
        .post(OVERPASS_URL)
        .body(query)
        .send()
        .await?
        .json()
        .await?;

    let roads = resp
        .elements
        .into_iter()
        .filter_map(|e| {
            let geom = e.geometry?;
            let highway = e.tags?.highway?;
            let points: Vec<[f64; 2]> = geom.into_iter().map(|p| [p.lat, p.lon]).collect();
            if points.len() < 2 {
                return None;
            }
            Some(Polyline {
                points,
                importance: RoadImportance(highway),
            })
        })
        .collect();

    Ok(roads)
}
