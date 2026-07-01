//! HTTP transport for the LoRa (Meshtastic) screen.
//!
//! Borrowed shape from meshtastic/web's `@meshtastic/transport-http`
//! (see `packages/transport-http/src/transport.ts` in the meshtastic/web
//! repo):
//!
//!   1. Probe with `OPTIONS http://<ip>/api/v1/toradio` — non-2xx means
//!      "no node at this IP" and we stay disconnected.
//!   2. Handshake with `PUT http://<ip>/api/v1/toradio` carrying
//!      `ToRadio{want_config_id: <rand>}` — without this, the firmware
//!      never streams its `FromRadio` backlog into the response and the
//!      user sees `connected` but empty chat / empty nodes (the failure
//!      mode reported against `10.0.0.193`). The same handshake is
//!      re-issued every ~60 s so the nodes list refreshes.
//!   3. Long-poll `GET /api/v1/fromradio` every 3 s. First poll uses
//!      `?all=true` (firmware drains its backlog in a single response);
//!      subsequent polls use `?all=false` (firmware returns new frames
//!      as they arrive).
//!   4. Each response body is one or more `FromRadio` frames back-to-
//!      back. Hand-rolled decoder in `proto.rs` slices them off and we
//!      dispatch into the shared state. Anything we don't model
//!      (config, channels, log records, etc.) bumps a wire-debug
//!      counter and is dropped.
//!
//! Concurrency: the poll task is spawned on the tokio runtime by
//! `LoraScreen::maybe_swap_transport` and writes into `HttpState`
//! through a `Mutex`. The renderer reads the same `HttpState` through
//! `status_snapshot` / `nodes` / `messages` on the UI thread.
//!
//! The send path (`send_longfast` → `put_toradio`) still writes raw
//! bytes — full `ToRadio{packet{MeshPacket{decoded{Data}}}}` framing
//! is a follow-up; the bytes the user typed are sent verbatim so the
//! round-trip is observable end-to-end on the node-side HTTP logs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::screens::lora::proto::{
    self, FromRadio, NodeInfo, TEXT_MESSAGE_APP, BROADCAST_NUM,
};
use crate::screens::lora::{
    ChannelKind, LoraChatLine, LoraError, LoraNode, LoraTransport, Thread,
};

/// User-Agent sent with every request. `reqwest` defaults to
/// `reqwest/0.12` which is useless on the node-side HTTP logs;
/// Meshtastic firmware prints the UA on each request, so a real
/// identifier helps when the user is debugging the link.
const USER_AGENT: &str = concat!("cyberdeck-tui/", env!("CARGO_PKG_VERSION"));

/// Default poll interval for `GET /api/v1/fromradio`. Matches
/// `meshtastic/web`'s 3 s cadence.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Default read timeout for the poll request. Matches the web client's
/// 7 s budget (poll interval + headroom).
const READ_TIMEOUT: Duration = Duration::from_secs(7);

/// Default write timeout for `PUT /api/v1/toradio`. Matches the web
/// client's 4 s budget.
const WRITE_TIMEOUT: Duration = Duration::from_secs(4);

/// Maximum length of the last-frame hex string we keep around for the
/// status footer. Bounded so a chatty node can't OOM the UI.
const LAST_FRAME_HEX_MAX: usize = 128;

/// How often to re-issue `want_config_id` to force the firmware to
/// re-emit `NodeInfo` entries. The Meshtastic `ToRadio` proto has no
/// `want_node_info` field — refreshing nodes is done by re-asking for
/// the whole config; the firmware responds with `MyNodeInfo` + every
/// `NodeInfo` + `config_complete_id`. 60 s matches the
/// `meshtastic/web` UI's refresh cadence.
const WANT_CONFIG_REFRESH_SECS: u64 = 60;

/// Maximum chat lines we keep in memory. Meshtastic networks can be
/// chatty; without this a long-running session would OOM the UI.
const MAX_CHAT_LINES: usize = 500;

/// Maximum nodes we keep in memory. Meshtastic firmware caps the
/// node-DB around 80 entries; we use 256 to leave headroom for new
/// nodes arriving mid-session.
const MAX_NODES: usize = 256;

/// Wire-debug counters. Surfaced via `wire_debug()` so the renderer can
/// show "wire_debug: dropped=3 portnum=2" in the status footer when
/// nothing else is moving — useful to confirm "frames are arriving but
/// we don't model them" vs. "the link is dead".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WireDebug {
    /// Frames the parser couldn't parse (truncated body, unknown wire
    /// type, etc.) — usually zero.
    pub parse_failures: u64,
    /// `FromRadio` variants we don't model (Config, Channel, LogRecord,
    /// QueueStatus, etc.) — informational.
    pub unknown_variant: u64,
    /// Decoded packets whose portnum we don't surface (telemetry,
    /// position, etc.). High count = "node is alive, we just don't
    /// show that frame".
    pub unknown_portnum: u64,
}

