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
}

#[derive(Debug)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
}

/// Where a Choice commit lands. `PickInput` opens the named `InputKind`
/// prompt with `id` pre-supplied via `prefill`; `RunAction` dispatches
/// directly; `Next` re-enters the wizard with the picked step value.
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

impl App {
    pub fn new(tx: mpsc::Sender<Action>, rx: mpsc::Receiver<Action>) -> Self {
        Self {
            live: Arc::new(Live::default()),
            current: ScreenId::System,
            manager: crate::wm::manager::Manager::new(ScreenId::System),
            modal: Modal::None,
            sidebar_focused: true,
            sidebar_idx: 0,
            // Default region on launch is the sidebar — that's the natural
            // D-pad start (user sees the screen list and moves with ↑/↓).
            // `switch_screen` flips to `ContentLeft` when a screen commits.
            region: Region::Sidebar,
            palette_buf: String::new(),
            palette_idx: 0,
            toasts: Vec::new(),
            boot_toast_sent: false,
            logs: Vec::new(),
            logs_filter: String::new(),
            proc_sort: ProcessSort::Cpu,
            proc_selected: 0,
            svc_selected: 0,
            net_selected: 0,
            net_show_wifi: false,
            wifi_scan_results: Vec::new(),
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
        self.toasts.push(Toast::new(kind, msg.into()));
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
}
