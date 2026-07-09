//! Weather layer — Open-Meteo current conditions.
//!
//! Mirrors the `weather` layer in
//! [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). We hit Open-Meteo's `current_weather=true` endpoint with
//! lat/lon (defaults to Seattle if the user hasn't configured a
//! city). No API key required.
//!
//! Sentinel severity:
//!   * `Green`  — wind < 50 km/h, temp in [-10, 35] °C
//!   * `Yellow` — wind 50–80 km/h, OR temp outside [-10, 35]
//!   * `Red`    — wind ≥ 80 km/h
//!
//! Note: this layer is intentionally passive. The City screen has its
//! own richer Open-Meteo pull (with hourly + daily forecast). The
//! Intel screen's weather row is the "glance" view.

use super::{LayerId, LayerStatus, Sentinel, Snapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct CurrentWeather {
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub windspeed: Option<f64>,
    #[serde(default)]
    pub weathercode: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ParsedWeather {
    pub temp_c: f64,
    pub wind_kmh: f64,
}

pub fn parse(body: &serde_json::Value) -> anyhow::Result<ParsedWeather> {
    let Some(cw) = body.get("current_weather") else {
        anyhow::bail!("open-meteo: missing `current_weather`");
    };
    let cw: CurrentWeather = serde_json::from_value(cw.clone())?;
    Ok(ParsedWeather {
        temp_c: cw.temperature.unwrap_or(0.0),
        wind_kmh: cw.windspeed.unwrap_or(0.0),
    })
}

pub fn snapshot_from(body: &serde_json::Value, last_ok_unix: i64) -> Snapshot {
    match parse(body) {
        Ok(p) => {
            let sentinel = if p.wind_kmh >= 80.0 {
                Sentinel::Red
            } else if p.wind_kmh >= 50.0 || p.temp_c < -10.0 || p.temp_c > 35.0 {
                Sentinel::Yellow
            } else {
                Sentinel::Green
            };
            Snapshot {
                layer: LayerId::Weather,
                status: LayerStatus::Ok { last_ok_unix },
                sentinel,
                summary: format!("Open-Meteo · {:.0}°C · {:.0} km/h wind", p.temp_c, p.wind_kmh),
                entity_count: 1,
                raw: body.clone(),
            }
        }
        Err(e) => Snapshot::error(LayerId::Weather, None, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_for_calm_mild() {
        let v = serde_json::json!({
            "current_weather": { "temperature": 18.0, "windspeed": 12.0 }
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Green);
        assert!(s.summary.contains("18"));
    }

    #[test]
    fn red_for_hurricane_wind() {
        let v = serde_json::json!({
            "current_weather": { "temperature": 25.0, "windspeed": 120.0 }
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Red);
    }

    #[test]
    fn yellow_for_extreme_heat() {
        let v = serde_json::json!({
            "current_weather": { "temperature": 40.0, "windspeed": 5.0 }
        });
        let s = snapshot_from(&v, 1);
        assert_eq!(s.sentinel, Sentinel::Yellow);
    }

    #[test]
    fn missing_current_weather_returns_error_snapshot() {
        let v = serde_json::json!({"hourly": {}});
        let s = snapshot_from(&v, 1);
        assert!(matches!(s.status, LayerStatus::Error { .. }));
    }
}