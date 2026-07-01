//! LoRa screen — Meshtastic over LAN HTTP bridge: longfast channel chat
//! on the left, nodes list with online status + hops on the right, input
//! strip at the bottom.
//!
//! Transports are abstracted behind a trait so the unit tests never block
//! on a real network socket. The real `HttpLoraTransport` lives behind the
//! user-supplied node IP (entered via the `i` modal) so a `LoRa` screen
//! on a box without a node just renders the connect-prompt placeholder
//! instead of hanging the renderer.
//!
//! Borrowed shape from meshtastic/web's `@meshtastic/transport-http`:
//!   * GET  http://<ip>/api/v1/fromradio?all=false  → bytes (3 s poll)
//!   * PUT  http://<ip>/api/v1/toradio              → bytes (frame write)
//!   * mark `connected = true` on first 2xx; `false` on transport error.
//! No protobuf decode lands in this slice — the HTTP backend exposes a
//! "wire-debug" counter so the user can confirm the link is up; full
//! proto parsing is a follow-up.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;
use crate::screens::lora::proto::node_id_to_num;

/// HTTP transport for talking to a Meshtastic node over LAN. Pulled in
/// behind the `http` feature flag so a default `cargo build` doesn't
/// grow the dep graph for users who don't own a node. See
/// `screens/lora/http.rs` for the wire shape and `screens/lora/proto.rs`
/// for the hand-rolled `FromRadio` decoder.
#[cfg(feature = "http")]
pub mod http;

/// Hand-rolled `FromRadio` / `ToRadio` protobuf helpers. See `proto.rs`
/// for the field numbers (read straight from
/// `packages/protobufs/meshtastic/mesh.proto` in the meshtastic/web repo)
/// and the rationale for not pulling in `prost`. Always available — the
/// types are pure data and the encoder/parser are pure functions, so
/// keeping them behind the `http` feature would just split the unit tests
/// for no benefit.
pub mod proto;

/// A node as seen by the local LoRa network — identifies the device, the
/// operator-chosen long/short names, and how many hops away from us
/// the most recent packet was.
///
/// `PartialEq` only — `snr` is `f32` and floats don't implement `Eq`.
/// Equality on the rest of the struct still works for tests.
#[derive(Debug, Clone, PartialEq)]
pub struct LoraNode {
    pub node_id: String,
    pub long_name: String,
    pub short_name: String,
    pub hops_away: u8,
    pub last_heard_secs: u64,
    pub snr: Option<f32>,
}

/// Threshold (seconds) for "online" in the nodes pane. Matches the
/// meshtastic/web UI convention: a node is considered online if the
/// last-heard timestamp is within the last 15 minutes. See
/// `LoraNode::is_online_at`.
pub const ONLINE_THRESHOLD_SECS: u64 = 15 * 60;

impl LoraNode {
    /// Best human-readable name we have for this node. Prefers the
    /// operator's `long_name`, then `short_name`, finally the raw
    /// `node_id`. Column padding is the renderer's job, not ours — this
    /// returns the raw value so callers comparing labels (tests, the
    /// chat `from` field, etc.) get a stable match.
    pub fn label(&self) -> String {
        if !self.long_name.is_empty() {
            return self.long_name.clone();
        }
        if !self.short_name.is_empty() {
            return self.short_name.clone();
        }
        self.node_id.clone()
    }

    /// Is this node online *as of* `now_secs`?
    ///
    /// `last_heard_secs` is interpreted defensively: values larger
    /// than `ABSOLUTE_CUTOFF_SECS` are treated as Unix-epoch seconds
    /// (meshtastic/web's convention), smaller values as seconds-ago
    /// (a relative-seconds convention some transports use). This
    /// keeps the API tolerant of either encoding without forcing the
    /// caller to normalise.
    ///
    /// `now_secs` should be in the same space as the absolute
    /// encoding — i.e. Unix-epoch seconds. At runtime that's
    /// `std::time::SystemTime::now().duration_since(UNIX_EPOCH).as_secs()`.
    /// For the relative branch we don't subtract from `now` (a
    /// 5-min-ago relative value is just `300`, not `now - 300`).
    ///
    /// The cutoff is deliberately tight: real Unix-epoch seconds in
    /// 2024 are ~`1.7e9`. Anything > 100 days in seconds-ago terms
    /// (≈ `8.6e6`) is implausible as a "seconds since heard" value —
    /// nodes older than that are stale, not "heard 100 days ago".
    /// So we treat anything ≥ 10^7 as absolute; anything below as
    /// relative. This keeps the 2024 epoch on the absolute side and
    /// typical relative values (0–3600 s) on the relative side.
    pub fn is_online_at(&self, now_secs: u64) -> bool {
        const ABSOLUTE_CUTOFF_SECS: u64 = 10_000_000; // ~115 days
        if self.last_heard_secs >= ABSOLUTE_CUTOFF_SECS {
            // Absolute: age = how many seconds elapsed since then.
            now_secs.saturating_sub(self.last_heard_secs) <= ONLINE_THRESHOLD_SECS
        } else {
            // Relative: the value already *is* "seconds since heard".
            self.last_heard_secs <= ONLINE_THRESHOLD_SECS
        }
    }

    /// Default "now" used when the renderer doesn't have a clock in
    /// scope. Returns the current Unix epoch in seconds.
    pub fn now_secs() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Convenience: `self.is_online_at(LoraNode::now_secs())`.
    pub fn is_online(&self) -> bool {
        self.is_online_at(Self::now_secs())
    }
}



/// One chat line on the longfast channel. `from` is the operator-chosen
/// label (`label()` above) when we know the sender, else `node_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoraChatLine {
    pub from: String,
    pub text: String,
    pub hops_away: u8,
    pub is_local: bool,
}

/// Which "channel" a chat line belongs to. Mirrors
/// `meshtastic/web`'s distinction between the shared `LongFast`
/// primary channel (broadcast in firmware-speak) and 1:1 direct
/// messages. Both surface via the same `TEXT_MESSAGE_APP = 1`
/// portnum; they're separated by `MeshPacket.to`:
///
///   * `MeshPacket.to == 0xFFFFFFFF` → `LongFast`
///   * `MeshPacket.to == <other node_num>` → `Direct(<node_num>)`
///
/// `LongFast` is the default for outbound when no thread is
/// explicitly active; `Direct(n)` is keyed by the *other end* of
/// the conversation (whichever end isn't us). See
/// `MeshClient.sendPacket` in `meshtastic/web`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    /// The shared primary / `LongFast` channel — broadcast on the wire.
    LongFast,
    /// 1:1 with the node whose num is the contained value.
    Direct(u32),
}

impl Default for ChannelKind {
    fn default() -> Self {
        ChannelKind::LongFast
    }
}

impl ChannelKind {
    /// `true` for the shared channel; `false` for DMs. Lets
    /// callers branch without exposing the enum's internals.
    pub fn is_longfast(&self) -> bool {
        matches!(self, ChannelKind::LongFast)
    }

    /// The other party's node num on the wire — `0xFFFFFFFF` for
    /// broadcast, the peer's num for DMs. Used directly as the
    /// `MeshPacket.to` field when encoding an outbound packet.
    pub fn to_num(&self) -> u32 {
        match self {
            ChannelKind::LongFast => proto::BROADCAST_NUM,
            ChannelKind::Direct(n) => *n,
        }
    }

    /// `LongFast` → `"LongFast"`, `Direct(n)` → the peer's operator
    /// label when known, else `!<hex>`. Cheap; the renderer can
    /// pad/truncate.
    pub fn display_label(&self, node_lookup: &dyn Fn(u32) -> Option<String>) -> String {
        match self {
            ChannelKind::LongFast => "LongFast".to_string(),
            ChannelKind::Direct(n) => node_lookup(*n)
                .unwrap_or_else(|| format!("!{:08x}", n)),
        }
    }
}

/// A single thread of chat lines — LongFast or a 1:1 DM. The renderer
/// builds the right pane as `vec![LongFast row] + nodes`, the user
/// navigates with the arrows, and `Enter` flips
/// `app.lora_active_thread` to either `LongFast` or `Direct(n)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub kind: ChannelKind,
    pub label: String,
    pub lines: Vec<LoraChatLine>,
}

