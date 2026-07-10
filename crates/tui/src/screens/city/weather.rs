//! Open-Meteo weather client.
//!
//! Free, no key, HTTPS, no documented rate limit.
//!
//! Docs: <https://open-meteo.com/en/docs>
//!
//! We fetch `current=temperature_2m,wind_speed_10m,wind_direction_10m,
//! weather_code,relative_humidity_2m,apparent_temperature` plus the
//! next-12h `hourly=precipitation_probability` so the right pane
//! can show a small sparkline.

use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

use super::geo::CityLocation;

/// Re-export the canonical `Weather` type from `cyberdeck-core` so the
/// TUI, CLI, and (future) web layer share one schema. The Open-Meteo
/// client deserializes straight into this struct.
pub use cyberdeck_core::city::Weather;

#[derive(Debug, thiserror::Error)]
pub enum WeatherError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),
    #[error("open-meteo returned an unexpected payload: {0}")]
    BadPayload(String),
}

/// Open-Meteo base URL. All endpoints live under `/v1/forecast`.
const OPEN_METEO_URL: &str = "https://api.open-meteo.com/v1/forecast";

/// User-Agent. Open-Meteo's free tier doesn't gate on UA but their
/// dashboard logs it, which helps when debugging "why is my city
/// returning no data" reports.
const USER_AGENT: &str = concat!("cyberdeck-tui/", env!("CARGO_PKG_VERSION"));

/// Request timeout. Open-Meteo typically responds in <300ms; 5s is
/// generous headroom.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How many hourly precipitation slots to surface for the right-pane
/// sparkline. 12h gives the user a useful "is it going to rain later?"
/// signal without consuming a lot of vertical space.
const HOURLY_PRECIP_HOURS: usize = 12;

/// Internal Open-Meteo response shape. Open-Meteo returns a flat
/// JSON object; we only model the fields we use. `current` is an
/// inline object; `hourly` is an object with parallel arrays (the
/// canonical Open-Meteo shape — time-indexed rather than per-row).
#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    current: Option<OpenMeteoCurrent>,
    hourly: Option<OpenMeteoHourly>,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoCurrent {
    #[serde(rename = "temperature_2m")]
    temperature_2m: Option<f32>,
    #[serde(rename = "apparent_temperature")]
    apparent_temperature: Option<f32>,
    #[serde(rename = "relative_humidity_2m")]
    relative_humidity_2m: Option<u8>,
    #[serde(rename = "wind_speed_10m")]
    wind_speed_10m: Option<f32>,
    #[serde(rename = "wind_direction_10m")]
    wind_direction_10m: Option<u16>,
    #[serde(rename = "weather_code")]
    weather_code: Option<u8>,
    #[serde(default = "default_is_day")]
    is_day: u8,
}

fn default_is_day() -> u8 {
    1
}

#[derive(Debug, Deserialize)]
struct OpenMeteoHourly {
    #[serde(rename = "precipitation_probability", default)]
    precipitation_probability: Vec<Option<u8>>,
}

pub struct FetchResult {
    pub weather: Weather,
    pub is_day: bool,
}

impl OpenMeteoResponse {
    fn into_result(self) -> Result<FetchResult, WeatherError> {
        let current = self
            .current
            .ok_or_else(|| WeatherError::BadPayload("missing `current` block".into()))?;
        // Required fields. Open-Meteo may omit `apparent_temperature`
        // for some coordinates; fall back to the air temperature in
        // that case so the right pane always shows a value.
        let temp_c = current
            .temperature_2m
            .ok_or_else(|| WeatherError::BadPayload("missing temperature_2m".into()))?;
        let feels_like_c = current.apparent_temperature.unwrap_or(temp_c);
        let humidity_pct = current.relative_humidity_2m.unwrap_or(0);
        let wind_kph = current.wind_speed_10m.unwrap_or(0.0);
        let wind_dir_deg = current.wind_direction_10m;
        let weather_code = current.weather_code.unwrap_or(0);
        let is_day = current.is_day != 0;

        // Hourly precipitation: cap at HOURLY_PRECIP_HOURS slots,
        // skipping `None` entries. Open-Meteo occasionally returns
        // gaps in the array; we don't want a `null` in the
        // sparkline.
        let next_12h_precip_pct = self.hourly.and_then(|h| {
            let v: Vec<u8> = h
                .precipitation_probability
                .into_iter()
                .take(HOURLY_PRECIP_HOURS)
                .flatten()
                .collect();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        });

        Ok(FetchResult {
            weather: Weather {
                temp_c,
                feels_like_c,
                humidity_pct,
                wind_kph,
                wind_dir_deg,
                weather_code,
                next_12h_precip_pct,
                fetched_at: chrono::Local::now(),
            },
            is_day,
        })
    }
}