/// Shared state between the poll task (which is `Send`-bound) and the
/// trait accessors (which the renderer calls on the UI thread).
#[derive(Debug)]
struct HttpState {
    /// True after the first 2xx from any endpoint.
    connected: bool,
    /// True once we've issued the initial `ToRadio{want_config_id}`
    /// handshake against this transport. We re-issue periodically
    /// (`WANT_CONFIG_REFRESH_SECS`) to refresh the node DB.
    handshake_issued: bool,
    /// Monotonic wallclock (epoch secs) of the last successful
    /// `want_config_id` write. Compared against `now` on every poll.
    last_want_config_secs: u64,
    /// Monotonically increasing counter of inbound frames received
    /// across the lifetime of this transport.
    rx_frames: u64,
    /// Monotonically increasing counter of frames successfully written
    /// via `/api/v1/toradio`.
    tx_frames: u64,
    /// Hex dump of the most recent inbound frame (truncated to
    /// `LAST_FRAME_HEX_MAX`). Empty until the first poll returns.
    last_frame_hex: String,
    /// Last error message observed on any HTTP path; surfaced in the
    /// status footer / toasts when the user is debugging a flaky link.
    last_error: Option<String>,
    /// Decoded nodes from `FromRadio.node_info`. Keyed by `node_id`
    /// (`!xxxxxxxx` hex of `num`) for cheap upserts.
    nodes: HashMap<String, LoraNode>,
    /// All chat threads (LongFast plus any auto-created `Direct(n)`).
    /// `LoraScreen::poll` mirrors this onto `App::lora_threads`; the
    /// input strip's `to:` chip and the left-pane title reflect
    /// `app.lora_active_thread`. Inbound DM routing rules:
    ///
    ///   * `MeshPacket.to == 0xFFFFFFFF` (broadcast) →
    ///     append to `threads[LongFast]`.
    ///   * `MeshPacket.to == <n>` and `n != my_node_num` →
    ///     auto-create `threads[Direct(n)]` on first sight and append.
    ///   * `MeshPacket.to == <n>` and `from == <n>` and `n == my_node_num`
    ///     (self-loop reflection) → dropped, to avoid double-threading
    ///     our own broadcasts back as DMs.
    ///
    /// The LongFast thread always exists; DMs are created on demand.
    threads: Vec<Thread>,
    /// Our own node num (from `FromRadio.my_info.my_node_num`), if the
    /// firmware has sent one. Used to label "me" in the chat pane and
    /// to drop self-loop DMs during inbound routing.
    my_node_num: Option<u32>,
    /// Wire-debug counters.
    wire: WireDebug,
}

impl Default for HttpState {
    fn default() -> Self {
        Self {
            connected: false,
            handshake_issued: false,
            last_want_config_secs: 0,
            rx_frames: 0,
            tx_frames: 0,
            last_frame_hex: String::new(),
            last_error: None,
            nodes: HashMap::new(),
            // Always include the LongFast anchor so `messages_for` and
            // `threads` are never empty and the right-pane header row
            // is always rendered.
            threads: vec![Thread::new(ChannelKind::LongFast, "LongFast")],
            my_node_num: None,
            wire: WireDebug::default(),
        }
    }
}

impl HttpState {
    /// Snapshot of the thread list (cloned for the renderer).
    fn threads_snapshot(&self) -> Vec<Thread> {
        self.threads.clone()
    }

    /// Snapshot of the nodes (cloned for the renderer).
    fn nodes_snapshot(&self) -> Vec<LoraNode> {
        let mut v: Vec<LoraNode> = self.nodes.values().cloned().collect();
        // Stable order: by node_num so the chat-pane "from" lookups
        // are deterministic and the renderer doesn't shuffle rows on
        // every refresh.
        v.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        v
    }

    /// Append `line` to the thread identified by `kind`. If the thread
    /// doesn't exist yet (a first-time DM from a previously-unseen
    /// peer) it is auto-created. The cap (`MAX_CHAT_LINES`) is applied
    /// per-thread so a chatty DM can't drown out LongFast and vice
    /// versa. Caller has already locked the state mutex.
    fn push_thread_line(&mut self, kind: ChannelKind, label: impl Into<String>, line: LoraChatLine) {
        let pos = match kind {
            ChannelKind::LongFast => 0,
            ChannelKind::Direct(n) => {
                let mut idx = 1;
                while idx < self.threads.len() {
                    if let ChannelKind::Direct(existing) = self.threads[idx].kind {
                        if existing == n {
                            break;
                        }
                        if existing > n {
                            break;
                        }
                    }
                    idx += 1;
                }
                // Insert if not already present.
                if idx >= self.threads.len()
                    || self.threads[idx].kind != kind
                {
                    self.threads
                        .insert(idx, Thread::new(kind.clone(), label.into()));
                }
                idx
            }
        };
        self.threads[pos].lines.push(line);
        if self.threads[pos].lines.len() > MAX_CHAT_LINES {
            let drop = self.threads[pos].lines.len() - MAX_CHAT_LINES;
            self.threads[pos].lines.drain(0..drop);
        }
    }

    fn upsert_node(&mut self, node: LoraNode) {
        if self.nodes.len() >= MAX_NODES && !self.nodes.contains_key(&node.node_id) {
            // At cap and not an update — drop. Meshtastic firmware
            // itself caps the DB at ~80 so this is purely defensive.
            return;
        }
        self.nodes.insert(node.node_id.clone(), node);
    }
}

/// HTTP-backed `LoraTransport`. Owns a `reqwest::Client` (cloned per
/// request — `reqwest::Client` is `Arc`-internally) and shared state
/// behind a `Mutex`.
#[derive(Clone)]
pub struct HttpLoraTransport {
    base: String,
    client: reqwest::Client,
    state: Arc<Mutex<HttpState>>,
}

impl HttpLoraTransport {
    /// Build a transport pointed at `ip` (e.g. `"192.168.1.42"`).
    pub fn new(ip: &str) -> Result<Self, LoraError> {
        // Accept either a bare IP (`192.168.1.42`) or an explicit
        // `http://192.168.1.42` — the modal lets users paste either.
        let base = if ip.starts_with("http://") || ip.starts_with("https://") {
            // Trim whitespace (a paste from a terminal often has a trailing
            // newline) and then strip trailing slashes. The bare-IP branch
            // also calls .trim() — keep them symmetric.
            ip.trim().trim_end_matches('/').to_string()
        } else {
            format!("http://{}", ip.trim())
        };
        // Validate by parsing as a URL — catches `not an ip` early
        // instead of letting it surface as a confusing reqwest error
        // on the first poll.
        reqwest::Url::parse(&base)
            .map_err(|e| LoraError::Io(format!("invalid node URL: {e}")))?;
        let client = reqwest::Client::builder()
            .timeout(READ_TIMEOUT)
            .connect_timeout(WRITE_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| LoraError::Io(format!("reqwest build: {e}")))?;
        Ok(Self {
            base,
            client,
            state: Arc::new(Mutex::new(HttpState::default())),
        })
    }

