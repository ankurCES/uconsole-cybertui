use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{interval, MissedTickBehavior};

use cyberdeck_core::audio::Sink;
use cyberdeck_core::bluetooth::BtDevice;
use cyberdeck_core::city::{CityLocation, Weather};
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
    pub intel_snapshots: Arc<RwLock<BTreeMap<LayerId, Snapshot>>>,

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
            intel_snapshots: Arc::new(RwLock::new(BTreeMap::new())),
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
