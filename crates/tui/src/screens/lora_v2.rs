//! LoRa screen v2 — Meshtastic chat (left) + saved node IPs (right).
//! Right panel manages user-persisted node endpoints via prefs.lora_nodes.
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::action::Action;
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::modal::{ConfirmModal, InputModal, Modal, ModalResult};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;
use crate::prefs::SavedLoraNode;
use crate::screens::lora::{FakeTransport, LoraChatLine, LoraTransport};
use crate::theme::Theme;

const ZONES: &[Zone] = &[Zone::Left, Zone::Right];

pub struct LoraScreenV2 {
    pub transport: Box<dyn LoraTransport + Send>,
    pub input:     String,
    pub scroll:    usize,
    /// Logical cursor in the saved-nodes panel. 0..nodes.len()-1 = a node;
    /// nodes.len() = the "[ + Add Node ]" entry.
    pub saved_sel: usize,
    /// IP of the currently active (connected-to) node. `None` = not connected.
    pub active_ip: Option<String>,
}

impl Default for LoraScreenV2 {
    fn default() -> Self {
        Self {
            transport: Box::new(FakeTransport::new()),
            input:     String::new(),
            scroll:    0,
            saved_sel: 0,
            active_ip: None,
        }
    }
}

// ── Thin modal wrappers that return domain-specific Actions ─────────────────

struct LoraAddModal(InputModal);

impl Modal for LoraAddModal {
    fn on_nav(&mut self, ev: NavEvent) -> ModalResult { self.0.on_nav(ev) }
    fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) { self.0.render(f, area, theme) }
    fn accepts_text_input(&self) -> bool { true }
    fn commit_action(&self, value: String) -> Action { Action::LoraNodeAdd(value) }
}

struct LoraDeleteModal { idx: usize, inner: ConfirmModal }

impl Modal for LoraDeleteModal {
    fn on_nav(&mut self, ev: NavEvent) -> ModalResult { self.inner.on_nav(ev) }
    fn render(&self, f: &mut Frame, area: Rect, theme: &Theme) { self.inner.render(f, area, theme) }
    // ponytail: "no" returns Tick — one harmless extra redraw, avoids a NoOp variant
    fn commit_action(&self, value: String) -> Action {
        if value == "yes" { Action::LoraNodeDelete(self.idx) } else { Action::Tick }
    }
}

// ── Screen impl ─────────────────────────────────────────────────────────────

