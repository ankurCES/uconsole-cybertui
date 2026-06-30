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

/// HTTP transport for talking to a Meshtastic node over LAN. Pulled in
/// behind the `http` feature flag so a default `cargo build` doesn't
/// grow the dep graph for users who don't own a node. See
/// `screens/lora/http.rs` for the wire-debug contract and the
/// protobuf-decode follow-up plan.
#[cfg(feature = "http")]
pub mod http;

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

/// What the LoRa screen can ask of the underlying transport. All methods
/// have an in-process `FakeTransport` for tests; the real transport lives
/// in `LoraScreen::new_http` and is selected at runtime by the IP modal.
pub trait LoraTransport {
    /// Snapshot of known nodes. Returns what the transport currently knows;
    /// the screen does NOT keep a separate authoritative copy until
    /// `apply_nodes` is called.
    fn nodes(&self) -> Vec<LoraNode>;
    /// Snapshot of chat lines already received on the longfast channel.
    fn messages(&self) -> Vec<LoraChatLine>;
    /// True when the transport has an active HTTP session. False means
    /// "no node" — the screen renders a connect-prompt instead.
    fn connected(&self) -> bool;
    /// Send `text` from the local node on the longfast channel. The
    /// transport echoes the line back through `messages()` so the chat
    /// pane can append it.
    fn send_longfast(&mut self, text: &str) -> Result<(), LoraError>;
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
#[derive(Debug, Default, Clone)]
pub struct FakeTransport {
    pub nodes: Vec<LoraNode>,
    pub messages: Vec<LoraChatLine>,
    pub connected: bool,
    /// Recorded outbound messages for test assertions.
    pub sent: Vec<String>,
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
}

impl LoraTransport for FakeTransport {
    fn nodes(&self) -> Vec<LoraNode> {
        self.nodes.clone()
    }
    fn messages(&self) -> Vec<LoraChatLine> {
        self.messages.clone()
    }
    fn connected(&self) -> bool {
        self.connected
    }
    fn send_longfast(&mut self, text: &str) -> Result<(), LoraError> {
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
        self.sent.push(trimmed.to_string());
        self.messages.push(LoraChatLine {
            from: "me".into(),
            text: trimmed.to_string(),
            hops_away: 0,
            is_local: true,
        });
        Ok(())
    }
}

/// The screen itself. Holds the chat/nodes snapshot, the chat input
/// buffer, and a transport behind the trait so tests can swap it for
/// `FakeTransport` without touching any HTTP code paths.
pub struct LoraScreen {
    pub transport: Box<dyn LoraTransport + Send>,
    /// Selected node in the right pane; drives the "to: <name>" hint only.
    pub nodes_selected: usize,
    /// IP this screen is currently configured against. Diffed against
    /// `app.lora_node_ip` on every `poll` so a fresh IP from the modal
    /// triggers a transport rebuild (FakeTransport → HttpLoraTransport,
    /// or IP-A → IP-B). `None` means "no node configured" and the
    /// screen stays on the constructor-supplied transport (usually
    /// `FakeTransport`). Persisted here so we don't rebuild on every
    /// tick — only when the value actually flips.
    pub current_node_ip: Option<String>,
}

impl LoraScreen {
    pub fn new(transport: Box<dyn LoraTransport + Send>) -> Self {
        Self {
            transport,
            nodes_selected: 0,
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
        app.lora_chat = self.transport.messages();
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
            // Cursor moves on the nodes list. Tab/Shift-Tab cycles the
            // whole TUI; this stays on the screen's own input handling.
            KeyCode::Tab => {
                if !app.lora_nodes.is_empty() {
                    self.nodes_selected =
                        (self.nodes_selected + 1) % app.lora_nodes.len();
                }
                return true;
            }
            KeyCode::BackTab => {
                if !app.lora_nodes.is_empty() {
                    self.nodes_selected = if self.nodes_selected == 0 {
                        app.lora_nodes.len() - 1
                    } else {
                        self.nodes_selected - 1
                    };
                }
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
            // Send the buffer on Enter.
            KeyCode::Enter => {
                let draft = app.lora_input.clone();
                app.lora_input.clear();
                match self.transport.send_longfast(&draft) {
                    Ok(()) => {
                        // Refresh the chat pane so the new line appears.
                        app.lora_chat = self.transport.messages();
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

        // Left: longfast channel chat.
        // The chat is a text log; we render `lora_chat` lines and clamp
        // `lora_chat_offset` (lines back from the tail) so an empty buffer
        // doesn't strand the cursor.
        //
        // Empty-state copy: when no node IP is configured yet the chat
        // pane is the most prominent surface the user sees, so we
        // surface the IP-modal binding here as the call-to-action.
        // Once an IP is set the chat goes back to the generic "no
        // messages yet" hint.
        let total = app.lora_chat.len();
        let visible_h = cols[0].height as usize;
        let max_off = total.saturating_sub(1);
        if app.lora_chat_offset > max_off {
            app.lora_chat_offset = max_off;
        }
        let end = total.saturating_sub(app.lora_chat_offset);
        let start = end.saturating_sub(visible_h);
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
            app.lora_chat[start..end]
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
                    .title(Span::styled(" longfast ", theme.title()))
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

        // Right: nodes list with hops.
        // Clamp the cursor so a stale index never panics on an empty list.
        if !app.lora_nodes.is_empty() {
            if self.nodes_selected >= app.lora_nodes.len() {
                self.nodes_selected = app.lora_nodes.len() - 1;
            }
        } else {
            self.nodes_selected = 0;
        }
        let right_items: Vec<ListItem> = if app.lora_nodes.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no nodes yet)",
                theme.dim(),
            )))]
        } else {
            let now = LoraNode::now_secs();
            app.lora_nodes
                .iter()
                .map(|n| {
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
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{} ", dot), dot_style),
                        Span::styled(format!("{:>8}", truncate(&n.label(), 8)), theme.accent),
                        Span::styled(format!("  {:<10}", hops), theme.fg),
                        Span::styled(format!("  {:>8}", snr), theme.dim()),
                    ]))
                })
                .collect()
        };
        let right_highlight = if app.lora_nodes.is_empty() {
            None
        } else {
            Some(self.nodes_selected.min(right_items.len() - 1))
        };
        let mut right_state = ListState::default().with_selected(right_highlight);
        let right_focused = matches!(app.region, crate::app::Region::ContentRight);
        let right = List::new(right_items)
            .block(
                Block::default()
                    .title(Span::styled(" nodes ", theme.title()))
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
        let to_node = if let Some(n) = app.lora_nodes.get(self.nodes_selected) {
            format!("to: {}", truncate(&n.label(), 8))
        } else {
            "to: (broadcast)".to_string()
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
        t.messages.push(line.clone());

        let mut app = make_app();
        let mut screen = LoraScreen::new(Box::new(t));
        screen.poll(&mut app);
        assert!(app.lora_connected);
        assert_eq!(app.lora_nodes, vec![node]);
        assert_eq!(app.lora_chat, vec![line]);
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
        assert_eq!(app.lora_chat.len(), 1);
        assert!(app.lora_chat[0].is_local);
        assert_eq!(app.lora_chat[0].text, "hi");
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
        assert!(app.lora_chat.is_empty(), "no line should be added");
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

        // Tab moves the nodes cursor; BackTab wraps.
        assert_eq!(screen.nodes_selected, 0);
        screen.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut app);
        assert_eq!(screen.nodes_selected, 1);
        screen.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut app);
        assert_eq!(screen.nodes_selected, 0, "Tab wraps to first");
        screen.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
        assert_eq!(screen.nodes_selected, 1, "BackTab wraps backward to last");
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
}
