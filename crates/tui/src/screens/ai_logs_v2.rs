//! S20 — AI conversation log viewer. Read-only trace of ai_messages.
//! D-pad: Up/Down navigate, Enter expand/collapse, B back.

use std::cell::Cell;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::live_data::{AiMessage, AiRole};
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

#[derive(Clone, Copy)]
enum EntryKind { User, Think, Asst }

struct Entry {
    kind: EntryKind,
    msg_idx: usize,
}

fn build_entries(msgs: &[AiMessage]) -> Vec<Entry> {
    let mut out = Vec::with_capacity(msgs.len() * 2);
    for (i, msg) in msgs.iter().enumerate() {
        if msg.role == AiRole::User {
            out.push(Entry { kind: EntryKind::User, msg_idx: i });
        } else {
            if !msg.thinking.is_empty() {
                out.push(Entry { kind: EntryKind::Think, msg_idx: i });
            }
            out.push(Entry { kind: EntryKind::Asst, msg_idx: i });
        }
    }
    out
}

pub struct AiLogsScreen {
    selected: usize,
    scroll: usize,
    expanded: bool,
    // ponytail: Cell tracks visible_rows from last render for scroll math in on_nav
    visible_rows: Cell<usize>,
}

impl Default for AiLogsScreen {
    fn default() -> Self {
        Self {
            selected: 0,
            scroll: 0,
            expanded: false,
            visible_rows: Cell::new(10),
        }
    }
}

impl ScreenV2 for AiLogsScreen {
    fn id(&self) -> ScreenId { ScreenId::AiLogs }
    fn title(&self) -> &str { "AI Logs" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   A expand   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                if self.selected < self.scroll {
                    self.scroll = self.selected;
                }
                Consumed::Yes
            }
            NavEvent::Down => {
                let n = ctx.live.ai_messages.try_read()
                    .map(|g| g.iter().map(|m| {
                        if m.role == AiRole::Assistant && !m.thinking.is_empty() { 2 } else { 1 }
                    }).sum::<usize>())
                    .unwrap_or(0);
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                    let visible = self.visible_rows.get().max(1);
                    if self.selected >= self.scroll + visible {
                        self.scroll = self.selected + 1 - visible;
                    }
                }
                Consumed::Yes
            }
            NavEvent::Confirm => {
                self.expanded = !self.expanded;
                Consumed::Yes
            }
            NavEvent::Back => {
                ctx.go_back();
                Consumed::Yes
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let msgs_guard = match ctx.live.ai_messages.try_read() {
            Ok(g) => g,
            Err(_) => return,
        };

        let entries = build_entries(&msgs_guard);
        let n = entries.len();

        // Layout: top list + optional detail panel when expanded
        let (list_area, detail_area) = if self.expanded && n > 0 {
            let list_h = (area.height / 2).max(4).min(area.height.saturating_sub(3));
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(list_h), Constraint::Min(0)])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // ── List panel ───────────────────────────────────────────────────────
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" ▧ AI Logs ", theme.title()));
        let inner = block.inner(list_area);
        frame.render_widget(block, list_area);

        if n == 0 {
            frame.render_widget(
                Paragraph::new("  No messages yet — visit the AI screen to chat.")
                    .style(Style::default().fg(theme.dim)),
                inner,
            );
            return;
        }

        let visible_h = inner.height as usize;
        self.visible_rows.set(visible_h);

        let selected = self.selected.min(n - 1);
        // Clamp scroll so selected stays in view
        let scroll = self.scroll.min(selected)
            .max(selected.saturating_sub(visible_h.saturating_sub(1)));

        let user_style  = Style::default().fg(theme.accent);
        let asst_style  = Style::default().fg(theme.fg);
        let think_style = Style::default().fg(theme.dim).add_modifier(Modifier::ITALIC);

        // Reserve chars for "  [N] ROLE   " prefix (≈13 chars for single-digit indices)
        let preview_width = (inner.width as usize).saturating_sub(14);

        let rows: Vec<ListItem<'static>> = entries.iter()
            .enumerate()
            .skip(scroll)
            .take(visible_h)
            .map(|(i, e)| {
                let msg = &msgs_guard[e.msg_idx];
                let cursor = if i == selected { "► " } else { "  " };
                let (role_tag, text, style) = match e.kind {
                    EntryKind::User  => ("USER ", &msg.content,  user_style),
                    EntryKind::Think => ("THINK", &msg.thinking, think_style),
                    EntryKind::Asst  => ("ASST ", &msg.content,  asst_style),
                };
                let preview = one_line_preview(text, preview_width);
                ListItem::new(Line::from(Span::styled(
                    format!("{cursor}[{i}] {role_tag}  {preview}"),
                    style,
                )))
            })
            .collect();

        frame.render_widget(List::new(rows), inner);

        // ── Detail panel ─────────────────────────────────────────────────────
        if let Some(darea) = detail_area {
            let entry = &entries[selected];
            let msg = &msgs_guard[entry.msg_idx];
            let (panel_title, full_text, style) = match entry.kind {
                EntryKind::User  => (" User ",      msg.content.as_str(),  user_style),
                EntryKind::Think => (" Thinking ",  msg.thinking.as_str(), think_style),
                EntryKind::Asst  => (" Assistant ", msg.content.as_str(),  asst_style),
            };
            let detail_block = Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(true))
                .title(Span::styled(panel_title, theme.title()));
            let detail_inner = detail_block.inner(darea);
            frame.render_widget(detail_block, darea);
            frame.render_widget(
                Paragraph::new(full_text.to_owned())
                    .style(style)
                    .wrap(Wrap { trim: false }),
                detail_inner,
            );
        }
    }
}

fn one_line_preview(s: &str, max_chars: usize) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    let mut chars = first.chars();
    let head: String = chars.by_ref().take(max_chars.saturating_sub(3)).collect();
    if chars.next().is_some() {
        format!("{head}...")
    } else {
        head.to_string()
    }
}
