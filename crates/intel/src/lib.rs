//! cyberdeck-intel — OSINT feeds + recon primitives.
//!
//! Inspired by [simplifaisoul/osiris](https://github.com/simplifaisoul/osiris)
//! (MIT). We re-implement the data domain taxonomy in Rust instead of
//! vendoring the TypeScript so the binary stays single-language and we
//! don't carry an unbounded transitive npm `node_modules` into the
//! install. Specifically:
//!
//! * No MapLibre / Next.js / Framer Motion (the TUI has its own
//!   braille mini-map renderer in `cyberdeck_tui::screens::city`).
//! * No external scanner backend: the Recon screen shells to local
//!   tools (`dig`, `whois`, `openssl`, `curl`, …) via the existing
//!   `tokio::process::Command` infrastructure in core.
//! * No Keyless vs key-required gate — every layer degrades to
//!   "key-required" in the same way Osiris does (FIRMS, N2YO).
//! * OFAC SDN mirror is a one-shot CSV, parsed once, lifetime =
//!   process. See [`sanctions`].
//!
//! ## Architecture
//!
//! ```text
//!   ┌──────────────────────────────────────────────────┐
//!   │               cyberdeck_tui::screens::intel      │
//!   │  (UI: layer grid + detail + braille mini-map)   │
//!   └──────────────────┬───────────────────────────────┘
//!                      │ Snapshot / refresh
//!                      ▼
//!   ┌──────────────────────────────────────────────────┐
//!   │                    this crate                    │
//!   │  ┌──────────┐ ┌──────────┐ ┌──────────────────┐ │
//!   │  │ flights  │ │earthquakes│ │ fires / sat / …  │ │
//!   │  └──────────┘ └──────────┘ └──────────────────┘ │
//!   │  ┌────────────────────────────────────────────┐  │
//!   │  │  recon::{dns, whois, ssl, cve, crypto, …}  │  │
//!   │  └────────────────────────────────────────────┘  │
//!   │  ┌────────────────────────────────────────────┐  │
//!   │  │  sanctions + ssrf (Osiris-derived, MIT)    │  │
//!   │  └────────────────────────────────────────────┘  │
//!   └──────────────────┬───────────────────────────────┘
//!                      │ HTTP
//!                      ▼
//!   upstream feeds — keyless by default
//! ```
//!
//! See `crates/tui/ROADMAP.md` § Phase 7 for the rollout plan.

#![warn(missing_debug_implementations)]
// Kept at warn (not deny) so the ws/mod templates can have off-by-one
// newtype patterns without failing the build — same policy as `core`.

use serde::{Deserialize, Serialize};

/// Identifier of a single OSINT data layer. Mirrors the 9 passive
/// layers we ship in M5. Each variant maps 1:1 to a module under
/// `src/`. The string form (`"flights"`, `"earthquakes"`, …) is the
/// stable identifier the CLI / daemon wire format uses, so renaming
/// a variant is a breaking change — append, don't reorder.
///
/// Ordered to match the deliverable order in `ROADMAP.md` § Phase 7
/// M5 (the kill screen reads top-to-bottom in this order). New
/// layers must be appended at the tail to keep on-disk prefs keys
/// stable across upgrades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerId {
    Flights,
    Earthquakes,
    Fires,
    Weather,
    Satellites,
    News,
    Cctv,
    Maritime,
    Conflicts,
}