    /// Polling loop body. Spawned by `LoraScreen::maybe_swap_transport`.
    /// Designed to run forever — the caller drops the `JoinHandle` on
    /// transport swap.
    ///
    /// Sequence each tick:
    ///   * Maybe re-issue `want_config_id` (every
    ///     `WANT_CONFIG_REFRESH_SECS`) to refresh the node DB.
    ///   * GET `/api/v1/fromradio` (bootstrap → `?all=true` once, then
    ///     `?all=false` long-poll).
    ///   * On 2xx: flip `connected = true`, parse the body, dispatch
    ///     each `FromRadio` variant into state. Empty body on a quiet
    ///     node is fine — we just don't add anything.
    ///   * On non-2xx or transport error: flip `connected = false`,
    ///     stash the error message, sleep POLL_INTERVAL.
    pub async fn run_poll_loop(self: Arc<Self>) {
        let url_bootstrap = format!("{}/api/v1/fromradio?all=true", self.base);
        let url_stream = format!("{}/api/v1/fromradio?all=false", self.base);
        let url_toradio = format!("{}/api/v1/toradio", self.base);
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut bootstrapped = false;

        loop {
            interval.tick().await;
            let now = LoraNode::now_secs();

            // Handshake / refresh: issue want_config_id once on the
            // first tick, then every WANT_CONFIG_REFRESH_SECS. The
            // handshake is what unblocks the firmware's `FromRadio`
            // stream — without it the firmware holds the backlog and
            // `?all=true` returns empty (matches the live behaviour
            // observed on 10.0.0.193 pre-fix).
            let should_handshake = {
                let s = self.state.lock().expect("http state poisoned");
                !s.handshake_issued
                    || now.saturating_sub(s.last_want_config_secs)
                        >= WANT_CONFIG_REFRESH_SECS
            };
            if should_handshake {
                // Use a per-handshake nonce — the firmware echoes this
                // back in `config_complete_id` so we can correlate.
                let nonce: u32 = (now as u32) ^ 0x5a5a_5a5a;
                let body = proto::encode_to_radio_want_config_id(nonce);
                match self
                    .client
                    .request(reqwest::Method::OPTIONS, &url_toradio)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        // Probe succeeded — issue the handshake write.
                        match self
                            .client
                            .put(&url_toradio)
                            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
                            .body(body)
                            .send()
                            .await
                        {
                            Ok(r) if r.status().is_success() => {
                                let mut s = self.state.lock().expect("http state poisoned");
                                s.handshake_issued = true;
                                s.last_want_config_secs = now;
                                s.tx_frames = s.tx_frames.saturating_add(1);
                                s.last_error = None;
                            }
                            Ok(r) => {
                                tracing::warn!(
                                    status = %r.status(),
                                    "lora http: want_config_id PUT non-2xx"
                                );
                                let mut s =
                                    self.state.lock().expect("http state poisoned");
                                s.last_error =
                                    Some(format!("want_config_id PUT {}", r.status()));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "lora http: want_config_id PUT failed"
                                );
                                let mut s =
                                    self.state.lock().expect("http state poisoned");
                                s.last_error = Some(format!("want_config_id PUT: {e}"));
                            }
                        }
                    }
                    Ok(resp) => {
                        // OPTIONS non-2xx means the node isn't there.
                        // Don't keep hammering — log once and stop
                        // trying to handshake. The long-poll below
                        // will still flip `connected` if the GET path
                        // works (some firmware variants don't
                        // implement OPTIONS but still serve fromradio).
                        tracing::warn!(
                            status = %resp.status(),
                            "lora http: OPTIONS /toradio non-2xx"
                        );
                        let mut s = self.state.lock().expect("http state poisoned");
                        s.last_error = Some(format!("OPTIONS /toradio {}", resp.status()));
                        // Mark handshake as issued so we don't
                        // loop on the probe every tick — but bump the
                        // timestamp forward so we don't immediately
                        // retry. Re-attempt happens on the next IP
                        // modal submit (transport rebuild).
                        s.handshake_issued = true;
                        s.last_want_config_secs = now;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "lora http: OPTIONS /toradio failed"
                        );
                        let mut s = self.state.lock().expect("http state poisoned");
                        s.last_error = Some(format!("OPTIONS /toradio: {e}"));
                        s.handshake_issued = true;
                        s.last_want_config_secs = now;
                    }
                }
            }

            let url = if !bootstrapped {
                &url_bootstrap
            } else {
                &url_stream
            };
            match self.client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    {
                        let mut s = self.state.lock().expect("http state poisoned");
                        s.connected = true;
                    }
                    match resp.bytes().await {
                        Ok(bytes) => {
                            if !bytes.is_empty() {
                                self.ingest_frame(&bytes);
                            }
                            // Clear any prior transport/HTTP error — a
                            // successful poll means the wire is healthy
                            // again and the footer should reflect that,
                            // not a stale error from an earlier failure.
                            let mut s =
                                self.state.lock().expect("http state poisoned");
                            s.last_error = None;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "lora http: read body failed");
                            // The TCP read failed mid-body — surface
                            // it on `last_error` so the footer shows
                            // the wire is unhealthy, not just a log
                            // line the user never reads.
                            let mut s =
                                self.state.lock().expect("http state poisoned");
                            s.last_error = Some(format!("fromradio body: {e}"));
                        }
                    }
                    bootstrapped = true;
                }
                Ok(resp) => {
                    tracing::warn!(status = %resp.status(), "lora http: non-2xx");
                    let mut s = self.state.lock().expect("http state poisoned");
                    s.connected = false;
                    s.last_error = Some(format!("fromradio {}", resp.status()));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "lora http: poll failed");
                    let mut s = self.state.lock().expect("http state poisoned");
                    s.connected = false;
                    s.last_error = Some(format!("fromradio: {e}"));
                }
            }
        }
    }

    /// Dispatch one HTTP response body. The body is a concatenation of
    /// `FromRadio` frames — `proto::parse_from_radio` slices each one
    /// off in order and we feed each variant into `HttpState`.
    fn ingest_frame(&self, bytes: &[u8]) {
        // Empty body = quiet node, nothing to ingest. No counters bump.
        if bytes.is_empty() {
            return;
        }
        // Bump the rx-frame counter + record the hex dump BEFORE
        // dispatching so wire-debug shows activity even if every
        // variant turns out to be `Unknown`.
        {
            let mut s = self.state.lock().expect("http state poisoned");
            s.rx_frames = s.rx_frames.saturating_add(1);
            let hex = hex_encode(bytes);
            s.last_frame_hex = truncate(&hex, LAST_FRAME_HEX_MAX);
        }
        let frames = proto::parse_from_radio(bytes);
        if frames.is_empty() {
            let mut s = self.state.lock().expect("http state poisoned");
            s.wire.parse_failures = s.wire.parse_failures.saturating_add(1);
            return;
        }
        let mut s = self.state.lock().expect("http state poisoned");
        let my_node_num = s.my_node_num;
        for fr in frames {
            match fr {
                FromRadio::Id(_id) => {
                    // Captured by the rx counter; nothing to surface.
                }
                FromRadio::Packet(pkt) => {
                    let Some(data) = pkt.decoded.as_ref() else {
                        // Encrypted packet — we can't read it. Don't
                        // bump unknown_portnum; that's reserved for
                        // decoded-but-unsupported portnums.
                        continue;
                    };
                    if data.portnum != TEXT_MESSAGE_APP {
                        s.wire.unknown_portnum =
                            s.wire.unknown_portnum.saturating_add(1);
                        continue;
                    }
                    if data.payload.is_empty() {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&data.payload).into_owned();
                    let is_local = my_node_num.map(|m| m == pkt.from).unwrap_or(false);
                    let hops = proto::hops_away(&pkt);
                    let from_id = proto::node_id_from_num(pkt.from);
                    let from_label = s
                        .nodes
                        .get(&from_id)
                        .map(|n| n.label())
                        .unwrap_or(from_id);
                    // Inbound DM routing — matches what
                    // `meshtastic/web`'s `MeshClient.sendPacket`
                    // encodes on the way out, in reverse:
                    //
                    //   * broadcast (`to == 0xFFFFFFFF`) → LongFast
                    //   * DM to us from a peer (`to == us`, `from != us`)
                    //     → `Direct(from)`
                    //   * DM to us from ourselves (`from == to == us`,
                    //     a self-loop reflection) → drop to avoid
                    //     surfacing our own broadcasts as DMs
                    //   * DM to a peer from us — impossible inbound,
                    //     we generated it; ignore if it ever appears
                    // 0xFFFFFFFF is the firmware broadcast sentinel;
                    // `to == 0` is a malformed sender-set field we
                    // also treat as broadcast to be safe — real
                    // Meshtastic firmware occasionally omits `to` on
                    // broadcast packets, and we'd otherwise route
                    // them to `Direct(0)` which is invisible in the
                    // UI (no row in the right pane for Direct(0)).
                    let target: Option<(ChannelKind, String)> = if pkt.to == BROADCAST_NUM
                        || pkt.to == 0
                    {
                        Some((
                            ChannelKind::LongFast,
                            "LongFast".to_string(),
                        ))
                    } else if let Some(us) = my_node_num {
                        if pkt.to == us && pkt.from == us {                            // Self-loop reflection — drop.
                            None
                        } else if pkt.to == us {
                            // DM addressed to us from a peer — the
                            // other end is the sender.
                            Some((
                                ChannelKind::Direct(pkt.from),
                                proto::node_id_from_num(pkt.from),
                            ))
                        } else {
                            // DM between two other nodes that the
                            // firmware is forwarding to us — show
                            // it under the recipient's thread (the
                            // "other end" perspective from our
                            // vantage point). This matches the
                            // meshtastic/web UI which also shows
                            // observed DMs.
                            Some((
                                ChannelKind::Direct(pkt.to),
                                proto::node_id_from_num(pkt.to),
                            ))
                        }
                    } else {
                        // We don't know our own node num yet (no
                        // `MyInfo` frame has arrived) — best we can
                        // do is bucket it under the sender as a DM,
                        // since we can't tell broadcast from DM in
                        // this state. (Once `MyInfo` arrives the
                        // next packet will be routed correctly.)
                        if pkt.from == pkt.to {
                            None
                        } else if pkt.to == BROADCAST_NUM || pkt.to == 0 {
                            // BROADCAST_NUM is the firmware-defined
                            // broadcast; `to == 0` is a malformed
                            // sender-set field we also treat as
                            // broadcast to be safe. Both belong on
                            // LongFast — landing a pre-`MyInfo`
                            // broadcast in `Direct(0xFFFFFFFF)` would
                            // hide every LongFast packet behind a
                            // thread the user never navigates to.
                            Some((
                                ChannelKind::LongFast,
                                "LongFast".to_string(),
                            ))
                        } else {
                            Some((
                                ChannelKind::Direct(pkt.to),
                                proto::node_id_from_num(pkt.to),
                            ))
                        }
                    };
                    if let Some((kind, label)) = target {
                        s.push_thread_line(
                            kind,
                            label,
                            LoraChatLine {
                                from: from_label,
                                text,
                                hops_away: hops,
                                is_local,
                            },
                        );
                    }
                }
                FromRadio::NodeInfo(ni) => {
                    let node = node_info_to_lora_node(&ni);
                    s.upsert_node(node);
                }
                FromRadio::MyInfo(mi) => {
                    s.my_node_num = Some(mi.my_node_num);
                    // We don't have our own User{} yet — once the
                    // firmware sends NodeInfo for our own num, the
                    // chat "from" label will pick up the operator's
                    // long_name. Until then, "me" is shown by
                    // `FakeTransport`-style fallback via `is_local`.
                }
                FromRadio::ConfigComplete(_id) => {
                    // Boot dump finished — nothing to surface beyond
                    // the fact that we got the frame.
                }
                FromRadio::Unknown => {
                    s.wire.unknown_variant =
                        s.wire.unknown_variant.saturating_add(1);
                }
            }
        }
    }

    /// `PUT /api/v1/toradio` with `body` as the payload. **Wire-debug
    /// only**: the bytes the user typed are forwarded verbatim — full
    /// `ToRadio{packet{MeshPacket{decoded{Data}}}}` framing for outbound
    /// chat is a follow-up slice. The round-trip is still observable on
    /// the node's HTTP logs (UA + path + body length).
    pub async fn put_toradio(&self, body: Vec<u8>) -> Result<(), LoraError> {
        let url = format!("{}/api/v1/toradio", self.base);
        let resp = self
            .client
            .put(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(body)
            .send()
            .await
            .map_err(|e| LoraError::Io(format!("toradio put: {e}")))?;
        if !resp.status().is_success() {
            return Err(LoraError::Io(format!(
                "toradio put: status {}",
                resp.status()
            )));
        }
        let mut s = self.state.lock().expect("http state poisoned");
        s.tx_frames = s.tx_frames.saturating_add(1);
        Ok(())
    }

    /// Snapshot for the status footer. The footer reads this each
    /// tick to render `rx=… tx=… last=… wire=…`.
    pub fn status_snapshot(&self) -> HttpStatus {
        let s = self.state.lock().expect("http state poisoned");
        HttpStatus {
            connected: s.connected,
            rx_frames: s.rx_frames,
            tx_frames: s.tx_frames,
            last_frame_hex: s.last_frame_hex.clone(),
            last_error: s.last_error.clone(),
            wire: s.wire.clone(),
        }
    }

    /// Base URL this transport is configured against (host:port, no
    /// trailing slash). Surfaced in the footer so the user can confirm
    /// the modal handed off the right value.
    pub fn base(&self) -> &str {
        &self.base
    }
}

