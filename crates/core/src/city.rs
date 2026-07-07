//! City data types shared between the TUI renderer, the CLI, and the
//! web layer.
//!
//! Step 4 — keeps these types in `core` (not in the tui crate) for
//! the same reason `net`, `power`, `sys` live in core: the CLI's
//! `cyberdeck city` verb (Step 10) and the optional web dashboard
//! both want to read the same payload. Keeping the type layer free
//! of ratatui / reqwest means it can be deserialized in a web
//! handler without dragging the renderer in.
//!
//! Conventions:
//!   * Units are SI (°C, km/h, degrees, WGS84 lat/lon). The TUI
//!     converts to imperial when `App::units == Imperial`.
//!   * `Option<T>` means "the data source didn't supply this"; never
//!     a sentinel value. Callers must handle `None` explicitly.
//!   * All types `#[derive(Serialize, Deserialize, Clone, Debug,
//!     PartialEq)]` so they round-trip JSON for the web layer and
//!     snapshot in tests.

use serde::{Deserialize, Serialize};

/// City as located by the IP geolocator (ip-api.com in production,
/// `CityRoads::location` when the user picks a bundled city by
/// name). Coordinates are WGS84 lat/lon in degrees.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CityLocation {
    /// Display name as the user typed it (or as ip-api returned it).
    pub name: String,
    pub country: String,
    /// ISO-3166 alpha-2 (e.g. `"US"`, `"JP"`). Empty when the user
    /// picked a bundled city whose JSON doesn't carry a country.
    pub country_code: String,
    pub region: String,
    pub lat: f64,
    pub lon: f64,
    /// `[min_lat, min_lon, max_lat, max_lon]`. `None` for IP-only
    /// hits (ip-api's free tier doesn't return a bbox); the roads
    /// loader derives one from the bundled polylines in that case.
    pub bbox: Option<[f64; 4]>,
    /// IANA tz database name (e.g. `"America/Los_Angeles"`).
    /// Empty if the source didn't supply it.
    pub timezone: String,
}

impl CityLocation {
    /// Centre of the bbox, or `(lat, lon)` if no bbox is known.
    /// Used as the default map viewport origin.
    pub fn centre(&self) -> (f64, f64) {
        match self.bbox {
            Some([min_lat, min_lon, max_lat, max_lon]) => {
                ((min_lat + max_lat) / 2.0, (min_lon + max_lon) / 2.0)
            }
            None => (self.lat, self.lon),
        }
    }

    /// `true` if `(lat, lon)` falls inside the location's bbox. When
    /// the bbox is unknown, returns `true` for the centre only (the
    /// weather marker is the only "thing at a point" we render, so
    /// `true` outside the centre is fine — the marker just doesn't
    /// clip).
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        match self.bbox {
            Some([min_lat, min_lon, max_lat, max_lon]) => {
                lat >= min_lat && lat <= max_lat && lon >= min_lon && lon <= max_lon
            }
            None => {
                (lat - self.lat).abs() < 1e-6 && (lon - self.lon).abs() < 1e-6
            }
        }
    }
}

/// Weather snapshot. Units are SI (see module docs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Weather {
    /// Air temperature at 2 m, °C.
    pub temp_c: f32,
    /// "Feels like" / apparent temperature at 2 m, °C.
    pub feels_like_c: f32,
    /// Relative humidity at 2 m, 0..100.
    pub humidity_pct: u8,
    /// Wind speed at 10 m, km/h.
    pub wind_kph: f32,
    /// Wind direction at 10 m, degrees from north (0..359).
    /// 0 = north, 90 = east, 180 = south, 270 = west. `None` if
    /// the source didn't supply it (calm wind sometimes omits the
    /// direction).
    pub wind_dir_deg: Option<u16>,
    /// WMO weather code. See `weather_code_label` for the mapping.
    pub weather_code: u8,
    /// Next-12h precipitation probability (0..100) for the
    /// sparkline in the right pane. `None` if the API didn't
    /// return hourly data.
    pub next_12h_precip_pct: Option<Vec<u8>>,
    /// When this snapshot was fetched (local time). Used by the
    /// renderer to age-out stale data ("5 min old" hint after 5 min).
    pub fetched_at: chrono::DateTime<chrono::Local>,
}

/// Coarse traffic density. The renderer maps to a colour:
/// `Fluid` ≈ green, `Light` ≈ yellow, `Heavy` ≈ orange,
/// `Gridlock` ≈ red. The synthetic model never returns `Gridlock`
/// for residential roads.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TrafficLevel {
    Fluid,
    Light,
    Heavy,
    Gridlock,
}

impl TrafficLevel {
    /// Numeric severity for "show the worst segment first" sort
    /// and similar comparisons. Higher = worse.
    pub fn severity(self) -> u8 {
        match self {
            TrafficLevel::Fluid => 0,
            TrafficLevel::Light => 1,
            TrafficLevel::Heavy => 2,
            TrafficLevel::Gridlock => 3,
        }
    }
}