impl LayerId {
    /// Stable, ordered slice used by the UI to paint the layer grid
    /// (left pane of the Intel screen). Insertion order = display
    /// order; do not sort — the visual rhythm depends on it.
    pub const ALL: &'static [LayerId] = &[
        LayerId::Flights,
        LayerId::Earthquakes,
        LayerId::Fires,
        LayerId::Weather,
        LayerId::Satellites,
        LayerId::News,
        LayerId::Cctv,
        LayerId::Maritime,
        LayerId::Conflicts,
    ];

    /// Single-character glyph for the header chip / sidebar indicator.
    /// Picked for legibility in a 1-cell wide tile at all terminal
    /// widths — Unicode block glyphs collapse to `?` on legacy
    /// 8-bit fonts, so we pick non-block when possible.
    pub const fn glyph(self) -> &'static str {
        match self {
            LayerId::Flights    => "✈",
            LayerId::Earthquakes => "⚠",
            LayerId::Fires      => "🔥",
            LayerId::Weather    => "☀",
            LayerId::Satellites => "🛰",
            LayerId::News       => "📰",
            LayerId::Cctv       => "📷",
            LayerId::Maritime   => "⚓",
            LayerId::Conflicts  => "⚔",
        }
    }

    /// Human label for the sidebar + tab strip. Sentence-cased to
    /// match the other screens (System, Network, Bluetooth, …).
    pub const fn label(self) -> &'static str {
        match self {
            LayerId::Flights    => "Flights",
            LayerId::Earthquakes => "Earthquakes",
            LayerId::Fires      => "Fires",
            LayerId::Weather    => "Weather",
            LayerId::Satellites => "Satellites",
            LayerId::News       => "News",
            LayerId::Cctv       => "CCTV",
            LayerId::Maritime   => "Maritime",
            LayerId::Conflicts  => "Conflicts",
        }
    }

    /// Recommended poll interval in seconds. Layer modules respect
    /// this — staggered cadences keep first paint fast and prevent
    /// the rate-limit gate from triggering. Mirrors Osiris's
    /// "aggressive polling relaxation (15-30 min intervals)".
    pub const fn poll_interval_secs(self) -> u32 {
        match self {
            LayerId::Flights     => 30,
            LayerId::Earthquakes => 60,
            LayerId::Fires       => 300,
            LayerId::Weather     => 540,
            LayerId::Satellites  => 720,
            LayerId::News        => 120,
            LayerId::Cctv        => 3600,
            LayerId::Maritime    => 21600,
            LayerId::Conflicts   => 3600,
        }
    }
}

/// Status of a single layer's last fetch. Mirrors what the OSINT
/// dashboard would call "data freshness" but kept small: the screen
/// only needs to know three things:
/// 1. Did it succeed at all?
/// 2. When was the last successful fetch?
/// 3. If not OK, why?
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerStatus {
    /// Never fetched since startup. Renders as `—` in the grid.
    Pending,
    /// Most recent fetch succeeded. Rendered in the theme's accent
    /// color with the timestamp in the right gutter.
    Ok { last_ok_unix: i64 },
    /// Most recent fetch failed. `reason` is a short one-line message
    /// suitable for the right pane ("rate-limited", "401 unauthorized",
    /// "key required", …). Rendered red.
    Error { last_ok_unix: Option<i64>, reason: String },
}

/// Sentinel state derived from a layer's most recent snapshot.
/// Used by the `intel_health` footer line and the Sentinel composite
/// tile. Mirrors how Osiris surfaces "* SANCTIONED *" red badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sentinel {
    /// Layer is OK and contains no alert-worthy data.
    Green,
    /// One entry is unusual but not dangerous (e.g. a single M4 quake).
    Yellow,
    /// One entry is dangerous (M5+, sanctioned wallet, intensity ≥ 4
    /// fire, sanctioned IP, etc.). Renders red on the right pane
    /// header.
    Red,
}

impl Sentinel {
    /// Stable short label used in the sentinel footer chip:
    /// `intel: 7/9 layers live · 1 RED`.
    pub const fn short(self) -> &'static str {
        match self {
            Sentinel::Green => "GREEN",
            Sentinel::Yellow => "YELLOW",
            Sentinel::Red => "RED",
        }
    }
}

/// One layer's most-recent snapshot. The `data: serde_json::Value`
/// is intentional: each layer module owns its `parse(&Value) -> …`
/// and the UI knows how to summarise any of them without this crate
/// knowing about every layer's domain type. Strict typing is kept
/// inside each module via `pub mod` types — the snapshot is just the
/// bag the UI carries across a thread boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub layer: LayerId,
    pub status: LayerStatus,
    pub sentinel: Sentinel,
    /// Human-readable one-line summary. Already-rendered text. The
    /// UI uses this for the layer grid's middle column so we don't
    /// need to render 9 different shapes on one grid.
    pub summary: String,
    /// Numeric entity count for the grid's right column ("1,284
    /// flights", "M2.5+ in last hour", …).
    pub entity_count: u64,
    /// Raw upstream payload, retained so the detail pane can drill
    /// without re-fetching. Cap to ~1 MiB at the layer module level.
    pub raw: serde_json::Value,
}

