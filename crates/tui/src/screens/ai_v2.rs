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
    fn hint(&self) -> &str { "▲▼ scroll   Tab thinking   A send   B back" }

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
            let loading = ctx.live.llama_ready.try_read()
                .map(|_| ()) // lock acquired = we read false above
                .err()
                .map(|_| "  ⏳ Starting AI model...")
                .unwrap_or("  ⏳ Starting AI model...");
            // Check if we should show "no model" error via llama_down state
            // ponytail: just show loading; LlamaDown arrives as a toast from apply_action
            frame.render_widget(
                Paragraph::new(loading).style(Style::default().fg(theme.dim)),
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

        // Build flat line list from all messages
        let mut lines: Vec<Line<'static>> = Vec::new();
        let code_style  = Style::default().fg(Color::Cyan);
        let think_style = Style::default().fg(theme.dim).add_modifier(Modifier::ITALIC);
        let user_style  = Style::default().fg(theme.accent);
        let dim_style   = Style::default().fg(theme.dim);

        for msg in msgs_guard.iter() {
            match msg.role {
                AiRole::User => {
                    lines.push(Line::from(Span::styled(
                        format!("You: {}", msg.content),
                        user_style,
                    )));
                    lines.push(Line::from("")); // blank spacer
                }
                AiRole::Assistant => {
                    if self.show_thinking && !msg.thinking.is_empty() {
                        lines.push(Line::from(Span::styled(
                            "  💭 Thinking...",
                            dim_style,
                        )));
                        for tline in msg.thinking.split('\n') {
                            lines.push(Line::from(Span::styled(
                                format!("    {}", tline),
                                think_style,
                            )));
                        }
                        lines.push(Line::from("")); // blank spacer
                    }

                    let mut in_code = false;
                    for cline in msg.content.split('\n') {
                        if cline.trim_start().starts_with("```") {
                            in_code = !in_code;
                            lines.push(Line::from(Span::styled(
                                cline.to_string(),
                                dim_style,
                            )));
                        } else if in_code {
                            lines.push(Line::from(Span::styled(
                                format!("  {cline}"),
                                code_style,
                            )));
                        } else {
                            lines.push(Line::from(cline.to_string()));
                        }
                    }

                    if msg.streaming {
                        lines.push(Line::from(Span::styled("▌", Style::default().fg(theme.accent))));
                    }
                    lines.push(Line::from("")); // blank spacer
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
