//! Mesh screen — Meshtastic over TCP/WiFi bridge: longfast channel chat
//! on the left, nodes list with hops on the right, input strip at the bottom.
//!
//! Transports are abstracted behind a trait so the unit tests never block
//! on a real network socket. The real `TcpMeshTransport` lives behind a
//! runtime gate so a `Mesh` screen on a box without a bridge just renders
//! the connect-prompt placeholder instead of hanging the renderer.
//!
//! Bridge default: `10.0.0.193:4403` (the documented Meshtastic-TCP port).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

/// A node as seen by the local mesh — identifies the device, the
/// operator-chosen long/short names, and how many hops away from us
/// the most recent packet was.
///
/// `PartialEq` only — `snr` is `f32` and floats don't implement `Eq`.
/// Equality on the rest of the struct still works for tests.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshNode {
    pub node_id: String,
    pub long_name: String,
    pub short_name: String,
    pub hops_away: u8,
    pub last_heard_secs: u64,
    pub snr: Option<f32>,
}

impl MeshNode {
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
}

/// One chat line on the longfast channel. `from` is the operator-chosen
/// label (`label()` above) when we know the sender, else `node_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshChatLine {
    pub from: String,
    pub text: String,
    pub hops_away: u8,
    pub is_local: bool,
}

/// What the mesh screen can ask of the underlying transport. All methods
/// have an in-process `FakeTransport` for tests; the real transport lives
/// in `Mesh` and is selected at runtime by `pub fn transport()`.
pub trait MeshTransport {
    /// Snapshot of known nodes. Returns what the transport currently knows;
    /// the screen does NOT keep a separate authoritative copy until
    /// `apply_nodes` is called.
    fn nodes(&self) -> Vec<MeshNode>;
    /// Snapshot of chat lines already received on the longfast channel.
    fn messages(&self) -> Vec<MeshChatLine>;
    /// True when the transport has an active serial handle. False means
    /// "no device" — the screen renders a connect-prompt instead.
    fn connected(&self) -> bool;
    /// Send `text` from the local node on the longfast channel. The
    /// transport echoes the line back through `messages()` so the chat
    /// pane can append it.
    fn send_longfast(&mut self, text: &str) -> Result<(), MeshError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshError {
    NotConnected,
    Empty,
    TooLong,
    Io(String),
}

/// A no-op transport used by tests and by a TUI running on a box without
/// a Meshtastic device. Public so the `App` can default to it.
#[derive(Debug, Default, Clone)]
pub struct FakeTransport {
    pub nodes: Vec<MeshNode>,
    pub messages: Vec<MeshChatLine>,
    pub connected: bool,
    /// Recorded outbound messages for test assertions.
    pub sent: Vec<String>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_nodes(nodes: Vec<MeshNode>) -> Self {
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

impl MeshTransport for FakeTransport {
    fn nodes(&self) -> Vec<MeshNode> {
        self.nodes.clone()
    }
    fn messages(&self) -> Vec<MeshChatLine> {
        self.messages.clone()
    }
    fn connected(&self) -> bool {
        self.connected
    }
    fn send_longfast(&mut self, text: &str) -> Result<(), MeshError> {
        if !self.connected {
            return Err(MeshError::NotConnected);
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(MeshError::Empty);
        }
        if trimmed.len() > 200 {
            return Err(MeshError::TooLong);
        }
        self.sent.push(trimmed.to_string());
        self.messages.push(MeshChatLine {
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
/// `FakeTransport` without touching any USB code paths.
pub struct MeshScreen {
    pub transport: Box<dyn MeshTransport + Send>,
    /// Selected node in the right pane; drives the "to: <name>" hint only.
    pub nodes_selected: usize,
}

impl MeshScreen {
    pub fn new(transport: Box<dyn MeshTransport + Send>) -> Self {
        Self {
            transport,
            nodes_selected: 0,
        }
    }

    /// Refresh `app.mesh_*` from the transport. Cheap; called on every
    /// `Action::Tick`. Keeps the visible state on `App` (so the test
    /// surface matches every other screen) and uses the transport as the
    /// source of truth for nodes + chat history.
    pub fn poll(&mut self, app: &mut App) {
        app.mesh_connected = self.transport.connected();
        app.mesh_nodes = self.transport.nodes();
        app.mesh_chat = self.transport.messages();
    }
}

impl Screen for MeshScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Mesh
    }
    fn title(&self) -> &'static str {
        "Mesh"
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn on_key(&mut self, key: KeyEvent, app: &mut App) -> bool {
        match key.code {
            // Up/Down on the chat scroll offset (tail is 0). Like the
            // System screen's right pane: j/k (Up/Down) step one line.
            KeyCode::Char('j') | KeyCode::Down => {
                app.mesh_chat_offset = app.mesh_chat_offset.saturating_add(1);
                return true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.mesh_chat_offset = app.mesh_chat_offset.saturating_sub(1);
                return true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                // Snap to the live tail.
                app.mesh_chat_offset = 0;
                return true;
            }
            KeyCode::Char('g') => {
                // Snap to the start of the log (oldest visible).
                app.mesh_chat_offset = usize::MAX;
                return true;
            }
            KeyCode::PageDown => {
                app.mesh_chat_offset = app.mesh_chat_offset.saturating_add(10);
                return true;
            }
            KeyCode::PageUp => {
                app.mesh_chat_offset = app.mesh_chat_offset.saturating_sub(10);
                return true;
            }
            // Cursor moves on the nodes list. Tab/Shift-Tab cycles the
            // whole TUI; this stays on the screen's own input handling.
            KeyCode::Tab => {
                if !app.mesh_nodes.is_empty() {
                    self.nodes_selected =
                        (self.nodes_selected + 1) % app.mesh_nodes.len();
                }
                return true;
            }
            KeyCode::BackTab => {
                if !app.mesh_nodes.is_empty() {
                    self.nodes_selected = if self.nodes_selected == 0 {
                        app.mesh_nodes.len() - 1
                    } else {
                        self.nodes_selected - 1
                    };
                }
                return true;
            }
            // Send the buffer on Enter.
            KeyCode::Enter => {
                let draft = app.mesh_input.clone();
                app.mesh_input.clear();
                match self.transport.send_longfast(&draft) {
                    Ok(()) => {
                        // Refresh the chat pane so the new line appears.
                        app.mesh_chat = self.transport.messages();
                        app.push_toast(crate::app::toast::ToastKind::Ok, "sent");
                    }
                    Err(MeshError::NotConnected) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            "mesh: not connected",
                        );
                    }
                    Err(MeshError::Empty) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            "empty message",
                        );
                    }
                    Err(MeshError::TooLong) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Warn,
                            "message too long (max 200)",
                        );
                    }
                    Err(MeshError::Io(s)) => {
                        app.push_toast(
                            crate::app::toast::ToastKind::Error,
                            format!("mesh io: {s}"),
                        );
                    }
                }
                return true;
            }
            KeyCode::Backspace => {
                app.mesh_input.pop();
                return true;
            }
            KeyCode::Char(c) => {
                if app.mesh_input.len() < 200 {
                    app.mesh_input.push(c);
                }
                return true;
            }
            _ => return false,
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Mesh ", theme.title()))
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
        // The chat is a text log; we render `mesh_chat` lines and clamp
        // `mesh_chat_offset` (lines back from the tail) so an empty buffer
        // doesn't strand the cursor.
        let total = app.mesh_chat.len();
        let visible_h = cols[0].height as usize;
        let max_off = total.saturating_sub(1);
        if app.mesh_chat_offset > max_off {
            app.mesh_chat_offset = max_off;
        }
        let end = total.saturating_sub(app.mesh_chat_offset);
        let start = end.saturating_sub(visible_h);
        let items: Vec<ListItem> = if total == 0 {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no messages yet — j/k scroll, type + Enter to send)",
                theme.dim(),
            )))]
        } else {
            app.mesh_chat[start..end]
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
        if !app.mesh_nodes.is_empty() {
            if self.nodes_selected >= app.mesh_nodes.len() {
                self.nodes_selected = app.mesh_nodes.len() - 1;
            }
        } else {
            self.nodes_selected = 0;
        }
        let right_items: Vec<ListItem> = if app.mesh_nodes.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no nodes yet)",
                theme.dim(),
            )))]
        } else {
            app.mesh_nodes
                .iter()
                .map(|n| {
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
                        Span::styled(format!("{:>8}", truncate(&n.label(), 8)), theme.accent),
                        Span::styled(format!("  {:<10}", hops), theme.fg),
                        Span::styled(format!("  {:>8}", snr), theme.dim()),
                    ]))
                })
                .collect()
        };
        let right_highlight = if app.mesh_nodes.is_empty() {
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
        let status = if app.mesh_connected {
            Span::styled("● connected ", theme.ok())
        } else {
            Span::styled("○ not connected ", theme.warn())
        };
        let to_node = if let Some(n) = app.mesh_nodes.get(self.nodes_selected) {
            format!("to: {}", truncate(&n.label(), 8))
        } else {
            "to: (broadcast)".to_string()
        };
        let input = Paragraph::new(Line::from(vec![
            status,
            Span::raw(" "),
            Span::styled(format!("{} ", to_node), theme.dim()),
            Span::styled("> ", theme.key()),
            Span::styled(app.mesh_input.clone(), theme.fg),
            Span::styled("▏", theme.accent),
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

/// Convenience: a transport that points at /dev/ttyUSB* (the most common
/// Meshtastic USB-CDC mount). The screen never calls this directly —
/// `App::default_mesh_transport` does, and only when the OS file actually
/// exists. Keeps the test surface free of USB code paths.
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
    fn empty_id_node() -> MeshNode {
        MeshNode {
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
            Err(MeshError::NotConnected)
        );
        assert!(t.sent.is_empty());
    }

    #[test]
    fn fake_transport_rejects_empty_message() {
        let mut t = FakeTransport::new().with_connected(true);
        assert_eq!(t.send_longfast("   \n"), Err(MeshError::Empty));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn fake_transport_rejects_overlong_message() {
        let mut t = FakeTransport::new().with_connected(true);
        let long = "a".repeat(201);
        assert_eq!(t.send_longfast(&long), Err(MeshError::TooLong));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn fake_transport_echoes_local_line_to_messages() {
        let mut t = FakeTransport::new().with_connected(true);
        t.send_longfast("hello mesh").unwrap();
        assert_eq!(t.sent, vec!["hello mesh".to_string()]);
        assert_eq!(t.messages().len(), 1);
        let line = &t.messages()[0];
        assert!(line.is_local);
        assert_eq!(line.text, "hello mesh");
        assert_eq!(line.from, "me");
    }

    #[test]
    fn mesh_node_label_prefers_long_then_short_then_id() {
        let n = MeshNode {
            node_id: "!abcdef01".into(),
            long_name: String::new(),
            short_name: "alpha".into(),
            hops_away: 2,
            last_heard_secs: 5,
            snr: Some(7.5),
        };
        assert_eq!(n.label(), "alpha");

        let m = MeshNode {
            node_id: "!abcdef01".into(),
            long_name: "Trailer Mesh".into(),
            short_name: "alpha".into(),
            hops_away: 2,
            last_heard_secs: 5,
            snr: Some(7.5),
        };
        assert_eq!(m.label(), "Trailer Mesh");

        // No operator-chosen names — fall back to a slice of the node id.
        // `label()` returns the slice verbatim; the render layer is what
        // pads to column width.
        let k = empty_id_node();
        assert_eq!(k.label(), "!abcdef01");
    }

    #[test]
    fn poll_copies_transport_state_into_app() {
        let node = MeshNode {
            node_id: "!aabbccdd".into(),
            long_name: "Trailer".into(),
            short_name: "TR".into(),
            hops_away: 1,
            last_heard_secs: 30,
            snr: Some(4.0),
        };
        let line = MeshChatLine {
            from: "TR".into(),
            text: "test ping".into(),
            hops_away: 1,
            is_local: false,
        };
        let mut t = FakeTransport::with_nodes(vec![node.clone()]).with_connected(true);
        t.messages.push(line.clone());

        let mut app = make_app();
        let mut screen = MeshScreen::new(Box::new(t));
        screen.poll(&mut app);
        assert!(app.mesh_connected);
        assert_eq!(app.mesh_nodes, vec![node]);
        assert_eq!(app.mesh_chat, vec![line]);
    }

    // Typing builds up the input buffer; Enter sends it via the transport.
    // The screen's `poll` after the Enter mirrors what the live main loop
    // does on every tick; `mesh_chat` then contains the new local line.
    #[test]
    fn typing_and_enter_sends_via_transport() {
        let mut app = make_app();
        let t = FakeTransport::new().with_connected(true);
        let mut screen = MeshScreen::new(Box::new(t));

        // Type "hi".
        assert!(screen.on_key(kc('h'), &mut app));
        assert!(screen.on_key(kc('i'), &mut app));
        assert_eq!(app.mesh_input, "hi");
        // Enter sends.
        assert!(screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app));
        // Buffer cleared, line echoed into messages via `poll`.
        assert_eq!(app.mesh_input, "");
        screen.poll(&mut app);
        assert_eq!(app.mesh_chat.len(), 1);
        assert!(app.mesh_chat[0].is_local);
        assert_eq!(app.mesh_chat[0].text, "hi");
    }

    // Enter with an empty/disconnected transport pushes a toast and does
    // NOT mutate `mesh_chat`. Important so a misconfigured box (no
    // /dev/ttyUSB*) doesn't fill the toast log with empty messages.
    #[test]
    fn enter_with_no_connection_pushes_warn_toast_and_no_message() {
        let mut app = make_app();
        let t = FakeTransport::new(); // disconnected
        let mut screen = MeshScreen::new(Box::new(t));

        assert!(screen.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app));
        assert!(app.mesh_chat.is_empty(), "no line should be added");
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
            MeshNode {
                node_id: "!aa".into(),
                long_name: "A".into(),
                short_name: "A".into(),
                hops_away: 1,
                last_heard_secs: 1,
                snr: Some(1.0),
            },
            MeshNode {
                node_id: "!bb".into(),
                long_name: "B".into(),
                short_name: "B".into(),
                hops_away: 2,
                last_heard_secs: 1,
                snr: Some(2.0),
            },
        ];
        let t = FakeTransport::with_nodes(nodes.clone());
        let mut screen = MeshScreen::new(Box::new(t));
        app.mesh_nodes = nodes;

        // Type "ab", backspace once → "a".
        screen.on_key(kc('a'), &mut app);
        screen.on_key(kc('b'), &mut app);
        assert_eq!(app.mesh_input, "ab");
        screen.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
        assert_eq!(app.mesh_input, "a");

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
        let mut screen = MeshScreen::new(Box::new(t));
        app.mesh_chat_offset = 5;
        screen.on_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE), &mut app);
        assert_eq!(app.mesh_chat_offset, 0, "G == live tail");
        screen.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE), &mut app);
        assert_eq!(app.mesh_chat_offset, usize::MAX, "g == oldest visible");
    }

    // Mesh transport trait is object-safe so Box<dyn MeshTransport + Send>
    // is usable from `MeshScreen` on a tokio task. If a future change
    // accidentally adds a generic method to the trait, this test fails.
    #[test]
    fn mesh_transport_is_object_safe() {
        fn assert_object_safe(_t: Box<dyn MeshTransport + Send>) {}
        assert_object_safe(Box::new(FakeTransport::new()));
    }
}