impl Thread {
    pub fn new(kind: ChannelKind, label: impl Into<String>) -> Self {
        Self { kind, label: label.into(), lines: Vec::new() }
    }
}

/// What the LoRa screen can ask of the underlying transport. All methods
/// have an in-process `FakeTransport` for tests; the real transport lives
/// in `LoraScreen::new_http` and is selected at runtime by the IP modal.
///
/// Backwards-compat: `messages()` and `send_longfast()` are kept as
/// **shim default-methods** that delegate to `messages_for(&LongFast)`
/// and `send_to(&LongFast, _)` respectively, so any implementor of
/// just the new methods automatically keeps working with callers that
/// pre-date the thread model.
pub trait LoraTransport {
    /// Snapshot of known nodes. Returns what the transport currently knows;
    /// the screen does NOT keep a separate authoritative copy until
    /// `apply_nodes` is called.
    fn nodes(&self) -> Vec<LoraNode>;

    /// Snapshot of the chat lines for a given thread (LongFast or a
    /// `Direct(n)`). New in the thread-aware cut — transports that
    /// only model broadcast can rely on the default implementation
    /// which returns an empty `Vec` for `Direct(n)`.
    fn messages_for(&self, _kind: &ChannelKind) -> Vec<LoraChatLine> {
        let _ = _kind;
        Vec::new()
    }

    /// Snapshot of every thread the transport currently knows about.
    /// The renderer reads this to build the right pane and to drive the
    /// input strip's `to:` chip. LongFast is always present (even
    /// when empty) so the right-pane list has a stable header row.
    fn threads(&self) -> Vec<Thread>;

    /// Backwards-compat shim — defaults to `messages_for(&ChannelKind::LongFast)`.
    /// Kept so existing callers (and `FakeTransport::messages`)
    /// compile unchanged.
    fn messages(&self) -> Vec<LoraChatLine> {
        self.messages_for(&ChannelKind::LongFast)
    }

    /// True when the transport has an active HTTP session. False means
    /// "no node" — the screen renders a connect-prompt instead.
    fn connected(&self) -> bool;

    /// Send `text` from the local node to the given thread (LongFast
    /// or `Direct(n)`). The transport should echo the line back through
    /// `messages_for(kind)` so the chat pane can append it.
    fn send_to(&mut self, _kind: &ChannelKind, _text: &str) -> Result<(), LoraError>;

