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

use serde::{Deserialize, Serialize};

use super::roads::{Polyline, RoadImportance};

const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
const USER_AGENT: &str = concat!("cyberdeck-tui/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(30);

// ── Public types for the enriched city data ──────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoiKind { FireStation, Police, Hospital }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AreaKind { Park, Forest, Water }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Poi { pub lat: f64, pub lon: f64, pub kind: PoiKind }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area { pub points: Vec<[f64; 2]>, pub kind: AreaKind }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CityData {
    pub roads: Vec<Polyline>,
    pub pois: Vec<Poi>,
    pub areas: Vec<Area>,
}

// ── Overpass response types ──────────────────────────────────────

#[derive(Deserialize)]
struct OverpassResp {
    elements: Vec<Element>,
}

#[derive(Deserialize)]
struct Element {
    #[serde(rename = "type")]
    elem_type: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    tags: Option<Tags>,
    geometry: Option<Vec<Pt>>,
}

#[derive(Deserialize)]
struct Tags {
    highway: Option<String>,
    amenity: Option<String>,
    leisure: Option<String>,
    landuse: Option<String>,
    natural: Option<String>,
    waterway: Option<String>,
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

/// Fetch roads, POIs, and area polygons in a single Overpass query.
pub async fn fetch_city_data(bbox: [f64; 4]) -> anyhow::Result<CityData> {
    let [min_lat, min_lon, max_lat, max_lon] = bbox;
    let bb = format!("{min_lat},{min_lon},{max_lat},{max_lon}");
    let query = format!(
        "[out:json][timeout:25];(\
         way[\"highway\"~\"^(motorway|trunk|primary|secondary|tertiary|residential)$\"]({bb});\
         node[\"amenity\"~\"^(fire_station|police|hospital|clinic)$\"]({bb});\
         way[\"leisure\"=\"park\"]({bb});\
         way[\"landuse\"~\"^(forest|wood)$\"]({bb});\
         way[\"natural\"=\"water\"]({bb});\
         way[\"waterway\"~\"^(river|stream|canal)$\"]({bb});\
         );out geom;"
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

    let mut data = CityData::default();

    for e in resp.elements {
        let tags = match &e.tags {
            Some(t) => t,
            None => continue,
        };

        // POI nodes
        if e.elem_type.as_deref() == Some("node") {
            if let (Some(lat), Some(lon)) = (e.lat, e.lon) {
                let kind = match tags.amenity.as_deref() {
                    Some("fire_station") => Some(PoiKind::FireStation),
                    Some("police") => Some(PoiKind::Police),
                    Some("hospital" | "clinic") => Some(PoiKind::Hospital),
                    _ => None,
                };
                if let Some(kind) = kind {
                    data.pois.push(Poi { lat, lon, kind });
                }
            }
            continue;
        }

        // Ways — roads or areas
        let geom = match &e.geometry {
            Some(g) if g.len() >= 2 => g,
            _ => continue,
        };
        let points: Vec<[f64; 2]> = geom.iter().map(|p| [p.lat, p.lon]).collect();

        if let Some(ref hw) = tags.highway {
            data.roads.push(Polyline {
                points,
                importance: RoadImportance(hw.clone()),
            });
            continue;
        }

        // Area classification
        let area_kind = if tags.leisure.as_deref() == Some("park") {
            Some(AreaKind::Park)
        } else if matches!(tags.landuse.as_deref(), Some("forest" | "wood")) {
            Some(AreaKind::Forest)
        } else if tags.natural.as_deref() == Some("water")
            || tags.waterway.is_some()
        {
            Some(AreaKind::Water)
        } else {
            None
        };
        if let Some(kind) = area_kind {
            data.areas.push(Area { points, kind });
        }
    }

    Ok(data)
}