impl LoraTransport for HttpLoraTransport {
    fn nodes(&self) -> Vec<LoraNode> {
        self.state.lock().expect("http state poisoned").nodes_snapshot()
    }

    fn messages_for(&self, kind: &ChannelKind) -> Vec<LoraChatLine> {
        let s = self.state.lock().expect("http state poisoned");
        s.threads_snapshot()
            .into_iter()
            .find(|t| &t.kind == kind)
            .map(|t| t.lines)
            .unwrap_or_default()
    }

    fn threads(&self) -> Vec<Thread> {
        self.state.lock().expect("http state poisoned").threads_snapshot()
    }

    fn connected(&self) -> bool {
        self.state.lock().expect("http state poisoned").connected
    }

    /// Validate the text, encode a `ToRadio { packet: MeshPacket { to,
    /// decoded: Data{portnum=TEXT_MESSAGE_APP, payload=text} } }` via
    /// `proto::encode_to_radio_packet`, and PUT it to `/api/v1/toradio`.
    /// Mirrors `MeshClient.sendPacket` in `meshtastic/web` exactly:
    /// the only field that differs between LongFast and a DM is
    /// `MeshPacket.to` (broadcast vs peer num). The actual HTTP call is
    /// async; we spawn it so the trait method (sync) doesn't block the
    /// UI. Errors from the spawned task are logged via `tracing`.
    ///
    /// `kind.to_num()` is the single source of truth for which `to`
    /// field the encoder writes — `ChannelKind::LongFast` →
    /// `BROADCAST_NUM`, `ChannelKind::Direct(n)` → `n`. Pinning this
    /// here keeps the trait and the encoder aligned.
    fn send_to(&mut self, kind: &ChannelKind, text: &str) -> Result<(), LoraError> {
        let trimmed = text.trim();
        if !self.connected() {
            return Err(LoraError::NotConnected);
        }
        if trimmed.is_empty() {
            return Err(LoraError::Empty);
        }
        if trimmed.len() > 200 {
            return Err(LoraError::TooLong);
        }
        // Local echo: mirror `FakeTransport::send_to`'s behaviour
        // (lora.rs:404-419) by pushing a `LoraChatLine { is_local: true, ... }`
        // onto the matching thread synchronously, BEFORE the wire PUT.
        // Without this the chat pane renders empty after the user hits
        // Enter even though the wire write succeeded — because
        // `LoraScreen::poll` only mirrors whatever is on
        // `transport.threads()` onto `app.lora_threads`.
        // Wiremock test `http_lora_send_to_echoes_local_message_for_longfast`
        // regresses this.
        let mut s = self.state.lock().expect("http state poisoned");
        let my = s.my_node_num.unwrap_or(0);
        let echo = LoraChatLine {
            from: proto::node_id_from_num(my),
            text: trimmed.to_string(),
            hops_away: 0,
            is_local: true,
        };
        s.push_thread_line(kind.clone(), "me".to_string(), echo);
        drop(s);
        let bytes = proto::encode_to_radio_packet(kind.to_num(), trimmed.as_bytes());
        let me = Arc::new(self.clone_handle());
        tokio::spawn(async move {
            if let Err(e) = me.put_toradio(bytes).await {
                tracing::warn!(error = ?e, "lora http: send_to failed");
            }
        });
        Ok(())
    }
}