    /// Backwards-compat shim — defaults to `send_to(&ChannelKind::LongFast, _)`.
    /// Kept so existing callers (and the long-standing
    /// `FakeTransport::send_longfast`) compile unchanged.
    fn send_longfast(&mut self, text: &str) -> Result<(), LoraError> {
        self.send_to(&ChannelKind::LongFast, text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoraError {
    NotConnected,
    Empty,
    TooLong,
    Io(String),
}

/// A no-op transport used by tests and by a TUI running on a box without
/// a Meshtastic device. Public so the `App` can default to it.
///
/// Thread model: holds a `Vec<Thread>` (`LongFast` plus zero-or-more
/// `Direct(n)` entries). The trait shims route
/// `messages()` → `messages_for(&LongFast)` and
/// `send_longfast(...)` → `send_to(&LongFast, ...)` so the
/// pre-threads test surface keeps working.
#[derive(Debug, Default, Clone)]
pub struct FakeTransport {
    pub nodes: Vec<LoraNode>,
    pub threads: Vec<Thread>,
    pub connected: bool,
    /// Recorded outbound LongFast messages for test assertions.
    pub sent: Vec<String>,
    /// Recorded outbound DM messages: `(to_node_num, text)`. Separate
    /// from `sent` so a test that mixes broadcast and DM assertions
    /// doesn't have to disambiguate.
    pub sent_dm: Vec<(u32, String)>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_nodes(nodes: Vec<LoraNode>) -> Self {
        Self {
            nodes,
            ..Self::default()
        }
    }

    pub fn with_connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }

    /// Find-or-create a LongFast thread. Used by the trait impls
    /// and by tests that want to seed a thread without going through
    /// the trait surface.
    pub fn longfast_thread_mut(&mut self) -> &mut Thread {
        if self.threads.is_empty() {
            self.threads.push(Thread::new(ChannelKind::LongFast, "LongFast"));
        }
        &mut self.threads[0]
    }

    /// Find-or-create a `Direct(n)` thread. Inserts in num-sorted
    /// order so the right-pane list is deterministic across calls.
    pub fn direct_thread_mut(&mut self, n: u32) -> &mut Thread {
        let kind = ChannelKind::Direct(n);
        let pos = self
            .threads
            .iter()
            .position(|t| t.kind == kind)
            .unwrap_or_else(|| {
                // Insert sorted by `to_num` after the LongFast
                // header so the right pane stays stable.
                let mut idx = self.threads.len();
                for (i, t) in self.threads.iter().enumerate().skip(1) {
                    if t.kind.to_num() > n {
                        idx = i;
                        break;
                    }
                }
                self.threads
                    .insert(idx, Thread::new(kind, format!("!{:08x}", n)));
                idx
            });
        &mut self.threads[pos]
    }

    /// Convenience for tests: number of lines currently held for `kind`.
    pub fn line_count_for(&self, kind: &ChannelKind) -> usize {
        self.threads
            .iter()
            .find(|t| &t.kind == kind)
            .map(|t| t.lines.len())
            .unwrap_or(0)
    }
}

impl LoraTransport for FakeTransport {
    fn nodes(&self) -> Vec<LoraNode> {
        self.nodes.clone()
    }
    fn threads(&self) -> Vec<Thread> {
        // Always include the LongFast header row even if the transport
        // has never received anything — keeps the right-pane layout
        // stable (one anchor + a list of nodes).
        if self.threads.is_empty() {
            return vec![Thread::new(ChannelKind::LongFast, "LongFast")];
        }
        if !self.threads.iter().any(|t| t.kind == ChannelKind::LongFast) {
            let mut t = self.threads.clone();
            t.insert(0, Thread::new(ChannelKind::LongFast, "LongFast"));
            return t;
        }
        self.threads.clone()
    }
    fn messages_for(&self, kind: &ChannelKind) -> Vec<LoraChatLine> {
        self.threads
            .iter()
            .find(|t| &t.kind == kind)
            .map(|t| t.lines.clone())
            .unwrap_or_default()
    }
    fn connected(&self) -> bool {
        self.connected
    }

    fn send_to(&mut self, kind: &ChannelKind, text: &str) -> Result<(), LoraError> {
        if !self.connected {
            return Err(LoraError::NotConnected);
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(LoraError::Empty);
        }
        if trimmed.len() > 200 {
            return Err(LoraError::TooLong);
        }
        let line = LoraChatLine {
            from: "me".into(),
            text: trimmed.to_string(),
            hops_away: 0,
            is_local: true,
        };
        match kind {
            ChannelKind::LongFast => {
                self.sent.push(trimmed.to_string());
                self.longfast_thread_mut().lines.push(line);
            }
            ChannelKind::Direct(n) => {
                self.sent_dm.push((*n, trimmed.to_string()));
                self.direct_thread_mut(*n).lines.push(line);
            }
        }
        Ok(())
    }
}

/// The screen itself. Holds the chat/nodes snapshot, the chat input
/// buffer, and a transport behind the trait so tests can swap it for
/// `FakeTransport` without touching any HTTP code paths.
pub struct LoraScreen {
    pub transport: Box<dyn LoraTransport + Send>,
    /// IP this screen is currently configured against. Diffed against
    /// `app.lora_node_ip` on every `poll` so a fresh IP from the modal
    /// triggers a transport rebuild (FakeTransport → HttpLoraTransport,
    /// or IP-A → IP-B). `None` means "no node configured" and the
    /// screen stays on the constructor-supplied transport (usually
    /// `FakeTransport`). Persisted here so we don't rebuild on every
    /// tick — only when the value actually flips.
    pub current_node_ip: Option<String>,
    /// Cursor over the virtual right-pane list:
    /// `vec![LongFast header row, ...nodes]`. `0` = header row,
    /// `>=1` = node at `(cursor - 1)`. Clamped on `render` (not
    /// wrapped) so an empty list never panics.
    pub cursor: usize,
}

impl LoraScreen {
    pub fn new(transport: Box<dyn LoraTransport + Send>) -> Self {
        Self {
            transport,
            cursor: 0,
            current_node_ip: None,
        }
    }

    /// Refresh `app.lora_*` from the transport. Cheap; called on every
    /// `Action::Tick`. Keeps the visible state on `App` (so the test
    /// surface matches every other screen) and uses the transport as the
    /// source of truth for nodes + chat history.
    ///
    /// Also detects `app.lora_node_ip` flips and rebuilds the transport
    /// (FakeTransport → HttpLoraTransport, or IP-A → IP-B). Rebuilding
    /// here keeps the renderer thread as the single source of truth for
    /// "which transport is live right now" — the modal submit arm only
    /// mutates `app.lora_node_ip` and emits a toast; the actual swap is
    /// visible to the renderer on the next tick.
    pub fn poll(&mut self, app: &mut App) {
        self.maybe_swap_transport(app);
        app.lora_connected = self.transport.connected();
        app.lora_nodes = self.transport.nodes();
        app.lora_threads = self.transport.threads();
        // Reconcile `lora_active_thread` against the transport's threads.
        //
        // Two failure modes to fix here:
        //
        //   1. User activated a DM via the right pane (Enter on a node
        //      row) but the transport hasn't yet seen an inbound DM
        //      from that peer — so `app.lora_threads` has no
        //      `Direct(n)` for it. `find(...)` in the renderer then
        //      returns `None` and the chat pane renders empty.
        //      Worse, on the *next* tick the previous behaviour was
        //      to snap `lora_active_thread` back to `LongFast`,
        //      which made the title flash on every selection.
        //
        //   2. LongFast is always present (the contract), so it never
        //      lands here.
        //
        // Fix: when `lora_active_thread` is `Direct(n)` and the thread
        // is missing, auto-create it from the matching `LoraNode` (if
        // we know the peer) or a placeholder label. This:
        //   * keeps `find(...)` happy so the chat pane renders an
        //     empty thread (with the "no messages yet" copy) instead
        //     of a placeholder,
        //   * prevents the title from flashing back to `longfast`
        //     because `lora_active_thread` stays pinned to `Direct(n)`,
        //   * keeps the transport's source of truth unmutated — the
        //     placeholder thread lives only on `app`, not on the
        //     transport. When the first real inbound DM from that
        //     peer arrives, `ingest_frame` lands it on the matching
        //     transport thread, and the next `poll()` mirrors it onto
        //     `app` (replacing our placeholder, which had `lines: []`).
        //
        // We DON'T snap back to `LongFast` here — keeping the user's
        // selection sticky is the whole point.
        if !app.lora_threads.iter().any(|t| t.kind == app.lora_active_thread) {
            if let ChannelKind::Direct(n) = app.lora_active_thread {
                let label = app
                    .lora_nodes
                    .iter()
                    .find(|node| {
                        node_id_to_num(&node.node_id) == n
                    })
                    .map(|node| node.label())
                    .unwrap_or_else(|| format!("!{:08x}", n));
                app.lora_threads.push(Thread::new(
                    ChannelKind::Direct(n),
                    label,
                ));
                // Sort DM threads by `to_num` after the LongFast
                // anchor so the right-pane list order stays stable
                // and matches the order the renderer expects when
                // cross-referencing against `app.lora_nodes`.
                sort_lora_threads(&mut app.lora_threads);
            } else {
                // LongFast or any other fallback — snap to LongFast
                // as a last resort (shouldn't happen given the
                // transport's `LongFast` anchor contract, but
                // defensive).
                app.lora_active_thread = ChannelKind::LongFast;
            }
        }
    }

    /// Diff `self.current_node_ip` against `app.lora_node_ip` and rebuild
    /// the transport on a change. See `poll` for rationale.
    ///
    /// Behaviour:
    ///   * `None` → `Some(ip)`: build an `HttpLoraTransport` (when the
    ///     `http` feature is on) or fall back to `FakeTransport` (when
    ///     it's off, so default builds keep working). Spawn the poll
    ///     loop in the feature-on path.
    ///   * `Some(ip)` → `Some(other)`: same as above with the new IP.
    ///   * `Some(ip)` → `None`: drop back to `FakeTransport` so the
    ///     screen doesn't keep trying to talk to a stale node.
    ///   * No change: no-op.
    fn maybe_swap_transport(&mut self, app: &mut App) {
        if self.current_node_ip == app.lora_node_ip {
            return;
        }
        let new_ip = app.lora_node_ip.clone();
        #[cfg(feature = "http")]
        {
            // Build + spawn the poll loop on the tokio runtime. The
            // transport behind `self.transport` is the *old* one until
            // we swap below — that's fine, the poll loop writes into
            // its own state via the `Arc<HttpState>` and the UI sees
            // the new state on the next `poll`.
            //
            // We keep `Arc<HttpLoraTransport>` only around the
            // long-lived poll task (so it owns its own refcount); the
            // `self.transport` slot holds a plain `HttpLoraTransport`
            // because `reqwest::Client` is internally `Arc`'d and the
            // shared `HttpState` is already behind a `Mutex`. Cloning
            // a `HttpLoraTransport` is cheap — two `Arc` bumps and a
            // string copy.
            if let Some(ip) = new_ip.as_deref() {
                match http::HttpLoraTransport::new(ip) {
                    Ok(http) => {
                        let for_poll = std::sync::Arc::new(http);
                        let handle = std::sync::Arc::clone(&for_poll);
                        // Startup self-check: emit an info-level event
                        // announcing the spawn so the user can see in
                        // `RUST_LOG=info` output that the live poll loop
                        // actually fired. Without this log, a
                        // silently-dropped spawn (feature off, future
                        // refactor removes the spawn, runtime
                        // unavailable) looks identical to "node is
                        // quiet" from the UI. The test
                        // `http_lora_screen_poll_actually_starts_the_http_poll_loop`
                        // in `tests/lora_http_live.rs` asserts on
                        // this line so a future regression can't
                        // re-silence it.
                        tracing::info!(host = %ip, "lora: poll loop spawned against {ip}");
                        // `tokio::spawn` requires the runtime; the
                        // TUI is launched inside a tokio runtime
                        // (`#[tokio::main]` in `main.rs`), so a
                        // detached spawn is the right shape.
                        tokio::spawn(async move {
                            handle.run_poll_loop().await;
                        });
                        // Take the inner transport back out of the Arc
                        // for `self.transport`. `Arc::try_unwrap` would
                        // panic on >1 refcount, so we just clone the
                        // inner — cheap, see the comment above.
                        self.transport = Box::new((*for_poll).clone());
                    }
                    Err(e) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Error,
                            format!("lora: bad node URL ({e:?})"),
                        );
                        // Leave the old transport in place so the
                        // screen keeps rendering. The user can fix the
                        // IP and re-submit.
                        self.current_node_ip = None;
                        return;
                    }
                }
            } else {
                // IP cleared — fall back to FakeTransport.
                self.transport = Box::new(FakeTransport::new());
            }
        }
        #[cfg(not(feature = "http"))]
        {
            // Without the `http` feature the IP value is informational
            // only — we can't build an HttpLoraTransport. Still drop
            // back to a fresh FakeTransport so the chat pane resets if
            // the user previously had a stale config.
            //
            // IMPORTANT: push a toast the first time we hit this
            // branch. The failure mode the user hit on `10.0.0.193`
            // — typed a valid IP, saw "not connected" forever, no
            // other feedback — happens because `FakeTransport`
            // reports `connected = false` and the install-script
            // default build doesn't link `reqwest`. Without this
            // toast the user has no way to know the feature flag is
            // the problem.
            if new_ip.is_some() {
                tracing::warn!(
                    "lora: http feature NOT enabled — poll loop will NOT spawn; \
                     rebuild with `cargo build -p cyberdeck-tui --features http` \
                     to see live messages. This is the silent no-poll \
                     failure mode."
                );
                app.push_toast(
                    crate::app::toast::ToastKind::Error,
                    "lora: http feature not enabled — rebuild with \
                     `cargo build -p cyberdeck-tui --features http`",
                );
            }
            let _ = new_ip;
            self.transport = Box::new(FakeTransport::new());
        }
        self.current_node_ip = new_ip;
    }
}

