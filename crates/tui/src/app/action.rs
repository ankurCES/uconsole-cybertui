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
    /// User keymap sub-mode commands. The Settings → Keys screen
    /// drives the user through `BeginCapture(NavAction)` to arm a
    /// single binding, then sends a stream of `CaptureKey(KeyEvent)`
    /// actions from the dispatcher until the user presses a real
    /// key (the next non-modifier `KeyEvent` becomes the binding).
    /// `Clear` removes a binding, `ResetAll` wipes every override
    /// (and is followed by a confirm modal by the caller), `ExitMode`
    /// returns to the regular Settings list.
    KeymapCmd(crate::keymap::KeymapCmd),
    /// Start a specific long-running action and report back via Toast.
    Run(RunAction),
    /// Confirm or cancel the active modal.
    ConfirmModal,
    CancelModal,
    /// Submit the input modal with the typed value.
    SubmitInput(String),
    /// Push a line into the log buffer (sent by the logs screen fetch task).
    LogPushed(crate::app::LogLine),
    /// Fix #1c — batched variant. The 1Hz log refiller collects every
    /// line from `recent_since(2)` into a single action so the
    /// dispatcher can dedupe + append once, then the renderer redraws
    /// once. Previously each line was its own `LogPushed` action,
    /// producing N redraws per second on a busy box.
    LogLines(Vec<crate::app::LogLine>),
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
    /// Module 6.2 — 15s refiller from `cyberdeck_core::process::list_with_ppid`.
    /// The dispatcher replaces `App::proc_tree` wholesale (no merge needed:
    /// the snapshot is a complete per-tick picture of /proc, so a merge
    /// would just have to undo it). PIDs that disappear between refills
    /// fall out of the next snapshot naturally.
    ProcTreeRefreshed(Vec<cyberdeck_core::process::ProcEntry>),
    /// Module 8.2 — 30s refiller of `cyberdeck_core::net::saved_connections`.
    /// Replaces `App::saved_connections` wholesale so the right pane can
    /// re-render on the next frame. Empty Vec on missing nmcli / non-NM
    /// box (the call site never returns `Err`).
    SavedConnectionsRefreshed(Vec<cyberdeck_core::net::SavedConnection>),
    /// Step 9 — user pressed `r` on the City screen. The dispatcher
    /// reacts by re-firing the ip-api → Open-Meteo pipeline out of band
    /// (the 10-minute City refiller continues to run independently).
    /// Results land in `Action::CityResolved` / `Action::CityWeatherRefreshed`
    /// which the City screen applies to its live state. The screen
    /// only enqueues this Action; the actual HTTP work lives in the
    /// dispatcher's spawned task so the UI thread never blocks.
    CityCtrlRefresh,
    /// Phase 2 — jump-to-slug from the City palette picker. The
    /// City screen's `apply_slug` path handles the rest (reset
    /// viewport_bbox, sync the location marker, save prefs).
    /// Distinct from `CityCtrlRefresh` which re-fetches IP-geo + weather.
    CityCtrlSet { slug: String },
    /// Step 9 — IP-geolocated location resolved (from the 10-min
    /// refiller or a `CityCtrlRefresh` tap). Dispatcher applies to
    /// `App::live.city_loc` so the City screen reads a stable snapshot
    /// on every render instead of holding its own copy. `slug` is the
    /// bundled-slug fallback the City screen uses for road data.
    CityResolved(crate::screens::city::geo::CityLocation),
    /// Step 9 — Open-Meteo weather snapshot returned. Dispatcher
    /// applies to `App::live.city_weather`; the City screen reads it
    /// each render. Failures don't emit an Action — the previous
    /// snapshot stays on screen.
    CityWeatherRefreshed(crate::screens::city::weather::Weather),
    /// M5 — Intel refiller pushed a fresh `Snapshot` for a single
    /// layer. Dispatcher upserts into `App::intel_snapshots` keyed by
    /// `LayerId`. The Intel screen reads the map on every render, so
    /// the grid updates on the very next frame. Failures land as
    /// `Snapshot::error(...)` (carrying `LayerStatus::Error`); the
    /// screen renders those rows in red.
    IntelSnapshot(cyberdeck_intel::Snapshot),
    /// S8 — Apply a new theme immediately (settings_v2 theme picker).
    /// Handled in run_v2: updates UiState::theme + Prefs::theme + saves prefs.
    SetTheme(crate::theme::ThemeName),
    /// S15 — LoRa saved-node management. `LoraNodeAdd` carries "ip [label]"
    /// (space-delimited; label is optional). `LoraNodeDelete` carries the
    /// index into `prefs.lora_nodes`. Both are handled in `apply_action`
    /// which mutates `state.prefs` and calls `save()`.
    LoraNodeAdd(String),
    LoraNodeDelete(usize),
    /// S19 — AI agent harness. AiSubmit is sent by the AI screen when the
    /// user presses Enter; apply_action appends the user message to
    /// live.ai_messages and spawns a stream_chat task. AiToken / AiThinkToken
    /// carry streaming SSE delta tokens; AiDone marks response complete.
    /// LlamaReady / LlamaDown report llama-server health poll results.
    AiSubmit(String),
    AiToken(String),
    AiThinkToken(String),
    AiToolLog(String),
    AiDone,
    LlamaReady,
    LlamaDown,
    /// Health poll timed out or process crashed — carries last stderr line.
    LlamaFailed(String),
    /// WhatsApp sidecar events.
    WhatsAppQr(String),
    WhatsAppConnected,
    WhatsAppDisconnected(String),
    WhatsAppContacts(Vec<(String, String)>),  // (jid, name)
    WhatsAppMessage {
        jid: String,
        text: String,
        from_me: bool,
        timestamp: u64,
    },
    WhatsAppSubmit(String, String), // (jid, text)
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
    /// Phase 2 — Editor screen: write the current buffer to a new
    /// path (Save As…). The path comes from a preceding `Modal::Input`
    /// of kind `InputKind::EditorSaveAs`; the dispatch handler reads
    /// `app.editor_path` and writes the buffer there. Distinct from
    /// `Ctrl-S` which always writes to `app.editor_path` in place.
    EditorSaveAs(String),
    /// Phase 2 — Editor screen: re-read `app.editor_path` from disk
    /// and replace `app.editor_buffer`. Used by the Reload dropdown
    /// item and the `F5` shortcut. A dirty buffer prompts a Discard
    /// confirm first.
    EditorReload,
    /// Phase 2 — City map: re-centre the viewport on the
    /// (lat, lon) implied by a click at `(col, row)` inside `rect`
    /// (which is the cached `app.city_map_rect`). Dispatched from
    /// the mouse handler in `main.rs`; the City screen's on_key arm
    /// does the actual reprojection. The screen itself reads the
    /// rect dimensions to compute the braille dot grid, so the
    /// rect must be the same one the renderer used this frame.
    CityPan {
        col: u16,
        row: u16,
        rect: ratatui::layout::Rect,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsKey {
    Theme,
    Mouse,
    NerdFont,
    WebServer,
}