/// Fetch current weather for `loc`. One-shot — the dispatcher
/// re-fires this on the 10-minute tick and on user-driven `r`.
pub async fn fetch(loc: &CityLocation) -> Result<FetchResult, WeatherError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let resp: OpenMeteoResponse = client
        .get(OPEN_METEO_URL)
        .query(&[
            ("latitude", loc.lat.to_string()),
            ("longitude", loc.lon.to_string()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,\
                 wind_speed_10m,wind_direction_10m,weather_code,is_day"
                    .to_string(),
            ),
            ("hourly", "precipitation_probability".to_string()),
            ("forecast_hours", HOURLY_PRECIP_HOURS.to_string()),
            ("timezone", "auto".to_string()),
        ])
        .send()
        .await?
        .json()
        .await?;
    resp.into_result()
}

/// WMO weather code → human label. Re-exported from `cyberdeck-core`
/// so the TUI and CLI share one mapping. See
/// `cyberdeck_core::city::weather_code_label` for the canonical impl.
pub use cyberdeck_core::city::weather_code_label as weather_label;

fn styled(s: &str, color: Color) -> Span<'static> {
    Span::styled(s.to_string(), Style::default().fg(color))
}

/// 4-line ASCII art icon for a WMO weather code + day/night flag.
pub fn weather_icon(wmo: u8, is_day: bool) -> Vec<Line<'static>> {
    match wmo {
        0 if is_day => vec![
            Line::from(styled("    \\   / ", Color::Yellow)),
            Line::from(styled("     .-.  ", Color::Yellow)),
            Line::from(styled("  -(   )- ", Color::Yellow)),
            Line::from(styled("     `-'  ", Color::Yellow)),
        ],
        0 => vec![
            Line::from(styled("     .-.  ", Color::Blue)),
            Line::from(styled("    (   ) ", Color::Blue)),
            Line::from(styled("     `-'  ", Color::Blue)),
            Line::from(styled("    *  *  ", Color::Blue)),
        ],
        1..=2 if is_day => vec![
            Line::from(vec![styled(" \\  /", Color::Yellow), styled("     ", Color::Gray)]),
            Line::from(vec![styled("  .--", Color::Yellow), styled("--. ", Color::Gray)]),
            Line::from(vec![styled("-(   ", Color::Yellow), styled("  ) ", Color::Gray)]),
            Line::from(styled("  `----' ", Color::Gray)),
        ],
        1..=2 => vec![
            Line::from(vec![styled("  .-", Color::Blue), styled("---.  ", Color::Gray)]),
            Line::from(vec![styled(" (  ", Color::Blue), styled("   )  ", Color::Gray)]),
            Line::from(styled("  `---'   ", Color::Gray)),
            Line::from(styled("          ", Color::DarkGray)),
        ],
        3 => vec![
            Line::from(styled("  .-----. ", Color::Gray)),
            Line::from(styled(" (       )", Color::Gray)),
            Line::from(styled("  `-----' ", Color::Gray)),
            Line::from(styled(" overcast ", Color::DarkGray)),
        ],
        45 | 48 => vec![
            Line::from(styled("_ - _ - _ ", Color::DarkGray)),
            Line::from(styled(" _ - _ -  ", Color::DarkGray)),
            Line::from(styled("_ - _ - _ ", Color::DarkGray)),
            Line::from(styled("   fog    ", Color::DarkGray)),
        ],
        51..=57 => vec![
            Line::from(styled("  .-----. ", Color::Gray)),
            Line::from(styled(" (  drz  )", Color::Gray)),
            Line::from(styled("  `-----' ", Color::Gray)),
            Line::from(styled("  ' ' ' ' ", Color::Cyan)),
        ],
        61..=67 | 80..=82 => vec![
            Line::from(styled("   .-.    ", Color::Gray)),
            Line::from(styled("  (   )   ", Color::Gray)),
            Line::from(styled(" / / / /  ", Color::Cyan)),
            Line::from(styled("/ / / /   ", Color::Cyan)),
        ],
        71..=77 | 85..=86 => vec![
            Line::from(styled("  .-----. ", Color::Gray)),
            Line::from(styled(" ( snow  )", Color::Gray)),
            Line::from(styled("  `-----' ", Color::Gray)),
            Line::from(styled(" * * * *  ", Color::White)),
        ],
        95..=99 => vec![
            Line::from(styled("  .-----. ", Color::DarkGray)),
            Line::from(styled(" (  ⚡   )", Color::Yellow)),
            Line::from(styled("  `-----' ", Color::DarkGray)),
            Line::from(styled(" /|/|/|/  ", Color::Yellow)),
        ],
        _ => vec![
            Line::from(styled("  .-----. ", Color::DarkGray)),
            Line::from(styled(" (  ???  )", Color::DarkGray)),
            Line::from(styled("  `-----' ", Color::DarkGray)),
            Line::from(styled("          ", Color::DarkGray)),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Wire shape: Open-Meteo returns `current` + `hourly` blocks.
    /// All required fields must round-trip into `Weather`.
    #[tokio::test]
    async fn fetch_parses_full_payload() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/forecast"))
            .and(query_param("latitude", "47.6062"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{
                    "current": {
                        "time": "2024-06-01T12:00",
                        "temperature_2m": 9.2,
                        "apparent_temperature": 7.4,
                        "relative_humidity_2m": 78,
                        "wind_speed_10m": 12.0,
                        "wind_direction_10m": 315,
                        "weather_code": 3
                    },
                    "hourly": {
                        "time": ["2024-06-01T13:00", "2024-06-01T14:00"],
                        "precipitation_probability": [10, 20]
                    }
                }"#,
            ))
            .mount(&server)
            .await;

        // Direct the Open-Meteo client at our mock by overriding the
        // base URL via a one-shot client. Production code uses the
        // module-level `OPEN_METEO_URL`; the test re-uses the same
        // deserialization path with a synthetic response.
        let loc = CityLocation {
            name: "Seattle".into(),
            country: "US".into(),
            country_code: "US".into(),
            region: "WA".into(),
            lat: 47.6062,
            lon: -122.3321,
            bbox: None,
            timezone: "America/Los_Angeles".into(),
        };
        let client = reqwest::Client::builder().build().unwrap();
        let resp: OpenMeteoResponse = client
            .get(format!("{}/v1/forecast", server.uri()))
            .query(&[("latitude", loc.lat.to_string())])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let w = resp.into_result().expect("valid payload").weather;
        assert!((w.temp_c - 9.2).abs() < 1e-3);
        assert!((w.feels_like_c - 7.4).abs() < 1e-3);
        assert_eq!(w.humidity_pct, 78);
        assert_eq!(w.wind_dir_deg, Some(315));
        assert_eq!(w.weather_code, 3);
        assert_eq!(
            w.next_12h_precip_pct.as_deref(),
            Some(&[10u8, 20][..])
        );
    }

    /// Defensive: a 200 without a `current` block must surface as
    /// `BadPayload`. Open-Meteo sometimes returns partial responses
    /// for marine coordinates; we shouldn't crash on those.
    #[tokio::test]
    async fn fetch_missing_current_maps_to_bad_payload() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/forecast"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"hourly": {}}"#))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let resp: OpenMeteoResponse = client
            .get(format!("{}/v1/forecast", server.uri()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(matches!(
            resp.into_result(),
            Err(WeatherError::BadPayload(_))
        ));
    }

    /// Defensive: missing `apparent_temperature` falls back to
    /// `temperature_2m` so the right pane always shows a value.
    #[tokio::test]
    async fn fetch_missing_feels_like_falls_back_to_temp() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/forecast"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{
                    "current": {
                        "time": "2024-06-01T12:00",
                        "temperature_2m": 9.2,
                        "relative_humidity_2m": 78,
                        "wind_speed_10m": 12.0,
                        "wind_direction_10m": 315,
                        "weather_code": 3
                    }
                }"#,
            ))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let resp: OpenMeteoResponse = client
            .get(format!("{}/v1/forecast", server.uri()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let w = resp.into_result().expect("valid payload").weather;
        assert!((w.feels_like_c - w.temp_c).abs() < 1e-3);
        // No hourly → next_12h_precip_pct is None.
        assert!(w.next_12h_precip_pct.is_none());
    }

    #[test]
    fn weather_label_maps_every_known_code() {
        // Sanity: every code we render must produce a stable label,
        // not "unknown" — a regression here means the right pane
        // would show "unknown" for a real weather condition.
        for code in [
            0u8, 1, 2, 3, 45, 48, 51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 71, 73, 75, 77, 80, 81,
            82, 85, 86, 95, 96, 99,
        ] {
            assert_ne!(
                weather_label(code),
                "unknown",
                "code {code} should have a human label"
            );
        }
    }

    #[test]
    fn weather_icon_returns_4_lines() {
        // Every major WMO code path and both day/night variants must
        // produce exactly 4 lines of ASCII art so the layout never
        // shifts vertically.
        let codes: &[(u8, bool)] = &[
            (0, true),   // clear day
            (0, false),  // clear night
            (1, true),   // partly cloudy day
            (2, false),  // partly cloudy night
            (3, true),   // overcast
            (45, true),  // fog
            (48, false), // fog (rime)
            (51, true),  // drizzle
            (61, true),  // rain
            (80, false), // rain showers
            (71, true),  // snow
            (85, false), // snow showers
            (95, true),  // thunderstorm
            (99, false), // thunderstorm w/ hail
            (255, true), // unknown / fallback
        ];
        for &(code, day) in codes {
            let lines = weather_icon(code, day);
            assert_eq!(
                lines.len(),
                4,
                "weather_icon({code}, {day}) returned {} lines, expected 4",
                lines.len(),
            );
        }
    }
}