impl Screen for LoraScreen {
    fn id(&self) -> ScreenId {
        ScreenId::LoRa
    }
    fn title(&self) -> &'static str {
        "LoRa"
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        // Right-pane navigation: when the user is focused on the
        // channels+nodes list, `j`/`k`/arrow keys move the cursor
        // through it (LongFast header row, then one row per node).
        // `Enter` opens the focused row as the active thread; `Esc`
        // snaps back to LongFast. `j`/`k` is the discoverable form;
        // arrow keys (Up/Down) get the same behaviour for keyboard
        // navigation habit.
        if app.region == crate::app::Region::ContentRight {
            let total_rows = 1usize.saturating_add(app.lora_nodes.len()); // header + nodes
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.cursor + 1 < total_rows {
                        self.cursor += 1;
                    }
                    return true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                    }
                    return true;
                }
                KeyCode::Enter => {
                    // 0 = LongFast header → activate LongFast.
                    // >=1 → the (cursor-1)-th node → activate Direct(n).
                    // Click sound may want to go here; we keep it pure.
                    if self.cursor == 0 {
                        app.lora_active_thread = ChannelKind::LongFast;
                    } else if let Some(n) = app.lora_nodes.get(self.cursor - 1) {
                        let num = node_id_to_num(&n.node_id);
                        app.lora_active_thread = ChannelKind::Direct(num);
                    }
                    return true;
                }
                KeyCode::Esc => {
                    // Snap back to LongFast regardless of cursor.
                    app.lora_active_thread = ChannelKind::LongFast;
                    return true;
                }
                _ => {} // fall through
            }
        }
        // Chat-pane scroll keys (j/k/G/g/PgUp/PgDn) — only meaningful
        // when the right pane is not the focus, since `j`/`k` would
        // otherwise collide with cursor navigation above.
        match key.code {
            // Up/Down on the chat scroll offset (tail is 0). Like the
            // System screen's right pane: j/k (Up/Down) step one line.
            KeyCode::Char('j') | KeyCode::Down => {
                app.lora_chat_offset = app.lora_chat_offset.saturating_add(1);
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.lora_chat_offset = app.lora_chat_offset.saturating_sub(1);
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                // Snap to the live tail.
                app.lora_chat_offset = 0;
                return true;
            }
            KeyCode::Char('g') => {
                // Snap to the start of the log (oldest visible).
                app.lora_chat_offset = usize::MAX;
                return true;
            }
            KeyCode::PageDown => {
                app.lora_chat_offset = app.lora_chat_offset.saturating_add(10);
                return true;
            }
            KeyCode::PageUp => {
                app.lora_chat_offset = app.lora_chat_offset.saturating_sub(10);
                return true;
            }
            // `i` opens the "Node IP" modal so the user can switch to a
            // different Meshtastic node without restarting the TUI. The
            // modal is pre-filled with the current IP (if any) so the
            // user can edit in place rather than retype. The submit
            // arm in `main.rs::run_input` (InputKind::LoraNodeIp)
            // stashes the typed value on `app.lora_node_ip`; the render
            // loop on this screen (Slice 4) sees the change and swaps
            // the transport. A `?` next to `i` is shown in the input
            // strip (see `render`) so the binding is discoverable.
            //
            // Gating: only when the chat compose line is empty.
            // Otherwise the `i` would be swallowed from the buffer,
            // breaking the user's ability to type "hi" / "ping" /
            // etc. — same mental model as vim's command/insert-mode
            // split. When the buffer is non-empty the keypress falls
            // through to the `Char(c)` arm below and appends.
            KeyCode::Char('i') if app.lora_input.is_empty() => {
                let prefill = app.lora_node_ip.clone().unwrap_or_default();
                app.modal = crate::app::Modal::Input {
                    prompt: "Node IP (Meshtastic HTTP API)".to_string(),
                    buf: prefill,
                    kind: crate::app::InputKind::LoraNodeIp,
                };
                return true;
            }
            // Send the buffer on Enter. Routes through `send_to` so the
            // active thread (LongFast by default, `Direct(n)` once the
            // user has opened a node row with `Enter`) is what the
            // transport encodes — broadcast on the wire for
            // LongFast, DM for `Direct(n)`. The transport echoes the
            // line back through `threads()` so the chat pane can
            // pick it up on the next `poll`.
            KeyCode::Enter => {
                let draft = app.lora_input.clone();
                app.lora_input.clear();
                let kind = app.lora_active_thread.clone();
                match self.transport.send_to(&kind, &draft) {
                    Ok(()) => {
                        // Refresh the thread list so the new line shows.
                        app.lora_threads = self.transport.threads();
                        app.push_toast(crate::app::toast::ToastKind::Ok, "sent");
                    }
                    Err(LoraError::NotConnected) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            "lora: not connected",
                        );
                    }
                    Err(LoraError::Empty) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            "empty message",
                        );
                    }
                    Err(LoraError::TooLong) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            "message too long (max 200)",
                        );
                    }
                    Err(LoraError::Io(s)) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Error,
                            format!("lora io: {s}"),
                        );
                    }
                }
                return true;
            }
            KeyCode::Backspace => {
                app.lora_input.pop();
                return true;
            }
            KeyCode::Char(c) => {
                if app.lora_input.len() < 200 {
                    app.lora_input.push(c);
                }
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" LoRa ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Bottom row: input strip + status.
        let body = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(body);

        // Left: chat pane for the currently-active thread. Pulls lines
        // out of `app.lora_threads[k]` so a LongFast chat and a DM
        // chat can sit side-by-side and the user swaps between them
        // with the right-pane arrow keys + Enter/Esc.
        //
        // Empty-state copy: when no node IP is configured yet the chat
        // pane is the most prominent surface the user sees, so we
        // surface the IP-modal binding here as the call-to-action.
        // Once an IP is set the chat goes back to the generic "no
        // messages yet" hint.
        let active_lines = app
            .lora_threads
            .iter()
            .find(|t| t.kind == app.lora_active_thread)
            .map(|t| t.lines.clone())
            .unwrap_or_default();
        let total = active_lines.len();
        let visible_h = cols[0].height as usize;
        let max_off = total.saturating_sub(1);
        if app.lora_chat_offset > max_off {
            app.lora_chat_offset = max_off;
        }
        let end = total.saturating_sub(app.lora_chat_offset);
        let start = end.saturating_sub(visible_h);
        // Title reflects which thread the user is composing into. For
        // LongFast the title is the channel name; for a DM we show
        // the peer label so the user can confirm before sending.
        let left_title = match &app.lora_active_thread {
            ChannelKind::LongFast => " longfast ".to_string(),
            ChannelKind::Direct(n) => format!(" dm:{:<8} ", truncate(&format!("!{:08x}", n), 8)),
        };
        let items: Vec<ListItem> = if total == 0 {
            let placeholder: String = if app.lora_node_ip.is_none() {
                "  press i to set the node IP".to_string()
            } else {
                "  (no messages yet — j/k scroll, type + Enter to send)".to_string()
            };
            vec![ListItem::new(Line::from(Span::styled(
                placeholder,
                theme.dim(),
            )))]
        } else {
            active_lines[start..end]
                .iter()
                .map(|l| {
                    let arrow = if l.is_local { ">" } else { "·" };
                    let hops = if l.hops_away == 0 && !l.is_local {
                        String::new()
                    } else {
                        format!(" [{}h]", l.hops_away)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{} ", arrow), theme.accent),
                        Span::styled(format!("{:<16}", truncate(&l.from, 16)), theme.dim()),
                        Span::styled(format!(" {}{} ", l.text, hops), theme.fg),
                    ]))
                })
                .collect()
        };
        let highlight = if total == 0 {
            None
        } else {
            Some(items.len().saturating_sub(1))
        };
        let mut left_state = ListState::default().with_selected(highlight);
        let left_focused = !matches!(app.region, crate::app::Region::ContentRight);
        let left = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(left_title, theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(left_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(left, cols[0], &mut left_state);

        // Right: navigable channels+nodes list. The shape is one
        // `[LongFast]` header row followed by one row per known
        // node, total `1 + lora_nodes.len()` rows. The cursor in
        // `self.cursor` (per the on_key binding) points at the
        // currently-focused row and is clamped to stay in range so
        // an empty list never panics. `Enter` activates the row as
        // a thread (LongFast for the header, `Direct(n)` for a node).
        let total_rows = 1usize.saturating_add(app.lora_nodes.len());
        if self.cursor >= total_rows {
            self.cursor = total_rows.saturating_sub(1);
        }
        let mut right_items: Vec<ListItem> = Vec::with_capacity(total_rows);
        // Header row — the LongFast channel entrypoint.
        let lf_label = match app.lora_active_thread {
            ChannelKind::LongFast => "[● LongFast]".to_string(),
            _ => "[○ LongFast]".to_string(),
        };
        right_items.push(ListItem::new(Line::from(Span::styled(
            lf_label,
            theme.title(),
        ))));
        if app.lora_nodes.is_empty() {
            right_items.push(ListItem::new(Line::from(Span::styled(
                "  (no nodes yet — talk to your radio first)",
                theme.dim(),
            ))));
        } else {
            let now = LoraNode::now_secs();
            for n in &app.lora_nodes {
                let online = n.is_online_at(now);
                let dot = if online { "●" } else { "○" };
                let dot_style = if online { theme.ok() } else { theme.warn() };
                let hops = if n.hops_away == 0 {
                    "direct".to_string()
                } else {
                    format!("{} hops", n.hops_away)
                };
                let snr = match n.snr {
                    Some(v) => format!("{:.1} dB", v),
                    None => "—".to_string(),
                };
                // Highlight the row whose DM is currently the active
                // thread so the user can see which conversation the
                // compose line is wired to.
                let peer_active = matches!(
                    app.lora_active_thread,
                    ChannelKind::Direct(num) if node_id_to_num(&n.node_id) == num
                );
                let name_style = if peer_active {
                    theme.ok()
                } else {
                    ratatui::style::Style::default().fg(theme.accent)
                };
                right_items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", dot), dot_style),
                    Span::styled(format!("{:>8}", truncate(&n.label(), 8)), name_style),
                    Span::styled(format!("  {:<10}", hops), theme.fg),
                    Span::styled(format!("  {:>8}", snr), theme.dim()),
                ])));
            }
        }
        let right_highlight = if app.lora_nodes.is_empty() && total_rows == 0 {
            None
        } else {
            Some(self.cursor.min(right_items.len().saturating_sub(1)))
        };
        let mut right_state = ListState::default().with_selected(right_highlight);
        let right_focused = matches!(app.region, crate::app::Region::ContentRight);
        let right = List::new(right_items)
            .block(
                Block::default()
                    .title(Span::styled(" channels & nodes ", theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(right_focused)),
            )
            .highlight_style(
                ratatui::style::Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg),
            )
            .highlight_symbol("▸ ");
        f.render_stateful_widget(right, cols[1], &mut right_state);

        // Bottom: input + status.
        let status = if app.lora_connected {
            Span::styled("● connected ", theme.ok())
        } else {
            Span::styled("○ not connected ", theme.warn())
        };
        // The compose line's `to:` chip now reflects the active
        // thread (`app.lora_active_thread`), NOT a node index. The
        // user flips it with `Enter` on the right pane's focused
        // row; `Esc` snaps it back. Keeping this in one place means
        // the chip and the left pane's title can never disagree.
        let to_node = match &app.lora_active_thread {
            ChannelKind::LongFast => "to: broadcast".to_string(),
            ChannelKind::Direct(n) => {
                // Prefer the operator-chosen label if we know the
                // peer; fall back to `!xxxxxxxx` so the user can
                // still tell which node they're replying to.
                let nid = format!("!{:08x}", n);
                let label = app
                    .lora_nodes
                    .iter()
                    .find(|cand| cand.node_id == nid)
                    .map(|cand| cand.label())
                    .unwrap_or_else(|| truncate(&nid, 8));
                format!("to: {}", label)
            }
        };
        // IP-modal binding chip — always visible so the user can
        // re-enter the modal (and switch to a different node) at any
        // time. The chip shows the *current* IP if one is set, so the
        // user can confirm what's configured. The chip's text is kept
        // out of the chat input buffer (rendered as a separate span
        // cluster, right-side of the strip) so typing still hits the
        // compose line.
        let ip_chip = if let Some(ip) = app.lora_node_ip.as_deref() {
            Span::styled(
                format!(" i:{} ", truncate(ip, 16)),
                theme.accent,
            )
        } else {
            Span::styled(" i: ip ", theme.warn())
        };
        let input = Paragraph::new(Line::from(vec![
            status,
            Span::raw(" "),
            Span::styled(format!("{} ", to_node), theme.dim()),
            Span::styled("> ", theme.key()),
            Span::styled(app.lora_input.clone(), theme.fg),
            Span::styled("▏", theme.accent),
            Span::raw("  "),
            ip_chip,
        ]))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(theme.border(false)),
        );
        let input_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
        f.render_widget(input, input_area);
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Stable sort for the `app.lora_threads` mirror: LongFast first (always
/// at index 0), then `Direct(n)` entries sorted ascending by `n`. Used
/// after `LoraScreen::poll` auto-seeds a `Direct(n)` placeholder so the
/// right-pane list order matches the order the renderer and tests
/// expect.
fn sort_lora_threads(threads: &mut Vec<Thread>) {
    threads.sort_by(|a, b| {
        match (&a.kind, &b.kind) {
            (ChannelKind::LongFast, ChannelKind::LongFast) => std::cmp::Ordering::Equal,
            (ChannelKind::LongFast, _) => std::cmp::Ordering::Less,
            (_, ChannelKind::LongFast) => std::cmp::Ordering::Greater,
            (ChannelKind::Direct(a), ChannelKind::Direct(b)) => a.cmp(b),
        }
    });
}

/// Convenience: detect whether a path looks like a Meshtastic USB-CDC
/// mount (`/dev/ttyUSB*` or `/dev/ttyACM*`). Kept around for the legacy
/// serial-transport path; the LoRa screen's runtime IP modal does NOT
/// depend on this. Public so the `App` can default to it.
#[cfg(unix)]
pub fn is_meshtastic_serial_path(p: &std::path::Path) -> bool {
    if !p.exists() {
        return false;
    }
    // Conservative: only flag ttyUSB*/ttyACM*. We deliberately do NOT
    // poke the device because poking could trigger bootloader resets on
    // some Meshtastic boards. Real auto-detection lands with a feature
    // gate; this is the default placeholder.
    let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    name.starts_with("ttyUSB") || name.starts_with("ttyACM")
}

#[cfg(not(unix))]
pub fn is_meshtastic_serial_path(_p: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Region;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc;

    fn make_app() -> App {
        let (tx, rx) = mpsc::channel(8);
        App::new(tx, rx)
    }

    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    // Tiny helper: short id string for a node that has no operator-chosen
    // names yet. We keep the id slice (NOT padded) because the node-id is
    // already a fixed-width prefix in the real Meshtastic protocol; the
    // 8-col right-pad we use for the "name" column is independent.
    fn empty_id_node() -> LoraNode {
        LoraNode {
            node_id: "!abcdef01".into(),
            long_name: String::new(),
            short_name: String::new(),
            hops_away: 0,
            last_heard_secs: 0,
            snr: None,
        }
    }

    #[test]
    fn fake_transport_rejects_send_when_disconnected() {
        let mut t = FakeTransport::new();
        // No device — send must fail with NotConnected without panicking.
        assert_eq!(
            t.send_longfast("hello"),
            Err(LoraError::NotConnected)
        );
        assert!(t.sent.is_empty());
    }

    #[test]
    fn fake_transport_rejects_empty_message() {
        let mut t = FakeTransport::new().with_connected(true);
        assert_eq!(t.send_longfast("   \n"), Err(LoraError::Empty));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn fake_transport_rejects_overlong_message() {
        let mut t = FakeTransport::new().with_connected(true);
        let long = "a".repeat(201);
        assert_eq!(t.send_longfast(&long), Err(LoraError::TooLong));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn fake_transport_echoes_local_line_to_messages() {
        let mut t = FakeTransport::new().with_connected(true);
        t.send_longfast("hello lora").unwrap();
        assert_eq!(t.sent, vec!["hello lora".to_string()]);
        assert_eq!(t.messages().len(), 1);
        let line = &t.messages()[0];
        assert!(line.is_local);
        assert_eq!(line.text, "hello lora");
        assert_eq!(line.from, "me");
    }

    #[test]
    fn lora_node_label_prefers_long_then_short_then_id() {
        let n = LoraNode {
            node_id: "!abcdef01".into(),
            long_name: String::new(),
            short_name: "alpha".into(),
            hops_away: 2,
            last_heard_secs: 5,
            snr: Some(7.5),
        };
        assert_eq!(n.label(), "alpha");

        let m = LoraNode {
            node_id: "!abcdef01".into(),
            long_name: "Trailer".into(),
            short_name: "alpha".into(),
            hops_away: 2,
            last_heard_secs: 5,
            snr: Some(7.5),
        };
        assert_eq!(m.label(), "Trailer");

        // No operator-chosen names — fall back to a slice of the node id.
        // `label()` returns the slice verbatim; the render layer is what
        // pads to column width.
        let k = empty_id_node();
        assert_eq!(k.label(), "!abcdef01");
    }

    #[test]
    fn poll_copies_transport_state_into_app() {
        let node = LoraNode {
            node_id: "!aabbccdd".into(),
            long_name: "Trailer".into(),
            short_name: "TR".into(),
            hops_away: 1,
            last_heard_secs: 30,
            snr: Some(4.0),
        };
        let line = LoraChatLine {
            from: "TR".into(),
            text: "test ping".into(),
            hops_away: 1,
            is_local: false,
        };
        let mut t = FakeTransport::with_nodes(vec![node.clone()]).with_connected(true);
        let mut th = Thread::new(ChannelKind::LongFast, "longfast");
        th.lines.push(line.clone());
        t.threads.push(th);

        let mut app = make_app();
        let mut screen = LoraScreen::new(Box::new(t));
        screen.poll(&mut app);
        assert!(app.lora_connected);
        assert_eq!(app.lora_nodes, vec![node]);
        assert_eq!(
            app.lora_threads.first().map(|th| th.lines.as_slice()),
            Some([line.clone()].as_slice()),
            "lora_threads must mirror transport.threads() (LongFast first)"
        );
    }

    // Typing builds up the input buffer; Enter sends it via the transport.
    // The screen's `poll` after the Enter mirrors what the live main loop
    // does on every tick; `lora_chat` then contains the new local line.
    #[test]
    fn typing_and_enter_sends_via_transport() {
        let mut app = make_app();
        let t = FakeTransport::new().with_connected(true);
        let mut screen = LoraScreen::new(Box::new(t));

        // Type "hi".
        assert!(screen.on_key(kc('h'), &mut app));
        assert!(screen.on_key(kc('i'), &mut app));
        assert_eq!(app.lora_input, "hi");
        // Enter sends.
        assert!(screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app));
        // Buffer cleared, line echoed into messages via `poll`.
        assert_eq!(app.lora_input, "");
        screen.poll(&mut app);
        // Default active thread is LongFast; the local echo lives there.
        let longfast = app
            .lora_threads
            .iter()
            .find(|th| th.kind == ChannelKind::LongFast)
            .expect("LongFast thread must exist after Enter");
        assert_eq!(longfast.lines.len(), 1);
        assert!(longfast.lines[0].is_local);
        assert_eq!(longfast.lines[0].text, "hi");
    }

    // Enter with an empty/disconnected transport pushes a toast and does
    // NOT mutate `lora_chat`. Important so a misconfigured box (no node
    // configured) doesn't fill the toast log with empty messages.
    #[test]
    fn enter_with_no_connection_pushes_warn_toast_and_no_message() {
        let mut app = make_app();
        let t = FakeTransport::new(); // disconnected
        let mut screen = LoraScreen::new(Box::new(t));

        assert!(screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app));
        assert!(
            app.lora_threads.iter().all(|th| th.lines.is_empty()),
            "no line should be added to any thread"
        );
        assert!(
            app.toasts.iter().any(|t| t.text.contains("not connected")),
            "expected a 'not connected' toast, got {:?}",
            app.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>()
        );
    }

    // `Backspace` deletes a char; Tab/BackTab move the nodes cursor.
    #[test]
    fn backspace_tab_navigate() {
        let mut app = make_app();
        let nodes = vec![
            LoraNode {
                node_id: "!aa".into(),
                long_name: "A".into(),
                short_name: "A".into(),
                hops_away: 1,
                last_heard_secs: 1,
                snr: Some(1.0),
            },
            LoraNode {
                node_id: "!bb".into(),
                long_name: "B".into(),
                short_name: "B".into(),
                hops_away: 2,
                last_heard_secs: 1,
                snr: Some(2.0),
            },
        ];
        let t = FakeTransport::with_nodes(nodes.clone());
        let mut screen = LoraScreen::new(Box::new(t));
        app.lora_nodes = nodes;

        // Type "ab", backspace once → "a".
        screen.on_key(kc('a'), &mut app);
        screen.on_key(kc('b'), &mut app);
        assert_eq!(app.lora_input, "ab");
        screen.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
        assert_eq!(app.lora_input, "a");

        // The right-pane cursor (`self.cursor`) is driven by Up/Down
        // (and j/k) under the new navigable-UI design. Down advances,
        // Up retreats, both clamp at the bounds (no wrap). The
        // handler is gated on `app.region == ContentRight`, so the
        // test must set the region before pressing the keys.
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        app.region = Region::ContentRight;
        assert_eq!(screen.cursor, 0);
        screen.on_key(down, &mut app);
        assert_eq!(screen.cursor, 1, "Down advances to first node");
        screen.on_key(down, &mut app);
        assert_eq!(screen.cursor, 2, "Down advances to second node");
        screen.on_key(down, &mut app);
        assert_eq!(screen.cursor, 2, "Down clamps at last row");
        screen.on_key(up, &mut app);
        assert_eq!(screen.cursor, 1, "Up retreats from last node");
        screen.on_key(up, &mut app);
        assert_eq!(screen.cursor, 0, "Up clamps at first row");
    }

    // G/g snap the chat scroll offset.
    #[test]
    fn g_and_g_set_chat_offset() {
        let mut app = make_app();
        let t = FakeTransport::new();
        let mut screen = LoraScreen::new(Box::new(t));
        app.lora_chat_offset = 5;
        screen.on_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE), &mut app);
        assert_eq!(app.lora_chat_offset, 0, "G == live tail");
        screen.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE), &mut app);
        assert_eq!(app.lora_chat_offset, usize::MAX, "g == oldest visible");
    }

    // Lora transport trait is object-safe so Box<dyn LoraTransport + Send>
    // is usable from `LoraScreen` on a tokio task. If a future change
    // accidentally adds a generic method to the trait, this test fails.
    #[test]
    fn lora_transport_is_object_safe() {
        fn assert_object_safe(_t: Box<dyn LoraTransport + Send>) {}
        assert_object_safe(Box::new(FakeTransport::new()));
    }

    // ── Slice 4b: DM-thread end-to-end reply ─────────────────────────
    //
    // This is the single most important user flow for the LoRa screen:
    //   1. user picks a peer in the right-pane nav,
    //   2. types into the input strip,
    //   3. hits Enter,
    //   4. sees their message bubble land in that peer's thread.
    //
    // It exercises the *whole* chain wired this slice:
    //   `Region::ContentRight` gate → Down + Enter on the nav cursor
    //   → typing → Enter → `send_to(&Direct(node_num), …)` →
    //   `screen.poll(&mut app)` → mirror `transport.threads()` into
    //   `app.lora_threads` → bubble appears in the right thread.
    //
    // Without this test, a regression in any single link of that chain
    // could ship unnoticed because none of the other tests cross
    // ChannelKind::LongFast → ChannelKind::Direct(_).
    #[test]
    fn send_to_dm_routes_into_dm_thread_via_poll() {
        // Seed a `FakeTransport` with the LongFast header thread and a
        // DM thread for `Direct(0x42)`. Pre-seed one inbound line in
        // the DM thread so we can assert the user's reply *appends*
        // (doesn't replace) and that `is_local=true` distinguishes it
        // from the peer.
        let mut transport = FakeTransport::new().with_connected(true);
        // LongFast at index 0 — implicit via `longfast_thread_mut`.
        let _ = transport.longfast_thread_mut();
        let dm_pre = LoraChatLine {
            from: "!00000042".into(),
            text: "ack me?".into(),
            hops_away: 1,
            is_local: false,
        };
        transport.direct_thread_mut(0x42).lines.push(dm_pre);
        // Seed a node so it shows up in the right-pane nav list (so
        // the Down + Enter on `app.region = ContentRight` actually
        // lands on a Direct thread).
        transport.nodes.push(LoraNode {
            node_id: "!00000042".into(),
            long_name: "trucker".into(),
            short_name: "T".into(),
            hops_away: 1,
            last_heard_secs: 0,
            snr: None,
        });

        // Build the screen + app and switch focus to the right pane so
        // Down + Enter route through the nav handler (not the input
        // strip). The nav handler indexes `app.lora_nodes` (NOT
        // `app.lora_threads`): `cursor = 0` ⇒ LongFast header,
        // `cursor = 1..len ⇒ Direct(node_id_to_num(app.lora_nodes[cursor-1]))`.
        // So we MUST `screen.poll(&mut app)` first to mirror the
        // seeded transport state into `app.lora_nodes`; otherwise
        // the cursor would have an empty rows list and Enter would
        // have nothing to activate.
        let mut screen = LoraScreen::new(Box::new(transport));
        let mut app = make_app();
        screen.poll(&mut app);
        app.region = Region::ContentRight;
        // Initial active thread is LongFast (per `LoraScreen::new`),
        // and the cursor starts at 0 (LongFast row). One Down moves
        // the cursor to the first DM row (`app.lora_nodes[0]`); Enter
        // activates it as Direct(node_id_to_num("!00000042")).
        assert_eq!(app.lora_nodes.len(), 1);
        assert_eq!(app.lora_nodes[0].node_id, "!00000042");
        screen.on_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app,
        );
        screen.on_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );

        // Sanity: we landed on Direct(0x42), NOT LongFast.
        assert_eq!(
            app.lora_active_thread,
            ChannelKind::Direct(0x42),
            "Down+Enter in ContentRight must activate Direct(0x42), \
             not stay on LongFast"
        );

        // Focus back to the input strip and type "hi" + Enter. We pick
        // a word WITHOUT `j`/`k` because both collide with the
        // chat-pane scroll handler at line 641 when the right pane
        // is not focused (`KeyCode::Char('j') | Down` and
        // `KeyCode::Char('k') | Up` are both captured there before
        // reaching the input-strip's `KeyCode::Char(c)` handler at
        // line 736 — those keys are deliberately shared between
        // nav-scroll and input-cursor, like vim). "hi" is the same
        // word the design doc uses for the smoke test.
        app.region = Region::ContentLeft;
        screen.on_key(kc('h'), &mut app);
        screen.on_key(kc('i'), &mut app);
        assert_eq!(app.lora_input, "hi");
        screen.on_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.lora_input, "");

        // poll() mirrors transport.threads() into app.lora_threads
        // — note: we observe the *user-visible* surface
        // (`app.lora_threads`) rather than reading `FakeTransport`'s
        // private `sent` / `sent_dm` logs, because the screen only
        // sees the transport through `Box<dyn LoraTransport + Send>`,
        // not the concrete `FakeTransport` type. The trait's
        // `messages_for(kind)` is what `screen.poll` uses internally
        // to populate `app.lora_threads`, so observing the mirror
        // *is* the proof that `send_to` was called correctly.
        screen.poll(&mut app);

        // The DM thread for 0x42 must now hold BOTH lines:
        //   - the seeded peer line ("ack me?"),
        //   - the user's reply ("hi") with is_local = true.
        let dm = app
            .lora_threads
            .iter()
            .find(|t| t.kind == ChannelKind::Direct(0x42))
            .expect("Direct(0x42) thread must exist after poll()");
        assert_eq!(
            dm.lines.len(),
            2,
            "Direct(0x42) thread must contain the seeded line + the user's reply"
        );
        assert!(!dm.lines[0].is_local, "seeded line is from peer, not local");
        assert_eq!(dm.lines[0].text, "ack me?");
        assert!(dm.lines[1].is_local, "user's reply must be marked is_local");
        assert_eq!(dm.lines[1].text, "hi");
        assert_eq!(
            dm.lines[1].from, "me",
            "local echo's `from` field is the literal string \"me\" — see \
             FakeTransport::send_to at lora.rs:404-409"
        );

        // LongFast thread must remain untouched — the user typed into
        // a DM, so broadcast had nothing to gain an echoed line.
        // (Together with `dm.lines.len() == 2` above, this also
        // proves the routing is DM-only: the user's "hi" went to
        // Direct(0x42), NOT to the broadcast channel.)
        let longfast = app
            .lora_threads
            .iter()
            .find(|t| t.kind == ChannelKind::LongFast)
            .expect("LongFast header thread must still exist");
        assert_eq!(
            longfast.lines.len(),
            0,
            "LongFast must NOT receive a local echo when the user is in a DM"
        );
    }

    // ── Slice 6: chat-invisible / title-flash regression pins ──────
    //
    // User-reported bug: after pressing Enter to activate a DM thread
    // in the right pane, the chat pane renders empty AND the title
    // flashes back to "longfast" on the next tick.
    //
    // Root cause was `LoraScreen::poll` snapping
    // `app.lora_active_thread` back to `LongFast` whenever the
    // transport's `threads()` did not yet contain a `Direct(n)`
    // entry (which is true until the first inbound DM from that
    // peer arrives). The renderer's `find(...)` then returns `None`
    // and the chat pane renders the empty-thread placeholder.
    //
    // Fix: when `lora_active_thread` is `Direct(n)` and the thread
    // is missing from the transport mirror, auto-seed it on
    // `App::lora_threads` with the peer's label so:
    //   (a) the title sticks to `dm:…` (no flash back to longfast),
    //   (b) the chat pane has a thread to render against (empty
    //       lines + the "(no messages yet)" copy),
    //   (c) typing still routes to that thread via
    //       `send_to(&Direct(n), …)` (covered by
    //       `send_to_dm_routes_into_dm_thread_via_poll` above).
    //
    // These tests pin each part of that contract.
    #[test]
    fn poll_does_not_snap_active_thread_back_when_direct_is_activated() {
        // Seed a FakeTransport with one node but no thread for that
        // node (matches the live path: the user has activated a node
        // in the right pane before the firmware has emitted any
        // inbound DM from them).
        let mut t = FakeTransport::new().with_connected(true);
        t.nodes.push(LoraNode {
            node_id: "!0000000a".into(),
            long_name: String::new(),
            short_name: String::new(),
            hops_away: 1,
            last_heard_secs: 0,
            snr: None,
        });
        // Critically: no `Direct(0xa)` thread exists in the
        // transport yet. The user's typical path on the real HTTP
        // backend looks like this until the first DM round-trips.

        let mut app = make_app();
        let mut screen = LoraScreen::new(Box::new(t));
        // Mirror transport state into App and activate the DM the
        // way the right-pane Enter handler would.
        screen.poll(&mut app);
        app.region = Region::ContentRight;
        screen.on_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app,
        );
        screen.on_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.lora_active_thread, ChannelKind::Direct(0xa));

        // Now run another `poll` — the previous behaviour snapped
        // back to `LongFast` here, which is exactly the title-flash
        // bug. After the fix, the active thread must stick.
        screen.poll(&mut app);
        assert_eq!(
            app.lora_active_thread,
            ChannelKind::Direct(0xa),
            "poll() must NOT snap lora_active_thread back to LongFast; \
             that was the user-visible title-flash bug"
        );
    }

    #[test]
    fn poll_auto_seeds_missing_direct_thread_when_user_activates_it() {
        // Companion test: when poll() pins the activated DM, it must
        // also create a matching `Direct(n)` entry on
        // `app.lora_threads` so the renderer's `find(...)` returns
        // `Some(...)` instead of falling back to the empty-thread
        // placeholder copy.
        let mut t = FakeTransport::new().with_connected(true);
        t.nodes.push(LoraNode {
            node_id: "!0000000a".into(),
            long_name: "trucker".into(),
            short_name: "T".into(),
            hops_away: 1,
            last_heard_secs: 0,
            snr: None,
        });

        let mut app = make_app();
        let mut screen = LoraScreen::new(Box::new(t));
        screen.poll(&mut app);
        app.region = Region::ContentRight;
        screen.on_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app,
        );
        screen.on_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );
        screen.poll(&mut app);

        let dm = app
            .lora_threads
            .iter()
            .find(|t| t.kind == ChannelKind::Direct(0xa))
            .expect("poll() must auto-seed a Direct(0xa) thread so \
                     the renderer can show the chat pane (even if empty)");
        // Label uses the operator-chosen long_name when known.
        assert_eq!(dm.label, "trucker");
        // The seeded thread is empty — first real inbound DM from
        // that peer will replace the placeholder via the transport's
        // own thread (lines populated, label preserved).
        assert!(
            dm.lines.is_empty(),
            "auto-seeded DM thread has no lines until inbound traffic arrives"
        );
    }

    #[test]
    fn sort_lora_threads_keeps_longfast_first_and_orders_dms_ascending() {
        // Pin the thread-list sort order so the right-pane list is
        // deterministic. The renderer doesn't depend on this order
        // but the renderer + tests cross-reference DM threads
        // against `app.lora_nodes`, where stable ordering matters
        // for diffs in screen captures.
        let mut threads = vec![
            Thread::new(ChannelKind::Direct(3), "three"),
            Thread::new(ChannelKind::Direct(1), "one"),
            Thread::new(ChannelKind::LongFast, "longfast"),
            Thread::new(ChannelKind::Direct(2), "two"),
        ];
        sort_lora_threads(&mut threads);
        let kinds: Vec<_> = threads.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                ChannelKind::LongFast,
                ChannelKind::Direct(1),
                ChannelKind::Direct(2),
                ChannelKind::Direct(3),
            ],
            "LongFast at index 0, then Direct(n) ascending"
        );
    }

    // ── Slice 5: online indicator ────────────────────────────────────
    //
    // `is_online_at` is the single source of truth for "should this
    // node's row show a filled or hollow dot?". The threshold is
    // 15 min by spec (matches the meshtastic/web UI convention).
    // The encoding heuristic — values > 1e12 are unix-epoch
    // seconds, smaller values are relative seconds — is also pinned
    // here so a future proto-decode slice can't quietly flip the
    // semantics.

    fn node_with_last_heard(last: u64) -> LoraNode {
        LoraNode {
            node_id: "!abcdef01".into(),
            long_name: "alpha".into(),
            short_name: "α".into(),
            hops_away: 1,
            last_heard_secs: last,
            snr: Some(-3.5),
        }
    }

    #[test]
    fn online_when_last_heard_within_15_min_unix_epoch() {
        // Real post-2001 unix-epoch values (≥ 1e12) so the heuristic
        // picks the absolute branch in `is_online_at`. Mid-2025
        // ≈ 1_750_000_000 (well above the cutoff).
        let now: u64 = 1_750_000_000;
        // 5 min ago in unix-epoch terms.
        let n = node_with_last_heard(now - 5 * 60);
        assert!(n.is_online_at(now));
    }

    #[test]
    fn offline_when_last_heard_just_over_15_min_ago_unix_epoch() {
        let now: u64 = 1_750_000_000;
        // 15 min + 1 sec ago — just outside the threshold.
        let n = node_with_last_heard(now - (ONLINE_THRESHOLD_SECS + 1));
        assert!(!n.is_online_at(now));
    }

    #[test]
    fn online_when_last_heard_is_zero_means_recent_relative() {
        // Relative encoding: 0 = "just heard" → online. `now` here
        // is irrelevant (the relative branch ignores it) but we
        // still pass one to keep the API uniform.
        let now: u64 = 1_750_000_000;
        let n = node_with_last_heard(0);
        assert!(n.is_online_at(now));
    }

    #[test]
    fn offline_when_last_heard_relative_just_over_threshold() {
        let now: u64 = 1_750_000_000;
        // Relative: 15 min + 1 sec ago → offline.
        let n = node_with_last_heard(ONLINE_THRESHOLD_SECS + 1);
        assert!(!n.is_online_at(now));
    }

    #[test]
    fn online_at_exact_threshold_boundary() {
        // Boundary check: `now - last == ONLINE_THRESHOLD_SECS` is
        // still online (the spec is "within 15 min"). Real
        // unix-epoch values so the absolute branch fires.
        let now: u64 = 1_750_000_000;
        let n = node_with_last_heard(now - ONLINE_THRESHOLD_SECS);
        assert!(n.is_online_at(now));
    }

