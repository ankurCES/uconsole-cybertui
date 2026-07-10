use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct StorageScreenV2 {
    selected: usize,
}

impl Default for StorageScreenV2 {
    fn default() -> Self { Self { selected: 0 } }
}

impl ScreenV2 for StorageScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Storage }
    fn title(&self) -> &str { "Storage" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        let count = ctx.live.filesystems.try_read().map(|v| v.len()).unwrap_or(0);
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Back => { ctx.go_back(); Consumed::Yes }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let block = Block::default()
            .title(Span::styled(" Storage ", theme.title()))
            .borders(Borders::ALL)
            .border_style(theme.border(true));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let items: Vec<ListItem<'static>> = if let Ok(fss) = ctx.live.filesystems.try_read() {
            if fss.is_empty() {
                vec![ListItem::new(Line::from(Span::styled("  (no filesystems detected)", theme.dim())))]
            } else {
                fss.iter().map(|fs| {
                    let bar_width = 20usize;
                    let filled = (fs.use_pct as usize * bar_width / 100).min(bar_width);
                    let bar: String = format!("[{}{}]",
                        "█".repeat(filled),
                        "░".repeat(bar_width - filled));
                    let pct_style = if fs.use_pct > 90 {
                        theme.error()
                    } else if fs.use_pct > 75 {
                        theme.warn()
                    } else {
                        theme.ok()
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("{:<20}", trunc(&fs.mounted_on, 20)), Style::default().fg(theme.fg)),
                        Span::styled(bar, pct_style),
                        Span::styled(format!(" {:>3}% ", fs.use_pct), pct_style),
                        Span::styled(format!("{}/{}", fs.used, fs.size), theme.dim()),
                    ]);
                    ListItem::new(line)
                }).collect()
            }
        } else {
            vec![ListItem::new(Line::from(Span::styled("  (refreshing…)", theme.dim())))]
        };

        let count = items.len();
        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, inner, &mut list_state);
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}
