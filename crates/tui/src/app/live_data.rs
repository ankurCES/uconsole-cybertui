use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{interval, MissedTickBehavior};

use cyberdeck_core::audio::Sink;
use cyberdeck_core::bluetooth::BtDevice;
use cyberdeck_core::city::{CityLocation, Weather};
use crate::screens::city::roads::Polyline;
use cyberdeck_core::display::DisplayOutput;
use cyberdeck_core::net::Interface;
use cyberdeck_core::packages::Package;
use cyberdeck_core::power::Battery;
use cyberdeck_core::process::Process;
use cyberdeck_core::services::Service;
use cyberdeck_core::storage::Filesystem;
use cyberdeck_core::sys::{Memory, SystemInfo, ThermalReading};
use cyberdeck_intel::{LayerId, Snapshot};

use crate::app::action::Action;

/// Role of a message in the AI conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiRole { User, #[default] Assistant }

/// One turn in the AI conversation. Shared via LiveData so the AI screen
/// and any future "AI log" screen both read the same history.
#[derive(Debug, Clone, Default)]
pub struct AiMessage {
    pub role: AiRole,
    pub thinking: String,  // content inside <think>...</think>
    pub content: String,   // answer text
    pub streaming: bool,   // true while tokens are still arriving
}

impl AiMessage {
    /// Full text representation for passing back to the LLM as history.
    pub fn full_text(&self) -> String {
        if self.thinking.is_empty() {
            self.content.clone()
        } else {
            format!("<think>{}</think>{}", self.thinking, self.content)
        }
    }
}

/// All live-refreshed data, shared via Arc so background tasks can write.
/// Mirrors the existing `Live` struct plus intel snapshots and refresher handles.
pub struct LiveData {
    pub info:            Arc<RwLock<SystemInfo>>,
    pub battery:         Arc<RwLock<Option<Battery>>>,
    pub thermals:        Arc<RwLock<Vec<ThermalReading>>>,
    pub interfaces:      Arc<RwLock<Vec<Interface>>>,
    pub active_ssid:     Arc<RwLock<Option<String>>>,
    pub services:        Arc<RwLock<Vec<Service>>>,
    pub filesystems:     Arc<RwLock<Vec<Filesystem>>>,
    pub upgradable:      Arc<RwLock<Vec<Package>>>,
    pub processes:       Arc<RwLock<Vec<Process>>>,
    pub displays:        Arc<RwLock<Vec<DisplayOutput>>>,
    pub sinks:           Arc<RwLock<Vec<Sink>>>,
    pub bluetooth:       Arc<RwLock<Vec<BtDevice>>>,
    pub web_enabled:     Arc<RwLock<bool>>,
    pub web_url:         Arc<RwLock<Option<String>>>,
    pub web_shutdown:    Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    pub web_ctrl:        Arc<Mutex<mpsc::Sender<(mpsc::Sender<Action>, Action)>>>,
    pub city_loc:        Arc<RwLock<Option<CityLocation>>>,
    pub city_weather:    Arc<RwLock<Option<Weather>>>,
    /// Roads fetched from Overpass API. `None` until first fetch lands;
    /// wrapped in `Arc` so `CityScreenV2` can clone the handle cheaply
    /// on each render without cloning the full road vector.
    pub city_roads:      Arc<RwLock<Option<std::sync::Arc<Vec<Polyline>>>>>,
    pub intel_snapshots: Arc<RwLock<BTreeMap<LayerId, Snapshot>>>,
    /// S19 — AI conversation history. Appended by apply_action on AiSubmit /
    /// AiToken / AiThinkToken / AiDone. Read by AiScreenV2::render.
    pub ai_messages: Arc<RwLock<Vec<AiMessage>>>,
    /// S19 — true once llama-server passes its health check.
    pub llama_ready:  Arc<RwLock<bool>>,

    /// Abort handles for background refreshers. Dropped on app exit.
    pub _refreshers: Vec<tokio::task::AbortHandle>,
}

impl Default for LiveData {
    fn default() -> Self {
        Self {
            info: Arc::new(RwLock::new(SystemInfo {
                hostname:    "?".into(),
                kernel:      "?".into(),
                os:          "Linux".into(),
                arch:        "?".into(),
                uptime_secs: 0,
                loadavg:     (0.0, 0.0, 0.0),
                memory: Memory {
                    total_kb:     0,
                    available_kb: 0,
                    used_pct:     0.0,
                },
                cpu_count: 1,
                cpu_model: "?".into(),
            })),
            battery:         Arc::new(RwLock::new(None)),
            thermals:        Arc::new(RwLock::new(Vec::new())),
            interfaces:      Arc::new(RwLock::new(Vec::new())),
            active_ssid:     Arc::new(RwLock::new(None)),
            services:        Arc::new(RwLock::new(Vec::new())),
            filesystems:     Arc::new(RwLock::new(Vec::new())),
            upgradable:      Arc::new(RwLock::new(Vec::new())),
            processes:       Arc::new(RwLock::new(Vec::new())),
            displays:        Arc::new(RwLock::new(Vec::new())),
            sinks:           Arc::new(RwLock::new(Vec::new())),
            bluetooth:       Arc::new(RwLock::new(Vec::new())),
            web_enabled:     Arc::new(RwLock::new(false)),
            web_url:         Arc::new(RwLock::new(None)),
            web_shutdown:    Arc::new(Mutex::new(None)),
            web_ctrl: Arc::new(Mutex::new(
                mpsc::channel::<(mpsc::Sender<Action>, Action)>(1).0,
            )),
            city_loc:        Arc::new(RwLock::new(None)),
            city_weather:    Arc::new(RwLock::new(None)),
            city_roads:      Arc::new(RwLock::new(None)),
            intel_snapshots: Arc::new(RwLock::new(BTreeMap::new())),
            ai_messages:     Arc::new(RwLock::new(Vec::new())),
            llama_ready:     Arc::new(RwLock::new(false)),
            _refreshers:     Vec::new(),
        }
    }
}