/// Which data source produced a traffic overlay. The renderer
/// surfaces this in the City footer so the provenance is honest
/// (synthetic vs HERE vs TomTom).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrafficSource {
    /// Synthetic, time-of-day model (default until real keys land).
    Synthetic,
    /// HERE Maps Flow (key required — follow-up).
    Here,
    /// TomTom Flow (key required — follow-up).
    TomTom,
}

/// WMO weather code → human label. Exposed at module scope so the
/// TUI, CLI, and any future web layer can share one mapping.
pub fn weather_code_label(code: u8) -> &'static str {
    match code {
        0 => "clear",
        1..=3 => "partly cloudy",
        45 | 48 => "fog",
        51..=57 => "drizzle",
        61..=67 => "rain",
        71..=77 => "snow",
        80..=82 => "showers",
        85..=86 => "snow showers",
        95 => "thunderstorm",
        96..=99 => "thunder + hail",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn city_centre_is_bbox_midpoint() {
        // Use f64-precision epsilon since (47.4 + 47.8) / 2.0 is
        // exactly 47.6 but a different midpoint (e.g. 47.401 +
        // 47.803) would land at 47.602 — the f64 midpoint is
        // always within 1e-9 of the mathematical midpoint, so
        // approximate_eq with an epsilon is the right contract.
        let loc = CityLocation {
            name: "Seattle".into(),
            country: "US".into(),
            country_code: "US".into(),
            region: "WA".into(),
            lat: 47.6,
            lon: -122.3,
            bbox: Some([47.4, -122.5, 47.8, -122.1]),
            timezone: "America/Los_Angeles".into(),
        };
        let (clat, clon) = loc.centre();
        assert!(
            (clat - 47.6).abs() < 1e-9,
            "lat midpoint off: {clat}"
        );
        assert!(
            (clon - -122.3).abs() < 1e-9,
            "lon midpoint off: {clon}"
        );
    }

    #[test]
    fn city_centre_falls_back_to_lat_lon_when_no_bbox() {
        let loc = CityLocation {
            name: "Nowhere".into(),
            country: "".into(),
            country_code: "".into(),
            region: "".into(),
            lat: 12.34,
            lon: 56.78,
            bbox: None,
            timezone: "".into(),
        };
        assert_eq!(loc.centre(), (12.34, 56.78));
    }

    #[test]
    fn contains_inside_bbox_is_true() {
        let loc = CityLocation {
            name: "x".into(),
            country: "".into(),
            country_code: "".into(),
            region: "".into(),
            lat: 0.0,
            lon: 0.0,
            bbox: Some([-1.0, -1.0, 1.0, 1.0]),
            timezone: "".into(),
        };
        assert!(loc.contains(0.0, 0.0));
        assert!(loc.contains(0.5, -0.5));
        assert!(loc.contains(-0.999, 0.999));
    }

    #[test]
    fn contains_outside_bbox_is_false() {
        let loc = CityLocation {
            name: "x".into(),
            country: "".into(),
            country_code: "".into(),
            region: "".into(),
            lat: 0.0,
            lon: 0.0,
            bbox: Some([-1.0, -1.0, 1.0, 1.0]),
            timezone: "".into(),
        };
        assert!(!loc.contains(2.0, 0.0));
        assert!(!loc.contains(0.0, 2.0));
    }

    #[test]
    fn traffic_severity_orders_fluid_lt_gridlock() {
        assert!(TrafficLevel::Fluid.severity() < TrafficLevel::Light.severity());
        assert!(TrafficLevel::Light.severity() < TrafficLevel::Heavy.severity());
        assert!(TrafficLevel::Heavy.severity() < TrafficLevel::Gridlock.severity());
    }

    #[test]
    fn weather_code_label_maps_every_known_code() {
        // Every code the renderer should ever see must produce a
        // stable label, not "unknown" — a regression here means
        // the right pane shows "unknown" for a real condition.
        for code in [
            0u8, 1, 2, 3, 45, 48, 51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 71, 73, 75, 77, 80, 81,
            82, 85, 86, 95, 96, 99,
        ] {
            assert_ne!(
                weather_code_label(code),
                "unknown",
                "code {code} should have a human label"
            );
        }
    }

    #[test]
    fn weather_round_trips_through_json() {
        let w = Weather {
            temp_c: 9.2,
            feels_like_c: 7.4,
            humidity_pct: 78,
            wind_kph: 12.0,
            wind_dir_deg: Some(315),
            weather_code: 3,
            next_12h_precip_pct: Some(vec![10, 20, 30, 40]),
            fetched_at: chrono::Local::now(),
        };
        let s = serde_json::to_string(&w).unwrap();
        let back: Weather = serde_json::from_str(&s).unwrap();
        assert_eq!(w, back);
    }
}