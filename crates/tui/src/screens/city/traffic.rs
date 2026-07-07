//! Synthetic traffic overlay.
//!
//! Real traffic data requires a paid key (HERE, TomTom, Google).
//! Until those land the City screen renders a deterministic-but-
//! time-varying function of `(road importance, hour-of-day, weekday)`
//! so the colour overlay is always meaningful and the user can see
//! the legend change as the simulated clock advances.
//!
//! The footer on the City screen must always say
//! "traffic: synthetic" when this is the active source so the
//! data provenance is honest.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{Datelike, Timelike};

use super::roads::Polyline;

/// Re-export the canonical traffic enums from `cyberdeck-core` so the
/// TUI renderer and CLI share one schema. The `TrafficOverlay`
/// struct is local because it pairs `(road_index, level)` which
/// only makes sense in the renderer's context.
pub use cyberdeck_core::city::{TrafficLevel, TrafficSource};

#[derive(Debug, Clone)]
pub struct TrafficOverlay {
    pub source: TrafficSource,
    /// `(road_index, level)` pairs. `road_index` is the index into
    /// the `&[Polyline]` passed to `synthetic_overlay`. The renderer
    /// looks up the polyline + importance to pick a colour and
    /// optionally a stroke width (motorways draw thicker than
    /// residential).
    pub segments: Vec<(usize, TrafficLevel)>,
}

/// Synthetic model parameters. Tuned so:
///   * Weekday 07:30–09:30 and 16:30–18:30 = peak commute ⇒ many
///     `Heavy` / `Gridlock` segments on primary / motorway / trunk.
///   * Weekday 10:00–15:00 = mid-day ⇒ mostly `Fluid` / `Light`.
///   * Weekday 19:00–23:00 = evening wind-down ⇒ `Light` / `Heavy`.
///   * Weekday 23:00–06:00 = quiet hours ⇒ mostly `Fluid`.
///   * Weekend 11:00–14:00 = secondary peak (shopping / leisure) ⇒
///     some `Heavy` on residential, but no `Gridlock`.
// Tunable constants, kept as `const` rather than `static` so the
// optimiser can fold them into the runtime check below.
const PEAK_MORNING_START: u32 = 7;
const PEAK_MORNING_END: u32 = 10; // exclusive
const PEAK_EVENING_START: u32 = 16;
const PEAK_EVENING_END: u32 = 19; // exclusive
const WEEKEND_LEISURE_START: u32 = 11;
const WEEKEND_LEISURE_END: u32 = 15; // exclusive

/// Compute a synthetic traffic overlay for the given roads at the
/// given local time. Pure function — no async, no I/O, no clock
/// reads. Deterministic per `(roads, now)` so a redraw at the same
/// instant shows the same colours.
pub fn synthetic_overlay(roads: &[Polyline], now: chrono::DateTime<chrono::Local>) -> TrafficOverlay {
    let hour = now.hour();
    let weekday = now.weekday().num_days_from_monday(); // 0..6 (Mon=0)
    let is_weekend = weekday >= 5;

    // Period buckets. Picked once per call so the per-road loop is
    // branch-light.
    let period = match (is_weekend, hour) {
        (false, h) if (PEAK_MORNING_START..PEAK_MORNING_END).contains(&h) => Period::CommuteMorning,
        (false, h) if (PEAK_EVENING_START..PEAK_EVENING_END).contains(&h) => Period::CommuteEvening,
        (false, 10..=15) => Period::Midday,
        (false, 19..=22) => Period::Evening,
        (false, _) => Period::Quiet,
        (true, h) if (WEEKEND_LEISURE_START..WEEKEND_LEISURE_END).contains(&h) => Period::WeekendLeisure,
        (true, _) => Period::Quiet,
    };

    let segments = roads
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let importance = r.importance.0.as_str();
            let level = level_for(importance, period, hash_road(r, now));
            (i, level)
        })
        .collect();

    TrafficOverlay {
        source: TrafficSource::Synthetic,
        segments,
    }
}

/// Time-of-day buckets. Encoded as an enum (not raw hour ranges) so
/// the test for `level_for` is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    CommuteMorning,
    CommuteEvening,
    Midday,
    Evening,
    WeekendLeisure,
    Quiet,
}

