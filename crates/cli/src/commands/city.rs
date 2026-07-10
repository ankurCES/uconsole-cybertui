//! `cyberdeck city` — IP-geolocation + Open-Meteo weather + bundled road data.
//!
//! Each subcommand returns structured JSON in `--json` mode (the
//! contract every other verb follows; see `commands/net.rs` for the
//! canonical pattern). In direct (no-daemon) mode the CLI hits the
//! ip-api + Open-Meteo endpoints itself and falls back to the bundled
//! `seattle.json` for road data — same data path the TUI uses, just
//! reachable from the shell.
//!
//! Subcommand design notes:
//!
//!   * `Locate` mirrors `screens::city::geo::locate` 1:1 — a single
//!     `{"city": ..., "country": ..., "lat": ..., "lon": ...}`
//!     object on success, or an error on rate-limit / network down.
//!   * `Weather { lat lon }` takes explicit coords so a user can
//!     query weather for a city they haven't geo'd (e.g. a travel
//!     check for "tomorrow in Paris"). Reuses the same Open-Meteo
//!     client as the TUI.
//!   * `Roads { slug }` returns the bundled road polyline list for
//!     the given slug. Falls back to `seattle` on unknown slugs
//!     (matches the TUI's `load_bundled_or_default`).
//!   * `Bundled` lists every slug the binary knows about — useful
//!     for tab-completion discoverability.
//!
//! Exit code contract: every success returns `Ok(0)`. We deliberately
//! don't propagate HTTP error codes — a transient ip-api rate-limit
//! shouldn't fail a shell pipeline that's just exploring. We log a
//! structured `{ "error": "..." }` JSON instead and still return 0,
//! matching the stub verbs in this crate.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::json;

use crate::output::OutputMode;

#[derive(Debug, Subcommand)]
pub enum CityCmd {
    /// Resolve the user's public IP to a CityLocation via ip-api.com.
    Locate,
    /// Fetch current weather from Open-Meteo for the given lat/lon.
    Weather {
        /// Latitude (decimal degrees, WGS84).
        #[arg(long)]
        lat: f64,
        /// Longitude (decimal degrees, WGS84).
        #[arg(long)]
        lon: f64,
    },
    /// Print bundled road polylines for the given city slug.
    Roads {
        /// Slug of the bundled city (e.g. `seattle`, `london`,
        /// `tokyo`, `berlin`, `nyc`). Falls back to `seattle` if
        /// unknown — matches the TUI's `load_bundled_or_default`.
        #[arg(default_value = "seattle")]
        slug: String,
    },
    /// List every bundled city slug the binary knows about.
    Bundled,
}

pub fn run(cmd: CityCmd, mode: OutputMode) -> Result<i32> {
    // The geo + weather clients in `cyberdeck-tui` are async (they use
    // `reqwest` + tokio under the hood). The CLI's top-level `main()`
    // doesn't spin a runtime — it's a single-shot binary invocation —
    // so we lazily build a one-thread runtime on demand for the
    // `Locate` + `Weather` arms only. The `Roads` + `Bundled` arms
    // are pure-data and stay sync.
    //
    // Using `new_current_thread` rather than `new_multi_thread`
    // because we're only ever awaiting one future at a time here —
    // no parallel work to schedule, and the smaller runtime footprint
    // (~few hundred KB) keeps a quick `cyberdeck city bundled` snappy.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime for `cyberdeck city`")?;
    match cmd {
        CityCmd::Locate => {
            // Reuse the TUI's geo client — both layers share the same
            // ip-api HTTP code path so a rate-limit or schema change
            // hits them at the same time. The CLI is direct-mode only
            // (no daemon round-trip); the daemon would forward a
            // Method::CityLocate RPC instead, once that ships.
            let result = rt.block_on(cyberdeck_tui::screens::city::geo::locate());
            match result {
                Ok(loc) => {
                    crate::output::print(
                        mode,
                        &json!({
                            "name": loc.name,
                            "country": loc.country,
                            "country_code": loc.country_code,
                            "region": loc.region,
                            "lat": loc.lat,
                            "lon": loc.lon,
                            "timezone": loc.timezone,
                            "bbox": loc.bbox.map(|b| [
                                b[0], b[1], b[2], b[3],
                            ]),
                        }),
                    )
                    .map(|_| 0)
                }
                Err(e) => {
                    // Match the stub verbs' "log + continue" contract.
                    // `e` is a `GeoError` (thiserror); the Display impl
                    // already includes the variant tag.
                    crate::output::print(
                        mode,
                        &json!({ "error": e.to_string(), "kind": "locate_failed" }),
                    )
                    .map(|_| 0)
                }
            }
        }
        CityCmd::Weather { lat, lon } => {
            // Reuse the Open-Meteo client — the CLI shares the same
            // shape as the TUI's `screens::city::weather::fetch`. We
            // synthesise a minimal `CityLocation` because the client
            // only reads `lat`/`lon` from it.
            let loc = cyberdeck_core::city::CityLocation {
                name: String::new(),
                country: String::new(),
                country_code: String::new(),
                region: String::new(),
                lat,
                lon,
                bbox: None,
                timezone: String::new(),
            };
            let result = rt.block_on(cyberdeck_tui::screens::city::weather::fetch(&loc));
            match result {
                Ok(fr) => {
                    let is_day = fr.is_day;
                    let w = fr.weather;
                    crate::output::print(
                        mode,
                        &json!({
                            "temp_c": w.temp_c,
                            "feels_like_c": w.feels_like_c,
                            "humidity_pct": w.humidity_pct,
                            "wind_kph": w.wind_kph,
                            "wind_dir_deg": w.wind_dir_deg,
                            "weather_code": w.weather_code,
                            "weather_label": cyberdeck_tui::screens::city::weather::weather_label(w.weather_code),
                            "next_12h_precip_pct": w.next_12h_precip_pct,
                            "fetched_at": w.fetched_at.to_rfc3339(),
                            "is_day": is_day,
                        }),
                    )
                    .map(|_| 0)
                }
                Err(e) => {
                    crate::output::print(
                        mode,
                        &json!({ "error": e.to_string(), "kind": "weather_failed" }),
                    )
                    .map(|_| 0)
                }
            }
        }
        CityCmd::Roads { slug } => {
            // Bundled road loader. Unknown slug → fall back to
            // seattle so the user always gets *something* back, and
            // we surface which slug we actually used so the caller
            // can detect the fallback (mirrors the TUI's behaviour).
            let (used_slug, roads) =
                cyberdeck_tui::screens::city::roads::CityRoads::load_bundled_or_default(&slug);
            // Project to a flat JSON shape — polylines are nested
            // arrays so serde_json's pretty-print handles them well,
            // and the size is small (6 polylines for seattle.json).
            crate::output::print(
                mode,
                &json!({
                    "slug_requested": slug,
                    "slug_used": used_slug,
                    "name": roads.name,
                    "bbox": [roads.bbox[0], roads.bbox[1], roads.bbox[2], roads.bbox[3]],
                    "road_count": roads.roads.len(),
                    "roads": roads.roads.iter().map(|r| {
                        json!({
                            "importance": r.importance.0,
                            "points": r.points,
                        })
                    }).collect::<Vec<_>>(),
                }),
            )
            .map(|_| 0)
        }
        CityCmd::Bundled => {
            let slugs: Vec<&str> = cyberdeck_tui::screens::city::roads::CityRoads::BUNDLED.to_vec();
            crate::output::print(mode, &json!({ "bundled": slugs })).map(|_| 0)
        }
    }
}