impl LiveData {
    /// Spawn background refreshers. Mirrors Live::spawn_refreshers cadences:
    /// 1Hz for sysinfo/thermal/battery/net, 5s for services, 15s for the rest.
    pub fn spawn_refreshers(self: &Arc<Self>, tx: mpsc::Sender<Action>) {
        // ── 1Hz: core system metrics ─────────────────────────────────────────
        let me = self.clone();
        let tx1 = tx.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(1));
            t.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                t.tick().await;
                let (info, batt, therm, ifaces, ssid) = tokio::join!(
                    cyberdeck_core::sys::info(),
                    cyberdeck_core::power::battery(),
                    cyberdeck_core::sys::thermals(),
                    cyberdeck_core::net::interfaces(),
                    cyberdeck_core::net::wifi_active_ssid(),
                );
                if let Ok(v) = info   { *me.info.write().await       = v; }
                if let Ok(v) = batt   { *me.battery.write().await    = Some(v); }
                if let Ok(v) = therm  { *me.thermals.write().await   = v; }
                if let Ok(v) = ifaces { *me.interfaces.write().await = v; }
                if let Ok(v) = ssid   { *me.active_ssid.write().await = v; }
                let _ = tx1.send(Action::Tick).await;
            }
        });

        // ── one-shot: IP geolocation + weather + roads for City screen ───────
        // Runs once at startup; the 10-min periodic task below re-runs it.
        {
            let city_loc     = self.city_loc.clone();
            let city_weather = self.city_weather.clone();
            let city_roads   = self.city_roads.clone();
            let tx_geo       = tx.clone();
            tokio::spawn(async move {
                refresh_city(city_loc, city_weather, city_roads, tx_geo).await;
            });
        }

        // ── 10-min: re-fetch geo + weather + roads ────────────────────────────
        {
            let city_loc     = self.city_loc.clone();
            let city_weather = self.city_weather.clone();
            let city_roads   = self.city_roads.clone();
            let tx_periodic  = tx.clone();
            tokio::spawn(async move {
                let mut t = interval(Duration::from_secs(600));
                t.set_missed_tick_behavior(MissedTickBehavior::Skip);
                t.tick().await; // skip first tick; one-shot already ran
                loop {
                    t.tick().await;
                    refresh_city(city_loc.clone(), city_weather.clone(), city_roads.clone(), tx_periodic.clone()).await;
                }
            });
        }

        // ── 5s: services ─────────────────────────────────────────────────────
        let me_svc = self.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(5));
            loop {
                t.tick().await;
                if let Ok(v) = cyberdeck_core::services::list_all().await {
                    *me_svc.services.write().await = v;
                }
            }
        });

        // ── 15s: slower resources ─────────────────────────────────────────────
        let me15 = self.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(15));
            loop {
                t.tick().await;
                let (fs, proc, dsp, aud, bt) = tokio::join!(
                    cyberdeck_core::storage::df(),
                    cyberdeck_core::process::list(),
                    cyberdeck_core::display::outputs(),
                    cyberdeck_core::audio::sinks(),
                    cyberdeck_core::bluetooth::list(),
                );
                if let Ok(v) = fs   { *me15.filesystems.write().await = v; }
                if let Ok(v) = proc { *me15.processes.write().await   = v; }
                if let Ok(v) = dsp  { *me15.displays.write().await    = v; }
                if let Ok(v) = aud  { *me15.sinks.write().await       = v; }
                if let Ok(v) = bt   { *me15.bluetooth.write().await   = v; }
            }
        });
    }
}

/// Shared logic: geo → weather → Overpass roads, all best-effort.
/// Called once at startup, every 10 minutes, and on manual `r` press.
pub async fn refresh_city(
    city_loc:     Arc<RwLock<Option<CityLocation>>>,
    city_weather: Arc<RwLock<Option<Weather>>>,
    city_roads:   Arc<RwLock<Option<std::sync::Arc<Vec<Polyline>>>>>,
    tx:           tokio::sync::mpsc::Sender<Action>,
) {
    let loc = match crate::screens::city::geo::locate().await {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("city geo locate failed: {e}");
            return;
        }
    };
    *city_loc.write().await = Some(loc.clone());
    let _ = tx.send(Action::Tick).await;

    // Weather — independent of roads; don't let a failure block road fetch.
    match crate::screens::city::weather::fetch(&loc).await {
        Ok(w) => { *city_weather.write().await = Some(w); }
        Err(e) => { tracing::warn!("city weather fetch failed: {e}"); }
    }
    let _ = tx.send(Action::Tick).await;

    // Roads — derive a bbox from ip-api coords (free tier returns no bbox).
    let bbox = loc.bbox.unwrap_or_else(|| {
        let span = 0.1; // ~11km radius
        [loc.lat - span, loc.lon - span, loc.lat + span, loc.lon + span]
    });
    match crate::screens::city::overpass::fetch_roads(bbox).await {
        Ok(roads) if !roads.is_empty() => {
            *city_roads.write().await = Some(std::sync::Arc::new(roads));
            let _ = tx.send(Action::Tick).await;
        }
        Ok(_) => { tracing::debug!("overpass returned 0 roads for bbox {bbox:?}"); }
        Err(e) => { tracing::warn!("overpass fetch failed: {e}"); }
    }
}
