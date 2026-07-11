//! WhatsApp Web screen — QR pairing → contacts list → chat view.
//! Talks to the Node.js sidecar (`scripts/whatsapp-bridge/bridge.mjs`) via
//! JSON lines over stdin/stdout, managed by a background task.

use std::io::{BufRead, Write as IoWrite};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use tokio::sync::mpsc;

use crate::app::action::Action;
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

static BRIDGE_STDIN: OnceLock<Mutex<Option<std::process::ChildStdin>>> = OnceLock::new();

pub fn send_wa_message(jid: &str, text: &str) {
    let stdin_lock = BRIDGE_STDIN.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = stdin_lock.lock() {
        if let Some(ref mut stdin) = *guard {
            let cmd = serde_json::json!({"type": "send", "jid": jid, "text": text});
            let _ = writeln!(stdin, "{}", cmd);
            let _ = stdin.flush();
        }
    }
}

fn send_bridge_cmd(cmd: &serde_json::Value) {
    let stdin_lock = BRIDGE_STDIN.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = stdin_lock.lock() {
        if let Some(ref mut stdin) = *guard {
            let _ = writeln!(stdin, "{}", cmd);
            let _ = stdin.flush();
        }
    }
}

/// Spawn the Node.js bridge sidecar. Call once from on_focus.
fn spawn_bridge(tx: mpsc::Sender<Action>) -> Option<Child> {
    let bridge_path = {
        let mut p = std::env::current_exe().ok()?;
        p.pop(); p.pop(); p.pop(); // target/debug/binary → repo root
        let candidate = p.join("scripts/whatsapp-bridge/bridge.mjs");
        if candidate.exists() {
            candidate
        } else {
            // fallback: relative to cwd
            std::path::PathBuf::from("scripts/whatsapp-bridge/bridge.mjs")
        }
    };

    let mut child = Command::new("node")
        .arg(&bridge_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdin = child.stdin.take()?;
    {
        let stdin_lock = BRIDGE_STDIN.get_or_init(|| Mutex::new(None));
        if let Ok(mut guard) = stdin_lock.lock() {
            *guard = Some(stdin);
        }
    }

    let stdout = child.stdout.take()?;
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let evt_type = val["type"].as_str().unwrap_or("");
            match evt_type {
                "qr" => {
                    if let Some(qr) = val["qr"].as_str() {
                        let _ = tx.try_send(Action::WhatsAppQr(qr.to_string()));
                    }
                }
                "connected" => {
                    let _ = tx.try_send(Action::WhatsAppConnected);
                    let _ = tx.try_send(Action::Toast(
                        crate::app::toast::ToastKind::Info,
                        "WhatsApp connected".into(),
                    ));
                }
                "disconnected" => {
                    let reason = val["reason"].as_str().unwrap_or("unknown").to_string();
                    let _ = tx.try_send(Action::WhatsAppDisconnected(reason));
                }
                "contacts" => {
                    if let Some(arr) = val["contacts"].as_array() {
                        let contacts: Vec<(String, String)> = arr.iter()
                            .filter_map(|c| {
                                let jid = c["jid"].as_str()?.to_string();
                                let name = c["name"].as_str()?.to_string();
                                Some((jid, name))
                            })
                            .collect();
                        let _ = tx.try_send(Action::WhatsAppContacts(contacts));
                    }
                }
                "message" => {
                    let jid = val["from"].as_str().unwrap_or("").to_string();
                    let text = val["text"].as_str().unwrap_or("").to_string();
                    let from_me = val["from_me"].as_bool().unwrap_or(false);
                    let timestamp = val["timestamp"].as_u64().unwrap_or(0);
                    let _ = tx.try_send(Action::WhatsAppMessage { jid, text, from_me, timestamp });
                }
                _ => {}
            }
        }
    });

    Some(child)
}

pub struct WhatsAppScreenV2 {
    started: bool,
    child: Option<Child>,
    selected_contact: usize,
    scroll: usize,
    input: String,
    active_jid: Option<String>,
}

impl Default for WhatsAppScreenV2 {
    fn default() -> Self {
        Self {
            started: false,
            child: None,
            selected_contact: 0,
            scroll: 0,
            input: String::new(),
            active_jid: None,
        }
    }
}

