//! LoRa screen v2 — Meshtastic chat (left) + nodes (right).
//! Reuses the transport trait and node/chat types from the existing
//! lora module; only the navigation and rendering are rewritten.
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;
use crate::screens::lora::{FakeTransport, LoraChatLine, LoraNode, LoraTransport};

const ZONES: &[Zone] = &[Zone::Left, Zone::Right];

pub struct LoraScreenV2 {
    pub transport: Box<dyn LoraTransport + Send>,
    pub input:     String,
    pub scroll:    usize,
    pub node_sel:  usize,
}

impl Default for LoraScreenV2 {
    fn default() -> Self {
        Self {
            transport: Box::new(FakeTransport::new()),
            input:     String::new(),
            scroll:    0,
            node_sel:  0,
        }
    }
}

impl ScreenV2 for LoraScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::LoRa }
    fn title(&self) -> &str { "LoRa" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼ scroll   ◀▶ pane   A send   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let zone = ZONES.get(ctx.nav.focus_zone).copied().unwrap_or(Zone::Left);
        match event {
            NavEvent::Left  => { ctx.nav.focus_zone = 0; Consumed::Yes }
            NavEvent::Right => { ctx.nav.focus_zone = 1; Consumed::Yes }
            NavEvent::Tab   => { ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % ZONES.len(); Consumed::Yes }
            NavEvent::BackTab => {
                let n = ZONES.len();
                ctx.nav.focus_zone = (ctx.nav.focus_zone + n - 1) % n;
                Consumed::Yes
            }
            NavEvent::Up if zone == Zone::Left => {
                self.scroll = self.scroll.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if zone == Zone::Left => {
                self.scroll = self.scroll.saturating_add(1);
                Consumed::Yes
            }
            NavEvent::Up if zone == Zone::Right => {
                self.node_sel = self.node_sel.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if zone == Zone::Right => {
                let nodes = self.transport.nodes();
                let n = nodes.len();
                if n > 0 { self.node_sel = (self.node_sel + 1).min(n - 1); }
                Consumed::Yes
            }
            NavEvent::Char(c) => {
                if self.input.len() < 200 { self.input.push(c); }
                Consumed::Yes
            }
            NavEvent::Backspace => {
                self.input.pop();
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let text = self.input.trim().to_string();
                if !text.is_empty() {
                    let _ = self.transport.send_longfast(&text);
                    self.input.clear();
                    self.scroll = 0;
                }
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let left_focused  = ctx.nav.focus_zone == 0;
        let right_focused = ctx.nav.focus_zone == 1;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // ── Left: chat messages ───────────────────────────────────────────────
        let messages = self.transport.messages();
        let visible_h = cols[0].height.saturating_sub(4) as usize; // block + input row
        let total = messages.len();
        let scroll = self.scroll.min(total.saturating_sub(visible_h));
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(visible_h);

        let mut chat_items: Vec<ListItem<'static>> = if total == 0 {
            vec![ListItem::new(Line::from(Span::styled(
                if self.transport.connected() {
                    "  (no messages yet — type and press A to send)"
                } else {
                    "  (not connected — set node IP via settings)"
                },
                Style::default().fg(theme.dim),
            )))]
        } else {
            messages[start..end].iter().map(|l: &LoraChatLine| {
                let name_style = if l.is_local { theme.ok() } else { Style::default().fg(theme.accent) };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("<{}> ", l.from), name_style),
                    Span::styled(l.text.clone(), Style::default().fg(theme.fg)),
                ]))
            }).collect()
        };

        // Input row appended at the bottom of the list
        chat_items.push(ListItem::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                if self.input.is_empty() { "(type to compose)".into() } else { self.input.clone() },
                if self.input.is_empty() { Style::default().fg(theme.dim) } else { Style::default().fg(theme.fg) },
            ),
        ])));

        let connected_title = if self.transport.connected() { " chat ● " } else { " chat ○ " };
        let chat = List::new(chat_items)
            .block(Block::default()
                .title(Span::styled(connected_title, theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(left_focused)));
        frame.render_widget(chat, cols[0]);

        // ── Right: nodes ─────────────────────────────────────────────────────
        let nodes = self.transport.nodes();
        let now  = LoraNode::now_secs();
        let node_sel = self.node_sel.min(nodes.len().saturating_sub(1));

        let node_items: Vec<ListItem<'static>> = if nodes.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no nodes seen)", Style::default().fg(theme.dim)
            )))]
        } else {
            nodes.iter().map(|n: &LoraNode| {
                let online = n.is_online_at(now);
                let dot_style = if online { theme.ok() } else { theme.dim() };
                ListItem::new(Line::from(vec![
                    Span::styled(if online { "● " } else { "○ " }, dot_style),
                    Span::styled(format!("{:<16}", n.label()), Style::default().fg(theme.fg)),
                    Span::styled(format!(" {}h", n.hops_away), Style::default().fg(theme.dim)),
                ]))
            }).collect()
        };

        let mut nodes_state = ratatui::widgets::ListState::default()
            .with_selected(if nodes.is_empty() { None } else { Some(node_sel) });
        let nodes_list = List::new(node_items)
            .block(Block::default()
                .title(Span::styled(format!(" nodes ({}) ", nodes.len()), theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(right_focused)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(nodes_list, cols[1], &mut nodes_state);
    }
}
