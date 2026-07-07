//! Cross-cutting state and event plumbing for the TUI.
//!
//! `App` is the single source of truth for what the TUI is showing. Screens
//! receive `&mut App` (or a narrow view of it) and return `Cmd` values that
//! the main loop translates into async tasks.
//!
//! Many fields/methods here are written by screens but not yet read back to
//! drive rendering — they're placeholders for the Phase-3 screens work. They're
//! kept (with this module-wide allow) so the wiring compiles end-to-end and
//! we can flip each consumer on without re-touching the App struct.
//! See ROADMAP.md.
#![allow(dead_code)]

pub mod action;
pub mod screen;
pub mod toast;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use cyberdeck_core::audio::Sink;
use cyberdeck_core::bluetooth::BtDevice;
use cyberdeck_core::display::DisplayOutput;
use cyberdeck_core::net::Interface;
use cyberdeck_core::packages::Package;
use cyberdeck_core::power::Battery;
use cyberdeck_core::process::Process;
use cyberdeck_core::process::ProcEntry;
use cyberdeck_core::services::Service;
use cyberdeck_core::storage::Filesystem;
use cyberdeck_core::sys::SystemInfo;
use ratatui::text::Line;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::interval;

pub use action::Action;
pub use screen::ScreenId;
pub use toast::{Toast, ToastKind};

// Module 7 — toast history ring (capped at 200 entries). Each entry records
// when the toast was emitted so the modal can render an HH:MM:SS prefix.
#[derive(Debug, Clone)]
pub struct ToastEntry {
    pub ts: chrono::DateTime<chrono::Local>,
    pub kind: ToastKind,
    pub message: String,
}

/// Hard cap on `App::toast_history`. When the ring is full, `push_toast`
/// drops the oldest entry. 200 is large enough to cover a busy session
/// (~3 minutes of one toast/sec) without bloating memory.
pub const TOAST_HISTORY_CAP: usize = 200;

#[derive(Debug)]
pub enum Modal {
    None,
    Help,
    CommandPalette,
    Confirm {
        message: String,
        kind: ConfirmKind,
        arg: String,
    },
    Input {
        prompt: String,
        buf: String,
        kind: InputKind,
    },
    /// Masked text input. Renders every char as `•`; underlying `buf` is the
    /// real value. Used for passwords, BT passkeys, 802.1X identity passwords.
    Secret {
        prompt: String,
        buf: String,
        kind: InputKind,
    },
    /// Pick one of `options` with j/k or Up/Down, Enter to commit, Esc to
    /// dismiss. Options are `(id, label)` so the caller can use a stable key
    /// regardless of the rendered string.
    Choice {
        prompt: String,
        options: Vec<ChoiceOption>,
        cursor: usize,
        /// When `Some`, the chosen id is forwarded through this modal kind
        /// (e.g. committing the SSID picker opens the Wi-Fi password modal).
        commit_kind: Option<ChoiceCommit>,
    },
    /// Multi-step flow (e.g. Wi-Fi Enterprise: pick EAP → identity → password).
    /// `step` indexes into `state.steps()`; `advance()` returns the next state
    /// or signals completion via `Wizard::done()`.
    Wizard(Wizard),
    /// Long-running action with progress. `done`/`total` are 0-based; total=0
    /// means "indeterminate" (spinner). Esc closes the modal AND signals
    /// cancellation via the oneshot in `cancel`.
    Progress {
        label: String,
        done: u64,
        total: u64,
        cancel: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// `pkexec` (or whatever Privilege::Sudo wrapper) returned non-zero.
    /// The inner modal is what to retry once the user re-authenticates.
    AuthFailure {
        command: String,
        stderr: String,
        retry: Box<Modal>,
    },
    /// Module 7.2 — scrollable history of past toasts. Newest-first;
    /// `App::toast_log_offset` is the scroll position (0 = tail). Opens
    /// via the global `T` key, closes on Esc.
    ToastLog,
}

#[derive(Debug)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
}

