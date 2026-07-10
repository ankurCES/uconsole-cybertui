use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::Span,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::modal::QuitConfirmModal;
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

static LOGO: &[&str] = &[
    "╔═══╗╔╗ ╔╗╔══╗ ╔═══╗╔══╗ ╔══╗ ╔═══╗╔╗╔═╗",
    "║╔══╝╚╗╔╝║║╔╗║ ║╔══╝║╔╗║ ║╔╗║ ║╔══╝║║║╔╝",
    "║║    ╚╝ ║║╚╝╚╗║╚══╗║╚╝╚╗║║║║ ║╚══╗║╚╝╝ ",
    "║║    ╔╗ ║║╔═╗║║╔══╝║╔═╗║║║║║ ║╔══╝║╔╗║  ",
    "║╚══╗╔╝╚╗║║╚═╝║║╚══╗║╚═╝║║╚╝║ ║╚══╗║║║╚╗",
    "╚═══╝╚══╝╚╝╚══╝╚═══╝╚═══╝╚══╝ ╚═══╝╚╝╚═╝",
];

// Circuit trace rows: connector ┬, scan line ░▒, connector ┴
static TRACES: &[&str] = &[
    "─┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬─",
    "░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒░▒",
    "─┴──┴──┴──┴──┴──┴──┴──┴──┴──┴──┴──┴──┴──┴─",
];

pub struct ScreensaverScreen;

impl Default for ScreensaverScreen {
    fn default() -> Self { Self }
}

impl ScreenV2 for ScreensaverScreen {
    fn id(&self) -> ScreenId { ScreenId::Screensaver }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn is_hidden(&self) -> bool { true }
    fn hint(&self) -> &str { "" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        match event {
            NavEvent::Back => { ctx.open_modal(Box::new(QuitConfirmModal)); Consumed::Yes }
            _ => { ctx.go_back(); Consumed::Yes }
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;
        let dim    = Style::default().fg(theme.dim);
        let accent = Style::default().fg(theme.accent);

        let block = Block::default().borders(Borders::ALL).border_style(accent);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let trace_h   = TRACES.len() as u16;
        let logo_h    = LOGO.len() as u16;
        let content_h = logo_h + 2; // logo + blank gap + prompt

        // Vertically center logo+prompt in the strip between the two trace bands.
        let avail  = inner.height.saturating_sub(trace_h * 2);
        let logo_y = inner.y + trace_h + avail.saturating_sub(content_h) / 2;

        // Top circuit traces
        for (i, t) in TRACES.iter().enumerate() {
            frame.render_widget(
                Paragraph::new(Span::styled(*t, dim)),
                Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 },
            );
        }

        // CYBERDECK logo — centered horizontally, accent colour
        for (i, line) in LOGO.iter().enumerate() {
            let y = logo_y + i as u16;
            if y >= inner.y + inner.height { break; }
            frame.render_widget(
                Paragraph::new(Span::styled(*line, accent)).alignment(Alignment::Center),
                Rect { x: inner.x, y, width: inner.width, height: 1 },
            );
        }

        // Prompt line
        let prompt_y = logo_y + logo_h + 1;
        if prompt_y < inner.y + inner.height {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "[ Press any key to return ]",
                    Style::default().fg(theme.fg),
                ))
                .alignment(Alignment::Center),
                Rect { x: inner.x, y: prompt_y, width: inner.width, height: 1 },
            );
        }

        // Bottom circuit traces (reversed: ┴ row nearest logo, ┬ row at edge)
        let bottom_y = inner.y + inner.height.saturating_sub(trace_h);
        for (i, t) in TRACES.iter().rev().enumerate() {
            let y = bottom_y + i as u16;
            if y >= inner.y + inner.height { break; }
            frame.render_widget(
                Paragraph::new(Span::styled(*t, dim)),
                Rect { x: inner.x, y, width: inner.width, height: 1 },
            );
        }
    }
}