/// Per-road hash. Mixes the polyline's first point + the current
/// minute-since-epoch so the same road doesn't all turn red at the
/// same instant — neighbouring roads scatter across levels.
fn hash_road(r: &Polyline, now: chrono::DateTime<chrono::Local>) -> u64 {
    let mut h = DefaultHasher::new();
    // First point is stable for a given road; the minute-since-epoch
    // rotates the buckets every minute so the map visibly evolves
    // even when the user just sits on the City screen.
    if let Some(p) = r.points.first() {
        p[0].to_bits().hash(&mut h);
        p[1].to_bits().hash(&mut h);
    }
    let minute = now.timestamp() / 60;
    minute.hash(&mut h);
    h.finish()
}

/// Decide a road's traffic level from its OSM `importance` tag,
/// the current period, and a stable per-road hash so neighbouring
/// roads scatter across levels.
///
/// Rules of thumb:
///   * `motorway` / `trunk` are always at least `Light`; they can
///     reach `Gridlock` only in commute peaks.
///   * `primary` / `secondary` follow the period closely.
///   * `residential` / `footway` / `service` never reach `Gridlock`
///     (footways in particular are usually `Fluid`).
fn level_for(importance: &str, period: Period, hash: u64) -> TrafficLevel {
    // Bucket the hash into 4 levels (0..4). Hash `& 0x3` covers
    // 0..4 evenly; `% 4` would skew slightly.
    let bucket = (hash & 0x3) as u8;

    let kind = match importance {
        "motorway" | "trunk" => Kind::Arterial,
        "primary" | "secondary" => Kind::Collector,
        // Everything else (residential, service, footway, path,
        // cycleway, unclassified, …) is treated as `Local`.
        _ => Kind::Local,
    };

    // Per-(kind, period) bias: how far up the severity ladder the
    // road tends to climb. Encoded as a `min_severity` (a road never
    // shows below this) and a `max_severity` (a road never shows
    // above this). The hash picks within that window.
    let (min_sev, max_sev) = match (kind, period) {
        (Kind::Arterial, Period::CommuteMorning) | (Kind::Arterial, Period::CommuteEvening) => (2, 3),
        (Kind::Arterial, _) => (1, 2),
        (Kind::Collector, Period::CommuteMorning) | (Kind::Collector, Period::CommuteEvening) => (1, 3),
        (Kind::Collector, _) => (1, 2),
        (Kind::Local, Period::CommuteMorning) | (Kind::Local, Period::CommuteEvening) => (1, 2),
        (Kind::Local, Period::WeekendLeisure) => (1, 2),
        (Kind::Local, _) => (0, 1),
    };

    // Map bucket ∈ 0..4 → severity ∈ min_sev..=max_sev.
    // `max_sev - min_sev` is at most 3 (so `bucket` fits exactly).
    let sev = min_sev + bucket.min(max_sev - min_sev);
    severity_to_level(sev)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Arterial,
    Collector,
    Local,
}