impl ScreenV2 for WhatsAppScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::WhatsApp }
    fn title(&self) -> &str { "WhatsApp" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Left, Zone::Right] }
    fn hint(&self) -> &str { "▲▼ select   Enter open/send   Esc back   Tab pane" }

    fn on_focus(&mut self, ctx: &mut UiContext<'_>) {
        if !self.started {
            self.started = true;
            self.child = spawn_bridge(ctx.tx.clone());
        }
    }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Tab => {
                ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % 2;
                Consumed::Yes
            }
            NavEvent::BackTab => {
                ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % 2;
                Consumed::Yes
            }
            NavEvent::Up => {
                if ctx.nav.focus_zone == 0 {
                    self.selected_contact = self.selected_contact.saturating_sub(1);
                } else {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                Consumed::Yes
            }
            NavEvent::Down => {
                if ctx.nav.focus_zone == 0 {
                    self.selected_contact += 1;
                } else {
                    self.scroll += 1;
                }
                Consumed::Yes
            }
            NavEvent::Confirm => {
                if ctx.nav.focus_zone == 0 {
                    // Select contact → open chat
                    if let Ok(contacts) = ctx.live.wa_contacts.try_read() {
                        if let Some((jid, _)) = contacts.get(self.selected_contact) {
                            self.active_jid = Some(jid.clone());
                            self.scroll = 0;
                            ctx.nav.focus_zone = 1;
                        }
                    }
                } else {
                    // Send message
                    let text = self.input.trim().to_string();
                    if !text.is_empty() {
                        if let Some(ref jid) = self.active_jid {
                            ctx.queue_action(Action::WhatsAppSubmit(jid.clone(), text.clone()));
                            ctx.queue_action(Action::WhatsAppMessage {
                                jid: jid.clone(),
                                text,
                                from_me: true,
                                timestamp: 0,
                            });
                            self.input.clear();
                        }
                    }
                }
                Consumed::Yes
            }
            NavEvent::Char(c) => {
                if ctx.nav.focus_zone == 1 {
                    self.input.push(c);
                }
                Consumed::Yes
            }
            NavEvent::Backspace => {
                if ctx.nav.focus_zone == 1 {
                    self.input.pop();
                }
                Consumed::Yes
            }
            NavEvent::Back => {
                if self.active_jid.is_some() && ctx.nav.focus_zone == 1 {
                    self.active_jid = None;
                    ctx.nav.focus_zone = 0;
                } else {
                    ctx.go_back();
                }
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let connected = ctx.live.wa_connected.try_read().map(|g| *g).unwrap_or(false);

        // Not connected — show QR or "starting..."
        if !connected {
            let qr_text = ctx.live.wa_qr.try_read()
                .ok()
                .and_then(|g| g.clone());

            let block = Block::default()
                .title(Span::styled(" ✆ WhatsApp — Scan QR ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(true));

            let content = match qr_text {
                Some(qr) => qr,
                None if self.started => "⏳ Starting WhatsApp bridge... run `cd scripts/whatsapp-bridge && npm install` if first time.".into(),
                None => "Press Enter to start WhatsApp bridge.".into(),
            };

            frame.render_widget(
                Paragraph::new(content)
                    .block(block)
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }

        // Connected — split into contacts (left) + chat (right)
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(area);

        let left_focused = ctx.nav.focus_zone == 0;
        let right_focused = ctx.nav.focus_zone == 1;

        // ── Left: contacts ──────────────────────────────────────────────────
        let contacts = ctx.live.wa_contacts.try_read()
            .map(|g| g.clone())
            .unwrap_or_default();

        let contact_items: Vec<ListItem> = contacts.iter().enumerate().map(|(i, (_jid, name))| {
            let style = if i == self.selected_contact {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let prefix = if Some(_jid.as_str()) == self.active_jid.as_deref() { "▶ " } else { "  " };
            ListItem::new(Line::from(Span::styled(format!("{prefix}{name}"), style)))
        }).collect();

        let contacts_block = Block::default()
            .title(Span::styled(format!(" Contacts ({}) ", contacts.len()), theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(left_focused));
        frame.render_widget(List::new(contact_items).block(contacts_block), cols[0]);

        // ── Right: chat ─────────────────────────────────────────────────────
        let chat_area = cols[1];
        let chat_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(chat_area);
        let (msg_area, input_area) = (chat_chunks[0], chat_chunks[1]);

        let chat_title = match &self.active_jid {
            Some(jid) => {
                contacts.iter().find(|(j, _)| j == jid)
                    .map(|(_, name)| format!(" Chat: {name} "))
                    .unwrap_or_else(|| format!(" Chat: {jid} "))
            }
            None => " Select a contact ".into(),
        };

        let msg_block = Block::default()
            .title(Span::styled(chat_title, theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(right_focused));
        let inner = msg_block.inner(msg_area);
        frame.render_widget(msg_block, msg_area);

        if let Some(ref active_jid) = self.active_jid {
            let messages = ctx.live.wa_messages.try_read()
                .map(|g| g.clone())
                .unwrap_or_default();

            let max_w = inner.width as usize;
            let mut lines: Vec<Line<'static>> = Vec::new();
            for m in messages.iter().filter(|m| m.jid == *active_jid) {
                let style = if m.from_me {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(theme.fg)
                };
                let prefix = if m.from_me { "You: " } else { "" };
                let text = format!("{prefix}{}", m.text);
                for wl in wrap_line(&text, max_w) {
                    lines.push(Line::from(Span::styled(wl, style)));
                }
            }

            let total = lines.len();
            let visible = inner.height as usize;
            let max_scroll = total.saturating_sub(visible);
            let display_scroll = self.scroll.min(max_scroll);

            let visible_lines: Vec<ListItem> = lines.into_iter()
                .skip(display_scroll)
                .take(visible)
                .map(ListItem::new)
                .collect();
            frame.render_widget(List::new(visible_lines), inner);
        } else {
            frame.render_widget(
                Paragraph::new("  ← Select a contact to start chatting")
                    .style(Style::default().fg(theme.dim)),
                inner,
            );
        }

        // Input box
        let cursor = if right_focused { "█" } else { "" };
        let input_text = format!("> {}{}", self.input, cursor);
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(right_focused));
        frame.render_widget(Paragraph::new(input_text).block(input_block), input_area);
    }
}

fn wrap_line(text: &str, max_w: usize) -> Vec<String> {
    if max_w == 0 { return vec![String::new()]; }
    if text.len() <= max_w { return vec![text.to_string()]; }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        if cur.is_empty() {
            if word.len() > max_w {
                for chunk in word.as_bytes().chunks(max_w) {
                    out.push(String::from_utf8_lossy(chunk).into_owned());
                }
            } else {
                cur = word.to_string();
            }
        } else if cur.len() + 1 + word.len() <= max_w {
            cur.push(' ');
            cur.push_str(word);
        } else {
            out.push(std::mem::take(&mut cur));
            if word.len() > max_w {
                for chunk in word.as_bytes().chunks(max_w) {
                    out.push(String::from_utf8_lossy(chunk).into_owned());
                }
            } else {
                cur = word.to_string();
            }
        }
    }
    if !cur.is_empty() { out.push(cur); }
    if out.is_empty() { out.push(String::new()); }
    out
}