impl ScreenV2 for LoraScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::LoRa }
    fn title(&self) -> &str { "LoRa" }
    fn focusable_zones(&self) -> &[Zone] { ZONES }
    fn hint(&self) -> &str { "▲▼ scroll   ◀▶ pane   A select/send   d delete node   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let zone = ZONES.get(ctx.nav.focus_zone).copied().unwrap_or(Zone::Left);
        match (event, zone) {
            (NavEvent::Left, _) => { ctx.nav.focus_zone = 0; Consumed::Yes }
            (NavEvent::Right, _) => { ctx.nav.focus_zone = 1; Consumed::Yes }
            (NavEvent::Tab, _) => {
                ctx.nav.focus_zone = (ctx.nav.focus_zone + 1) % ZONES.len();
                Consumed::Yes
            }
            (NavEvent::BackTab, _) => {
                let n = ZONES.len();
                ctx.nav.focus_zone = (ctx.nav.focus_zone + n - 1) % n;
                Consumed::Yes
            }

            // ── Left pane: chat ──────────────────────────────────────────
            (NavEvent::Up, Zone::Left) => {
                self.scroll = self.scroll.saturating_sub(1);
                Consumed::Yes
            }
            (NavEvent::Down, Zone::Left) => {
                self.scroll += 1;
                Consumed::Yes
            }
            (NavEvent::Char(c), Zone::Left) => {
                if self.input.len() < 200 { self.input.push(c); }
                Consumed::Yes
            }
            (NavEvent::Backspace, Zone::Left) => {
                self.input.pop();
                Consumed::Yes
            }
            (NavEvent::Confirm, Zone::Left) => {
                let text = self.input.trim().to_string();
                if !text.is_empty() {
                    let _ = self.transport.send_longfast(&text);
                    self.input.clear();
                    self.scroll = 0;
                }
                Consumed::Yes
            }

            // ── Right pane: saved nodes ──────────────────────────────────
            (NavEvent::Up, Zone::Right) => {
                self.saved_sel = self.saved_sel.saturating_sub(1);
                Consumed::Yes
            }
            (NavEvent::Down, Zone::Right) => {
                // max logical index = nodes.len() (the "+" entry)
                let max = ctx.prefs.lora_nodes.len();
                self.saved_sel = (self.saved_sel + 1).min(max);
                Consumed::Yes
            }
            (NavEvent::Confirm, Zone::Right) => {
                let nodes = &ctx.prefs.lora_nodes;
                if self.saved_sel < nodes.len() {
                    self.active_ip = Some(nodes[self.saved_sel].ip.clone());
                } else {
                    ctx.open_modal(Box::new(LoraAddModal(InputModal::new(
                        "Add Node",
                        "IP [optional label]:",
                    ))));
                }
                Consumed::Yes
            }
            (NavEvent::Char('d'), Zone::Right) => {
                let idx = self.saved_sel;
                if idx < ctx.prefs.lora_nodes.len() {
                    let ip = ctx.prefs.lora_nodes[idx].ip.clone();
                    ctx.open_modal(Box::new(LoraDeleteModal {
                        idx,
                        inner: ConfirmModal::new("Delete Node", format!("Remove {}?", ip)),
                    }));
                }
                Consumed::Yes
            }

            (NavEvent::Back, _) => { ctx.go_back(); Consumed::Yes }
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

        // ── Left: chat ───────────────────────────────────────────────────────
        let messages = self.transport.messages();
        let visible_h = cols[0].height.saturating_sub(4) as usize;
        let total = messages.len();
        let scroll = self.scroll.min(total.saturating_sub(visible_h));
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(visible_h);

        let mut chat_items: Vec<ListItem<'static>> = if total == 0 {
            vec![ListItem::new(Line::from(Span::styled(
                if self.transport.connected() {
                    "  (no messages yet — type and press A to send)"
                } else {
                    "  (not connected — select a node →)"
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
        chat_items.push(ListItem::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                if self.input.is_empty() { "(type to compose)".into() } else { self.input.clone() },
                if self.input.is_empty() { Style::default().fg(theme.dim) } else { Style::default().fg(theme.fg) },
            ),
        ])));

        let connected_title = if self.transport.connected() { " chat ● " } else { " chat ○ " };
        frame.render_widget(
            List::new(chat_items)
                .block(Block::default()
                    .title(Span::styled(connected_title, theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(left_focused))),
            cols[0],
        );

        // ── Right: saved nodes ───────────────────────────────────────────────
        let nodes = &ctx.prefs.lora_nodes;
        // Logical saved_sel: 0..nodes.len()-1 = node, nodes.len() = "+".
        // Visual index when empty: "+" is at visual index 1 (after the empty msg).
        let (node_items, visual_sel) = build_node_items(nodes, self.saved_sel, &self.active_ip, theme);

        let mut list_state = ListState::default().with_selected(Some(visual_sel));
        frame.render_stateful_widget(
            List::new(node_items)
                .block(Block::default()
                    .title(Span::styled(format!(" nodes ({}) ", nodes.len()), theme.title()))
                    .borders(Borders::ALL)
                    .border_style(theme.border(right_focused)))
                .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
                .highlight_symbol("▶ "),
            cols[1],
            &mut list_state,
        );
    }
}

/// Build the nodes list items and the visual selection index.
/// Separates the fiddly index math from the render path.
fn build_node_items<'a>(
    nodes: &[SavedLoraNode],
    saved_sel: usize,
    active_ip: &Option<String>,
    theme: &crate::theme::Theme,
) -> (Vec<ListItem<'a>>, usize) {
    let add_entry = ListItem::new(Line::from(Span::styled(
        "  [ + Add Node ]",
        Style::default().fg(theme.accent),
    )));

    if nodes.is_empty() {
        let items = vec![
            ListItem::new(Line::from(Span::styled(
                "  No saved nodes — press Enter to add one",
                Style::default().fg(theme.dim),
            ))),
            add_entry,
        ];
        // saved_sel is always 0 ("+") when empty; visual index of "+" = 1
        (items, 1)
    } else {
        let mut items: Vec<ListItem<'a>> = nodes.iter().map(|n| {
            let is_active = active_ip.as_deref() == Some(n.ip.as_str());
            let label = match &n.label {
                Some(l) => format!("{} ({})", n.ip, l),
                None    => n.ip.clone(),
            };
            let style = if is_active {
                Style::default().fg(theme.ok)
            } else {
                Style::default().fg(theme.fg)
            };
            let prefix = if is_active { "● " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(label, style),
            ]))
        }).collect();
        items.push(add_entry);
        // Clamp saved_sel to [0, nodes.len()] — "+" is at nodes.len().
        let visual_sel = saved_sel.min(nodes.len());
        (items, visual_sel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_nodes_visual_sel_is_add_entry() {
        let (items, sel) = build_node_items(&[], 0, &None, &crate::theme::Theme::by_name(crate::theme::ThemeName::Dark));
        assert_eq!(items.len(), 2); // empty msg + "+"
        assert_eq!(sel, 1);         // "+" is highlighted
    }

    #[test]
    fn nonempty_nodes_sel_clamps_to_add_entry() {
        let nodes = vec![
            SavedLoraNode { ip: "10.0.0.1".to_string(), label: None },
        ];
        let (items, sel) = build_node_items(&nodes, 99, &None, &crate::theme::Theme::by_name(crate::theme::ThemeName::Dark));
        assert_eq!(items.len(), 2); // 1 node + "+"
        assert_eq!(sel, 1);         // clamped to nodes.len() = "+"
    }

    #[test]
    fn active_ip_marks_correct_entry() {
        let nodes = vec![
            SavedLoraNode { ip: "10.0.0.1".to_string(), label: None },
            SavedLoraNode { ip: "10.0.0.2".to_string(), label: Some("home".to_string()) },
        ];
        let (items, _) = build_node_items(
            &nodes, 0,
            &Some("10.0.0.2".to_string()),
            &crate::theme::Theme::by_name(crate::theme::ThemeName::Dark),
        );
        // 2 nodes + "+" = 3 items; no panic.
        assert_eq!(items.len(), 3);
    }
}
