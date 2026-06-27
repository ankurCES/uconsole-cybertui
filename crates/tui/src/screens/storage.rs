//! Storage screen: df + lsblk summary.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::screen::{Screen, ScreenId};
use crate::app::App;
use crate::theme::Theme;

pub struct StorageScreen;

impl Screen for StorageScreen {
    fn id(&self) -> ScreenId {
        ScreenId::Storage
    }
    fn title(&self) -> &'static str {
        "Storage"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, app: &mut App, theme: &Theme, focus: bool) {
        let block = Block::default()
            .title(Span::styled(" Storage ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(focus));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut items: Vec<ListItem> = Vec::new();
        items.push(ListItem::new(Line::from(Span::styled(
            format!(
                "  {:<24} {:<8} {:<6} {:<6} {:<6} {:<4}  {}",
                "source", "fstype", "size", "used", "avail", "use%", "mount"
            ),
            theme.title(),
        ))));
        if let Ok(fs) = app.live.filesystems.try_read() {
            for m in fs.iter() {
                let style = if m.use_pct > 90 {
                    theme.error()
                } else if m.use_pct > 75 {
                    theme.warn()
                } else {
                    ratatui::style::Style::default().fg(theme.fg)
                };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(format!("{:<24}", m.source), theme.fg),
                    Span::styled(format!("{:<8}", m.fstype), theme.dim()),
                    Span::styled(format!("{:<6}", m.size), theme.fg),
                    Span::styled(format!("{:<6}", m.used), theme.fg),
                    Span::styled(format!("{:<6}", m.avail), theme.fg),
                    Span::styled(format!("{:<4}", format!("{}%", m.use_pct)), style),
                    Span::styled(format!("  {}", m.mounted_on), theme.accent),
                ])));
                let bar = usage_bar(m.use_pct);
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", theme.dim()),
                    Span::styled(format!("  {bar}"), style),
                ])));
            }
        }
        if items.len() == 1 {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (no filesystems reported)",
                theme.dim(),
            ))));
        }
        let list = List::new(items).block(Block::default().borders(Borders::NONE));
        f.render_widget(list, inner);

        // Footer hint.
        let hints = Paragraph::new(Line::from(Span::styled(
            "  mount/unmount actions coming next milestone",
            theme.dim(),
        )));
        let hint_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        f.render_widget(hints, hint_area);
    }
}

fn usage_bar(pct: u8) -> String {
    let filled = (pct as usize) / 5; // 0..=20
    let empty = 20 - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}
