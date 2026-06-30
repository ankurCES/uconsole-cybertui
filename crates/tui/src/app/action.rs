//! All user-driven intents flow through `Action`. The main loop converts
//! keyboard input into Actions; each action handler returns a `Cmd` (a
//! boxed async closure) that the loop spawns.
#![allow(dead_code)] // many variants are Phase-1/2 wiring; see ROADMAP.md

use crossterm::event::KeyEvent;

#[derive(Debug, Clone)]
pub enum Action {
    /// Periodic "wake up" so the UI knows to redraw.
    Tick,
    /// User pressed a key — forwarded to the focused screen.
    Key(KeyEvent),
    /// Switch to a screen (also triggered by the sidebar and the palette).
    Goto(crate::app::screen::ScreenId),
    /// Cycle to the next/previous screen, skipping any screen whose
    /// `Screen::is_hidden` returns true. Mirrors orbital's
    /// Tab/Shift-Tab widget navigation with hidden-widget skipping.
    CycleScreen(bool),
    /// Quit the app cleanly.
    Quit,
    /// Push a toast from a background task (success/error of a long action).
    Toast(crate::app::toast::ToastKind, String),
    /// Toggle a binary setting.
    Toggle(crate::app::screen::SettingsKey),
    /// Start a specific long-running action and report back via Toast.
    Run(RunAction),
    /// Confirm or cancel the active modal.
    ConfirmModal,
    CancelModal,
    /// Submit the input modal with the typed value.
    SubmitInput(String),
    /// Push a line into the log buffer (sent by the logs screen fetch task).
    LogPushed(crate::app::LogLine),
    /// Module 2.4 — user pressed `r` on the Logs screen. Dispatcher
    /// reacts by spawning an immediate `recent_since(60)` fetch (vs.
    /// the 1Hz refiller's 2s sliding window) and routes results back
    /// through the normal `LogPushed` pipeline so dedupe + ordering
    /// keep working. The screen only enqueues this Action; the actual
    /// journalctl invocation lives in the dispatcher so the UI thread
    /// never blocks on a process spawn.
    RefreshLogs,
    /// Live refresh of a specific resource (manual `r` press).
    Refresh(crate::app::screen::ScreenId),
    /// Result of a Wi-Fi scan. Written into `app.wifi_scan_results` so the
    /// right pane can render the networks on the next frame.
    WifiScanResult(Vec<cyberdeck_core::net::WifiNetwork>),
    /// Result of a Bluetooth device scan. Written into
    /// `app.live.bluetooth` so the bluetooth screen can render the
    /// device list on the next frame.
    BluetoothScanResult(Vec<cyberdeck_core::bluetooth::BtDevice>),
    /// Module 5.3 — one second of RX/TX byte deltas for a single
    /// interface, as measured by the 1Hz network refiller. The
    /// dispatcher appends the deltas to `App::net_history` so the
    /// header sparkline (Module 5.4) can render them on the next
    /// frame. We're lossy on receiver drop — if the main loop has
    /// already exited the channels are torn down, so retrying here
    /// would just block the refiller.
    NetSample {
        iface: String,
        rx_delta: u64,
        tx_delta: u64,
    },
}

#[derive(Debug, Clone)]
pub enum RunAction {
    WifiConnect {
        ssid: String,
        password: Option<String>,
    },
    WifiDisconnect,
    ServiceStart(String),
    ServiceStop(String),
    ServiceRestart(String),
    ServiceEnable(String),
    ServiceDisable(String),
    ProcessKill(i32),
    ProcessRenice(i32, i32),
    PackageInstall(String),
    PackageRemove(String),
    PackageUpdate,
    PackageUpgrade,
    SetGovernor(String),
    SetBrightness(u8),
    SetVolume {
        target: String,
        percent: u8,
    },
    MuteSink {
        target: String,
        mute: bool,
    },
    SetDefaultSink(String),
    SetInterfaceUp(String, bool),
    BluetoothConnect(String),
    BluetoothDisconnect(String),
    BluetoothPair(String),
    BluetoothTrust(String),
    BluetoothPower(bool),
    /// Refresh the paired-device list via `bluetoothctl devices`. The
    /// result lands in `app.live.bluetooth` via the existing Live
    /// registry refresh path.
    BluetoothScan,
    /// Trigger an immediate Wi-Fi scan. The result lands in
    /// `app.wifi_scan_results` via the broadcast loop; the right pane
    /// redraws automatically on the next frame.
    WifiScan,
    /// Phase 6: connect to a WPA-Enterprise SSID. Fields map to NM 802-1x
    /// settings. Implemented in `cyberdeck_core::net::wifi_connect_enterprise`.
    WifiEnterpriseConnect {
        ssid: String,
        eap: String,
        identity: String,
        password: Option<String>,
        anon_or_cert: Option<String>,
    },
    Reboot,
    Shutdown,
    Suspend,
    Hibernate,
    WebStart,
    WebStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsKey {
    Theme,
    Mouse,
    NerdFont,
    WebServer,
}