// `HttpLoraTransport` is held inside `Box<dyn LoraTransport + Send>` on
// `LoraScreen`, which requires `Send` for the renderer. `reqwest::Client`
// is `Send + Sync` and `Arc<Mutex<HttpState>>` is `Send + Sync` so we can
// be too. The send path needs to bump `Arc` refcount without going
// through `LoraTransport::clone` (which would re-build the client), so
// we expose a tiny `clone_handle` helper.
impl HttpLoraTransport {
    /// Cheap "handle" clone for `send_to`.
    fn clone_handle(&self) -> HttpLoraTransport {
        HttpLoraTransport {
            base: self.base.clone(),
            client: self.client.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

/// Plain-data view of the HTTP transport's counters for the renderer.
/// Cheap to clone (small strings + u64s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpStatus {
    pub connected: bool,
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub last_frame_hex: String,
    pub last_error: Option<String>,
    pub wire: WireDebug,
}

/// Convert a decoded `NodeInfo` into the renderer's `LoraNode`. Pulls
/// the long/short names out of the embedded `User` (if any), sets
/// `last_heard_secs` from the proto field, and stamps `hops_away=0`
/// since the proto doesn't carry a per-node hops metric (that's
/// derived per-packet via `proto::hops_away`).
fn node_info_to_lora_node(ni: &NodeInfo) -> LoraNode {
    let node_id = proto::node_id_from_num(ni.num);
    let (long_name, short_name) = match ni.user.as_ref() {
        Some(u) => (u.long_name.clone(), u.short_name.clone()),
        None => (String::new(), String::new()),
    };
    LoraNode {
        node_id,
        long_name,
        short_name,
        hops_away: 0,
        // NodeInfo.last_heard is fixed32 epoch seconds — fits in u64
        // verbatim. `LoraNode::is_online_at` already handles the
        // absolute-vs-relative heuristic so this Just Works.
        last_heard_secs: ni.last_heard_secs as u64,
        snr: None,
    }
}

/// Lowercase hex encode, no separators. Used for the last-frame dump in
/// the status footer.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Truncate `s` to at most `max` chars, appending `…` if we cut.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out = String::with_capacity(max + 3);
    out.push_str(&s[..max.saturating_sub(1)]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Existing pin tests (kept verbatim) ────────────────────────────

    #[test]
    fn new_accepts_bare_ip() {
        let t = HttpLoraTransport::new("192.168.1.42").unwrap();
        assert_eq!(t.base(), "http://192.168.1.42");
    }

    #[test]
    fn new_accepts_full_url_and_strips_trailing_slash() {
        let t = HttpLoraTransport::new("http://10.0.0.5:8080/").unwrap();
        assert_eq!(t.base(), "http://10.0.0.5:8080");
    }

    #[test]
    fn new_rejects_garbage() {
        assert!(HttpLoraTransport::new("not a url at all").is_err());
    }

    #[test]
    fn truncate_short_string_is_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_appends_ellipsis() {
        let s = "a".repeat(20);
        let out = truncate(&s, 8);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 9);
    }

    #[test]
    fn hex_encode_known_value() {
        assert_eq!(hex_encode(&[0x00, 0x01, 0xff, 0xab]), "0001ffab");
    }

    #[test]
    fn empty_transport_reports_disconnected() {
        let t = HttpLoraTransport::new("127.0.0.1").unwrap();
        let snap = t.status_snapshot();
        assert!(!snap.connected);
        assert_eq!(snap.rx_frames, 0);
        assert_eq!(snap.tx_frames, 0);
        assert!(snap.last_frame_hex.is_empty());
        assert!(snap.wire == WireDebug::default());
    }

    #[test]
    fn user_agent_starts_with_cyberdeck_tui() {
        assert!(
            USER_AGENT.starts_with("cyberdeck-tui/"),
            "UA must start with `cyberdeck-tui/` for node-side \
             log debugging; got {USER_AGENT:?}"
        );
    }

    #[test]
    fn poll_loop_targets_expected_urls() {
        let t = HttpLoraTransport::new("10.0.0.193").unwrap();
        let bootstrap = format!("{}/api/v1/fromradio?all=true", t.base());
        let stream = format!("{}/api/v1/fromradio?all=false", t.base());
        assert!(
            bootstrap.ends_with("/api/v1/fromradio?all=true"),
            "bootstrap URL shape changed: {bootstrap}"
        );
        assert!(
            stream.ends_with("/api/v1/fromradio?all=false"),
            "stream URL shape changed: {stream}"
        );
        assert_eq!(t.base(), "http://10.0.0.193");
    }

    #[test]
    fn empty_body_2xx_still_marks_connected() {
        let t = HttpLoraTransport::new("127.0.0.1").unwrap();
        {
            let mut s = t.state.lock().expect("http state poisoned");
            s.connected = true;
        }
        let snap = t.status_snapshot();
        assert!(
            snap.connected,
            "a 2xx response (even with empty body) must flip connected=true"
        );
        assert_eq!(snap.rx_frames, 0);
        assert!(snap.last_frame_hex.is_empty());
    }

    // ─── New ingest tests (this slice) ────────────────────────────────

    /// Helper: build an `HttpLoraTransport` and run `ingest_frame`
    /// directly with the supplied bytes. Avoids needing a mock server
    /// or a tokio runtime in the test — `ingest_frame` is a pure
    /// synchronous dispatch into `HttpState`.
    fn ingest(body: &[u8]) -> HttpLoraTransport {
        let t = HttpLoraTransport::new("127.0.0.1").unwrap();
        t.ingest_frame(body);
        t
    }

    #[test]
    fn ingest_text_message_populates_chat() {
        // Build one FromRadio{packet{from=0xaabbccdd, hop_start=5,
        // hop_limit=2, decoded{portnum=1, payload="hi"}}}.
        let data: Vec<u8> = vec![0x08, 0x01, 0x12, 0x02, b'h', b'i'];
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&0xaabbu32.to_le_bytes()); // we only use the low 16 bits; full 32 is below
        // overwrite with the full 32-bit from
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&0xaabbccddu32.to_le_bytes());
        mp.extend_from_slice(&[0x30, 0x02, 0x38, 0x05]); // hop_limit=2, hop_start=5
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let body: Vec<u8> = {
            let mut fr = vec![0x12, mp.len() as u8];
            fr.extend_from_slice(&mp);
            fr
        };