#[test]
    fn online_threshold_constant_matches_spec() {
        // Pin the constant so a future "tweak it to 30 min" change
        // shows up as a deliberate test update.
        assert_eq!(ONLINE_THRESHOLD_SECS, 15 * 60);
    }

    /// Live regression for #2 ("live messages don't show"). The wiremock
    /// tests at `crates/tui/tests/lora_http_live.rs` prove the transport
    /// path works end-to-end. This test proves the UI path: a chat line
    /// pushed onto the transport's LongFast thread must surface in the
    /// renderer's data source (`app.lora_threads[*].lines`) after a
    /// `poll()` tick, and the active thread must stay LongFast so the
    /// chat pane is the one drawn.
    #[test]
    fn poll_surfaces_inbound_longfast_line_in_chat_pane() {
        let mut t = FakeTransport::new().with_connected(true);
        // Seed an inbound chat line on the LongFast thread, mimicking
        // what `ingest_frame` does on the real HTTP transport when a
        // broadcast arrives.
        t.longfast_thread_mut().lines.push(LoraChatLine {
            from: "!aabbccdd".into(),
            text: "hello-from-peer".into(),
            hops_away: 1,
            is_local: false,
        });

        let mut app = make_app();
        let mut screen = LoraScreen::new(Box::new(t));
        screen.poll(&mut app);

        // The mirrored threads must contain the inbound line.
        let lf = app
            .lora_threads
            .iter()
            .find(|th| th.kind == ChannelKind::LongFast)
            .expect("LongFast thread must exist after poll()");
        assert!(
            lf.lines.iter().any(|l| l.text == "hello-from-peer"),
            "inbound LongFast line must surface in app.lora_threads[LongFast].lines; \
             got {:?}",
            lf.lines
        );
        // Active thread stays LongFast so the chat pane is drawn.
        assert_eq!(app.lora_active_thread, ChannelKind::LongFast);
    }

    /// Live regression for #1 ("title flashes real quick and goes back to
    /// longfast"). The existing `poll_does_not_snap_active_thread_back_when_direct_is_activated`
    /// test covers ONE poll after Enter. This test covers FIVE consecutive
    /// polls — closer to the live cadence where the user sees the title
    /// flash over multiple frames before it settles.
    #[test]
    fn clicking_node_row_keeps_active_thread_direct_across_five_polls() {
        let mut t = FakeTransport::new().with_connected(true);
        t.nodes.push(LoraNode {
            node_id: "!0000000a".into(),
            long_name: "trucker".into(),
            short_name: "T".into(),
            hops_away: 1,
            last_heard_secs: 0,
            snr: None,
        });

        let mut app = make_app();
        let mut screen = LoraScreen::new(Box::new(t));
        screen.poll(&mut app);
        app.region = Region::ContentRight;
        // Move cursor down to the node row (cursor=1), then Enter.
        screen.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert_eq!(
            app.lora_active_thread,
            ChannelKind::Direct(0xa),
            "Enter on node row must activate Direct(0xa) before any further poll"
        );

        // Now poll 5 times — if anything snaps the active thread back
        // to LongFast across these polls, the user sees the title flash.
        for i in 0..5 {
            screen.poll(&mut app);
            assert_eq!(
                app.lora_active_thread,
                ChannelKind::Direct(0xa),
                "poll #{} must NOT snap lora_active_thread back to LongFast; \
                 that was the user-visible title-flash bug",
                i
            );
        }
    }
}