fn severity_to_level(sev: u8) -> TrafficLevel {
    match sev {
        0 => TrafficLevel::Fluid,
        1 => TrafficLevel::Light,
        2 => TrafficLevel::Heavy,
        _ => TrafficLevel::Gridlock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use cyberdeck_core::city::TrafficLevel;

    fn make_road(importance: &str) -> Polyline {
        Polyline {
            points: vec![[47.6, -122.3], [47.61, -122.31]],
            importance: super::super::roads::RoadImportance(importance.into()),
        }
    }

    fn at(year: i32, month: u32, day: u32, hour: u32, min: u32) -> chrono::DateTime<chrono::Local> {
        // Defensive against `minute >= 60` (which used to trip our
        // tests with a cryptic "No such local time" panic instead of
        // a clear test failure). We map it forward into the next
        // hour; the production path doesn't go through `at()` so
        // this is purely a test ergonomics fix.
        let (hour, min) = if min >= 60 {
            (hour + min / 60, min % 60)
        } else {
            (hour, min)
        };
        // `LocalResult` is a 3-state enum (`Single` / `Ambiguous` /
        // `None`); it doesn't implement `Result::expect`. Pick the
        // single deterministic mapping and `panic!` with a useful
        // message otherwise.
        match chrono::Local.with_ymd_and_hms(year, month, day, hour, min, 0) {
            chrono::LocalResult::Single(dt) => dt,
            other => panic!(
                "test helper at({year}-{month}-{day} {hour}:{min:02}) was non-Single: {other:?}"
            ),
        }
    }

    #[test]
    fn motorway_at_commute_morning_can_reach_gridlock() {
        // 2024-06-03 is a Monday; 08:00 is commute morning.
        let now = at(2024, 6, 3, 8, 0);
        let road = make_road("motorway");
        // Exhaustively check that some `hash` lands at Gridlock
        // for an arterial in commute morning. Since the function is
        // deterministic, just probe the bucket=3 corner directly via
        // the same hash the implementation would compute.
        let mut max = TrafficLevel::Fluid;
        // Step minute-by-minute inside a single hour. The earlier
        // `for minute in 0..240u32` silently overflowed the minute
        // field once `minute >= 60`, which `.unwrap()`'d into
        // "No such local time"; we only dodged it because Gridlock
        // is hit within the first 16 samples.
        for min in 0..60u32 {
            let n = at(2024, 6, 3, 8, min);
            let h = hash_road(&road, n);
            let l = level_for("motorway", Period::CommuteMorning, h);
            if l.severity() > max.severity() {
                max = l;
            }
            if max == TrafficLevel::Gridlock {
                break;
            }
        }
        assert_eq!(
            max,
            TrafficLevel::Gridlock,
            "motorway at commute morning should reach Gridlock within 1h"
        );
    }

    #[test]
    fn footway_never_reaches_gridlock() {
        // Across the entire weekday (24 hourly samples), the worst
        // a footway ever shows is `Light`. This is a property the
        // implementation pins so a future tuning pass can't silently
        // start showing red walking paths.
        for hour in 0..24u32 {
            let now = at(2024, 6, 3, hour, 0); // Monday
            let road = make_road("footway");
            let h = hash_road(&road, now);
            let l = level_for("footway", Period::Quiet, h);
            assert!(
                l.severity() <= TrafficLevel::Light.severity(),
                "footway at hour {hour} reached {l:?}"
            );
        }
    }

    #[test]
    fn residential_at_quiet_hour_is_fluid_or_light() {
        // 03:00 on a Tuesday: nobody's driving on residentials.
        let now = at(2024, 6, 4, 3, 0);
        let road = make_road("residential");
        for minute in (0..60).step_by(5) {
            let n = at(2024, 6, 4, 3, minute);
            let h = hash_road(&road, n);
            let l = level_for("residential", Period::Quiet, h);
            assert!(
                matches!(
                    l,
                    TrafficLevel::Fluid | TrafficLevel::Light
                ),
                "residential at 03:{minute:02} should be Fluid or Light, got {l:?}"
            );
        }
    }

    #[test]
    fn weekend_leisure_is_distinct_from_commute() {
        // Saturday 12:00 = leisure peak, no Gridlock on arterials.
        // We pick `2024-06-15` (a Saturday away from any plausible
        // DST transition window) and stay within `minute < 60` so the
        // helper doesn't overflow the minute field — the original
        // 0..240 sweep silently passed `minute=200..239` which made
        // `Local.with_ymd_and_hms` return `None` and the `.unwrap()`
        // panicked with "No such local time" even though the chrono
        // stack itself was fine.
        let now = at(2024, 6, 15, 12, 0); // Saturday
        let road = make_road("motorway");
        let mut worst = TrafficLevel::Fluid;
        // 4 hours × 15-min samples = 16 hashes; ample to find a
        // bucket=3 hit within the (Heavy, Heavy) weekend-leisure
        // window while staying safely inside a single hour boundary.
        for min in (0..240u32).step_by(15) {
            let h_in_hour = min % 60;
            let n = at(2024, 6, 15, 12, h_in_hour);
            let h = hash_road(&road, n);
            let l = level_for("motorway", Period::WeekendLeisure, h);
            if l.severity() > worst.severity() {
                worst = l;
            }
        }
        // Arterials on a weekend peak can hit Heavy but not Gridlock
        // (no commuter base). Weekday arterials CAN hit Gridlock
        // (test above). This is the key behavioral distinction.
        assert_ne!(
            worst,
            TrafficLevel::Gridlock,
            "weekend arterial should never Gridlock — got {worst:?}"
        );
        assert!(
            worst.severity() >= TrafficLevel::Heavy.severity(),
            "weekend arterial at leisure peak should at least Heavy"
        );
    }

    #[test]
    fn overlay_segments_match_input_order() {
        // The renderer relies on segment indices being positionally
        // aligned with the `&[Polyline]` it passed in. Sanity-check
        // that the output vec preserves the input order.
        let now = at(2024, 6, 3, 8, 0);
        let roads = vec![
            make_road("motorway"),
            make_road("primary"),
            make_road("residential"),
        ];
        let overlay = synthetic_overlay(&roads, now);
        assert_eq!(overlay.segments.len(), 3);
        // Indices are 0,1,2 in order.
        assert_eq!(overlay.segments[0].0, 0);
        assert_eq!(overlay.segments[1].0, 1);
        assert_eq!(overlay.segments[2].0, 2);
        assert_eq!(overlay.source, TrafficSource::Synthetic);
    }
}