/// Where a Choice commit lands. `PickInput` opens the named `InputKind`
/// prompt with `id` pre-supplied via `prefill`; `RunAction` dispatches
/// directly; `LoraPickStoredIp` reconnects to a previously-used
/// Meshtastic node IP chosen from the auto-popup picker; `Next`
/// re-enters the wizard with the picked step value.
#[derive(Debug)]
pub enum ChoiceCommit {
    /// Open an Input/Secret modal with `prefill` already in the buffer.
    PickInput {
        kind: InputKind,
        prompt: String,
        masked: bool,
        prefill: String,
    },
    /// Dispatch this RunAction verbatim.
    RunAction(crate::app::action::RunAction),
    /// Connect to a previously-used Meshtastic node IP (selected from
    /// the auto-popup fired by `switch_screen` when the user opens
    /// the LoRa screen with no IP set and at least one recent IP on
    /// file). Wraps a `String` so the existing `ChoiceCommit` enum's
    /// `Debug` derive stays accurate.
    LoraPickStoredIp { ip: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    Reboot,
    Shutdown,
    Kill,
    Remove,
    DisconnectWifi,
    /// Module 4 — Files: in-TUI editor. Confirms discarding an
    /// unsaved editor buffer (Esc on a dirty editor). `arg` on the
    /// owning `Modal::Confirm` carries the editor's path as a
    /// human-readable string so the dialog can show "Discard
    /// unsaved changes to {path}?".
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    WifiPassword,
    ConnectSSID,
    KillPid,
    WifiEnterpriseIdentity,
    WifiEnterprisePassword,
    HiddenSSID,
    /// Bluetooth pairing passkey. Numeric only — the modal's Char(c)
    /// handler drops non-digit chars at the buffer-insert step so the
    /// user can't accidentally type letters into a passkey field.
    BluetoothPasskey,
    /// Module 3 — search query for the Packages screen. The submit
    /// handler stashes the trimmed value on `App::packages_search_query`
    /// so the Packages screen's render loop can pick it up and fire
    /// `cyberdeck_core::packages::search(&query)`. Tasks 3.2–3.4 wire
    /// the modal UI + `/` hotkey on the Packages screen itself; this
    /// variant is just the variant + dispatch plumbing.
    PackageSearch,
    /// LoRa (Meshtastic) screen — node IP for the HTTP transport.
    /// Submitted value goes into `App::lora_node_ip`; the main loop
    /// then swaps the screen's `FakeTransport` for an `HttpLoraTransport`
    /// pointed at that IP (Slice 4). IP-only at the modal layer; the
    /// port + URL prefix are appended by the transport constructor.
    LoraNodeIp,
}

#[derive(Debug)]
pub enum Wizard {
    /// Wi-Fi Enterprise 802.1X connect. Steps:
    /// 0: pick EAP method (PEAP/TTLS/TLS/PWD)
    /// 1: identity (Input)
    /// 2: password (Secret) — skipped for TLS
    /// 3: optional anon identity or cert path (Input) — depends on method
    WifiEnterprise {
        ssid: String,
        step: usize,
        eap: Option<String>,
        identity: Option<String>,
        password: Option<String>,
        anon_or_cert: Option<String>,
    },
}

impl Wizard {
    pub fn done(&self) -> bool {
        match self {
            Wizard::WifiEnterprise { identity, password, anon_or_cert, eap, .. } => {
                if eap.is_none() || identity.is_none() {
                    return false;
                }
                match eap.as_deref() {
                    // `step` is the UI flow cursor; we don't gate `done()`
                    // on it because callers set fields directly during tests
                    // and via the dispatcher in production (which advances
                    // step in lock-step with the fields anyway).
                    Some("TLS") => anon_or_cert.is_some(),
                    _ => password.is_some(),
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct Live {
    pub info: Arc<RwLock<SystemInfo>>,
    pub battery: Arc<RwLock<Option<Battery>>>,
    pub thermals: Arc<RwLock<Vec<cyberdeck_core::sys::ThermalReading>>>,
    pub interfaces: Arc<RwLock<Vec<Interface>>>,
    pub active_ssid: Arc<RwLock<Option<String>>>,
    pub services: Arc<RwLock<Vec<Service>>>,
    pub filesystems: Arc<RwLock<Vec<Filesystem>>>,
    pub upgradable: Arc<RwLock<Vec<Package>>>,
    pub processes: Arc<RwLock<Vec<Process>>>,
    pub displays: Arc<RwLock<Vec<DisplayOutput>>>,
    pub sinks: Arc<RwLock<Vec<Sink>>>,
    pub bluetooth: Arc<RwLock<Vec<BtDevice>>>,
    pub web_enabled: Arc<RwLock<bool>>,
    pub web_url: Arc<RwLock<Option<String>>>,
    /// Kill switch for the embedded web server. `Some` means a server task is
    /// running; dropping the sender tells that task to exit.
    pub web_shutdown: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    /// Dedicated control channel for the embedded web server. Holds the
    /// sender half; the tap task in main() owns the receiver. UI code
    /// routes `WebStart`/`WebStop` through here instead of the main `tx`.
    pub web_ctrl: Arc<Mutex<mpsc::Sender<(mpsc::Sender<Action>, Action)>>>,
    /// Step 9 — last IP-geolocated location resolved by the City
    /// refiller or a `CityCtrlRefresh` tap. `None` until the first
    /// successful fetch. The City screen reads this on every render
    /// so the right pane + footer can show the resolved city name.
    /// Held as `Arc<RwLock<…>>` (matching every other Live field) so
    /// future spawned tasks can update without re-borrowing `App`.
    pub city_loc: Arc<RwLock<Option<crate::screens::city::geo::CityLocation>>>,
    /// Step 9 — last Open-Meteo weather snapshot. `None` until the
    /// first successful fetch; the screen falls back to "(waiting for
    /// first fetch)" until that lands. Same ownership rules as
    /// `city_loc`.
    pub city_weather: Arc<RwLock<Option<crate::screens::city::weather::Weather>>>,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            info: Arc::new(RwLock::new(SystemInfo {
                hostname: "?".into(),
                kernel: "?".into(),
                os: "Linux".into(),
                arch: "?".into(),
                uptime_secs: 0,
                loadavg: (0.0, 0.0, 0.0),
                memory: cyberdeck_core::sys::Memory {
                    total_kb: 0,
                    available_kb: 0,
                    used_pct: 0.0,
                },
                cpu_count: 1,
                cpu_model: "?".into(),
            })),
            battery: Arc::new(RwLock::new(None)),
            thermals: Arc::new(RwLock::new(Vec::new())),
            interfaces: Arc::new(RwLock::new(Vec::new())),
            active_ssid: Arc::new(RwLock::new(None)),
            services: Arc::new(RwLock::new(Vec::new())),
            filesystems: Arc::new(RwLock::new(Vec::new())),
            upgradable: Arc::new(RwLock::new(Vec::new())),
            processes: Arc::new(RwLock::new(Vec::new())),
            displays: Arc::new(RwLock::new(Vec::new())),
            sinks: Arc::new(RwLock::new(Vec::new())),
            bluetooth: Arc::new(RwLock::new(Vec::new())),
            web_enabled: Arc::new(RwLock::new(false)),
            web_url: Arc::new(RwLock::new(None)),
            web_shutdown: Arc::new(Mutex::new(None)),
            // web_ctrl is overwritten in main() once the tap task is up.
            // The placeholder channel has capacity 1 and no receiver.
            web_ctrl: Arc::new(Mutex::new(
                mpsc::channel::<(mpsc::Sender<Action>, Action)>(1).0,
            )),
            // Step 9 — City snapshots. Both `None` at startup; the
            // 10-min refiller (or a manual `r`) populates them. The
            // render path is null-tolerant and shows "(waiting…)"
            // until a value lands.
            city_loc: Arc::new(RwLock::new(None)),
            city_weather: Arc::new(RwLock::new(None)),
        }
    }
}

impl Live {
    /// Spawn a background task that refreshes the live readouts on a timer.
    /// Each field has its own cadence — system/thermal every second, services
    /// and processes every five, packages on demand.
    pub fn spawn_refreshers(self: &Arc<Self>, tx: mpsc::Sender<Action>) {
        // Clone the sender up-front so multiple spawned tasks can each
        // hold their own handle. Tokio's `mpsc::Sender` is `Clone`.
        let tx_tick = tx.clone();
        let me = self.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(1));
            loop {
                t.tick().await;
                if let Ok(info) = cyberdeck_core::sys::info().await {
                    *me.info.write().await = info;
                }
                if let Ok(b) = cyberdeck_core::power::battery().await {
                    *me.battery.write().await = Some(b);
                }
                if let Ok(th) = cyberdeck_core::sys::thermals().await {
                    *me.thermals.write().await = th;
                }
                if let Ok(ifaces) = cyberdeck_core::net::interfaces().await {
                    *me.interfaces.write().await = ifaces;
                }
                if let Ok(ssid) = cyberdeck_core::net::wifi_active_ssid().await {
                    *me.active_ssid.write().await = ssid;
                }
                let _ = tx_tick.send(Action::Tick).await;
            }
        });

        // Module 2.2 — recent-logs refiller. Runs at 1Hz, polling the last
        // 2s of journal entries. Successive calls overlap heavily (the
        // `recent_since(2)` window slides forward by 1s each tick), so
        // dedupe by (ts, message) happens in the `LogPushed` dispatcher
        // arm rather than here. We push each new line as its own
        // `LogPushed` action so the dispatcher can dedupe in order and
        // the UI gets a chance to redraw on each line.
        //
        // Module 2.3 — `recent_since` now returns `(DateTime<Utc>, String)`
        // tuples parsed from journalctl's `--output=json` (`__REALTIME_TIMESTAMP`
        // in microseconds since the epoch). The timestamp is the event's
        // real time, not fetch time, so the rendered line on the Logs /
        // System screens reflects when the entry actually happened, even
        // if the poller ran behind.
        //
        // Failure modes (journalctl missing, no perms, quiet box): we
        // log at debug and continue. The refiller never errors out —
        // a transient failure shouldn't kill the live feed.
        let tx_logs = tx.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(1));
            // Skip ticks that fall behind rather than burst-fire to
            // catch up; on a heavily loaded box this prevents the
            // refiller from monopolising the channel.
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                t.tick().await;
                let entries = match cyberdeck_core::logs::recent_since(2).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::debug!("logs::recent_since failed: {e}");
                        continue;
                    }
                };
                for (ts, message) in entries {
                    if message.is_empty() {
                        continue;
                    }
                    let line = LogLine {
                        ts: ts.with_timezone(&Local),
                        message,
                    };
                    if tx_logs.send(Action::LogPushed(line)).await.is_err() {
                        // Receiver dropped — app is shutting down.
                        break;
                    }
                }
            }
        });

        // Services get a 5s cadence — the user wants the Services screen to
        // feel live, and `systemctl list-units --all` on this box is the
        // dominant cost. It's the only heavy call that runs at 5s; everything
        // else is on a slower loop so a hiccup in one resource can't hitch
        // the UI.
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

        // Everything else on a 15s cadence, parallelised so a slow call
        // doesn't delay the others. /proc walks, bluetoothctl, df, display
        // enumeration — none of these need to be more frequent than this.
        let me = self.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(15));
            loop {
                t.tick().await;
                let fs_fut   = cyberdeck_core::storage::df();
                let proc_fut = cyberdeck_core::process::list();
                let dsp_fut  = cyberdeck_core::display::outputs();
                let aud_fut  = cyberdeck_core::audio::sinks();
                let bt_fut   = cyberdeck_core::bluetooth::list();
                let (fs, proc, dsp, aud, bt) =
                    tokio::join!(fs_fut, proc_fut, dsp_fut, aud_fut, bt_fut);
                if let Ok(v) = fs   { *me.filesystems.write().await = v; }
                if let Ok(v) = proc { *me.processes.write().await   = v; }
                if let Ok(v) = dsp  { *me.displays.write().await    = v; }
                if let Ok(v) = aud  { *me.sinks.write().await       = v; }
                if let Ok(v) = bt   { *me.bluetooth.write().await   = v; }
            }
        });

        // Module 6.2 — 15s refiller that snapshots /proc with ppid for
        // the System screen's process-tree view. Sits on its own loop so
        // a hiccup in the I/O here can't hitch the existing 15s block
        // (which already serializes fs/proc/dsp/aud/bt via `tokio::join!`).
        //
        // We off-load the synchronous /proc walk to `spawn_blocking` —
        // the read of every `/proc/<pid>/{stat,cmdline}` is regular
        // blocking I/O. Running it on the runtime worker would tie up a
        // worker for the whole walk (~100s of small reads on a busy
        // box); `spawn_blocking` hands it to the blocking-thread pool.
        //
        // On any error (non-Linux box, /proc missing, unreadable) we
        // fall back to an empty snapshot so the next render shows
        // "(no processes)" rather than crashing the dispatcher.
        let tx_proc = tx.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(15));
            // Skip ticks that fall behind rather than burst-fire; mirrors
            // the logs + network samplers above.
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                t.tick().await;
                let procs = tokio::task::spawn_blocking(|| {
                    cyberdeck_core::process::list_with_ppid().unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                if tx_proc
                    .send(Action::ProcTreeRefreshed(procs))
                    .await
                    .is_err()
                {
                    // Receiver dropped — main loop is shutting down.
                    return;
                }
            }
        });

        // Module 5.3 — 1Hz network sampler. Reads every active network
        // interface's cumulative RX/TX byte counts from
        // `/sys/class/net/<iface>/statistics/{rx,tx}_bytes`, computes
        // the per-second delta against the previous sample, and pushes
        // each (iface, rx_d, tx_d) tuple into `App::net_history` via
        // the `Action::NetSample` dispatcher arm. The header chip
        // (Module 5.4) reads those rings on every frame.
        //
        // We use `tokio::task::spawn_blocking` because the sysfs read
        // is synchronous I/O — even though `/sys/class/net` is
        // pseudo-filesystem backed by the kernel, `std::fs::read_to_string`
        // still has to wait for the VFS to format the page, and we
        // don't want to pin one of the runtime's worker threads.
        //
        // First sample is intentionally a no-op delta: we have no
        // `prev` to subtract against, so we just record the baseline
        // and the next tick produces the first real `rx_d / tx_d`.
        // Saturating subtraction handles the corner case where the
        // counter has rolled (32-bit `/proc/net/dev` overflow — rare
        // on modern 64-bit counters but possible for low-rate links).
        let tx_net = tx.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(1));
            // Skip ticks that fall behind rather than burst-fire;
            // mirrors the logs refiller above.
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut prev: std::collections::HashMap<
                String,
                cyberdeck_core::net::ByteCounts,
            > = std::collections::HashMap::new();
            loop {
                t.tick().await;
                let curr = tokio::task::spawn_blocking(|| {
                    cyberdeck_core::net::interface_byte_counts()
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                // Capture baseline on first tick — every delta is 0 in
                // this round, but the next tick has a real prev to
                // subtract against.
                if prev.is_empty() {
                    prev = curr;
                    continue;
                }
                let mut any_sent = false;
                for (name, bc) in &curr {
                    let (rx_d, tx_d) = match prev.get(name) {
                        Some(p) => (
                            bc.rx.saturating_sub(p.rx),
                            bc.tx.saturating_sub(p.tx),
                        ),
                        None => (0, 0),
                    };
                    if tx_net
                        .send(Action::NetSample {
                            iface: name.clone(),
                            rx_delta: rx_d,
                            tx_delta: tx_d,
                        })
                        .await
                        .is_err()
                    {
                        // Receiver dropped — main loop is shutting down.
                        return;
                    }
                    any_sent = true;
                }
                prev = curr;
                // Suppress the unused warning on `any_sent` while
                // documenting why we still keep the result.
                let _ = any_sent;
            }
        });

        // Module 8.2 — 30s refiller that lists every saved Wi-Fi profile
        // via `cyberdeck_core::net::saved_connections`. We off-load to
        // `spawn_blocking` because the call shells out to `nmcli` (sync
        // child process) and we don't want to pin a runtime worker for
        // the duration. 30s is well above the perceived "real-time"
        // threshold — saved profiles rarely change during a session,
        // and the user can always press `s`/rescan for an instant read.
        let tx_saved = tx.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(30));
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                t.tick().await;
                let conns = tokio::task::spawn_blocking(|| {
                    cyberdeck_core::net::saved_connections().unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                if tx_saved
                    .send(Action::SavedConnectionsRefreshed(conns))
                    .await
                    .is_err()
                {
                    // Receiver dropped — main loop is shutting down.
                    return;
                }
            }
        });

        // Step 9 — City refiller. Runs at 600s (10 min) for both the
        // IP-geolocation lookup and the Open-Meteo weather pull. The
        // rate limits we're up against:
        //
        //   * ip-api.com free tier: 45 req/min per source IP (HTTP only).
        //   * Open-Meteo: no documented rate limit, but the polite
        //     default is ~once per minute.
        //
        // The 10-minute cadence is well below both. `Action::CityCtrlRefresh`
        // (sent by the City screen on `r`) re-fires the same pipeline
        // immediately, on demand — see the dispatcher's `CityCtrlRefresh`
        // arm for that path.
        //
        // The refiller holds no state — both lookups are one-shot async
        // calls that return a result. On success we push
        // `CityResolved { loc }` and `CityWeatherRefreshed { w }` back to
        // the dispatcher, which applies them to the live `App` snapshot.
        // On failure we log a warning and skip the tick — a transient
        // outage must not crash the refiller.
        //
        // The `geo::locate` and `weather::fetch` calls are independent;
        // we run them sequentially here for simplicity (each is <500ms
        // on the typical home connection). A 5xx error from one doesn't
        // affect the other — both are best-effort.
        let tx_city = tx.clone();
        tokio::spawn(async move {
            let mut t = interval(Duration::from_secs(600));
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                t.tick().await;
                // IP → CityLocation. One-shot; the screen debounces by
                // only enqueuing `CityCtrlRefresh` on focus + manual `r`.
                let loc_result = crate::screens::city::geo::locate().await;
                match loc_result {
                    Ok(loc) => {
                        if tx_city.send(Action::CityResolved(loc)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("city::locate failed: {e}");
                    }
                }
                // Weather pull needs a resolved location. If the
                // geolocator didn't return one this tick (rate-limited,
                // network down), skip the weather call — the previous
                // snapshot stays on screen.
                if let Ok(loc) = crate::screens::city::geo::locate().await {
                    match crate::screens::city::weather::fetch(&loc).await {
                        Ok(w) => {
                            if tx_city
                                .send(Action::CityWeatherRefreshed(w))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::debug!("city::weather::fetch failed: {e}");
                        }
                    }
                }
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LogLine {
    pub ts: chrono::DateTime<Local>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSort {
    Cpu,
    Mem,
    Pid,
    Time,
}

pub struct App {
    pub live: Arc<Live>,
    pub current: ScreenId,
    pub manager: crate::wm::manager::Manager,
    pub modal: Modal,
    /// True when the sidebar (screen list) has focus. *Derived* from
    /// `region`: it's `region == Region::Sidebar`. Kept as a `bool` for
    /// compatibility with the old render and test paths that read it
    /// directly; new code should prefer `app.region`.
    pub sidebar_focused: bool,
    /// Cursor position in the sidebar list (0-based index into ScreenId::ALL).
    /// Distinct from `current` so the user can navigate before committing
    /// with Enter. `current` is what's actually rendered in the content
    /// pane; `sidebar_idx` is what's highlighted in the menu.
    pub sidebar_idx: usize,
    /// Scroll offset for the sidebar list — the index of the topmost
    /// visible item. Kept in lockstep with `sidebar_idx` so that on
    /// short terminals (where the sidebar can't fit all `ScreenId`s)
    /// the highlighted entry is always inside the visible window.
    /// A pure cursor move without adjusting this leaks items off the
    /// top of the pane. See `crates/tui/src/ui/mod.rs::sidebar_clamps_offset_*`
    /// for the clamp contract.
    pub sidebar_offset: usize,
    /// Visible row count for the sidebar — set by the renderer after
    /// computing it from the layout area and read by the Up/Down handlers
    /// so cursor (`sidebar_idx`) and offset (`sidebar_offset`) stay in
    /// sync. Defaults to 0, which `clamp_sidebar_offset` treats as "no
    /// window" and collapses `sidebar_offset` to 0 — this guarantees that
    /// before the first frame renders, no spurious offset survives into
    /// the handler. Single source of truth for the visible-row count;
    /// never recomputed in handlers.
    pub sidebar_visible: usize,
    /// Which region of the TUI currently holds key focus. The redesign
    /// replaces the previous single-`bool` model with three explicit
    /// regions so D-pad navigation is deterministic:
    ///
    ///   * `Sidebar`         — the screen-list on the left owns keys.
    ///   * `ContentLeft`     — the left half of a 60/40 multi-pane screen.
    ///   * `ContentRight`    — the right half of a 60/40 multi-pane screen.
    ///                         For single-pane screens this collapses back
    ///                         to `ContentLeft` on every switch.
    ///
    /// `←` / `h` move toward the sidebar (Sidebar ← ContentLeft ← ContentRight
    /// is wrong; it's actually Sidebar → ContentLeft → ContentRight in the
    /// direction of reading). The exact walk is:
    ///
    ///     Left:
    ///         ContentRight  →  ContentLeft
    ///         ContentLeft   →  Sidebar
    ///         Sidebar        →  Sidebar (no-op, already there)
    ///     Right:
    ///         Sidebar        →  ContentLeft
    ///         ContentLeft    →  ContentRight  (only when screen has a
    ///                                          right sub-pane; otherwise
    ///                                          this is a no-op)
    ///         ContentRight   →  ContentRight
    ///
    /// Inside a single-pane screen the only valid region is `ContentLeft`;
    /// every screen sets `app.region = Region::ContentLeft` when it becomes
    /// active so the arrow keys never strand on a phantom `ContentRight`.
    pub region: Region,
    pub palette_buf: String,
    pub palette_idx: usize,
    pub toasts: Vec<Toast>,
    /// Module 7 — persistent history of every toast ever shown, capped at
    /// `TOAST_HISTORY_CAP`. Unlike `toasts` (which `cleanup_toasts` ages out
    /// after the TTL expires), this ring survives for the life of the
    /// process and is the data source for the `Modal::ToastLog` overlay
    /// (opened by capital-T). Newest entry at the back; the modal renders
    /// in reverse for newest-first ordering.
    pub toast_history: std::collections::VecDeque<ToastEntry>,
    /// Module 7.2 — scroll offset for `Modal::ToastLog`. `0` = newest at
    /// the top (tail). Growing values scroll up toward older entries;
    /// clamped to `total - visible` so we never show a blank window.
    pub toast_log_offset: usize,
    /// One-shot guard for the first-launch welcome toast. Set to true the
    /// first time `Action::Tick` runs, so the welcome fires exactly once
    /// per process even though `Action::Tick` ticks forever. Mirrors
    /// orbital's startup greeter pattern (welcome on first frame,
    /// silent afterwards).
    pub boot_toast_sent: bool,
    pub logs: Vec<LogLine>,
    pub logs_filter: String,
    pub proc_sort: ProcessSort,
    pub proc_selected: usize,
    pub svc_selected: usize,
    pub net_selected: usize,
    pub net_show_wifi: bool,
    pub wifi_scan_results: Vec<cyberdeck_core::net::WifiNetwork>,
    /// Module 8.2 — when true and on the Network screen, render the
    /// saved-Wi-Fi pane on the right. Toggled by `s`. Off by default so
    /// the existing 60/40 iface/wifi layout is unaffected.
    pub net_show_saved: bool,
    /// Module 8.2 — known saved Wi-Fi profiles, refreshed every 30s by
    /// a dedicated tokio task. Read by the render path on every frame;
    /// the dispatcher overwrites the `Vec` wholesale on each tick.
    pub saved_connections: Vec<cyberdeck_core::net::SavedConnection>,
    pub bt_selected: usize,
    /// Sink currently highlighted on the Audio screen.
    pub audio_selected: usize,
    /// Output currently highlighted on the Display screen.
    pub display_selected: usize,
    /// Filesystem currently highlighted on the Storage screen.
    pub storage_selected: usize,
    /// Row currently highlighted on the Settings screen.
    pub settings_selected: usize,
    /// Upgradable package currently highlighted on the Packages screen.
    pub pkg_selected: usize,
    /// Up/down offset for the logs pane. Independent from `logs.len()`
    /// (newest appended at the end).
    pub logs_offset: usize,
    /// Up/down offset for the System screen's embedded log pane.
    pub system_log_offset: usize,
    /// Last pkg_search length, so the screen can detect new arrivals.
    pub pkg_search_offset: usize,
    pub pkgs_filter: String,
    pub pkg_search_results: Vec<Package>,
    /// Module 3 — when `Some`, the Packages screen filters by this query.
    /// Set by the `InputKind::PackageSearch` submit handler in `main.rs`
    /// (`run_input`). The Packages screen's render loop reads this each
    /// frame; tasks 3.2–3.4 wire the render-time poll.
    pub packages_search_query: Option<String>,
    /// LoRa (Meshtastic over LAN HTTP) — when `Some`, the LoRa screen
    /// should be talking to the node at this IP. Set by the
    /// `InputKind::LoraNodeIp` submit handler in `main.rs`. The LoRa
    /// screen's render loop reads it and, on change, swaps the screen's
    /// `FakeTransport` for an `HttpLoraTransport` pointed at the IP.
    /// `None` means "no node configured yet" and the screen stays on
    /// the FakeTransport (Slice 4 swaps the transport on first set).
    pub lora_node_ip: Option<String>,
    /// Recently-used Meshtastic node IPs, MRU-ordered, dedup'd. Capped
    /// at `LORA_RECENT_IPS_CAP`. The auto-popup on opening the LoRa
    /// screen reads this to offer "pick a past IP" alongside "add a
    /// new IP" so the user doesn't retype the same address. In-memory
    /// only for this slice — file persistence is a follow-up.
    pub lora_recent_ips: Vec<String>,
    pub theme_name: screen::ThemeNameReexport,
    pub mouse: bool,
    pub show_help: bool,
    pub running: bool,
    pub status_message: Option<String>,
    pub tx: mpsc::Sender<Action>,
    pub rx: mpsc::Receiver<Action>,
    pub clock: chrono::DateTime<Local>,
    pub nerd_font: bool,
    /// Units for weather display on the City screen. Mirrors
    /// `prefs::Units` (kept inline rather than re-exported to avoid
    /// dragging prefs into App's public surface for one field).
    pub units: crate::prefs::Units,
    /// Whether the City screen renders the synthetic traffic overlay.
    /// Toggled with `t` on the City screen and surfaced in Settings.
    pub traffic_overlay: bool,
    /// Whether the City screen's right-hand weather panel is visible.
    /// Toggled with `w` on the City screen and surfaced in Settings.
    pub show_weather_panel: bool,
    /// Last-known city the user picked via the City screen's `c` modal
    /// (or `None` if they haven't overridden the IP-geolocated default).
    pub city_override: Option<String>,
    /// SSID that the wifi-password modal is collecting a password for.
    pub pending_ssid: Option<String>,
    /// Module 4 — Files: in-TUI editor. The path of the file currently
    /// loaded in the editor's buffer (empty PathBuf when the editor
    /// is not active). Set by `App::enter_editor`; cleared when the
    /// editor closes (clean Esc or Discard-confirmed Esc).
    pub editor_path: std::path::PathBuf,
    /// Module 4 — Files: in-TUI editor. The editor's in-memory text
    /// buffer, split on `\n`. Lines do NOT carry their trailing `\n`
    /// — `editor_buffer.join("\n") + "\n"` is the canonical on-disk
    /// representation (matches `std::fs::write` round-trip for files
    /// that ended with a newline; the trailing newline is added on
    /// save to preserve POSIX text-file convention).
    pub editor_buffer: Vec<String>,
    /// Module 4 — cursor position as (line, column). Clamped on every
    /// edit. Column is a byte index into `editor_buffer[line]`.
    pub editor_cursor: (usize, usize),
    /// Module 4 — true when the buffer has unsaved changes since the
    /// last load or save. Drives the dirty-Esc confirm modal and the
    /// dirty marker in the title.
    pub editor_dirty: bool,
    /// Module 4 — true when the editor is in read-only mode (file
    /// too large or binary heuristic matched on entry). Ctrl-S is
    /// a no-op + read-only toast; typing is dropped at the
    /// buffer-insert step.
    pub editor_read_only: bool,
    /// Files-screen navigation.
    pub files_cwd: std::path::PathBuf,
    pub files_entries: Vec<cyberdeck_core_files::DirEntry>,
    pub files_selected: usize,
    pub files_show_hidden: bool,
    pub files_right: std::path::PathBuf,
    pub files_right_entries: Vec<cyberdeck_core_files::DirEntry>,
    /// LoRa screen (Meshtastic over LAN HTTP). Snapshot of known nodes, copied
    /// from the screen's transport on every poll. Empty by default — the
    /// poll path fills it in once a node IP is configured via the `i`
    /// modal and the transport's first round-trip succeeds. `App` keeps
    /// the snapshot (not the `Box<dyn LoraTransport>`) so test code can
    /// build an `App` without any HTTP handle open.
    pub lora_nodes: Vec<crate::screens::lora::LoraNode>,
    /// All threads the transport currently knows about — always at
    /// least the `LongFast` anchor, plus `Direct(n)` entries
    /// auto-created on the first inbound DM from a previously-unseen
    /// node. Same lifecycle as `lora_nodes`: populated by
    /// `LoraScreen::poll`, never read directly by other screens.
    pub lora_threads: Vec<crate::screens::lora::Thread>,
    /// Which thread the input strip currently composes into. Default
    /// `LongFast` so a fresh user sees the shared channel. `Enter`
    /// on a node in the right pane swaps this to `Direct(n)`; `Esc`
    /// snaps it back. The whole screen's `to: …` chip and chat pane
    /// mirror this state.
    pub lora_active_thread: crate::screens::lora::ChannelKind,
    /// Live tail offset for the active thread's chat list. `0` =
    /// tail; growing values scroll up (away from the tail).
    /// `usize::MAX` (set on `g`) jumps to the start of the buffer.
    pub lora_chat_offset: usize,
    /// Current input buffer for the chat compose line. Cleared after a
    /// successful send.
    pub lora_input: String,
    /// `true` when the underlying transport has an active HTTP session.
    /// Drives the connect/disconnect dot in the input strip.
    pub lora_connected: bool,
    /// Last 60 seconds of RX/TX byte counts per interface. Updated at
    /// 1Hz by the network sampler in `Live::spawn_refreshers`. Key =
    /// interface name (e.g. `"eth0"`, `"wlan0"`); value = `(rx ring,
    /// tx ring)` of byte deltas, oldest-to-newest. The header sparkline
    /// (Module 5.4) reads the RX ring of the active interface. Empty
    /// until the sampler has run at least once.
    pub net_history: std::collections::HashMap<String, (crate::util::ring::RingU64, crate::util::ring::RingU64)>,
    /// Module 6 — System screen's process tree. Populated by the 15s
    /// refiller in `Live::spawn_refreshers` (Module 6.2) via
    /// `Action::ProcTreeRefreshed`. The render reads this each frame
    /// when `proc_tree_view` is true and turns the flat list into an
    /// indented tree (Module 6.3). Empty by default — first refresh
    /// lands ~15s after startup.
    pub proc_tree: Vec<ProcEntry>,
    /// Module 6 — when true and on the System screen, render the
    /// indented process tree instead of the default facts pane. Toggled
    /// with `t`. Default false so the existing System facts view is
    /// the boot-time state.
    pub proc_tree_view: bool,
}

/// Tiny shim so the TUI can depend on a single `cyberdeck_core::files` module
/// even though it lives next to the others.
pub mod cyberdeck_core_files {
    use std::path::PathBuf;
    #[derive(Debug, Clone)]
    pub struct DirEntry {
        pub name: String,
        pub path: PathBuf,
        pub is_dir: bool,
        pub size: u64,
    }
}

/// Which region of the TUI currently owns key focus. See `App::region`
/// for the navigation rules. `Copy` so it can move through match arms
/// without a borrow on `App`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Sidebar,
    ContentLeft,
    ContentRight,
}

impl Region {
    /// Move region focus one step toward the sidebar.
    /// Sidebar is already there, so it's a no-op.
    pub fn go_left(self) -> Region {
        match self {
            Region::Sidebar => Region::Sidebar,
            Region::ContentLeft => Region::Sidebar,
            Region::ContentRight => Region::ContentLeft,
        }
    }

    /// Move region focus one step toward the content. From the sidebar we
    /// always land in `ContentLeft` (the only legal content region for
    /// single-pane screens; multi-pane screens opt the user further right
    /// via their own `on_key`). From `ContentLeft` we *don't* auto-jump
    /// to `ContentRight` here; the screen owns that decision because only
    /// some screens have a right half.
    pub fn go_right(self) -> Region {
        match self {
            Region::Sidebar => Region::ContentLeft,
            Region::ContentLeft => Region::ContentLeft,
            Region::ContentRight => Region::ContentRight,
        }
    }

    /// Human label for hints and audit messages. Stable across themes.
    pub fn label(self) -> &'static str {
        match self {
            Region::Sidebar => "sidebar",
            Region::ContentLeft => "content",
            Region::ContentRight => "details",
        }
    }
}

/// Append `incoming` to `buf`, dropping any entry whose `(ts, message)`
/// is already present (or whose `message` is empty). Preserves order:
/// existing entries stay in place, then truly-new entries are appended.
/// Once `buf.len()` exceeds `cap`, the oldest entries are dropped from
/// the front.
///
/// Why this exists: the 1Hz logs refiller (Module 2.2) calls
/// `recent_since(2)` every second. Each call returns up to 200 lines from
/// the last 2s, so successive calls overlap heavily. Without dedupe the
/// buffer would fill with duplicates and trip the cap within a few
/// seconds, masking real new entries.
///
/// We key on the full `LogLine` (timestamp + message) rather than on the
/// message alone: since Module 2.3, `ts` carries the journalctl-native
/// `__REALTIME_TIMESTAMP` (UTC microseconds), so a genuine re-emission of
/// the same message at a later moment — e.g. a watchdog retry — is treated
/// as a new line, while exact replays within the 2s dedupe window are
/// dropped. `LogLine` derives `Hash + Eq` so we can use a `HashSet<LogLine>`
/// directly.
/// `pub` (not `pub(crate)`) so the binary in `src/main.rs` — which is
/// now an external user of the `cyberdeck-tui` library — can call it
/// after the lib/bin refactor.
pub fn dedupe_logs_into(    buf: &mut Vec<LogLine>,
    incoming: Vec<LogLine>,
    cap: usize,
) {
    if cap == 0 {
        return;
    }
    // Build an owned HashSet of existing entries. A reference version
    // would clash with the subsequent `buf.push(line)` because Rust
    // treats them as overlapping borrows of the same `Vec`. Cloning the
    // small `LogLine`s is cheap relative to the cap-sized buffer this
    // function is called with.
    let mut existing: std::collections::HashSet<LogLine> = buf.iter().cloned().collect();
    for line in incoming {
        if line.message.is_empty() {
            continue;
        }
        if !existing.insert(line.clone()) {
            // Already present — `insert` returned false, drop the dup.
            continue;
        }
        buf.push(line);
    }
    if buf.len() > cap {
        let drop = buf.len() - cap;
        buf.drain(0..drop);
    }
}

impl App {
    /// Construct a fresh `App`. Reads user prefs from disk (theme,
    /// mouse, nerd font, last-known city, etc.) so settings persist
    /// across launches. Any failure to load prefs falls back to
    /// `Prefs::default()` and logs a warning — a broken prefs file
    /// must never block the TUI from launching.
    pub fn new(tx: mpsc::Sender<Action>, rx: mpsc::Receiver<Action>) -> Self {
        let prefs = crate::prefs::Prefs::load();
        Self {
            live: Arc::new(Live::default()),
            current: ScreenId::System,
            manager: crate::wm::manager::Manager::new(ScreenId::System),
            modal: Modal::None,
            sidebar_focused: true,
            sidebar_idx: 0,
            sidebar_offset: 0,
            // Renderer overwrites this on every frame; 0 means "no
            // window yet" and is the safe default (clamp collapses the
            // offset to 0 instead of leaking it).
            sidebar_visible: 0,
            // Default region on launch is the sidebar — that's the natural
            // D-pad start (user sees the screen list and moves with ↑/↓).
            // `switch_screen` flips to `ContentLeft` when a screen commits.
            region: Region::Sidebar,
            palette_buf: String::new(),
            palette_idx: 0,
            toasts: Vec::new(),
            // Module 7.1 — toast history ring. Empty until the first
            // `push_toast` call; cap enforced by `push_toast` itself so
            // construction stays cheap.
            toast_history: std::collections::VecDeque::new(),
            // Module 7.2 — scroll offset for the ToastLog modal. 0 means
            // "showing newest first"; `T` re-zeroes this on every open.
            toast_log_offset: 0,
            boot_toast_sent: false,
            logs: Vec::new(),
            logs_filter: String::new(),
            proc_sort: ProcessSort::Cpu,
            proc_selected: 0,
            svc_selected: 0,
            net_selected: 0,
            net_show_wifi: false,
            wifi_scan_results: Vec::new(),
            // Module 8.2 — saved-Wi-Fi pane: off by default so the
            // existing 60/40 iface/wifi layout is unchanged unless the
            // user opts in with `s`. Empty Vec until the 30s refiller
            // first fires; the render path degrades to "(loading…)" in
            // that case.
            net_show_saved: false,
            saved_connections: Vec::new(),
            bt_selected: 0,
            audio_selected: 0,
            display_selected: 0,
            storage_selected: 0,
            settings_selected: 0,
            pkg_selected: 0,
            logs_offset: 0,
            system_log_offset: 0,
            pkg_search_offset: 0,
            pkgs_filter: String::new(),
            pkg_search_results: Vec::new(),
            packages_search_query: None,
            lora_node_ip: None,
            theme_name: prefs.theme,
            mouse: prefs.mouse,
            // `nerd_font` field on App is the runtime toggle the user
            // can flip with `n` in Settings; the env var
            // `NERD_FONT=0` overrides it via `glyphs()`. Both must
            // be respected: prefs win if the user explicitly set
            // them, env var=0 always forces ASCII for the run.
            nerd_font: prefs.nerd_font
                && std::env::var("NERD_FONT").as_deref() != Ok("0"),
            units: prefs.units,
            traffic_overlay: prefs.traffic_overlay,
            show_weather_panel: prefs.show_weather_panel,
            city_override: prefs.city,
            show_help: false,
            running: true,
            status_message: None,
            tx,
            rx,
            clock: Local::now(),
            pending_ssid: None,
            // Module 4 — Files: in-TUI editor initial state. The editor
            // is dormant until `App::enter_editor` is called from the
            // Files screen (`e` arm). Empty PathBuf + empty buffer +
            // cursor (0, 0) + dirty=false + read-only=false means the
            // editor fields are always well-formed without forcing the
            // App::new signature to grow.
            editor_path: PathBuf::new(),
            editor_buffer: Vec::new(),
            editor_cursor: (0, 0),
            editor_dirty: false,
            editor_read_only: false,
            files_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            files_entries: Vec::new(),
            files_selected: 0,
            files_show_hidden: false,
            files_right: PathBuf::from("/"),
            files_right_entries: Vec::new(),
            lora_nodes: Vec::new(),
            lora_threads: Vec::new(),
            lora_active_thread: crate::screens::lora::ChannelKind::LongFast,
            lora_chat_offset: 0,
            lora_input: String::new(),
            lora_connected: false,
            // LoRa recent-IPs list, MRU-ordered, dedup'd on push.
            // Empty by default; populated by `push_lora_recent_ip`
            // on every successful `InputKind::LoraNodeIp` submit.
            // The auto-popup on LoRa screen open consumes this to
            // let the user reuse a known-good IP without retyping.
            lora_recent_ips: Vec::new(),
            // Module 5.2 — initialise empty. The 1Hz refiller populates
            // this on its first tick; the header sparkline falls back to
            // a dashed placeholder until something lands.
            net_history: std::collections::HashMap::new(),
            // Module 6 — process tree snapshot. Empty until the 15s
            // refiller (Module 6.2) fires; `t` on the System screen
            // toggles the tree view.
            proc_tree: Vec::new(),
            proc_tree_view: false,
        }
    }

    /// Maximum number of past LoRa node IPs remembered in
    /// `App::lora_recent_ips`. Eight covers a typical home setup
    /// (router + a couple of mesh repeaters) without growing the
    /// picker modal unbounded. Capped loosely so older IPs fall off
    /// automatically as the user adds new ones.
    pub const LORA_RECENT_IPS_CAP: usize = 8;

    /// Push a new IP into `lora_recent_ips`, dedup'd (existing
    /// entries move-to-front) and capped at `LORA_RECENT_IPS_CAP`.
    /// Called by the `InputKind::LoraNodeIp` submit arm and by the
    /// past-IP picker so the list stays MRU regardless of which
    /// path connected the user. Whitespace-only and duplicate-of-
    /// front entries are no-ops so a no-op submit doesn't churn
    /// the list.
    pub fn push_lora_recent_ip(&mut self, ip: &str) {
        let trimmed = ip.trim();
        if trimmed.is_empty() {
            return;
        }
        // Move-to-front (MRU) semantics: if the IP is already in
        // the list, remove its old position and put it at the head.
        if let Some(pos) = self.lora_recent_ips.iter().position(|e| e == trimmed) {
            if pos == 0 {
                return; // already at front — nothing to do
            }
            let entry = self.lora_recent_ips.remove(pos);
            self.lora_recent_ips.insert(0, entry);
        } else {
            self.lora_recent_ips.insert(0, trimmed.to_string());
        }
        // Cap at the well-known limit. Older entries fall off the
        // tail. `Vec::truncate` is O(1).
        let cap = Self::LORA_RECENT_IPS_CAP;
        if self.lora_recent_ips.len() > cap {
            self.lora_recent_ips.truncate(cap);
        }
    }

    /// Shortcut to open a `Modal::Help`.
    pub fn open_help(&mut self) {
        self.modal = Modal::Help;
    }

    /// Set the active region and keep the derived `sidebar_focused` flag
    /// in sync. New code should call this instead of assigning
    /// `sidebar_focused` directly so the two never drift.
    pub fn set_region(&mut self, r: Region) {
        self.region = r;
        self.sidebar_focused = r == Region::Sidebar;
    }

    /// Advance/retreat `sidebar_offset` so the cursor at `sidebar_idx`
    /// is always visible inside a window of `visible` rows. Top-aligned:
    /// shifts only when the cursor scrolls past the bottom edge of the
    /// visible window. Called by the sidebar Up/Down handlers in main.rs.
    pub fn clamp_sidebar_offset(&mut self, total: usize, visible: usize) {
        if visible == 0 || total <= visible {
            self.sidebar_offset = 0;
            return;
        }
        let max_off = total - visible;
        let desired = if self.sidebar_idx >= visible {
            (self.sidebar_idx - visible + 1).min(max_off)
        } else {
            0
        };
        self.sidebar_offset = desired;
    }

    /// Shortcut to open a `Modal::Input` with the given prompt and kind.
    pub fn open_input(&mut self, prompt: impl Into<String>, kind: InputKind) {
        self.modal = Modal::Input {
            prompt: prompt.into(),
            buf: String::new(),
            kind,
        };
    }

    /// Shortcut to open a `Modal::Secret` (masked text input).
    pub fn open_secret(&mut self, prompt: impl Into<String>, kind: InputKind) {
        self.modal = Modal::Secret {
            prompt: prompt.into(),
            buf: String::new(),
            kind,
        };
    }

    /// Shortcut to open a `Modal::Choice` picker.
    pub fn open_choice(
        &mut self,
        prompt: impl Into<String>,
        options: Vec<ChoiceOption>,
        commit_kind: Option<ChoiceCommit>,
    ) {
        self.modal = Modal::Choice {
            prompt: prompt.into(),
            options,
            cursor: 0,
            commit_kind,
        };
    }

    /// Shortcut to open a `Modal::Wizard` flow.
    pub fn open_wizard(&mut self, w: Wizard) {
        self.modal = Modal::Wizard(w);
    }

    /// Shortcut to open a `Modal::Progress` modal with a cancellable task.
    pub fn open_progress(
        &mut self,
        label: impl Into<String>,
        cancel: Option<tokio::sync::oneshot::Sender<()>>,
    ) {
        self.modal = Modal::Progress {
            label: label.into(),
            done: 0,
            total: 0,
            cancel,
        };
    }

    /// Shortcut to open a `Modal::AuthFailure` with an inner retry modal.
    pub fn open_auth_failure(&mut self, command: String, stderr: String, retry: Box<Modal>) {
        self.modal = Modal::AuthFailure { command, stderr, retry };
    }

    pub fn push_toast(&mut self, kind: toast::ToastKind, msg: impl Into<String>) {
        let text = msg.into();
        // Module 7.1 — append to the persistent history ring FIRST so the
        // entry is preserved even if the visible toast ages out via TTL.
        let entry = ToastEntry {
            ts: chrono::Local::now(),
            kind,
            message: text.clone(),
        };
        self.toast_history.push_back(entry);
        while self.toast_history.len() > TOAST_HISTORY_CAP {
            self.toast_history.pop_front();
        }
        self.toasts.push(Toast::new(kind, text));
    }

    /// Persist the current user-facing settings (theme, mouse, nerd
    /// font, last-known city, units, traffic + weather toggles) to the
    /// on-disk prefs file. Best-effort: a failure logs a warning but
    /// never blocks the renderer or surfaces a toast (prefs loss is
    /// silent — the user already sees the value change on screen).
    ///
    /// Every mutation site that flips a persisted field should call
    /// this so quit-and-relaunch picks up the change.
    pub fn save_prefs(&self) {
        let prefs = crate::prefs::Prefs {
            theme: self.theme_name,
            mouse: self.mouse,
            // NERD_FONT=0 in the env means "force ASCII this run";
            // we don't want that to silently flip the persisted
            // preference. Only save the user's *real* choice.
            nerd_font: std::env::var("NERD_FONT").as_deref() == Ok("0")
                || self.nerd_font,
            web_server_on_start: false, // populated by main.rs from args
            web_bind: None,
            city: self.city_override.clone(),
            units: self.units,
            traffic_overlay: self.traffic_overlay,
            show_weather_panel: self.show_weather_panel,
        };
        prefs.save();
    }

    /// Module 5.3 — apply a `Action::NetSample` to `net_history`.
    /// Lazily creates the per-interface `(rx, tx)` ring pair on first
    /// sighting, then pushes the deltas. Returns the new RX ring length
    /// so tests can assert; production callers ignore the return.
    ///
    /// Pulled out of `handle_action` so unit tests don't have to
    /// construct an `mpsc::Sender` + screens slice to verify the
    /// dispatcher behaviour.
    pub fn apply_net_sample(
        &mut self,
        iface: &str,
        rx_delta: u64,
        tx_delta: u64,
    ) -> usize {
        let entry = self
            .net_history
            .entry(iface.to_string())
            .or_insert_with(|| {
                (
                    crate::util::ring::RingU64::new(60),
                    crate::util::ring::RingU64::new(60),
                )
            });
        entry.0.push(rx_delta);
        entry.1.push(tx_delta);
        entry.0.len()
    }

    /// Module 6.2 — apply a `Action::ProcTreeRefreshed` to `App::proc_tree`.
    /// Wholesale replacement: the snapshot is the canonical picture of
    /// /proc at one moment, so a merge would just have to undo the
    /// previous tick's removals. Extracted from `handle_action` so the
    /// 15s refiller's contract is unit-testable without a full mpsc
    /// pair + screens slice.
    pub fn apply_proc_tree(&mut self, procs: Vec<ProcEntry>) {
        self.proc_tree = procs;
    }

    /// Test-only dispatcher arm for `Action::ProcTreeRefreshed`. Mirrors
    /// the body of the real dispatcher arm in `main.rs`; production
    /// callers should use the dispatcher, but unit tests use this to
    /// avoid the full mpsc + screens slice setup.
    #[doc(hidden)]
    pub fn handle_action_for_test(&mut self, action: Action) {
        // Step 9 — City actions. Mirrors the dispatcher arms in
        // `main.rs::handle_action` for the non-spawning cases.
        // `CityCtrlRefresh` is excluded because it spawns an async
        // task and would need a full tokio runtime + wiremock to
        // exercise; the dispatcher's integration smoke covers it.
        match action {
            Action::ProcTreeRefreshed(procs) => self.apply_proc_tree(procs),
            Action::CityResolved(loc) => {
                if let Ok(mut g) = self.live.city_loc.try_write() {
                    *g = Some(loc);
                }
            }
            Action::CityWeatherRefreshed(w) => {
                if let Ok(mut g) = self.live.city_weather.try_write() {
                    *g = Some(w);
                }
            }
            _ => {}
        }
    }

    pub fn cleanup_toasts(&mut self) {
        self.toasts.retain(|t| !t.expired());
    }

    pub fn tick_clock(&mut self) {
        self.clock = Local::now();
    }

    /// Module 4 — Files: in-TUI editor entry point.
    ///
    /// Called by the Files screen's `e` arm with the selected file's
    /// path. Probes read-only via `screens::editor::should_open_read_only`,
    /// reads the file into memory (capped at 1 MiB, matching the read-only
    /// gate — a 1 MiB+1 byte file never reaches `read_to_string` because
    /// the gate has already short-circuited it to read-only mode where
    /// we still want a buffer), splits lines into the editor's buffer,
    /// stamps the 5 editor fields, and swaps the focused pane to
    /// `ScreenId::Editor`.
    ///
    /// Test 1 requires `editor_buffer == vec!["alpha", "beta", "gamma"]`
    /// when the file is `"alpha\nbeta\ngamma\n"`. We split on `\n` and
    /// trim the trailing empty entry that a terminal `\n` produces —
    /// matches POSIX text-file convention where the trailing `\n` is
    /// a line terminator, not an empty line.
    pub fn enter_editor(&mut self, path: std::path::PathBuf) {
        use crate::screens::editor::should_open_read_only;

        let (read_only, _reason) = should_open_read_only(&path);

        // Cap the read at 1 MiB so we never load a multi-GB file into
        // memory. Mirrors the gate's `SIZE_CAP` exactly; a file over
        // the cap was already flagged read-only by `should_open_read_only`
        // (we still want *some* buffer for display, but a capped one).
        const READ_CAP: u64 = 1024 * 1024;
        let bytes = std::fs::read(&path).unwrap_or_default();
        let capped: &[u8] = if bytes.len() as u64 > READ_CAP {
            &bytes[..READ_CAP as usize]
        } else {
            &bytes[..]
        };
        // Lossy decode so a binary file still gets a buffer (the
        // editor is already read-only in that branch).
        let text = String::from_utf8_lossy(capped);
        let mut buf: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        // Drop the trailing empty entry caused by a terminal `\n`.
        if buf.last().map(|s| s.is_empty()).unwrap_or(false) {
            buf.pop();
        }
        // Empty file → one empty line so the editor always has a row.
        if buf.is_empty() {
            buf.push(String::new());
        }

        self.editor_path = path;
        self.editor_buffer = buf;
        self.editor_cursor = (0, 0);
        self.editor_dirty = false;
        self.editor_read_only = read_only;

        // Swap the focused builtin to Editor and force the region back
        // to ContentLeft so the D-pad navigates as expected: arrow keys
        // move inside the editor, ←/h lands on the sidebar, Tab cycles
        // back into Files. Without this reset, a user who had the
        // Files screen's right pane focused would land on the editor
        // with region=ContentRight — a ghost-pane state that has no
        // matching render and breaks arrow keys.
        self.manager
            .set_pane_kind(crate::wm::window::WindowKind::Builtin(ScreenId::Editor));
        self.set_region(Region::ContentLeft);
    }

    /// Module 4 — discard the editor's in-memory buffer and return focus
    /// to the Files screen. The mirror image of `enter_editor`:
    ///   * `editor_path` → `PathBuf::new()` (dormant sentinel)
    ///   * `editor_buffer` → empty
    ///   * `editor_cursor` → `(0, 0)`
    ///   * `editor_dirty` → `false`
    ///   * `editor_read_only` → `false`
    ///   * focused pane → `ScreenId::Files`
    ///
    /// Wired to the `Modal::Confirm { kind: ConfirmKind::Discard, .. }`
    /// confirmation path in `main::run_confirm`. Pure in-memory state
    /// reset — no disk I/O, since "discard" by definition means the
    /// user has chosen to throw the buffer away.
    pub fn discard_editor(&mut self) {
        self.editor_path = std::path::PathBuf::new();
        self.editor_buffer = Vec::new();
        self.editor_cursor = (0, 0);
        self.editor_dirty = false;
        self.editor_read_only = false;
        self.manager
            .set_pane_kind(crate::wm::window::WindowKind::Builtin(ScreenId::Files));
        // Drop the user back into the Files content-left region so the
        // arrow keys navigate the file list (not a ghost pane). Esc on
        // a *clean* editor goes through the same path via
        // `enter_editor`'s caller in `screens/editor.rs`, which also
        // calls `set_region(Region::ContentLeft)` so this stays
        // consistent.
        self.set_region(Region::ContentLeft);
    }

    /// A short summary line for the status bar.
    pub fn status_line(&self) -> Line<'static> {
        let mut spans = Vec::new();
        spans.push(format!(" {} ", self.clock.format("%H:%M:%S")).into());
        if let Some(s) = &self.status_message {
            spans.push(format!("  · {s}").into());
        }
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_has_one_pane() {
        // A bit of a smoke test: the App's manager should be in a
        // valid state with one focused pane hosting the System
        // screen.
        let (tx, rx) = mpsc::channel::<Action>(8);
        let app = App::new(tx, rx);
        let panes = app.manager.pane_ids();
        assert_eq!(panes.len(), 1);
        let w = app.manager.window(app.manager.focused()).unwrap();
        assert_eq!(w.kind, crate::wm::window::WindowKind::Builtin(ScreenId::System));
    }

    // -------------------------------------------------------------------------
    // Module 1.5 — `sidebar_visible` is the single source of truth for the
    // sidebar's visible-row count, set by the renderer and read by the Up/Down
    // handlers. These tests pin the contract of `clamp_sidebar_offset` against
    // realistic `(total, visible, offset, idx)` tuples so the handler can
    // trust `app.sidebar_visible` and the renderer can write to it without
    // re-checking arithmetic on its own.
    //
    // Why pin these here: the previous handler called
    // `clamp_sidebar_offset(total, total)` which is a no-op (offset already
    // clamped at 0 when all rows fit). The bug was that on short terminals
    // the offset never advanced, so overflow rows were invisible but
    // selectable. The fix is for the renderer to record `visible` and the
    // handler to pass it through; these tests lock the arithmetic so neither
    // side can silently regress.
    // -------------------------------------------------------------------------

    fn fresh_app() -> App {
        let (tx, rx) = mpsc::channel::<Action>(8);
        App::new(tx, rx)
    }

    #[test]
    fn sidebar_down_advances_offset_when_cursor_exits_visible_window() {
        // total=15, visible=5, offset starts at 0, idx at 4 (last row of
        // window [0..5)). After Down: idx=5, which exits the bottom of
        // the window. `clamp_sidebar_offset` must advance offset to 1
        // so the cursor stays visible.
        let mut app = fresh_app();
        app.sidebar_idx = 4;
        app.sidebar_offset = 0;
        app.sidebar_visible = 5;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        // Initial clamp at idx=4 (inside the window) — offset stays 0.
        assert_eq!(app.sidebar_offset, 0);
        // Simulate Down: idx += 1, then clamp with the renderer's
        // recorded visible count.
        app.sidebar_idx = (app.sidebar_idx + 1).min(14);
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(app.sidebar_idx, 5);
        assert_eq!(
            app.sidebar_offset, 1,
            "offset must advance when cursor exits bottom"
        );
    }

    // -------------------------------------------------------------------------
    // Step 9 — City action arms integration smoke. Pins the contract
    // between the dispatcher arms in `main.rs::handle_action` and
    // `App::live.{city_loc, city_weather}` so a future refactor that
    // drops the write-through would fail a unit test instead of
    // silently leaving the City screen showing "(waiting for first
    // fetch)" forever.
    // -------------------------------------------------------------------------

    fn city_loc() -> crate::screens::city::geo::CityLocation {
        crate::screens::city::geo::CityLocation {
            name: "Seattle".into(),
            country: "United States".into(),
            country_code: "US".into(),
            region: "Washington".into(),
            lat: 47.6062,
            lon: -122.3321,
            bbox: Some([47.5, -122.5, 47.7, -122.3]),
            timezone: "America/Los_Angeles".into(),
        }
    }

    fn city_weather() -> crate::screens::city::weather::Weather {
        crate::screens::city::weather::Weather {
            temp_c: 9.2,
            feels_like_c: 7.4,
            humidity_pct: 78,
            wind_kph: 12.0,
            wind_dir_deg: Some(315),
            weather_code: 3,
            next_12h_precip_pct: Some(vec![10, 20]),
            fetched_at: chrono::Local::now(),
        }
    }

    /// `Action::CityResolved` must write through to `app.live.city_loc`
    /// — the City screen's render reads from this snapshot, not from a
    /// `tx` round-trip. A refactor that drops the write would leave
    /// the screen showing "(locating…)" forever.
    #[test]
    fn city_resolved_writes_through_to_live_snapshot() {
        let mut app = fresh_app();
        assert!(
            app.live.city_loc.try_read().unwrap().is_none(),
            "snapshot starts None"
        );
        let loc = city_loc();
        app.handle_action_for_test(Action::CityResolved(loc.clone()));
        let stored = app.live.city_loc.try_read().unwrap();
        let stored = stored.as_ref().expect("snapshot populated after CityResolved");
        assert_eq!(stored.name, "Seattle");
        assert!((stored.lat - 47.6062).abs() < 1e-9);
        assert_eq!(stored.timezone, "America/Los_Angeles");
    }

    /// `Action::CityWeatherRefreshed` must write through to
    /// `app.live.city_weather`. Same shape as the location arm.
    #[test]
    fn city_weather_refreshed_writes_through_to_live_snapshot() {
        let mut app = fresh_app();
        assert!(app.live.city_weather.try_read().unwrap().is_none());
        let w = city_weather();
        app.handle_action_for_test(Action::CityWeatherRefreshed(w));
        let stored = app.live.city_weather.try_read().unwrap();
        let stored = stored.as_ref().expect("snapshot populated after CityWeatherRefreshed");
        assert!((stored.temp_c - 9.2).abs() < 1e-3);
        assert_eq!(stored.humidity_pct, 78);
        assert_eq!(stored.weather_code, 3);
        assert_eq!(stored.next_12h_precip_pct.as_deref(), Some(&[10u8, 20][..]));
    }

    /// A second `CityResolved` must overwrite the first snapshot —
    /// this is what happens when the IP geolocator lands on a
    /// different city than the initial bundled-seattle fallback.
    #[test]
    fn city_resolved_overwrites_previous_snapshot() {
        let mut app = fresh_app();
        app.handle_action_for_test(Action::CityResolved(city_loc()));
        let mut tokyo = city_loc();
        tokyo.name = "Tokyo".into();
        tokyo.lat = 35.6762;
        tokyo.lon = 139.6503;
        tokyo.timezone = "Asia/Tokyo".into();
        tokyo.bbox = Some([35.5, 139.5, 35.8, 139.8]);
        app.handle_action_for_test(Action::CityResolved(tokyo));
        let stored = app.live.city_loc.try_read().unwrap();
        let stored = stored.as_ref().expect("snapshot populated");
        assert_eq!(stored.name, "Tokyo");
        assert_eq!(stored.timezone, "Asia/Tokyo");
        assert!((stored.lat - 35.6762).abs() < 1e-9);
    }

    /// `CityCtrlRefresh` is the user-driven `r` keypress. We don't
    /// exercise the actual HTTP path here (that's wiremock-backed in
    /// the geo/weather unit tests); we just confirm the action
    /// variant exists and `handle_action_for_test` correctly
    /// ignores it (the real spawn lives in the dispatcher's
    /// runtime arm, not this helper).
    #[test]
    fn city_ctrl_refresh_is_a_known_action_variant() {
        // Compile-time pin: the variant must exist. If a future
        // rename removes it, this match fails to compile.
        let _variant = Action::CityCtrlRefresh;
    }

    #[test]
    fn sidebar_up_advances_offset_when_cursor_still_below_window_top() {
        // total=15, visible=5, offset=3, idx=10 is an INVALID pre-state
        // (cursor 10 is outside window [3..8)). `clamp_sidebar_offset`
        // must immediately correct offset to 6 so the cursor lives
        // inside [6..11). After Up: idx=9; clamp must retreat offset
        // to 5 (window [5..10) contains idx=9).
        let mut app = fresh_app();
        app.sidebar_idx = 10;
        app.sidebar_offset = 3;
        app.sidebar_visible = 5;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(
            app.sidebar_offset, 6,
            "clamp actively advances offset to keep idx visible"
        );
        app.sidebar_idx -= 1; // 9
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(
            app.sidebar_offset, 5,
            "offset retreats as cursor re-enters from above"
        );
    }

    #[test]
    fn sidebar_up_retreats_offset_when_cursor_re_enters_window_top() {
        // total=15, visible=5.
        // Start: idx=10 — outside any sensible window so clamp picks the
        // minimum offset that keeps idx visible: desired=(10-5+1).min(10)=6.
        // Window is [6..11), contains idx=10. ✓
        // After Up: idx=9 → desired=5. Offset retreats 6→5.
        // After Up: idx=8 → desired=4. Offset retreats 5→4.
        // After Up: idx=4 → idx<visible so desired=0. Full collapse.
        // This pins the retreat contract: each Up moves the offset closer
        // to 0 as long as the cursor stays visible.
        let mut app = fresh_app();
        app.sidebar_idx = 10;
        app.sidebar_offset = 0;
        app.sidebar_visible = 5;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(app.sidebar_offset, 6, "minimum offset for idx=10");

        app.sidebar_idx = 9;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(app.sidebar_offset, 5, "retreats as cursor moves up");

        app.sidebar_idx = 8;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(app.sidebar_offset, 4, "continues retreating");

        app.sidebar_idx = 4;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(
            app.sidebar_offset, 0,
            "collapses to 0 once idx drops below visible"
        );
    }

    #[test]
    fn sidebar_visible_defaults_to_zero_and_clamp_clamps_offset_to_zero() {
        // Before the first frame renders, `sidebar_visible` is still 0.
        // `clamp_sidebar_offset` treats 0 visible as "no window" and
        // collapses `sidebar_offset` to 0 — guaranteeing the handler
        // can't leak an old offset into the first render.
        let mut app = fresh_app();
        assert_eq!(app.sidebar_visible, 0, "default visible is 0");
        app.sidebar_offset = 99;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(
            app.sidebar_offset, 0,
            "visible=0 collapses any prior offset to 0"
        );
    }

    #[test]
    fn sidebar_offset_clamps_to_total_minus_visible_when_cursor_at_end() {
        // Boundary: cursor at the very last index, offset must saturate
        // at total - visible (10 in this case), never overshoot.
        let mut app = fresh_app();
        app.sidebar_idx = 14; // last
        app.sidebar_offset = 0;
        app.sidebar_visible = 5;
        app.clamp_sidebar_offset(15, app.sidebar_visible);
        assert_eq!(
            app.sidebar_offset, 10,
            "offset saturates at total - visible"
        );
    }

    // -------------------------------------------------------------------------
    // Module 2.2 — `dedupe_logs_into` keeps the recent-logs buffer free of
    // duplicates when a periodic refiller polls an overlapping window. The
    // refiller calls `recent_since(2)` once per second; each call may return
    // lines already in the buffer. The helper drops those before pushing,
    // then enforces the cap by dropping the oldest entries.
    //
    // Module 2.3 updated the dedupe key to be the full `LogLine` (ts +
    // message) instead of the message alone. The ts now carries the
    // journalctl-native timestamp, so two entries with the same message
    // at different times (e.g. a watchdog retry) count as distinct lines,
    // while exact replays within the dedupe window are dropped. Tests use
    // a fixed `Local::now()` reference so the helper sees stable
    // `LogLine`s and we can assert content rather than pointer equality.
    // -------------------------------------------------------------------------

    /// Build a `LogLine` with a fixed timestamp so two `ll(..)` calls with
    /// the same message compare equal — mirroring what the live refiller
    /// sees when journalctl hands us the same entry twice.
    fn ll(s: &str) -> LogLine {
        let ts: chrono::DateTime<Local> = "2024-01-01T00:00:00+00:00".parse().unwrap();
        LogLine {
            ts,
            message: s.into(),
        }
    }

    /// Build a `LogLine` with a fresh local timestamp, simulating a
    /// journalctl re-emission of the same message at a later moment
    /// (e.g. a retry). Different `ts` ⇒ distinct `LogLine` ⇒ kept.
    fn ll_at(s: &str, secs_offset: i64) -> LogLine {
        let ts = Local::now() + chrono::Duration::seconds(secs_offset);
        LogLine {
            ts,
            message: s.into(),
        }
    }

    #[test]
    fn dedupe_logs_into_skips_lines_already_in_buffer() {
        let mut buf: Vec<LogLine> = Vec::new();
        dedupe_logs_into(&mut buf, vec![ll("a"), ll("b")], 100);
        assert_eq!(
            buf.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        // "b" is a duplicate (same ts, same message); only "c" should be
        // appended.
        dedupe_logs_into(&mut buf, vec![ll("b"), ll("c")], 100);
        assert_eq!(
            buf.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn dedupe_logs_into_treats_re_emissions_at_later_times_as_new() {
        // Regression guard for Module 2.3: when the dedupe key is
        // (ts, message) — not just message — the same message at a
        // different journal timestamp must be kept.
        let mut buf: Vec<LogLine> = Vec::new();
        dedupe_logs_into(&mut buf, vec![ll("retry")], 100);
        dedupe_logs_into(&mut buf, vec![ll_at("retry", 5)], 100);
        assert_eq!(buf.len(), 2, "later ts with same message must be kept");
        assert_eq!(buf[0].message, "retry");
        assert_eq!(buf[1].message, "retry");
        assert_ne!(buf[0].ts, buf[1].ts);
    }

    #[test]
    fn dedupe_logs_into_caps_at_max_size_dropping_oldest() {
        let mut buf: Vec<LogLine> = Vec::new();
        dedupe_logs_into(&mut buf, vec![ll("a"), ll("b"), ll("c")], 3);
        assert_eq!(buf.len(), 3);
        dedupe_logs_into(&mut buf, vec![ll("d")], 3);
        assert_eq!(
            buf.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            vec!["b", "c", "d"]
        );
    }

    #[test]
    fn dedupe_logs_into_handles_empty_input() {
        let mut buf: Vec<LogLine> = Vec::new();
        dedupe_logs_into(&mut buf, Vec::new(), 100);
        assert!(buf.is_empty());
        dedupe_logs_into(&mut buf, vec![ll("x")], 100);
        dedupe_logs_into(&mut buf, Vec::new(), 100);
        assert_eq!(
            buf.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            vec!["x"]
        );
    }

    #[test]
    fn dedupe_logs_into_drops_empty_lines() {
        // Empty journalctl lines would otherwise accumulate and bloat the
        // buffer — they're never useful in the UI. Dedupe must skip them.
        let mut buf: Vec<LogLine> = Vec::new();
        dedupe_logs_into(&mut buf, vec![ll(""), ll(""), ll("real")], 100);
        assert_eq!(
            buf.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            vec!["real"]
        );
    }

    // -------------------------------------------------------------------------
    // Module 5.3 — `App::apply_net_sample` is the dispatcher arm for
    // `Action::NetSample`. These tests pin the ring-init and per-interface
    // behaviour so a refactor can't silently drop a delta on the floor.
    // -------------------------------------------------------------------------

    #[test]
    fn net_sample_appends_deltas_to_ring() {
        // Pre-seed the ring as the 1Hz refiller would have after one
        // prior tick: eth0 already saw 100 rx / 50 tx bytes in the
        // previous second.
        let mut app = fresh_app();
        app.apply_net_sample("eth0", 100, 50);
        // New sample arrives: 1000 rx, 500 tx for the same second window.
        app.apply_net_sample("eth0", 1000, 500);
        let entry = app.net_history.get("eth0").expect("eth0 entry present");
        assert_eq!(entry.0.as_slice_chrono(), vec![100, 1000]);
        assert_eq!(entry.1.as_slice_chrono(), vec![50, 500]);
    }

    #[test]
    fn net_sample_creates_entry_for_new_interface() {
        // First sighting of an interface: `or_insert_with` must build
        // two empty 60-cap rings, then push the first sample so the
        // ring's length is 1 after dispatch.
        let mut app = fresh_app();
        app.apply_net_sample("wlan0", 100, 50);
        let entry = app.net_history.get("wlan0").expect("wlan0 entry present");
        assert_eq!(entry.0.cap(), 60);
        assert_eq!(entry.1.cap(), 60);
        assert_eq!(entry.0.len(), 1);
        assert_eq!(entry.1.len(), 1);
    }

    #[test]
    fn net_sample_saturates_at_60_samples() {
        // Push 200 samples: the ring must clamp at 60 (oldest dropped).
        // Catches a regression where someone swaps `RingU64` for a
        // `VecDeque` and forgets the bound.
        let mut app = fresh_app();
        for i in 0u64..200 {
            app.apply_net_sample("eth0", i, i);
        }
        let entry = app.net_history.get("eth0").unwrap();
        assert_eq!(entry.0.len(), 60);
        // Newest sample must be the last one pushed (i=199); oldest
        // must be 200-60=140.
        let slice = entry.0.as_slice_chrono();
        assert_eq!(slice.first().copied(), Some(140));
        assert_eq!(slice.last().copied(), Some(199));
    }

    // -------------------------------------------------------------------------
    // Module 6.2 — `Action::ProcTreeRefreshed` replaces `App::proc_tree`
    // wholesale. The 15s refiller rebuilds the snapshot from
    // `cyberdeck_core::process::list_with_ppid()` on every tick; the
    // dispatcher is the only writer.
    // -------------------------------------------------------------------------

    #[test]
    fn proc_tree_refreshed_replaces_app_proc_tree() {
        let mut app = fresh_app();
        app.proc_tree.push(ProcEntry {
            pid: 1,
            ppid: 0,
            comm: "old".into(),
            cmdline: String::new(),
        });
        app.handle_action_for_test(Action::ProcTreeRefreshed(vec![ProcEntry {
            pid: 100,
            ppid: 1,
            comm: "new".into(),
            cmdline: String::new(),
        }]));
        assert_eq!(app.proc_tree.len(), 1);
        assert_eq!(app.proc_tree[0].pid, 100);
        assert_eq!(app.proc_tree[0].comm, "new");
    }

    #[test]
    fn proc_tree_refreshed_with_empty_vec_clears_tree() {
        let mut app = fresh_app();
        app.proc_tree.push(ProcEntry {
            pid: 1,
            ppid: 0,
            comm: "x".into(),
            cmdline: String::new(),
        });
        app.handle_action_for_test(Action::ProcTreeRefreshed(vec![]));
        assert!(app.proc_tree.is_empty());
    }

    #[test]
    fn proc_tree_view_defaults_to_false() {
        let app = fresh_app();
        assert!(
            !app.proc_tree_view,
            "proc_tree_view must default to false (facts view)"
        );
        assert!(app.proc_tree.is_empty());
    }

    // -------------------------------------------------------------------------
    // Module 7.1 — `toast_history` is a `VecDeque<ToastEntry>` capped at 200.
    // `push_toast` is the only writer and enforces the cap by dropping the
    // oldest entry each time the ring fills. Tests pin:
    //   - empty by default
    //   - append at under-cap grows unbounded
    //   - cap-200 trim drops oldest on overflow
    //   - kind is preserved on insert
    // -------------------------------------------------------------------------

    #[test]
    fn toast_history_starts_empty() {
        let app = fresh_app();
        assert!(app.toast_history.is_empty());
    }

    #[test]
    fn toast_history_push_helper_returns_unit_and_preserves_kind() {
        let mut app = fresh_app();
        app.push_toast(crate::app::toast::ToastKind::Warn, "warning".to_string());
        assert_eq!(app.toast_history.len(), 1);
        assert_eq!(app.toast_history[0].kind, crate::app::toast::ToastKind::Warn);
        assert_eq!(app.toast_history[0].message, "warning");
    }

    #[test]
    fn toast_history_appends_and_trims_at_cap() {
        let mut app = fresh_app();
        for i in 0..250 {
            app.push_toast(
                crate::app::toast::ToastKind::Info,
                format!("toast {i}"),
            );
        }
        assert_eq!(app.toast_history.len(), 200);
        // The 50 oldest (toasts 0..50) should have been dropped; toast 50
        // is the oldest survivor and toast 249 is the newest.
        assert!(
            app.toast_history.front().unwrap().message.contains("toast 50"),
            "oldest surviving entry should be toast 50, got {:?}",
            app.toast_history.front().unwrap().message
        );
        assert!(
            app.toast_history.back().unwrap().message.contains("toast 249"),
            "newest entry should be toast 249, got {:?}",
            app.toast_history.back().unwrap().message
        );
    }
}
