//! S19 — AI chat screen. Talks to the llama-server sidecar via Action channel.
//! D-pad: arrows scroll, Enter submits, Tab toggles thinking blocks, B goes back.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::app::action::Action;
use crate::app::live_data::AiRole;
use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct AiScreenV2 {
    input: String,
    scroll: usize,      // lines from top; auto-adjusted to bottom during streaming
    auto_scroll: bool,  // track bottom automatically
    show_thinking: bool,
}

impl Default for AiScreenV2 {
    fn default() -> Self {
        Self {
            input: String::new(),
            scroll: 0,
            auto_scroll: true,
            show_thinking: true,
        }
    }
}

impl ScreenV2 for AiScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Ai }
    fn title(&self) -> &str { "AI" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   Tab thinking   Enter send   Esc back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Up => {
                if self.scroll > 0 {
                    self.scroll -= 1;
                    self.auto_scroll = false;
                }
                Consumed::Yes
            }
            NavEvent::Down => {
                self.scroll += 1;
                // re-enable auto-scroll if user scrolled to the bottom
                // (the render will clamp scroll to valid range)
                Consumed::Yes
            }
            NavEvent::Char(c) => {
                self.input.push(c);
                Consumed::Yes
            }
            NavEvent::Backspace => {
                self.input.pop();
                Consumed::Yes
            }
            NavEvent::Confirm => {
                let text = self.input.trim().to_string();
                if !text.is_empty() {
                    self.input.clear();
                    self.auto_scroll = true;
                    ctx.queue_action(Action::AiSubmit(text));
                }
                Consumed::Yes
            }
            NavEvent::Tab => {
                self.show_thinking = !self.show_thinking;
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

        // Check readiness
        let llama_ready = ctx.live.llama_ready.try_read()
            .map(|g| *g)
            .unwrap_or(false);

        // Split: messages above, input box at bottom
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area);
        let (msg_area, input_area) = (chunks[0], chunks[1]);

        // ── Input box ────────────────────────────────────────────────────────
        let cursor = if llama_ready { "█" } else { "" };
        let input_text = format!("> {}{}", self.input, cursor);
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(true))
            .title(Span::styled(" Input ", theme.title()));
        frame.render_widget(
            Paragraph::new(input_text).block(input_block),
            input_area,
        );

        // ── Message area ─────────────────────────────────────────────────────
        let msgs_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border(false))
            .title(Span::styled(" ◈ AI ", theme.title()));
        let inner = msgs_block.inner(msg_area);
        frame.render_widget(msgs_block, msg_area);

        if !llama_ready {
            let err_msg = ctx.live.llama_error.try_read()
                .ok()
                .and_then(|g| g.clone());
            let text = match err_msg {
                Some(e) => format!("  ⚠ AI failed: {e}"),
                None => "  ⏳ Starting AI model...".into(),
            };
            frame.render_widget(
                Paragraph::new(text).style(Style::default().fg(theme.dim)),
                inner,
            );
            return;
        }

        let msgs_guard = match ctx.live.ai_messages.try_read() {
            Ok(g) => g,
            Err(_) => return, // lock contended, skip frame
        };

        if msgs_guard.is_empty() {
            frame.render_widget(
                Paragraph::new("  Type a message and press Enter to chat with the local AI.")
                    .style(Style::default().fg(theme.dim)),
                inner,
            );
            return;
        }

        // Build flat line list from all messages, word-wrapped to fit
        let max_w = inner.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let code_style  = Style::default().fg(Color::Cyan);
        let think_style = Style::default().fg(theme.dim).add_modifier(Modifier::ITALIC);
        let user_style  = Style::default().fg(theme.accent);
        let dim_style   = Style::default().fg(theme.dim);
        let tool_style  = Style::default().fg(Color::Yellow);

        for msg in msgs_guard.iter() {
            match msg.role {
                AiRole::User => {
                    for wl in wrap_line(&format!("You: {}", msg.content), max_w) {
                        lines.push(Line::from(Span::styled(wl, user_style)));
                    }
                    lines.push(Line::from(""));
                }
                AiRole::Assistant => {
                    if self.show_thinking && !msg.thinking.is_empty() {
                        lines.push(Line::from(Span::styled(
                            "  💭 Thinking...",
                            dim_style,
                        )));
                        for tline in msg.thinking.split('\n') {
                            for wl in wrap_line(&format!("    {}", tline), max_w) {
                                lines.push(Line::from(Span::styled(wl, think_style)));
                            }
                        }
                        lines.push(Line::from(""));
                    }

                    // Show tool calls inline
                    if !msg.tool_log.is_empty() {
                        for tl in &msg.tool_log {
                            for wl in wrap_line(&format!("  🔧 {tl}"), max_w) {
                                lines.push(Line::from(Span::styled(wl, tool_style)));
                            }
                        }
                        lines.push(Line::from(""));
                    }

                    let mut in_code = false;
                    for cline in msg.content.split('\n') {
                        if cline.trim_start().starts_with("```") {
                            in_code = !in_code;
                            lines.push(Line::from(Span::styled(
                                truncate_or_pad(cline, max_w),
                                dim_style,
                            )));
                        } else if in_code {
                            lines.push(Line::from(Span::styled(
                                truncate_or_pad(&format!("  {cline}"), max_w),
                                code_style,
                            )));
                        } else {
                            for wl in wrap_line(cline, max_w) {
                                lines.push(Line::from(wl));
                            }
                        }
                    }

                    if msg.streaming {
                        lines.push(Line::from(Span::styled("▌", Style::default().fg(theme.accent))));
                    }
                    lines.push(Line::from(""));
                }
            }
        }

        let total = lines.len();
        let visible = inner.height as usize;

        // Compute display offset: auto-scroll pins to bottom
        let max_scroll = total.saturating_sub(visible);
        let display_scroll = if self.auto_scroll {
            max_scroll
        } else {
            self.scroll.min(max_scroll)
        };

        let visible_lines: Vec<ListItem<'static>> = lines
            .into_iter()
            .skip(display_scroll)
            .take(visible)
            .map(ListItem::new)
            .collect();

        frame.render_widget(List::new(visible_lines), inner);
    }
}

/// Word-wrap `text` to fit within `max_w` columns. Breaks at spaces
/// when possible, hard-breaks mid-word when a single word exceeds width.
fn wrap_line(text: &str, max_w: usize) -> Vec<String> {
    if max_w == 0 { return vec![String::new()]; }
    if text.len() <= max_w { return vec![text.to_string()]; }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        if cur.is_empty() {
            if word.len() > max_w {
                // hard-break long word
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

/// Truncate to max_w for code lines (no wrapping — preserves formatting).
fn truncate_or_pad(s: &str, max_w: usize) -> String {
    if s.len() <= max_w { s.to_string() } else { format!("{}…", &s[..max_w.saturating_sub(1)]) }
}