impl Snapshot {
    /// Construct an "error" snapshot — the layer module catches its
    /// own `fetch` error and returns one of these so the refiller
    /// loop can keep ticking instead of breaking the join.
    pub fn error(layer: LayerId, last_ok_unix: Option<i64>, reason: impl Into<String>) -> Self {
        Self {
            layer,
            status: LayerStatus::Error {
                last_ok_unix,
                reason: reason.into(),
            },
            sentinel: Sentinel::Green, // errors don't auto-trip the sentinel
            summary: String::new(),
            entity_count: 0,
            raw: serde_json::Value::Null,
        }
    }
}

/// Sentinel rollup — the worst severity across all layers. Used by
/// `App::live.intel_health` to drive the footer chip and by the CLI
/// `cyberdeck intel sentinel` verb. Ties break toward the lower
/// ordinal (Green < Yellow < Red) so the worst wins.
pub fn worst_sentinel<I: IntoIterator<Item = Sentinel>>(iter: I) -> Sentinel {
    iter.into_iter().max().unwrap_or(Sentinel::Green)
}

/// One module per layer. Each module is a self-contained pair of
/// `fetch` (M5) + `parse` (covered here) so a regression to a single
/// upstream's JSON shape only touches one file.
pub mod flights;
pub mod earthquakes;
pub mod fires;
pub mod weather;
pub mod satellites;
pub mod news;
pub mod cctv;
pub mod maritime;
pub mod conflicts;
/// M5 — staggered per-layer refiller. Spawns one tokio task per
/// `LayerId`; each polls its upstream, parses, and pushes a
/// `Snapshot` into an mpsc. The TUI's `App::spawn_refreshers` is the
/// consumer (subscribes via the `Action::IntelSnapshot` dispatcher
/// arm). See module docs.
pub mod refiller;

/// M7 — Recon action console. Seven OSINT primitives (`dns`, `whois`,
/// `ip`, `ssl`, `cve`, `crypto`, `sanctions`) each own a `run(query)`
/// function. The Recon screen drives them through a 7-tab UI; the CLI
/// (`cyberdeck recon ...`) will reuse the same `run()`s once that
/// parity task lands. `ssrf` is the shared safety gate every primitive
/// that would otherwise touch a network endpoint runs through first.
///
/// Not derived from a separate upstream — modelled after Osiris's
/// "Active Recon" tabs (DNS / WHOIS / IP / SSL / CVE) and the
/// payloads in `crates/intel/testdata/sanctions_sample.csv`. License:
/// MIT, sourced from simplifaisoul/osiris upstream.
pub mod recon;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_layer_ids_have_unique_labels() {
        // The UI uses `label()` as a key in a few places (prefs map,
        // tab strip). Collisions would silently merge layers.
        let mut seen = std::collections::BTreeSet::new();
        for id in LayerId::ALL {
            assert!(seen.insert(id.label()), "duplicate label for {:?}", id);
        }
    }

    #[test]
    fn poll_intervals_are_staggered() {
        // Quick property-ish check: no two consecutive layers in the
        // grid share an interval, so the staggered refiller never
        // bursts >2 layers at a tick.
        let pairs = LayerId::ALL.windows(2);
        for w in pairs {
            assert_ne!(
                w[0].poll_interval_secs(),
                w[1].poll_interval_secs(),
                "consecutive layers {:?} and {:?} share a poll interval",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn sentinel_rollup_picks_worst() {
        assert_eq!(
            worst_sentinel([Sentinel::Green, Sentinel::Red, Sentinel::Yellow]),
            Sentinel::Red
        );
        assert_eq!(
            worst_sentinel([Sentinel::Yellow]),
            Sentinel::Yellow
        );
        assert_eq!(
            worst_sentinel(std::iter::empty()),
            Sentinel::Green
        );
    }
}
