use std::path::PathBuf;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::screen::{ScreenId, ScreenV2, Zone};
use crate::nav::event::{Consumed, NavEvent};
use crate::nav::UiContext;

pub struct FilesScreenV2 {
    cwd: PathBuf,
    entries: Vec<(String, bool)>, // (name, is_dir)
    selected: usize,
    loaded: bool,
}

impl Default for FilesScreenV2 {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_string());
        Self {
            cwd: PathBuf::from(home),
            entries: Vec::new(),
            selected: 0,
            loaded: false,
        }
    }
}

impl FilesScreenV2 {
    fn load_entries(&mut self) {
        self.entries.clear();
        if self.cwd.parent().is_some() {
            self.entries.push(("..".to_string(), true));
        }
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            let mut dirs: Vec<String> = Vec::new();
            let mut files: Vec<String> = Vec::new();
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir { dirs.push(name); } else { files.push(name); }
            }
            dirs.sort();
            files.sort();
            for d in dirs { self.entries.push((d, true)); }
            for f in files { self.entries.push((f, false)); }
        }
        self.selected = 0;
        self.loaded = true;
    }

    fn enter_selected(&mut self) {
        let sel = self.selected.min(self.entries.len().saturating_sub(1));
        if let Some((name, is_dir)) = self.entries.get(sel) {
            if *is_dir {
                if name == ".." {
                    if let Some(p) = self.cwd.parent() {
                        self.cwd = p.to_path_buf();
                    }
                } else {
                    self.cwd = self.cwd.join(name);
                }
                self.load_entries();
            }
        }
    }
}

impl ScreenV2 for FilesScreenV2 {
    fn id(&self) -> ScreenId { ScreenId::Files }
    fn title(&self) -> &str { "Files" }
    fn focusable_zones(&self) -> &[Zone] { &[Zone::Main] }
    fn hint(&self) -> &str { "▲▼ scroll   A open dir   B back" }

    fn on_nav(&mut self, event: NavEvent, ctx: &mut UiContext<'_>) -> Consumed {
        if !self.loaded { self.load_entries(); }
        let count = self.entries.len();
        match event {
            NavEvent::Up => {
                self.selected = self.selected.saturating_sub(1);
                Consumed::Yes
            }
            NavEvent::Down if count > 0 => {
                self.selected = (self.selected + 1).min(count - 1);
                Consumed::Yes
            }
            NavEvent::Confirm => {
                self.enter_selected();
                Consumed::Yes
            }
            NavEvent::Back => {
                if self.cwd.parent().is_some() {
                    if let Some(p) = self.cwd.parent() {
                        self.cwd = p.to_path_buf();
                    }
                    self.load_entries();
                    Consumed::Yes
                } else {
                    ctx.go_back();
                    Consumed::Yes
                }
            }
            _ => Consumed::No,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &UiContext<'_>) {
        let theme = &ctx.ui.theme;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        // Path header
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  ", theme.dim()),
                Span::styled(self.cwd.to_string_lossy().to_string(), Style::default().fg(theme.accent)),
            ])).block(Block::default()
                .title(Span::styled(" Files ", theme.title()))
                .borders(Borders::ALL)
                .border_style(theme.border(false))),
            chunks[0],
        );

        // Entry list
        let items: Vec<ListItem<'static>> = if self.entries.is_empty() {
            vec![ListItem::new(Line::from(Span::styled("  (empty directory)", theme.dim())))]
        } else {
            self.entries.iter().map(|(name, is_dir)| {
                let (glyph, style) = if *is_dir {
                    ("▶ ", theme.ok())
                } else {
                    ("  ", Style::default().fg(theme.fg))
                };
                ListItem::new(Line::from(vec![
                    Span::styled(glyph, style),
                    Span::styled(name.clone(), style),
                ]))
            }).collect()
        };

        let count = items.len();
        let sel = self.selected.min(count.saturating_sub(1));
        let mut list_state = ListState::default().with_selected(if count == 0 { None } else { Some(sel) });
        let list = List::new(items)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border(true)))
            .highlight_style(Style::default().fg(theme.selection_fg).bg(theme.selection_bg))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, chunks[1], &mut list_state);
    }
}
