use std::io::Stdout;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tokio::sync::mpsc;

use crate::app::action::Action;
use crate::app::live_data::LiveData;
use crate::app::screen::{ScreenId, ScreenRegistry};
use crate::app::state::AppState;
use crate::modal::render_modal_overlay;
use crate::nav::dispatch::dispatch_key;
use crate::nav::menu_stack::MenuStack;
use crate::nav::UiContext;
use crate::prefs::Prefs;
use crate::screens;

type Tui = ratatui::Terminal<CrosstermBackend<Stdout>>;

/// v2 event loop. Creates its own AppState + ScreenRegistry, starts
/// background refreshers, and runs until the user quits.
pub async fn run_v2(terminal: &mut Tui) -> anyhow::Result<()> {
    let prefs = Prefs::load();
    let live = Arc::new(LiveData::default());
    let (tx, rx) = mpsc::channel::<Action>(256);
    // Keep a clone for dispatch_key; passing &state.tx inside draw_v2 while
    // also holding &mut state would split-borrow the struct.
    let dispatch_tx = tx.clone();

    live.spawn_refreshers(tx.clone());

    let mut state = AppState::new(prefs, live, tx, rx);
    // v2 root is MainMenu, not Overworld.
    state.nav.stack = MenuStack::with_root(ScreenId::MainMenu);

    let mut screens = build_registry();
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            terminal
                .draw(|f| draw_v2(f, &mut state, &mut screens, &dispatch_tx))
                .context("terminal draw")?;
            needs_redraw = false;
        }

        tokio::select! {
            maybe = state.rx.recv() => {
                match maybe {
                    Some(Action::Quit) | None => return Ok(()),
                    Some(_) => {}
                }
                // Drain burst: coalesce multiple refresher ticks into one redraw.
                while let Ok(a) = state.rx.try_recv() {
                    if matches!(a, Action::Quit) { return Ok(()); }
                }
                needs_redraw = true;
            }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                if event::poll(Duration::from_millis(0)).context("event poll")? {
                    match event::read().context("event read")? {
                        Event::Key(key) => {
                            dispatch_key(key, &mut state, &mut screens, &dispatch_tx);
                            needs_redraw = true;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn draw_v2(
    f: &mut Frame,
    state: &mut AppState,
    screens: &mut ScreenRegistry,
    tx: &mpsc::Sender<Action>,
) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // content (rows 0-21)
            Constraint::Length(1), // hint bar  (row 22)
            Constraint::Length(1), // status bar (row 23)
        ])
        .split(area);
    let (content, hint_row, status_row) = (chunks[0], chunks[1], chunks[2]);

    let current_id = state.nav.stack.current();

    // Grab hint string before UiContext borrows state.ui/nav.
    let hint = screens
        .get_mut(current_id)
        .map(|s| s.hint().to_owned())
        .unwrap_or_default();

    // Clone sender so we can pass &tx to UiContext without split-borrowing state.
    let tx_ref = tx;

    // Screen render — UiContext holds &mut UiState and &mut NavigationState.
    {
        let ctx = UiContext {
            live:  &state.live,
            prefs: &state.prefs,
            ui:    &mut state.ui,
            nav:   &mut state.nav,
            tx:    tx_ref,
        };
        if let Some(screen) = screens.get_mut(current_id) {
            screen.render(f, content, &ctx);
        }
    } // ctx + screen borrows drop here

    let dim = state.ui.theme.dim;

    // Hint bar
    f.render_widget(
        Paragraph::new(Span::raw(hint)).style(Style::default().fg(dim)),
        hint_row,
    );

    // Status/clock bar
    let clock = state.ui.clock.format("%H:%M:%S").to_string();
    let status = state.ui.status_msg.as_deref().unwrap_or("");
    let bar = if status.is_empty() {
        clock
    } else {
        format!("{clock}  {status}")
    };
    f.render_widget(
        Paragraph::new(Span::raw(bar)).style(Style::default().fg(dim)),
        status_row,
    );

    // Modal overlay — rendered last so it sits on top.
    if let Some(modal) = state.ui.modal.as_ref() {
        render_modal_overlay(f, area, modal.as_ref(), &state.ui.theme);
    }
}

fn build_registry() -> ScreenRegistry {
    use screens::{
        city_v2::CityScreenV2,
        intel_v2::IntelScreenV2,
        lora_v2::LoraScreenV2,
        main_menu::MainMenuScreen,
        network_v2::NetworkScreenV2,
        recon_v2::ReconScreenV2,
        stubs_v2::{
            AudioScreenV2, BluetoothScreenV2, DisplayScreenV2, EditorScreenV2, FilesScreenV2,
            LogsScreenV2, PackagesScreenV2, PowerScreenV2, ProcessesScreenV2, ServicesScreenV2,
            SettingsScreenV2, StorageScreenV2,
        },
        submenu::SubMenuScreen,
        system_v2::SystemScreenV2,
    };

    let mut r = ScreenRegistry::new();
    r.register(Box::new(MainMenuScreen::default()));
    r.register(Box::new(SubMenuScreen::default()));
    r.register(Box::new(SystemScreenV2::default()));
    r.register(Box::new(NetworkScreenV2::default()));
    r.register(Box::new(LoraScreenV2::default()));
    r.register(Box::new(IntelScreenV2::default()));
    r.register(Box::new(ReconScreenV2::default()));
    r.register(Box::new(CityScreenV2::default()));
    r.register(Box::new(BluetoothScreenV2));
    r.register(Box::new(PowerScreenV2));
    r.register(Box::new(DisplayScreenV2));
    r.register(Box::new(AudioScreenV2));
    r.register(Box::new(StorageScreenV2));
    r.register(Box::new(PackagesScreenV2));
    r.register(Box::new(ProcessesScreenV2));
    r.register(Box::new(ServicesScreenV2));
    r.register(Box::new(FilesScreenV2));
    r.register(Box::new(LogsScreenV2));
    r.register(Box::new(SettingsScreenV2));
    r.register(Box::new(EditorScreenV2));
    r
}
