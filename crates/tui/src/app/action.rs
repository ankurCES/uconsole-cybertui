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
    /// Live refresh of a specific resource (manual `r` press).
    Refresh(crate::app::screen::ScreenId),
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