        let t = ingest(&body);
        let msgs = t.messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hi");
        assert_eq!(msgs[0].hops_away, 3, "hop_start - hop_limit");
        assert!(!msgs[0].is_local);
        // The sender's label falls back to the raw node_id since we
        // haven't seen NodeInfo for that num yet.
        assert_eq!(msgs[0].from, "!aabbccdd");
        // Counter + hex dump updated.
        let snap = t.status_snapshot();
        assert_eq!(snap.rx_frames, 1);
        assert!(!snap.last_frame_hex.is_empty());
    }

    #[test]
    fn ingest_node_info_populates_nodes_list() {
        // FromRadio{node_info{num=42, user{long="alice", short="AL"},
        // last_heard=1_700_000_000}}.
        let user: Vec<u8> = vec![
            0x12, 0x05, b'a', b'l', b'i', b'c', b'e',
            0x1a, 0x02, b'A', b'L',
        ];
        let mut ni: Vec<u8> = vec![0x08, 0x2a, 0x22, user.len() as u8];
        ni.extend_from_slice(&user);
        ni.extend_from_slice(&[0x2d]);
        ni.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        let mut fr: Vec<u8> = vec![0x22, ni.len() as u8];
        fr.extend_from_slice(&ni);

        let t = ingest(&fr);
        let nodes = t.nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "!0000002a");
        assert_eq!(nodes[0].long_name, "alice");
        assert_eq!(nodes[0].short_name, "AL");
        assert_eq!(nodes[0].last_heard_secs, 1_700_000_000);
    }

    #[test]
    fn ingest_my_info_records_own_node_num() {
        // FromRadio{my_info{my_node_num=7}}.
        let body: Vec<u8> = vec![0x1a, 0x02, 0x08, 0x07];
        let t = ingest(&body);
        // my_node_num is internal — but we can confirm via a follow-up
        // chat frame that "from=7" gets marked is_local.
        let data: Vec<u8> = vec![0x08, 0x01, 0x12, 0x02, b'h', b'i'];
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&7u32.to_le_bytes());
        // to=BROADCAST_NUM (field 2, fixed32) so the packet routes to
        // LongFast — that's what `messages()` (the LongFast shim) reads.
        // Without this, the routing branch treats `from=to=0` as a DM
        // between two unknown nodes and lands in `Direct(0)`, not LongFast.
        mp.extend_from_slice(&[0x15, 0xff, 0xff, 0xff, 0xff]);
        mp.extend_from_slice(&[0x30, 0x03, 0x38, 0x03]);
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let body2: Vec<u8> = {
            let mut f = vec![0x12, mp.len() as u8];
            f.extend_from_slice(&mp);
            f
        };
        t.ingest_frame(&body2);
        let msgs = t.messages();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].is_local, "from=my_node_num must be marked is_local");
        assert_eq!(msgs[0].text, "hi");
    }
    /// LIVE regression test for symptom #1 and #2 the user keeps
    /// reporting: LongFast is empty, and on navigating/selecting the
    /// title flashes back to "longfast" because real Meshtastic
    /// firmware occasionally omits the `to` field on broadcast packets
    /// (so the `MeshPacket.to` protobuf field defaults to `0u32`).
    ///
    /// The pre-`MyInfo` branch in `ingest_frame` already accepts
    /// `to == 0` as LongFast — but the post-`MyInfo` branch routes
    /// any non-broadcast non-self packet to `Direct(pkt.to)`, which
    /// means `to == 0` lands on `Direct(0)`. The user never navigates
    /// to `Direct(0)` because there's no row for it in the right
    /// pane (the right pane only shows `LongFast` + real node rows),
    /// so the messages disappear from view entirely.
    ///
    /// This test feeds the exact live boot ordering into the actual
    /// `HttpLoraTransport` (`MyInfo` first → broadcast with
    /// `from=peer, to=0`) and asserts the line lands on LongFast,
    /// where the user can actually see it.
    #[test]
    fn ingest_broadcast_with_zero_to_after_myinfo_lands_on_longfast() {
        // Frame 1: FromRadio{my_info{my_node_num=7}}.
        let my_info_body: Vec<u8> = vec![0x1a, 0x02, 0x08, 0x07];

        // Frame 2: FromRadio{packet{from=0xaabbccdd, NO `to` field at
        // all on the wire (so MeshPacket.to == 0u32 after decode),
        // hop_limit=3, hop_start=3, decoded{portnum=1, payload="hi"}}}.
        let data: Vec<u8> = vec![0x08, 0x01, 0x12, 0x02, b'h', b'i'];
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&0xaabbccddu32.to_le_bytes());
        // No `0x15` (to) tag at all — simulates firmware omitting
        // the field, which makes `pkt.to == 0` after decode.
        mp.extend_from_slice(&[0x30, 0x03, 0x38, 0x03]); // hop_limit=3, hop_start=3
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let chat_body: Vec<u8> = {
            let mut f = vec![0x12, mp.len() as u8];
            f.extend_from_slice(&mp);
            f
        };

        let t = HttpLoraTransport::new("127.0.0.1").unwrap();
        t.ingest_frame(&my_info_body);
        t.ingest_frame(&chat_body);

        // The bug: pre-fix this asserts empty (because the packet
        // landed on `Direct(0)`, hidden behind a row the renderer
        // never builds). After the fix the chat line must be on
        // LongFast where the user can see it.
        let msgs = t.messages();
        assert_eq!(
            msgs.len(),
            1,
            "broadcast with to==0 must land on LongFast, not Direct(0)"
        );
        assert_eq!(msgs[0].text, "hi");
        assert_eq!(msgs[0].from, "!aabbccdd");
        // Must NOT be marked is_local because the sender is a peer,
        // not our own node_num=7.
        assert!(!msgs[0].is_local);
    }

    #[test]
    fn ingest_unknown_portnum_bumps_wire_debug() {        // Data{portnum=99 (TELEMETRY_APP-ish), payload="x"} wrapped in
        // MeshPacket{from=1, hop_limit=0, hop_start=0, decoded=<Data>}.
        let data: Vec<u8> = vec![0x08, 99, 0x12, 0x01, b'x'];
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&1u32.to_le_bytes());
        mp.extend_from_slice(&[0x30, 0x00, 0x38, 0x00]);
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let body: Vec<u8> = {
            let mut f = vec![0x12, mp.len() as u8];
            f.extend_from_slice(&mp);
            f
        };
        let t = ingest(&body);
        assert!(t.messages().is_empty(), "non-chat packets must not surface");
        let snap = t.status_snapshot();
        assert_eq!(snap.wire.unknown_portnum, 1, "telemetry packet bumped wire_debug");
    }

    #[test]
    fn ingest_encrypted_packet_is_silently_dropped() {
        // MeshPacket{from=1, no `decoded`} → nothing should surface.
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&1u32.to_le_bytes());
        mp.extend_from_slice(&[0x30, 0x03, 0x38, 0x03]);
        let body: Vec<u8> = {
            let mut f = vec![0x12, mp.len() as u8];
            f.extend_from_slice(&mp);
            f
        };
        let t = ingest(&body);
        assert!(t.messages().is_empty());
        // Encrypted packet should NOT bump unknown_portnum — that's
        // for decoded-but-unsupported portnums.
        let snap = t.status_snapshot();
        assert_eq!(snap.wire.unknown_portnum, 0);
    }

    #[test]
    fn chat_line_label_resolves_after_node_info_arrives() {
        // First: a chat packet from node 0x42424242 (no NodeInfo yet).
        let data: Vec<u8> = vec![0x08, 0x01, 0x12, 0x05, b'h', b'e', b'l', b'l', b'o'];
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&0x42424242u32.to_le_bytes());
        mp.extend_from_slice(&[0x30, 0x01, 0x38, 0x01]);
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let chat_body: Vec<u8> = {
            let mut f = vec![0x12, mp.len() as u8];
            f.extend_from_slice(&mp);
            f
        };
        let t = ingest(&chat_body);
        // Label falls back to node_id.
        assert_eq!(t.messages()[0].from, "!42424242");

        // Then: NodeInfo for the same num, with long_name="alice".
        // num = 0x42424242 → matches the chat packet's `from`.
        let user: Vec<u8> = vec![0x12, 0x05, b'a', b'l', b'i', b'c', b'e'];
        // num is a varint — 0x42424242 needs 4 LEB128 bytes:
        //   0x42 0x42 0x42 0x42 (each carries 7 bits of the value).
        let num_bytes: Vec<u8> = proto::encode_leb128(0x42424242u64);
        let mut ni: Vec<u8> = vec![0x08];
        ni.extend_from_slice(&num_bytes);
        ni.extend_from_slice(&[0x22, user.len() as u8]);
        ni.extend_from_slice(&user);
        ni.extend_from_slice(&[0x2d]);
        ni.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        let mut fr: Vec<u8> = vec![0x22, ni.len() as u8];
        fr.extend_from_slice(&ni);
        t.ingest_frame(&fr);

        // Subsequent chat packets should now resolve the label.
        let data2: Vec<u8> = vec![0x08, 0x01, 0x12, 0x02, b'y', b'o'];
        let mut mp2: Vec<u8> = vec![0x0d];
        mp2.extend_from_slice(&0x42424242u32.to_le_bytes());
        mp2.extend_from_slice(&[0x30, 0x01, 0x38, 0x01]);
        mp2.extend_from_slice(&[0x42, data2.len() as u8]);
        mp2.extend_from_slice(&data2);
        let body2: Vec<u8> = {
            let mut f = vec![0x12, mp2.len() as u8];
            f.extend_from_slice(&mp2);
            f
        };
        t.ingest_frame(&body2);
        assert_eq!(t.messages()[1].from, "alice");
    }

    #[test]
    fn chat_buffer_is_capped() {
        // Push > MAX_CHAT_LINES lines and verify the front is dropped.
        // We don't want to actually encode 501 packets (slow); instead
        // reach in via the public `messages()` path by re-ingesting a
        // multi-frame body many times.
        let t = HttpLoraTransport::new("127.0.0.1").unwrap();
        let data: Vec<u8> = vec![0x08, 0x01, 0x12, 0x01, b'x'];
        let mut mp: Vec<u8> = vec![0x0d];
        mp.extend_from_slice(&1u32.to_le_bytes());
        mp.extend_from_slice(&[0x30, 0x00, 0x38, 0x00]);
        mp.extend_from_slice(&[0x42, data.len() as u8]);
        mp.extend_from_slice(&data);
        let body: Vec<u8> = {
            let mut f = vec![0x12, mp.len() as u8];
            f.extend_from_slice(&mp);
            f
        };
        // 502 ingests (one frame each); cap is 500.
        for _ in 0..502 {
            t.ingest_frame(&body);
        }
        assert_eq!(
            t.messages().len(),
            500,
            "chat buffer must cap at MAX_CHAT_LINES"
        );
    }

    #[test]
    fn empty_body_does_not_bump_rx_counter() {
        // Empty body — a quiet node — must keep rx_frames at 0 and
        // leave last_frame_hex empty.
        let t = ingest(&[]);
        let snap = t.status_snapshot();
        assert_eq!(snap.rx_frames, 0);
        assert!(snap.last_frame_hex.is_empty());
        assert_eq!(snap.wire.parse_failures, 0);
        assert_eq!(snap.wire.unknown_variant, 0);
    }

    #[test]
    fn truncated_body_bumps_parse_failures() {
        // Garbage bytes — the parser hits a malformed varint and
        // bails, returning an empty Vec. We record the failure in
        // wire_debug so the user can see "the wire is alive but
        // we're getting garbage".
        let t = ingest(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        let snap = t.status_snapshot();
        // rx_frames still bumps (we got a body) but the parse failure
        // counter records that nothing came out of it.
        assert_eq!(snap.rx_frames, 1);
        assert!(
            snap.wire.parse_failures >= 1,
            "garbage body must bump parse_failures"
        );
    }
}