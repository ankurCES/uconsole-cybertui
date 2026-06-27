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
use cyberdeck_core::services::Service;
use cyberdeck_core::storage::Filesystem;
use cyberdeck_core::sys::SystemInfo;
use ratatui::text::Line;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::interval;

pub use action::Action;
pub use screen::ScreenId;
pub use toast::Toast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    Reboot,
    Shutdown,
    Kill,
    Remove,
    DisconnectWifi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    WifiPassword,
    ConnectSSID,
    KillPid,
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
        }
    }
}

impl Live {
    /// Spawn a background task that refreshes the live readouts on a timer.
    /// Each field has its own cadence — system/thermal every second, services
    /// and processes every five, packages on demand.
    pub fn spawn_refreshers(self: &Arc<Self>, tx: mpsc::Sender<Action>) {
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
                let _ = tx.send(Action::Tick).await;
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
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub ts: chrono::DateTime<Local>,
    pub line: String,
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
    pub focus: Focus,
    pub modal: Modal,
    pub palette_buf: String,
    pub palette_idx: usize,
    pub toasts: Vec<Toast>,
    pub logs: Vec<LogLine>,
    pub logs_filter: String,
    pub proc_sort: ProcessSort,
    pub proc_selected: usize,
    pub svc_selected: usize,
    pub net_selected: usize,
    pub net_show_wifi: bool,
    pub wifi_scan_results: Vec<cyberdeck_core::net::WifiNetwork>,
    pub pkgs_filter: String,
    pub pkg_search_results: Vec<Package>,
    pub theme_name: screen::ThemeNameReexport,
    pub mouse: bool,
    pub show_help: bool,
    pub running: bool,
    pub status_message: Option<String>,
    pub tx: mpsc::Sender<Action>,
    pub rx: mpsc::Receiver<Action>,
    pub clock: chrono::DateTime<Local>,
    pub nerd_font: bool,
    /// SSID that the wifi-password modal is collecting a password for.
    pub pending_ssid: Option<String>,
    /// Files-screen navigation.
    pub files_cwd: std::path::PathBuf,
    pub files_entries: Vec<cyberdeck_core_files::DirEntry>,
    pub files_selected: usize,
    pub files_show_hidden: bool,
    pub files_right: std::path::PathBuf,
    pub files_right_entries: Vec<cyberdeck_core_files::DirEntry>,
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

impl App {
    pub fn new(tx: mpsc::Sender<Action>, rx: mpsc::Receiver<Action>) -> Self {
        Self {
            live: Arc::new(Live::default()),
            current: ScreenId::System,
            focus: Focus::Sidebar,
            modal: Modal::None,
            palette_buf: String::new(),
            palette_idx: 0,
            toasts: Vec::new(),
            logs: Vec::new(),
            logs_filter: String::new(),
            proc_sort: ProcessSort::Cpu,
            proc_selected: 0,
            svc_selected: 0,
            net_selected: 0,
            net_show_wifi: false,
            wifi_scan_results: Vec::new(),
            pkgs_filter: String::new(),
            pkg_search_results: Vec::new(),
            theme_name: screen::ThemeNameReexport::Dark,
            mouse: true,
            show_help: false,
            running: true,
            status_message: None,
            tx,
            rx,
            clock: Local::now(),
            nerd_font: std::env::var("NERD_FONT").as_deref() != Ok("0"),
            pending_ssid: None,
            files_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            files_entries: Vec::new(),
            files_selected: 0,
            files_show_hidden: false,
            files_right: PathBuf::from("/"),
            files_right_entries: Vec::new(),
        }
    }

    pub fn push_toast(&mut self, kind: toast::ToastKind, msg: impl Into<String>) {
        self.toasts.push(Toast::new(kind, msg.into()));
    }

    pub fn cleanup_toasts(&mut self) {
        self.toasts.retain(|t| !t.expired());
    }

    pub fn tick_clock(&mut self) {
        self.clock = Local::now();
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
