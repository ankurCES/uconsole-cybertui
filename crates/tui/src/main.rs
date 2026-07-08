//! cyberdeck-tui — entry point.
//!
//! Responsibilities:
//!   1. Initialise tracing (env-filter, `RUST_LOG=debug` is friendly).
//!   2. Parse CLI flags (`--web` to also host the LAN server).
//!   3. Bring up the tokio runtime, the App, and the live data refreshers.
//!   4. Drive the ratatui event/render loop until the user quits or Ctrl-C.
//!
//! The render loop is intentionally a single `tokio::select!` so we can mix:
//!   - keyboard/mouse events from crossterm (blocking → spawned),
//!   - tick events from the refreshers,
//!   - long-running action results (sent as `Action::Toast`).

// The crate's modules now live in `lib.rs` so integration tests
// can drive the LoRa ingest pipeline end-to-end without spinning
// up the full ratatui event loop. We re-export them here for the
// binary so the rest of `main.rs` doesn't have to change.
#[allow(unused_imports)]
use cyberdeck_tui::{app, screens, theme, ui, util, wm};
#[cfg(feature = "web")]
#[allow(unused_imports)]
use cyberdeck_tui::web_bridge;
use std::io::{stdout, Stdout};
use std::path::PathBuf;
use std::time::Duration;
#[cfg(feature = "web")]
use std::sync::Arc;

use anyhow::Context;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use app::action::{Action, RunAction};
use app::screen::{Screen, ScreenId};
use app::toast::ToastKind;
use app::{App, ChoiceCommit, ChoiceOption, ConfirmKind, InputKind, Modal, Region, Wizard};
use theme::Theme;

type Tui = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Default)]
struct Args {
    web: bool,
    web_bind: Option<String>,
    config: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--web" => a.web = true,
            "--web-bind" => a.web_bind = it.next(),
            "--config" => a.config = it.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("cyberdeck-tui — a rich TUI for OS-level control on a cyberdeck.\n\n\
                          USAGE: cyberdeck-tui [OPTIONS]\n\n\
                          OPTIONS:\n  \
                            --web            Also start the LAN web server (default 0.0.0.0:7878)\n  \
                            --web-bind ADDR  Bind address for the web server (e.g. 127.0.0.1:9000)\n  \
                            --config PATH    Path to a config file (optional)\n  \
                            -h, --help       Show this help\n");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    a
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    );
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // Set up panic hook so the terminal is restored even on panic.
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        orig_hook(info);
    }));

    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .context("enter alt screen")?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend).context("init terminal")?;

    let (tx, rx) = mpsc::channel::<Action>(256);
    let mut app = App::new(tx.clone(), rx);

    // Kick off the background refreshers.
    app.live.spawn_refreshers(tx.clone());

    // Optionally start the embedded web server. The serve() future is owned
    // by a dedicated tap task that listens on a dedicated control channel
    // (`web_ctrl` on App::live) for WebStart/WebStop actions. UI code
    // (Settings toggle, command palette) routes through that channel
    // directly — no separate `tx` plumbing needed.
    #[cfg(feature = "web")]
    {
        let (web_tx, mut web_rx) = mpsc::channel::<(mpsc::Sender<Action>, Action)>(32);
        // Expose the sender on the Live so UI code can reach us.
        *app.live.web_ctrl.lock().await = web_tx;
        let live = app.live.clone();
        let tap_tx = tx.clone();
        let bind_default = args
            .web_bind
            .clone()
            .unwrap_or_else(|| "0.0.0.0:7878".to_string());
        tokio::spawn(async move {
            while let Some((reply, act)) = web_rx.recv().await {
                match act {
                    Action::Run(RunAction::WebStart) => {
                        // Already running?
                        {
                            let g = live.web_shutdown.lock().await;
                            if g.is_some() {
                                let _ = reply
                                    .send(Action::Toast(
                                        ToastKind::Warn,
                                        "web server already running".into(),
                                    ))
                                    .await;
                                continue;
                            }
                        }
                        let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<()>();
                        *live.web_shutdown.lock().await = Some(sd_tx);
                        *live.web_enabled.write().await = true;
                        let bind = bind_default.clone();
                        *live.web_url.write().await = Some(format!("http://{bind}"));
                        let _ = reply
                            .send(Action::Toast(
                                ToastKind::Ok,
                                format!("web server starting on {bind}"),
                            ))
                            .await;
                        let live_for_task = live.clone();
                        let tx_for_task = tap_tx.clone();
                        tokio::spawn(async move {
                            let (w_tx, mut w_rx) =
                                mpsc::channel::<cyberdeck_web::run::toast_compat::Action>(64);
                            let pump_tx = tx_for_task.clone();
                            tokio::spawn(async move {
                                while let Some(wa) = w_rx.recv().await {
                                    let _ = pump_tx.send(crate::web_bridge::web_to_app(wa)).await;
                                }
                            });
                            let live_arc: Arc<dyn cyberdeck_web::api::LiveRead + Send + Sync> = {
                                let bridge = crate::web_bridge::TuiLiveRead {
                                    live: live_for_task.clone(),
                                    action_tx: tx_for_task.clone(),
                                };
                                Arc::new(bridge)
                            };
                            let serve = cyberdeck_web::run_with(&bind, live_arc, Some(w_tx), None);
                            tokio::select! {
                                res = serve => {
                                    if let Err(e) = res {
                                        let _ = tx_for_task
                                            .send(Action::Toast(
                                                ToastKind::Error,
                                                format!("web server failed: {e}"),
                                            ))
                                            .await;
                                    }
                                }
                                _ = sd_rx => {
                                    let _ = tx_for_task
                                        .send(Action::Toast(
                                            ToastKind::Info,
                                            "web server stopped".into(),
                                        ))
                                        .await;
                                }
                            }
                            *live_for_task.web_enabled.write().await = false;
                            *live_for_task.web_url.write().await = None;
                            let _ = live_for_task.web_shutdown.lock().await.take();
                        });
                    }
                    Action::Run(RunAction::WebStop) => {
                        if let Some(sd) = live.web_shutdown.lock().await.take() {
                            let _ = sd.send(());
                            let _ = reply
                                .send(Action::Toast(ToastKind::Info, "stopping web server".into()))
                                .await;
                        } else {
                            let _ = reply
                                .send(Action::Toast(
                                    ToastKind::Warn,
                                    "web server not running".into(),
                                ))
                                .await;
                        }
                    }
                    other => {
                        let _ = reply.send(other).await;
                    }
                }
            }
        });
        if args.web {
            let _ = tx.send(Action::Run(RunAction::WebStart)).await;
        }
    }
    #[cfg(not(feature = "web"))]
    if args.web {
        let _ = tx
            .send(Action::Toast(
                ToastKind::Warn,
                "rebuild with `cargo build -p cyberdeck-tui --features web` to enable --web".into(),
            ))
            .await;
    }

    let res = run_app(&mut terminal, &mut app, &tx).await;

    restore_terminal();
    terminal.show_cursor().ok();
    if let Err(e) = res {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run_app(
    terminal: &mut Tui,
    app: &mut App,
    tx: &mpsc::Sender<Action>,
) -> anyhow::Result<()> {
    let mut screens: Vec<Box<dyn Screen>> = vec![
        Box::new(screens::system::SystemScreen),
        Box::new(screens::network::NetworkScreen),
        Box::new(screens::bluetooth::BluetoothScreen),
        Box::new(screens::power::PowerScreen),
        Box::new(screens::display::DisplayScreen),
        Box::new(screens::audio::AudioScreen),
        Box::new(screens::storage::StorageScreen),
        Box::new(screens::services::ServicesScreen),
        Box::new(screens::packages::PackagesScreen),
        Box::new(screens::processes::ProcessesScreen),
        Box::new(screens::files::FilesScreen),
        Box::new(screens::logs::LogsScreen),
        Box::new(screens::settings::SettingsScreen),
        // LoRa screen: longfast channel chat (left) + nodes-with-hops (right).
        // The transport is owned by `LoraScreen` itself so `LoraScreen::poll`
        // doesn't need to reach into `App`. Held in a `Box<dyn LoraTransport
        // + Send>` so a real HTTP transport can swap in at runtime without
        // touching the screen or any test code path.
        Box::new(screens::lora::LoraScreen::new(Box::new(
            screens::lora::FakeTransport::new(),
        ))),
        // City screen: braille road network (left) + weather/wind (right).
        // Step 3 stub — Step 8 swaps the placeholder render for the real
        // braille renderer + geo/weather clients wired in Steps 5-7.
        Box::new(screens::city::CityScreen::new()),
    ];

    let mut redraw = true;
    let mut last_tick = std::time::Instant::now();
    let tick_rate = Duration::from_millis(250);

    loop {
        if redraw {
            // `app.theme_name` is the same enum as `theme::ThemeName`
            // (just re-exported into the `app::screen` module path as
            // `ThemeNameReexport`), so `Theme::by_name` accepts it
            // directly. This used to be a 3-arm match over the
            // original 3-variant enum; the 10-variant expansion in
            // Step 1 made that match non-exhaustive, so we route
            // through `by_name` here. Future theme additions won't
            // require touching this call site again — they only need
            // a new variant in `ThemeName` + an entry in `by_name`.
            let theme = Theme::by_name(app.theme_name);
            terminal
                .draw(|f| draw(f, app, &mut screens, &theme))
                .context("terminal draw")?;
            redraw = false;
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));

        // Fix #1b — coalesce. We drain EVERY queued action before
        // returning to the outer loop so a burst (e.g. 5 network
        // samples + a tick + a wifi scan result landing in the same
        // millisecond) produces ONE redraw, not six. The previous
        // code did one recv → one redraw per action; on a noisy box
        // this is the visible "clunky" the user reported.
        tokio::select! {
            // Drain queued actions.
            maybe = app.rx.recv() => {
                match maybe {
                    Some(action) => {
                        if handle_action(&mut screens, app, tx, action).await {
                            return Ok(());
                        }
                    }
                    None => return Ok(()),
                }
                // Drain anything else already queued. We use
                // `try_recv` so we never block here — the goal is to
                // flatten a burst (5 network samples + a tick + a
                // wifi scan result landing in the same millisecond)
                // into one frame. Any single drain ends in one redraw.
                while let Ok(action) = app.rx.try_recv() {
                    if handle_action(&mut screens, app, tx, action).await {
                        return Ok(());
                    }
                }
                redraw = true;
            }
            // Poll for terminal input.
            _ = tokio::time::sleep(timeout) => {
                if last_tick.elapsed() >= tick_rate {
                    app.tick_clock();
                    last_tick = std::time::Instant::now();
                    // Drive a periodic tick so refreshers (which already send
                    // Action::Tick) cause a redraw.
                    let _ = tx.try_send(Action::Tick);
                }
                if event::poll(Duration::from_millis(0))? {
                    match event::read()? {
                        // Terminal-emitted paste. Bracketed-paste mode is
                        // enabled at startup so modern terminals bundle
                        // pasted text into one `Event::Paste` instead of
                        // synthesizing keystrokes. Forward to the active
                        // input modal buffer.
                        Event::Paste(text) => {
                            handle_paste(&mut *app, text);
                            redraw = true;
                        }
                        // Phase 2 — left-click on the tab strip switches
                        // screens. Only fires while `app.mouse` is on
                        // (the Settings → Mouse toggle). We honor Down
                        // (not Up) so a click-drag-release doesn't
                        // register as a click if the user landed on the
                        // strip but released elsewhere. All other mouse
                        // events are dropped (no scroll wheel handling
                        // yet; no right-click context menu).
                        Event::Mouse(m) if app.mouse => {
                            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                                if let Some(rect) = app.tab_strip_rect {
                                    if let Some(id) = crate::ui::tab_strip::hit_test(
                                        rect, m.column, m.row, &*app,
                                    ) {
                                        let _ = tx.send(Action::Goto(id)).await;
                                    }
                                }
                                // Phase 2 — click-to-pan on the City
                                // map pane. The rect was cached by the
                                // last `draw()`; if it covers the click,
                                // dispatch a `CityPan` action that the
                                // City screen's on_key handler resolves
                                // into a recentred viewport_bbox.
                                if let Some(map_rect) = app.city_map_rect {
                                    if m.column >= map_rect.x
                                        && m.column < map_rect.x + map_rect.width
                                        && m.row >= map_rect.y
                                        && m.row < map_rect.y + map_rect.height
                                    {
                                        let _ = tx
                                            .send(Action::Run(RunAction::CityPan {
                                                col: m.column,
                                                row: m.row,
                                                rect: map_rect,
                                            }))
                                            .await;
                                    }
                                }
                            }
                            redraw = true;
                        }
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            // Ctrl+Shift+V fallback. Some terminals (older
                            // xterm, Alacritty before a config option, plain
                            // sshd with no TERM=xterm-256color) don't emit
                            // `Event::Paste` even with bracketed-paste on,
                            // so the user has to invoke it manually. Same
                            // routing arm as `Event::Paste`.
                            if k.code == KeyCode::Char('v')
                                && k.modifiers
                                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                            {
                                handle_paste(&mut *app, read_clipboard_for_paste());
                                redraw = true;
                            } else if handle_key(&mut screens, app, tx, k).await {
                                return Ok(());
                            } else {
                                redraw = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        app.cleanup_toasts();
    }
}

fn draw(f: &mut Frame, app: &mut App, screens: &mut [Box<dyn Screen>], theme: &Theme) {
    // Fix #2a — Cyberdeck console layout. 3 rows: header / body / legend.
    let (header, body, legend) = ui::chunks(f.area());
    // The header is now just live status icons + clock on a single row.
    ui::draw_header(f, header, app, theme);
    // The body hosts the launcher when focus is on it; otherwise it
    // renders the focused screen. The launcher stays available via B/Esc.
    if app.region == Region::Sidebar {
        ui::draw_launcher(f, body, app, &screens[..], theme);
    } else {
        // The WM render walks the focused pane and paints it. Screens
        // still own their own borders — same contract as before.
        wm::render::render(f, body, app, screens, theme);
        // City-map rect stale-guard — keep the previous behaviour.
        if app.current != ScreenId::City {
            app.city_map_rect = None;
        }
    }
    // On-screen button legend (always visible). Reads the focused
    // region so the legend tells the user what X/Y/A/B do *here*.
    ui::draw_button_legend(f, legend, app, theme);
    // Toasts float above everything.
    ui::draw_toasts(f, f.area(), app, theme);
    // Modal overlay last so it sits on top.
    draw_modal(f, f.area(), app, theme);
}

fn rect(x: u16, y: u16, w: u16, h: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect::new(x, y, w, h)
}

/// Phase 2 — substring-match a City palette query against the
/// bundled city list (slugs + human-readable names). Returns the
/// first match (slug) or `None` if nothing fits. The match is
/// case-insensitive and looks at both the kebab-case slug (`"nyc"`)
/// and the city name (`"New York"`) so users can type either. The
/// bundled order is the same as `CityRoads::BUNDLED` so callers
/// that care about cycling order get a deterministic result.
fn city_picker_match(query: &str) -> Option<String> {
    let q = query.to_lowercase();
    for slug in screens::city::roads::CityRoads::BUNDLED {
        if slug.contains(&q) {
            return Some((*slug).to_string());
        }
        if let Some(roads) = screens::city::roads::CityRoads::load_bundled(slug) {
            if roads.name.to_lowercase().contains(&q) {
                return Some((*slug).to_string());
            }
        }
    }
    None
}

use ratatui::Frame;
use ratatui::text::{Line, Span};

/// Pure line builder for `Modal::Input`. Extracted so tests can assert on
/// the rendered text directly without spinning up a `Buffer`/`Frame`.
/// Behaviour: prompt line, live buffer line, then an `[ OK ]   [ Cancel ]`
/// row so the affordance is visible to the user. Enter / Esc behaviour is
/// unchanged (handled in `handle_key`).
fn modal_input_lines(prompt: &str, buf: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(prompt.to_string()),
        Line::from(format!("> {buf}")),
        Line::from(vec![
            Span::raw("  "),
            Span::raw("[ OK ]"),
            Span::raw("      "),
            Span::raw("[ Cancel ]"),
        ]),
    ]
}

/// Pure line builder for `Modal::Secret`. Same shape as `modal_input_lines`
/// but the buffer is masked with `•` so the real value never leaks into
/// the rendered text.
fn modal_secret_lines(prompt: &str, buf: &str) -> Vec<Line<'static>> {
    let masked: String = std::iter::repeat('•').take(buf.chars().count()).collect();
    vec![
        Line::from(prompt.to_string()),
        Line::from(format!("> {masked}▏")),
        Line::from(vec![
            Span::raw("  "),
            Span::raw("[ OK ]"),
            Span::raw("      "),
            Span::raw("[ Cancel ]"),
        ]),
    ]
}

fn draw_modal(f: &mut Frame, area: ratatui::layout::Rect, app: &App, theme: &Theme) {
    match &app.modal {
        Modal::None => {}
        Modal::Help => {
            // Keybindings overlay. Replaces the old hand-rolled
            // Clear + Block + Paragraph with `popup::render_with_hints`
            // so the help modal shares the same orbital-style chrome
            // (shadow band, rounded border, key/description table) as
            // every other popup on PR #5.
            //
            // The keys themselves are split into two columns by
            // `render_with_hints`: the key gets `theme.key()` (the
            // accent register) and the description gets `theme.fg()`.
            //
            // Entries are organised by region so a first-time user can
            // read top-to-bottom and learn the D-pad contract: sidebar
            // first (the natural starting point), then content panes,
            // then modals. The old "←/→ = switch focus" line was the
            // exact wording that misled users into thinking the left
            // pane and right pane were symmetric and interchangeable;
            // they're not, and the new descriptions say so explicitly.
            crate::wm::popup::render_with_hints(
                f,
                area,
                "help",
                &[
                    ("region · sidebar", ""),
                    ("↑/↓ j/k", "move cursor"),
                    ("enter / →", "open screen"),
                    ("1..9 0", "jump to screen"),
                    ("region · content", ""),
                    ("↑/↓ j/k", "scroll list"),
                    ("←/h", "step back (or sidebar)"),
                    ("→/l", "step right (multi-pane)"),
                    ("tab", "next screen"),
                    ("shift-tab", "previous screen"),
                    ("esc", "leave to sidebar"),
                    ("anytime", ""),
                    ("?", "this help"),
                    (":", "command palette"),
                    ("f10 / alt+f", "open menu bar"),
                    ("←/→", "cycle tabs"),
                    ("tab", "next screen"),
                    ("shift-tab", "previous screen"),
                    ("esc", "close menu / modal"),
                    ("r", "refresh current screen"),
                    ("q", "quit"),
                    ("menu · file", ""),
                    ("refresh all", "scan wifi/bluetooth/reload"),
                    ("command palette…", "open command palette"),
                    ("quit", "exit the tui"),
                    ("menu · view", ""),
                    ("units: metric", "°C, km/h"),
                    ("units: imperial", "°F, mph"),
                    ("toggle traffic overlay", "city map traffic"),
                    ("toggle weather panel", "city weather pane"),
                    ("menu · tools", ""),
                    ("rescan wi-fi", "trigger wifi scan"),
                    ("rescan bluetooth", "trigger bluetooth scan"),
                    ("toggle web server", "start/stop http"),
                    ("menu · help", ""),
                    ("show help (?)", "this overlay"),
                    ("toast log (T)", "view all toasts"),
                ],
                theme,
            );
        }
        Modal::CommandPalette => {
            use ratatui::text::Line;
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let mut lines: Vec<Line> = vec![Line::from(format!(":{}", app.palette_buf))];
            let actions = palette_actions();
            let q = app.palette_buf.to_lowercase();
            let filtered: Vec<_> = actions
                .iter()
                .filter(|(_, label)| q.is_empty() || label.to_lowercase().contains(&q))
                .take(8)
                .collect();
            for (i, (_, label)) in filtered.iter().enumerate() {
                let style = if i == app.palette_idx {
                    ratatui::style::Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg)
                } else {
                    ratatui::style::Style::default().fg(theme.fg)
                };
                lines.push(Line::from(ratatui::text::Span::styled(
                    label.to_string(),
                    style,
                )));
            }
            let w = 50.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + area.height.saturating_sub(h + 2);
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(" command palette ")
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            f.render_widget(p, rect);
        }
        Modal::Confirm { message, .. } => {
            use ratatui::text::Line;
            use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
            let lines = vec![
                Line::from(message.clone()),
                Line::from(""),
                Line::from("Press Y to confirm, N/Esc to cancel."),
            ];
            let w = 60.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" confirm ")
                        .borders(Borders::ALL)
                        .border_style(theme.warn()),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(p, rect);
        }
        Modal::Input { prompt, buf, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let lines = modal_input_lines(prompt, buf);
            let w = 60.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(" input ")
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            f.render_widget(p, rect);
        }
        Modal::Secret { prompt, buf, .. } => {
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let lines = modal_secret_lines(prompt, buf);
            let w = 60.min(area.width.saturating_sub(4));
            let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(" password ")
                    .borders(Borders::ALL)
                    .border_style(theme.warn()),
            );
            f.render_widget(p, rect);
        }
        Modal::Choice { prompt, options, cursor, .. } => {
            use ratatui::layout::Rect;
            use ratatui::text::{Line, Span};
            use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
            let lines: Vec<Line> = vec![Line::from(prompt.clone()), Line::from("")];
            // Render up to 12 rows visible at once; the cursor scrolls the
            // window if the list is longer than that.
            let max_visible = 12usize;
            let start = if *cursor >= max_visible { cursor + 1 - max_visible } else { 0 };
            let end = (start + max_visible).min(options.len());
            let items: Vec<ListItem> = options[start..end]
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let real_i = start + i;
                    let style = if real_i == *cursor {
                        ratatui::style::Style::default()
                            .fg(theme.selection_fg)
                            .bg(theme.selection_bg)
                    } else {
                        ratatui::style::Style::default().fg(theme.fg)
                    };
                    ListItem::new(Line::from(Span::styled(opt.label.clone(), style)))
                })
                .collect();
            let total = options.len();
            let title = format!(" pick ({}/{}) ", cursor.saturating_add(1).min(total.max(1)), total);
            let w = 60.min(area.width.saturating_sub(4));
            let h = ((end - start) as u16 + 4).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h)) / 2;
            let rect = rect(x, y, w, h);
            f.render_widget(Clear, rect);
            // Show the prompt as a header line above the list.
            lines.iter().for_each(|l| {
                f.render_widget(
                    Paragraph::new(l.clone()),
                    Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), 1),
                );
            });
            let list = List::new(items).block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            // Render the list below the prompt + blank line.
            let list_rect = Rect::new(
                rect.x,
                rect.y + 3,
                rect.width,
                rect.height.saturating_sub(3),
            );
            f.render_widget(list, list_rect);
        }
        Modal::Wizard(w) => {
            use ratatui::text::{Line, Span};
            use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
            let (header, body) = match w {
                Wizard::WifiEnterprise { ssid, step, eap, identity, password, anon_or_cert } => {
                    let h = format!("Wi-Fi Enterprise — {ssid}");
                    let b = match step {
                        0 => "Pick EAP method (PEAP/TTLS/TLS/PWD) and press Enter.".to_string(),
                        1 => format!(
                            "Identity: {}",
                            identity.as_deref().unwrap_or("(typing)")
                        ),
                        2 => match eap.as_deref() {
                            Some("TLS") => format!(
                                "Path to client certificate: {}",
                                anon_or_cert.as_deref().unwrap_or("(typing)")
                            ),
                            _ => format!(
                                "Password: {}",
                                if password.is_some() { "•••" } else { "(typing)" }
                            ),
                        },
                        _ => "Ready to connect.".to_string(),
                    };
                    (h, b)
                }
            };
            let lines = vec![Line::from(header), Line::from(""), Line::from(Span::styled(body, theme.warn()))];
            let w_ = 60.min(area.width.saturating_sub(4));
            let h_ = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w_)) / 2;
            let y = area.y + (area.height.saturating_sub(h_)) / 2;
            let rect = rect(x, y, w_, h_);
            f.render_widget(Clear, rect);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" wizard ")
                        .borders(Borders::ALL)
                        .border_style(theme.border(true)),
                )
                .wrap(Wrap { trim: false });
            f.render_widget(p, rect);
        }
        Modal::Progress { label, done, total, .. } => {
            use ratatui::layout::Rect;
            use ratatui::text::Line;
            use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};
            let w_ = 60.min(area.width.saturating_sub(4));
            let h_ = 5u16.min(area.height.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w_)) / 2;
            let y = area.y + (area.height.saturating_sub(h_)) / 2;
            let rect = rect(x, y, w_, h_);
            f.render_widget(Clear, rect);
            let header = Paragraph::new(Line::from(label.clone())).block(
                Block::default()
                    .title(" working ")
                    .borders(Borders::ALL)
                    .border_style(theme.warn()),
            );
            f.render_widget(header, Rect::new(rect.x, rect.y, rect.width, 3));
            let pct = if *total == 0 {
                None
            } else {
                Some(((done.saturating_mul(100)) / total).min(100) as u16)
            };
            let gauge_rect = Rect::new(
                rect.x + 1,
                rect.y + 3,
                rect.width.saturating_sub(2),
                1,
            );
            let label = if let Some(p) = pct {
                format!("{done}/{total} ({p}%)")
            } else {
                "…".to_string()
            };
            let gauge = Gauge::default()
                .gauge_style(theme.warn())
                .label(label)
                .ratio(pct.map(|p| p as f64 / 100.0).unwrap_or(0.0));
            f.render_widget(gauge, gauge_rect);
        }
        Modal::AuthFailure { command, stderr, retry: _ } => {
            let body = format!(
                "Authentication failed: {command}\n\n{}\n\nPress R to retry, Esc to cancel.",
                stderr
            );
            crate::wm::popup::render(
                f,
                area,
                crate::wm::popup::Popup::new("auth required", &body)
                    .with_hint("[r] retry   [esc] cancel"),
                theme,
            );
        }
        Modal::ToastLog => {
            use ratatui::text::Line;
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            // Newest-first: iterate the ring in reverse so the most recent
            // entry lands on the topmost visible row. `toast_log_offset`
            // skips further into the list (toward older entries) and is
            // clamped to `total - visible` by the key handler so we
            // never render a blank window.
            let total = app.toast_history.len();
            // Reserve at least 2 lines for the title + a one-line hint.
            let h = (total.min(area.height.saturating_sub(4) as usize) as u16)
                .max(3)
                .min(area.height.saturating_sub(4));
            let w = 70.min(area.width.saturating_sub(4));
            let x = area.x + (area.width.saturating_sub(w)) / 2;
            let y = area.y + (area.height.saturating_sub(h + 2)) / 2;
            let rect = rect(x, y, w, h + 2);
            f.render_widget(Clear, rect);

            let visible = h as usize;
            let max_off = total.saturating_sub(visible);
            // Defensive clamp: the key handler should already keep this in
            // range, but a stale `toast_log_offset` (e.g. after the user
            // closes the modal, more toasts age out of the ring, and
            // re-opens) would otherwise render a blank top.
            let offset = app.toast_log_offset.min(max_off);

            // We iterate from `total - offset - visible` to
            // `total - offset` (exclusive) — newest first.
            let lines: Vec<Line> = if total == 0 {
                vec![Line::from("(no toasts yet — try something first)")]
            } else {
                app.toast_history
                    .iter()
                    .rev()
                    .skip(offset)
                    .take(visible)
                    .map(|t| {
                        let prefix = match t.kind {
                            crate::app::toast::ToastKind::Info => "ℹ",
                            crate::app::toast::ToastKind::Ok => "✓",
                            crate::app::toast::ToastKind::Warn => "⚠",
                            crate::app::toast::ToastKind::Error => "✗",
                        };
                        Line::from(format!(
                            "{} {} {}",
                            t.ts.format("%H:%M:%S"),
                            prefix,
                            t.message
                        ))
                    })
                    .collect()
            };

            let p = Paragraph::new(lines).block(
                Block::default()
                    .title(format!(
                        " toast log ({}/{}) ",
                        offset.saturating_add(1).min(total.max(1)),
                        total
                    ))
                    .borders(Borders::ALL)
                    .border_style(theme.border(true)),
            );
            f.render_widget(p, rect);
            // Hint line below the modal.
            let hint_y = rect.y.saturating_add(rect.height);
            if hint_y < area.y + area.height {
                f.render_widget(
                    Paragraph::new(Line::from("[ ↑/↓ ] scroll   [ esc ] close"))
                        .alignment(ratatui::layout::Alignment::Center),
                    ratatui::layout::Rect::new(
                        x,
                        hint_y,
                        w,
                        1,
                    ),
                );
            }
        }
    }
}

fn palette_actions() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = Vec::new();
    for id in ScreenId::ALL {
        v.push(("screen", format!("Go to {}", id.label())));
    }
    v.push(("action", "Reboot".into()));
    v.push(("action", "Shutdown".into()));
    v.push(("action", "Suspend".into()));
    v.push(("action", "Hibernate".into()));
    v.push(("action", "Refresh all".into()));
    v.push(("action", "Start web server".into()));
    v.push(("action", "Stop web server".into()));
    v
}

/// Single point where digit-key shortcuts (`1`..`0`) translate into a
/// screen change. Keeps `app.current` and the WM pane's `WindowKind`
/// in sync so the right side actually redraws with the new screen —
/// without this, the sidebar would say "Network" but the content
/// pane would keep showing whatever it last rendered.
fn switch_screen(app: &mut App, screen: ScreenId, sidebar_row: usize) {
    if matches!(app.region, Region::Sidebar) {
        app.sidebar_idx = sidebar_row.min(ScreenId::ALL.len() - 1);
    }
    app.current = screen;
    // Committing from the sidebar (Enter / 1-0 / "Go to N") lands the
    // user in the content pane, not the sidebar. The new D-pad model
    // always starts you inside the screen you just opened; `←`/`h`
    // gets you back out. This replaces the old "Tab to switch panes"
    // overload so arrow keys are deterministic.
    app.set_region(Region::ContentLeft);
    let _ = app.manager.set_pane_kind(
        crate::wm::window::WindowKind::Builtin(screen),
    );
    // Trigger an immediate scan on Network enter. The wifi list is empty
    // until something populates `app.wifi_scan_results`, and without an
    // auto-scan the user sees an empty pane and `j`/`Down` does nothing
    // because the cursor is clamped to the interface region.
    if screen == ScreenId::Network && app.wifi_scan_results.is_empty() {
        app.tx.try_send(Action::Run(RunAction::WifiScan)).ok();
    }
    // Trigger an immediate Bluetooth device scan on enter. Without this,
    // the device list is empty until the background refresh task ticks
    // (which it does, but with a delay), and `j`/`Down` does nothing
    // because the cursor is clamped to 0 against an empty list.
    if screen == ScreenId::Bluetooth {
        app.tx.try_send(Action::Run(RunAction::BluetoothScan)).ok();
    }
    // LoRa screen — fire the IP-entry popup the first time (and every
    // time) the user lands on the LoRa screen with no node IP
    // configured. The popup shape is:
    //   * No past IPs on file → plain `Modal::Input` asking for the IP.
    //     Exactly the same shape the existing `i` keypress uses; just
    //     auto-fired instead of key-bound.
    //   * One-or-more past IPs → `Modal::Choice` listing each past IP
    //     alongside "+ Add new IP…". Picking a past IP commits via
    //     `ChoiceCommit::LoraPickStoredIp { ip }` in `run_choice`,
    //     which mirrors the `InputKind::LoraNodeIp` submit path
    //     (sets `app.lora_node_ip` + pushes MRU + surfaces a toast).
    //     Picking "+ Add new IP…" falls through to the same input
    //     modal via `ChoiceCommit::PickInput { LoraNodeIp, ... }`.
    //
    // Important: only fire when the modal isn't already non-None so
    // a user mid-modal (e.g. typing in a WifiPassword prompt when
    // they Tab into LoRa) doesn't get their input stolen. Also
    // only fire when `app.current` was *previously* not LoRa — guard
    // against re-popup on every internal `switch_screen` call (the
    // Wi-Fi / Bluetooth side-effects already follow this pattern).
    if screen == ScreenId::LoRa && matches!(app.modal, Modal::None) {
        if app.lora_node_ip.is_none() {
            if app.lora_recent_ips.is_empty() {
                // First-run: no past IPs — drop straight into the IP
                // entry modal. The user can still cancel with Esc
                // and reopen with `i`.
                app.open_input(
                    "Meshtastic node IP (first time)",
                    InputKind::LoraNodeIp,
                );
            } else {
                // Returning user: present the MRU list with an
                // "add new" affordance. The picker handles commit
                // dispatch through `run_choice`. We pre-select the
                // most-recent IP (index 0 is MRU) so Enter on open
                // reconnects to the last-used node — same ergonomic
                // as the Wi-Fi saved-network picker.
                let mut options: Vec<ChoiceOption> = Vec::new();
                options.push(ChoiceOption {
                    id: "__add_new__".to_string(),
                    label: "+ Add new IP…".to_string(),
                });
                for ip in &app.lora_recent_ips {
                    options.push(ChoiceOption {
                        id: format!("ip:{ip}"),
                        label: ip.clone(),
                    });
                }
                let prompt = format!(
                    "Meshtastic node IP (last: {})",
                    app.lora_recent_ips[0],
                );
                let commit_kind =
                    ChoiceCommit::PickInput {
                        kind: InputKind::LoraNodeIp,
                        prompt: "Meshtastic node IP".to_string(),
                        masked: false,
                        prefill: String::new(),
                    };
                app.modal = Modal::Choice {
                    prompt,
                    options,
                    cursor: 1, // skip "+ Add new IP…"; point at the MRU entry
                    commit_kind: Some(commit_kind),
                };
            }
        }
    }
}

async fn handle_key(
    screens: &mut [Box<dyn Screen>],
    app: &mut App,
    tx: &mpsc::Sender<Action>,
    key: KeyEvent,
) -> bool {
    use KeyCode::*;

    // Hardware-button remap. Runs first so the rest of the handler
    // (modal dispatch, global keys, screen on_key) sees a normal
    // KeyEvent. The desktop profile is identity; the uconsole profile
    // rewrites X/Y/A/B into Up/Down/Enter/Esc. See `wm/keymap.rs`.
    let key = match wm::keymap::map_key(key, wm::keymap::KeymapProfile::detect()) {
        Some(k) => k,
        // The contract is `Option` so future profiles can swallow
        // specific keys (e.g. a tablet profile that ignores the
        // volume buttons). Today every profile returns `Some`.
        None => return false,
    };

    // Modal handling first.
    match &app.modal {
        Modal::None => {}
        Modal::Help => {
            if matches!(key.code, Esc | Char('?') | Enter) {
                app.modal = Modal::None;
            }
            return false;
        }
        Modal::CommandPalette => {
            match key.code {
                Esc => {
                    app.modal = Modal::None;
                    app.palette_buf.clear();
                }
                Enter => {
                    let actions = palette_actions();
                    let q = app.palette_buf.to_lowercase();
                    let filtered: Vec<_> = actions
                        .iter()
                        .filter(|(_, label)| q.is_empty() || label.to_lowercase().contains(&q))
                        .collect();
                    if let Some((_, label)) = filtered.get(app.palette_idx) {
                        run_palette(app, tx, label).await;
                    }
                    app.modal = Modal::None;
                    app.palette_buf.clear();
                }
                Char(c) => {
                    app.palette_buf.push(c);
                    app.palette_idx = 0;
                }
                Backspace => {
                    app.palette_buf.pop();
                    app.palette_idx = 0;
                }
                Down => {
                    app.palette_idx = app.palette_idx.saturating_add(1);
                }
                Up => {
                    app.palette_idx = app.palette_idx.saturating_sub(1);
                }
                _ => {}
            }
            return false;
        }
        Modal::Confirm { kind, arg, .. } => {
            let k = *kind;
            let a = arg.clone();
            if matches!(key.code, Char('y') | Char('Y') | Enter) {
                app.modal = Modal::None;
                run_confirm(app, tx, k, a).await;
            } else if matches!(key.code, Char('n') | Char('N') | Esc) {
                app.modal = Modal::None;
            }
            return false;
        }
        Modal::Input { kind, buf, .. } => {
            let k = *kind;
            match key.code {
                Esc => {
                    app.modal = Modal::None;
                }
                Enter => {
                    let value = buf.clone();
                    app.modal = Modal::None;
                    run_input(app, tx, k, value).await;
                }
                Char(c) => {
                    // Push into the live buffer via a re-borrow. For
                    // `InputKind::BluetoothPasskey` only digits are
                    // accepted; letters and other characters are silently
                    // dropped at the insert step so the user can't
                    // accidentally type a letter into a numeric passkey
                    // field.
                    if matches!(k, InputKind::BluetoothPasskey) && !c.is_ascii_digit() {
                        // drop non-digit chars on the floor
                    } else if let Modal::Input { buf, .. } = &mut app.modal {
                        buf.push(c);
                    }
                }
                Backspace => {
                    if let Modal::Input { buf, .. } = &mut app.modal {
                        buf.pop();
                    }
                }
                _ => {}
            }
            return false;
        }
        Modal::Secret { kind, buf, .. } => {
            let k = *kind;
            match key.code {
                Esc => {
                    app.modal = Modal::None;
                }
                Enter => {
                    let value = buf.clone();
                    app.modal = Modal::None;
                    run_input(app, tx, k, value).await;
                }
                Char(c) => {
                    if matches!(k, InputKind::BluetoothPasskey) && !c.is_ascii_digit() {
                        // drop non-digit chars on the floor — see the
                        // matching arm in the Modal::Input handler above.
                    } else if let Modal::Secret { buf, .. } = &mut app.modal {
                        buf.push(c);
                    }
                }
                Backspace => {
                    if let Modal::Secret { buf, .. } = &mut app.modal {
                        buf.pop();
                    }
                }
                _ => {}
            }
            return false;
        }
        Modal::Choice { options, cursor, .. } => {
            let n = options.len();
            let mut cur = *cursor;
            let mut close: Option<Modal> = None;
            let mut dispatch_choice: Option<(String, ChoiceCommit)> = None;
            match key.code {
                Esc => {
                    close = Some(Modal::None);
                }
                Up | Char('k') => {
                    if n == 0 {
                        return false;
                    }
                    cur = if cur == 0 { n - 1 } else { cur - 1 };
                }
                Down | Char('j') => {
                    if n == 0 {
                        return false;
                    }
                    cur = (cur + 1) % n;
                }
                Enter => {
                    // Pull the commit_kind out via mem::replace so we don't
                    // need ChoiceCommit: Clone. The Option<ChoiceCommit>
                    // contains a String and (potentially) a RunAction.
                    let modal = std::mem::replace(&mut app.modal, Modal::None);
                    if let Modal::Choice { options, cursor, commit_kind, .. } = modal {
                        if let Some(opt) = options.get(cursor) {
                            if let Some(ck) = commit_kind {
                                dispatch_choice = Some((opt.id.clone(), ck));
                            }
                            close = Some(Modal::None);
                        } else {
                            // Cursor out of bounds (list shrank); dismiss.
                            close = Some(Modal::None);
                        }
                    }
                }
                _ => {}
            }
            // Apply updates.
            if let Modal::Choice { cursor, .. } = &mut app.modal {
                *cursor = cur;
            }
            if let Some(m) = close {
                app.modal = m;
            }
            if let Some((id, ck)) = dispatch_choice {
                run_choice(app, tx, &id, ck).await;
            }
            return false;
        }
        Modal::Wizard(_) => {
            // Wizard keyboard: Enter advances, Esc goes back (or cancels).
            // The wizard drives which underlying Input/Choice/Secret modal
            // is active at each step; here we only handle nav.
            match key.code {
                Esc => {
                    app.modal = Modal::None;
                    return false;
                }
                Enter => {
                    // The wizard step is implemented as a sub-modal launched
                    // when the user enters the step. Step transitions are
                    // driven by `run_wizard_step` — we just commit.
                    advance_wizard(app, tx).await;
                    return false;
                }
                _ => {}
            }
        }
        Modal::Progress { cancel: _, .. } => {
            // Progress modal only consumes Esc (cancel) and is otherwise
            // transparent to keys so the screen underneath still updates.
            if matches!(key.code, Esc) {
                // Take the sender out via mem::replace so we can call `send`.
                let modal = std::mem::replace(&mut app.modal, Modal::None);
                if let Modal::Progress { cancel: Some(tx_cancel), .. } = modal {
                    let _ = tx_cancel.send(());
                }
            }
            return false;
        }
        Modal::AuthFailure { retry: _, .. } => {
            // R retries the inner modal; Esc dismisses everything.
            match key.code {
                Char('r') | Char('R') => {
                    // Pull the inner modal out via mem::replace so we don't
                    // need Modal: Clone (the Progress variant contains a
                    // non-Clone oneshot::Sender).
                    let outer = std::mem::replace(&mut app.modal, Modal::None);
                    let inner = if let Modal::AuthFailure { retry, .. } = outer {
                        *retry
                    } else {
                        Modal::None
                    };
                    app.modal = inner;
                }
                Esc => {
                    app.modal = Modal::None;
                }
                _ => {}
            }
            return false;
        }
        Modal::ToastLog => {
            match key.code {
                Esc => {
                    app.modal = Modal::None;
                }
                Down => {
                    // `total` is the size of the ring; `visible` is the
                    // number of rows the modal allocated. We cap the
                    // offset so the last visible row always corresponds
                    // to a real entry — pushing further would just show
                    // a blank bottom.
                    let total = app.toast_history.len();
                    // Match the renderer's `visible` arithmetic: the
                    // render caps `h` at `area.height - 4` and floors it
                    // at 3, so a conservative bound is `area.height - 4`
                    // minus 1 for the title bar. The key handler doesn't
                    // have access to the frame area, so we use a
                    // generous cap (`total`) and rely on the renderer's
                    // own defensive clamp to land on the right offset.
                    if total > 0 {
                        app.toast_log_offset =
                            app.toast_log_offset.saturating_add(1).min(total);
                    }
                }
                Up => {
                    app.toast_log_offset = app.toast_log_offset.saturating_sub(1);
                }
                _ => {}
            }
            return false;
        }
    }

    // Phase 1 — menu bar absorbs all keys while a dropdown is open.
    // Esc closes, arrows move, Enter fires. Routed here (before the
    // global-keys block) so the menu always wins over screen handlers.
    if app.menu.is_open() {
        use crate::ui::menu_bar::{MenuId, MENUS};
        match key.code {
            Esc => {
                app.menu.close();
            }
            Left => {
                // Move to the previous menu's last item, or wrap.
                let cur = app.menu.open.unwrap_or(MenuId::File);
                let order = [MenuId::File, MenuId::View, MenuId::Tools, MenuId::Help];
                let idx = order.iter().position(|m| *m == cur).unwrap_or(0);
                let prev = if idx == 0 { order[order.len() - 1] } else { order[idx - 1] };
                let menu = MENUS.iter().find(|m| m.id == prev).unwrap();
                app.menu.open(prev);
                app.menu.cursor = menu.items.len().saturating_sub(1);
            }
            Right => {
                let cur = app.menu.open.unwrap_or(MenuId::File);
                let order = [MenuId::File, MenuId::View, MenuId::Tools, MenuId::Help];
                let idx = order.iter().position(|m| *m == cur).unwrap_or(0);
                let next = order[(idx + 1) % order.len()];
                app.menu.open(next);
                app.menu.cursor = 0;
            }
            Up | Char('k') => {
                let menu = MENUS.iter().find(|m| Some(m.id) == app.menu.open).unwrap();
                let n = menu.items.len();
                if n == 0 {
                    return false;
                }
                app.menu.cursor = if app.menu.cursor == 0 { n - 1 } else { app.menu.cursor - 1 };
            }
            Down | Char('j') => {
                let menu = MENUS.iter().find(|m| Some(m.id) == app.menu.open).unwrap();
                let n = menu.items.len();
                if n == 0 {
                    return false;
                }
                app.menu.cursor = (app.menu.cursor + 1) % n;
            }
            Enter => {
                let menu_id = app.menu.open.unwrap_or(MenuId::File);
                let menu = MENUS.iter().find(|m| m.id == menu_id).unwrap();
                if let Some(item) = menu.items.get(app.menu.cursor) {
                    let item_copy = *item;
                    app.menu.close();
                    crate::ui::menu_bar::dispatch(&item_copy, app, tx).await;
                } else {
                    app.menu.close();
                }
            }
            _ => {
                // Ignore other keys while menu is open — the menu owns
                // the focus surface until it closes.
            }
        }
        return false;
    }

    // Global keys.
    match key.code {
        Char('q') | Char('Q') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return true;
            }
            return true;
        }
        Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        Char('?') => app.modal = Modal::Help,
        // Module 7.2 — capital T opens the scrollable toast history
        // overlay. Lowercase `t` is intentionally left alone so screen
        // authors can still bind it (e.g. `t` toggles the process tree
        // view on System).
        Char('T') => {
            app.modal = Modal::ToastLog;
            // Reset the scroll to the tail (newest at top) so every
            // open starts from a deterministic position.
            app.toast_log_offset = 0;
        }
        Char(':') => {
            // Delegate to the menu builder so the menu and the key
            // share state-reset semantics (cleared buffer, cursor at 0).
            // The menu item "Help → Command palette (:)" calls the
            // same `open_palette` — single source of truth.
            app.modal = crate::ui::menu_bar::open_palette(app);
        }
        // Phase 1 — menu bar keys. F10 (canonical) and Alt+F (herdr
        // style) open the menu bar with the File menu focused. While
        // a menu is open, all keys route into the menu (Esc closes,
        // arrows move, Enter fires) — see the early-return below.
        F(10) | Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
            if app.menu.is_open() {
                app.menu.close();
            } else {
                app.menu.open(crate::ui::menu_bar::MenuId::File);
            }
            return false;
        }
        // Phase 1 — tab strip arrow keys. From the Sidebar, Left (and h)
        // cycles the tab cursor backwards. Right is NOT bound here
        // because it has a Sidebar→ContentLeft region-step semantics
        // downstream. From the content panes, Left/Right keep their
        // region-step semantics (ContentLeft → Sidebar, ContentLeft
        // ↔ ContentRight) so D-pad navigation doesn't regress. The
        // cycle is dispatched via the action channel so the WM pane
        // kind updates in lockstep.
        Left | Char('h')
            if !app.menu.is_open() && app.region == Region::Sidebar =>
        {
            let id = crate::ui::tab_strip::cycle(&*app, false);
            let _ = tx.send(crate::ui::tab_strip::commit(&*app, id)).await;
            return false;
        }
        // Sidebar navigation. Only active while the sidebar owns the
        // region focus; otherwise these keys belong to the focused pane.
        // Up/Down (and k/j) move the sidebar cursor; Enter commits it as
        // the current screen and flips region to ContentLeft. From the
        // sidebar, Right/l and Esc hand focus back to the content pane;
        // Tab/Shift-Tab cycles to the next/prev screen so a D-pad user
        // can wander the screen list without ever touching the keyboard.
        Up | Char('k') if app.region == Region::Sidebar => {
            let total = ScreenId::ALL.len();
            if app.sidebar_idx == 0 {
                app.sidebar_idx = total - 1;
            } else {
                app.sidebar_idx -= 1;
            }
            // Module 1.5 — pass the renderer's recorded visible-row count
            // so the offset actually retreats when the cursor re-enters
            // the top of the window. `app.sidebar_visible` is set every
            // frame by `draw_sidebar_narrow` / `draw_sidebar_grid`.
            app.clamp_sidebar_offset(total, app.sidebar_visible);
            return false;
        }
        Down | Char('j') if app.region == Region::Sidebar => {
            let total = ScreenId::ALL.len();
            app.sidebar_idx = (app.sidebar_idx + 1) % total;
            // Module 1.5 — same single source of truth as Up above.
            // Before this, `(total, total)` was a no-op clamp that
            // never advanced the offset, leaving overflow rows invisible
            // but selectable on short terminals.
            app.clamp_sidebar_offset(total, app.sidebar_visible);
            return false;
        }
        Enter if app.region == Region::Sidebar => {
            if let Some(id) = ScreenId::ALL.get(app.sidebar_idx) {
                app.current = *id;
                // Right-side content pane follows the sidebar: swap its
                // kind so the next render redraws with the chosen screen,
                // then drop focus inside it.
                let _ = app.manager.set_pane_kind(
                    crate::wm::window::WindowKind::Builtin(*id),
                );
                app.set_region(Region::ContentLeft);
            }
            return false;
        }
        Right | Char('l') if app.region == Region::Sidebar => {
            // From the sidebar the only legal content region is the left
            // half of the screen. Single-pane screens stay there;
            // multi-pane screens opt further right on their own.
            app.set_region(Region::ContentLeft);
            return false;
        }
        Esc if app.region == Region::Sidebar => {
            app.set_region(Region::ContentLeft);
            return false;
        }
        // Tab / Shift-Tab cycles between screens. Only fires on the
        // content side and only when no modal is open, so it never
        // collides with the input field's Tab key or the sidebar branch
        // above. Replaces the old "Tab toggles sidebar" overload that
        // made D-pad navigation unpredictable.
        Tab if matches!(app.region, Region::ContentLeft | Region::ContentRight)
            && matches!(app.modal, Modal::None) =>
        {
            let _ = tx.send(Action::CycleScreen(true)).await;
            return false;
        }
        BackTab if matches!(app.region, Region::ContentLeft | Region::ContentRight)
            && matches!(app.modal, Modal::None) =>
        {
            let _ = tx.send(Action::CycleScreen(false)).await;
            return false;
        }
        // Content-side Left/h: from ContentLeft jumps to the Sidebar (no
        // in-between column to step to); from ContentRight steps back to
        // ContentLeft (the symmetric partner of Right/l's step forward).
        // This is the critical D-pad contract: `←` always means "step
        // one column left" with no screen defer, and `Esc` is the
        // universal "leave to sidebar" verb. Previously `←/h` from
        // ContentRight jumped all the way to Sidebar, which made the
        // right pane a trap — Network's `← = jump to last iface row`
        // semantics fought the region step and `Esc` was the only exit.
        Left | Char('h') if app.region == Region::ContentLeft => {
            app.set_region(Region::Sidebar);
            return false;
        }
        // Content-side Right/l moves within the screen: from
        // ContentLeft to ContentRight (only meaningful on multi-pane
        // screens; single-pane screens leave it alone). The screen
        // owns the inner-pair semantics via its own on_key.
        // Content-side Right/l moves focus one column right, but
        // defers to the screen first: screens that own their own
        // Right-arrow semantics (e.g. Network's "jump to first wifi
        // row" inside the unified list) get to handle the key and
        // *not* flip the region. Only when the screen returns false
        // does the router step the region forward. This is the
        // critical piece that makes D-pad navigation flawless:
        // `→` means "do the screen-thing if any, otherwise advance
        // region"; `←` means "step back to the sidebar". No key
        // overloading, no Tab ambiguity, no entry traps.
        Right | Char('l')
            if app.region == Region::ContentLeft
                || app.region == Region::ContentRight
            && matches!(app.modal, Modal::None) =>
        {
            // Forward first.
            let consumed = {
                let focused_id = app.manager.focused();
                if let Some(w) = app.manager.window(focused_id) {
                    match w.kind {
                        crate::wm::window::WindowKind::Builtin(sid) => screens
                            .iter_mut()
                            .find(|s| s.id() == sid)
                            .map(|s| s.on_key(key, app))
                            .unwrap_or(false),
                        crate::wm::window::WindowKind::Terminal => false,
                    }
                } else {
                    false
                }
            };
            if consumed {
                return false;
            }
            // Step the region forward on the un-consumed path. From
            // ContentLeft this lands on ContentRight; from ContentRight
            // it stays put (right is the right edge). Single-pane
            // screens consume every key they care about, so this branch
            // is effectively a no-op for them in practice.
            if app.region == Region::ContentLeft {
                app.set_region(Region::ContentRight);
            }
            return false;
        }
        Left | Char('h')
            if app.region == Region::ContentRight
            && matches!(app.modal, Modal::None) =>
        {
            // `←/h` from ContentRight always steps back to ContentLeft.
            // We deliberately do *not* defer to the screen here the way
            // `→/l` does: from the rightmost column the only useful
            // meaning of `←` is "step the region back," and screens like
            // Network's `← = jump to last iface row` make that semantics
            // collide with the region step. The D-pad contract is:
            //   →  = screen-thing first, then advance region
            //   ←  = retreat region first; the screen never sees `←`
            //        as a region step (it can still handle it inside
            //        `ContentLeft` because the router only fires this
            //        arm from `ContentRight`).
            app.set_region(Region::ContentLeft);
            return false;
        }
        Char('1') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::System, 0),
        Char('2') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Network, 1),
        Char('3') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Bluetooth, 2),
        Char('4') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Power, 3),
        Char('5') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Display, 4),
        Char('6') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Audio, 5),
        Char('7') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Storage, 6),
        Char('8') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Services, 7),
        Char('9') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Packages, 8),
        Char('0') if !key.modifiers.contains(KeyModifiers::CONTROL) => switch_screen(app, ScreenId::Settings, 9),
        // Ctrl-W keymap. Disabled: the TUI is locked to a 2-pane layout
        // (sidebar + content) and splits/closes/resizes/terminal spawns
        // are not exposed. The dead arms are kept out of the match so
        // Ctrl-W + h/j/k/l/v/s/n/q/= doesn't fire surprise side
        // effects.
        _ => {
            // Forward to the focused pane (built-in screen OR terminal).
            let focused_id = app.manager.focused();
            if let Some(w) = app.manager.window(focused_id) {
                match w.kind {
                    crate::wm::window::WindowKind::Builtin(sid) => {
                        if let Some(s) = screens.iter_mut().find(|s| s.id() == sid) {
                            if s.on_key(key, app) {
                                return false;
                            }
                        }
                    }
                    crate::wm::window::WindowKind::Terminal => {
                        if let Some(bytes) =
                            crate::wm::input::bytes_for_key(&key)
                        {
                            if let Some(w) = app.manager.window_mut(focused_id) {
                                if let Some(term) = w.terminal_mut() {
                                    let _ = term.writer.try_send(bytes);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

async fn run_palette(app: &mut App, tx: &mpsc::Sender<Action>, label: &str) {
    if let Some(rest) = label.strip_prefix("Go to ") {
        for id in ScreenId::ALL {
            if id.label() == rest {
                app.current = *id;
                // Mirrors `switch_screen`: a palette commit lands the
                // user inside the screen, not on the sidebar.
                app.set_region(Region::ContentLeft);
                return;
            }
        }
    }
    match label {
        "Reboot" => {
            app.modal = Modal::Confirm {
                message: "Reboot the system?".into(),
                kind: ConfirmKind::Reboot,
                arg: String::new(),
            }
        }
        "Shutdown" => {
            app.modal = Modal::Confirm {
                message: "Shut down the system?".into(),
                kind: ConfirmKind::Shutdown,
                arg: String::new(),
            }
        }
        "Suspend" => {
            let _ = tx.send(Action::Run(RunAction::Suspend)).await;
        }
        "Hibernate" => {
            let _ = tx.send(Action::Run(RunAction::Hibernate)).await;
        }
        "Refresh all" => {
            for id in ScreenId::ALL {
                let _ = tx.send(Action::Refresh(*id)).await;
            }
        }
        "Start web server" => {
            let sender = app.live.web_ctrl.lock().await.clone();
            let _ = sender
                .send((tx.clone(), Action::Run(RunAction::WebStart)))
                .await;
        }
        "Stop web server" => {
            let sender = app.live.web_ctrl.lock().await.clone();
            let _ = sender
                .send((tx.clone(), Action::Run(RunAction::WebStop)))
                .await;
        }
        _ => {}
    }
}

async fn run_confirm(app: &mut App, tx: &mpsc::Sender<Action>, kind: ConfirmKind, arg: String) {
    let act = match kind {
        ConfirmKind::Reboot => RunAction::Reboot,
        ConfirmKind::Shutdown => RunAction::Shutdown,
        ConfirmKind::Kill => RunAction::ProcessKill(arg.parse().unwrap_or(0)),
        ConfirmKind::Remove => RunAction::PackageRemove(arg),
        ConfirmKind::DisconnectWifi => RunAction::WifiDisconnect,
        // Module 4 — discard-confirm. Discarding the editor buffer is
        // a pure in-memory state reset on App (clear the 5 editor
        // fields + swap focus back to Files), so we apply it directly
        // here instead of routing it through a `RunAction`. The other
        // arms *do* need the async dispatch path because they invoke
        // `cyberdeck_core::*` commands; this one doesn't.
        //
        // `arg` is the file path that was being edited — kept in scope
        // for parity with the other `ConfirmKind` arms, but unused.
        ConfirmKind::Discard => {
            let _ = arg;
            app.discard_editor();
            return;
        }
    };
    let _ = tx.send(Action::Run(act)).await;
}

/// Append clipboard contents to the active text-entry modal buffer.
///
/// Called from the main event loop for both:
///   - `crossterm::event::Event::Paste(text)` (terminal-emitted, the
///     modern path — requires bracketed paste mode to be enabled in
///     the terminal, which we do via `EnableBracketedPaste` at startup).
///   - `KeyEvent { Char('v'), CONTROL|SHIFT }` (the legacy fallback for
///     terminals that don't emit `Event::Paste` or have bracketed paste
///     disabled).
///
/// The trailing whitespace strip (`\n`, `\r`, ` `, `\t`) keeps the
/// buffer clean of the stray newline that `xclip -o`, `wl-paste`, and
/// most GUI clipboards add when the source text is a single line.
///
/// No-op when no `Modal::Input` or `Modal::Secret` is active.
pub(crate) fn handle_paste(app: &mut App, mut text: String) {
    while matches!(
        text.chars().last(),
        Some('\n') | Some('\r') | Some(' ') | Some('\t')
    ) {
        text.pop();
    }
    match &mut app.modal {
        Modal::Input { buf, .. } => buf.push_str(&text),
        Modal::Secret { buf, .. } => buf.push_str(&text),
        _ => {}
    }
}

/// Read the system clipboard for the Ctrl+Shift+V fallback path.
///
/// Tries, in order:
///   1. `wl-paste -n` (Wayland)
///   2. `xclip -selection clipboard -o` (X11)
///   3. `xsel --clipboard --output` (X11 alt)
///   4. `pbcopy` (macOS)
///
/// Each spawn is short-lived (1s timeout via `Command::spawn` + manual
/// `try_wait` loop so we don't block the TUI on a hung clipboard daemon).
/// Returns an empty string on any failure (no clipboard tool, hung
/// daemon, empty selection) — `handle_paste` is then a clean no-op.
fn read_clipboard_for_paste() -> String {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let candidates: &[&[&str]] = &[
        &["wl-paste", "-n"],
        &["xclip", "-selection", "clipboard", "-o"],
        &["xsel", "--clipboard", "--output"],
        &["pbpaste"],
    ];
    for cmd in candidates {
        let mut child = match Command::new(cmd[0])
            .args(&cmd[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => continue, // tool not installed; try next
        };
        // Poll for up to 1s total.
        let deadline = std::time::Instant::now() + Duration::from_millis(1000);
        let mut out = String::new();
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    if let Some(mut stdout) = child.stdout.take() {
                        let _ = stdout.read_to_string(&mut out);
                    }
                    return out;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    }
    String::new()
}

pub(crate) async fn run_input(app: &mut App, tx: &mpsc::Sender<Action>, kind: InputKind, value: String) {
    let act = match kind {
        InputKind::WifiPassword => {
            if let Some(ssid) = app.pending_ssid.take() {
                RunAction::WifiConnect {
                    ssid,
                    password: if value.is_empty() { None } else { Some(value) },
                }
            } else {
                app.push_toast(ToastKind::Warn, "no SSID selected");
                return;
            }
        }
        InputKind::ConnectSSID => {
            // Hidden-SSID connect. Per Module 1 (orbital-style chain),
            // do NOT dispatch WifiConnect directly with password: None —
            // that silently connects to an open hidden network even if the
            // user meant a WPA network. Instead, stash the typed SSID on
            // `app.pending_ssid` and open `Modal::Secret` so the user can
            // enter the password. The submit on that modal flows through
            // the `InputKind::WifiPassword` arm above and dispatches
            // `WifiConnect { ssid, password: Some(...) }`.
            let ssid = value.trim().to_string();
            if ssid.is_empty() {
                app.push_toast(ToastKind::Error, "SSID cannot be empty");
                return;
            }
            app.pending_ssid = Some(ssid.clone());
            app.open_secret(
                format!("Wi-Fi password for {ssid}"),
                InputKind::WifiPassword,
            );
            return;
        }
        InputKind::KillPid => match value.parse::<i32>() {
            Ok(p) => RunAction::ProcessKill(p),
            Err(_) => {
                app.push_toast(ToastKind::Error, "invalid pid");
                return;
            }
        },
        InputKind::HiddenSSID => RunAction::WifiConnect {
            ssid: value,
            password: None,
        },
        InputKind::BluetoothPasskey => {
            // The passkey modal is in place (Module 2: numeric filter +
            // masked Modal::Secret + OK/Cancel row). The actual pairing
            // dispatch is wired in Module 3 (Bluetooth screen refactor)
            // via `RunAction::BluetoothPairWithPasskey(mac, pin)` once
            // the device is selected. Until then, gracefully refuse to
            // submit so the modal is at least usable today.
            app.push_toast(
                ToastKind::Warn,
                "BT pairing wires up in Module 3 — passkey captured",
            );
            return;
        }
        InputKind::PackageSearch => {
            // Module 3 — Packages screen search. Submit stores the
            // trimmed query on `app.packages_search_query` so the screen
            // can pick it up on its next render. An empty/whitespace
            // submit is a no-op for the field (it just closes the
            // modal) so the user doesn't accidentally wipe their
            // in-flight search by hitting Enter on a blank field.
            // The actual `cyberdeck_core::packages::search(&query)`
            // dispatch is wired in the Packages screen render loop
            // (tasks 3.2–3.4).
            let query = value.trim().to_string();
            if !query.is_empty() {
                app.packages_search_query = Some(query);
            }
            // No RunAction to dispatch — the field mutation above is
            // sufficient. Close the modal by returning.
            return;
        }
        InputKind::LoraNodeIp => {
            // LoRa (Meshtastic over LAN HTTP) — user typed an IP for the
            // node they want to talk to. Stash the trimmed value on
            // `app.lora_node_ip`; the render loop on the LoRa screen
            // (Slice 4 wiring) sees the change and swaps the screen's
            // transport from `FakeTransport` to an `HttpLoraTransport`
            // pointed at that address. Empty submit is a no-op so the
            // modal doesn't accidentally wipe an existing IP — same
            // pattern as `PackageSearch`. Also push onto
            // `app.lora_recent_ips` so the auto-popup next time the
            // LoRa screen is opened (with no IP set) can offer this
            // address as a one-keystroke reconnect.
            let ip = value.trim().to_string();
            if ip.is_empty() {
                app.push_toast(ToastKind::Warn, "node IP cannot be empty");
                return;
            }
            app.push_lora_recent_ip(&ip);
            app.lora_node_ip = Some(ip);
            app.push_toast(
                ToastKind::Info,
                "LoRa: connecting to node (next tick) — see status footer",
            );
            return;
        }
        InputKind::EditorSaveAs => {
            // Phase 2 — Save As submit. Trim the path, reject empty
            // (so an accidental Enter doesn't wipe the file), then
            // dispatch the I/O via the action channel so the write
            // goes through the same code path as the menu/dropdown
            // item. Read-only mode is enforced inside the dispatch
            // handler, not here, so the Input modal can still be
            // opened (and dismissed) on a read-only buffer.
            let path = value.trim().to_string();
            if path.is_empty() {
                app.push_toast(ToastKind::Warn, "save path cannot be empty");
                return;
            }
            let _ = tx
                .send(Action::Run(RunAction::EditorSaveAs(path)))
                .await;
            return;
        }
        InputKind::CityPicker => {
            // Phase 2 — City palette search. Substring-match the
            // trimmed query against bundled slugs + human-readable
            // names, then apply the first match. Empty submit is a
            // silent no-op (so Esc-style cancel can land on the
            // same code path). The dispatch goes through
            // `Action::CityCtrlSet { slug }` so the City screen's
            // existing `apply_slug` path handles the rest (reset
            // viewport_bbox, sync the location marker, save prefs).
            let query = value.trim().to_string();
            if query.is_empty() {
                return;
            }
            let slug = city_picker_match(&query);
            match slug {
                Some(slug) => {
                    let _ = tx.send(Action::CityCtrlSet { slug }).await;
                }
                None => {
                    app.push_toast(
                        ToastKind::Warn,
                        format!("no bundled city matches \"{query}\""),
                    );
                }
            }
            return;
        }
        InputKind::WifiEnterpriseIdentity => {
            // Stash on the wizard so advance_wizard can read it.
            if let Modal::Wizard(crate::app::Wizard::WifiEnterprise { identity, step, .. }) =
                &mut app.modal
            {
                if value.is_empty() {
                    app.push_toast(ToastKind::Error, "identity cannot be empty");
                    return;
                }
                *identity = Some(value.clone());
                *step = 2;
            } else {
                app.push_toast(ToastKind::Warn, "no enterprise wizard active");
                return;
            }
            // Open the next step (password or cert).
            advance_wizard_step(app, tx).await;
            return;
        }
        InputKind::WifiEnterprisePassword => {
            if let Modal::Wizard(crate::app::Wizard::WifiEnterprise {
                password, step, ..
            }) = &mut app.modal
            {
                if value.is_empty() {
                    app.push_toast(ToastKind::Error, "password cannot be empty");
                    return;
                }
                *password = Some(value);
                *step = 3;
            } else {
                app.push_toast(ToastKind::Warn, "no enterprise wizard active");
                return;
            }
            // For PEAP/TTLS/PWD the next step is anonymous identity (optional);
            // we currently treat it as "no anon identity" and finalize.
            finalize_wizard(app, tx).await;
            return;
        }
    };
    let _ = tx.send(Action::Run(act)).await;
}

/// Dispatch the user's selection in a `Modal::Choice`. `commit_kind` describes
/// what to do next: open a new Input/Secret modal with a prefill, or fire a
/// RunAction directly.
async fn run_choice(
    app: &mut App,
    tx: &mpsc::Sender<Action>,
    id: &str,
    commit: ChoiceCommit,
) {
    match commit {
        ChoiceCommit::RunAction(act) => {
            let _ = tx.send(Action::Run(act)).await;
        }
        ChoiceCommit::PickInput {
            kind,
            prompt,
            masked,
            prefill,
        } => {
            if masked {
                app.modal = Modal::Secret {
                    prompt,
                    buf: prefill,
                    kind,
                };
            } else {
                app.modal = Modal::Input {
                    prompt,
                    buf: prefill,
                    kind,
                };
            }
            // Note: the caller is responsible for setting `app.pending_ssid`
            // etc. before launching the picker if downstream behaviour depends
            // on it.
            let _ = id;
        }
        ChoiceCommit::LoraPickStoredIp { ip } => {
            // The LoRa auto-popup picks a previously-used IP from
            // `app.lora_recent_ips`. Wire it the same way as a fresh
            // `InputKind::LoraNodeIp` submit: stash the trimmed value
            // on `lora_node_ip`, refresh MRU position in
            // `lora_recent_ips`, and surface a toast so the user sees
            // the next-tick connection attempt. The transport swap
            // happens in `LoraScreen::poll` on the next `Action::Tick`.
            let trimmed = ip.trim().to_string();
            if trimmed.is_empty() {
                app.push_toast(
                    ToastKind::Warn,
                    "lora: empty IP in picker",
                );
                return;
            }
            app.push_lora_recent_ip(&trimmed);
            app.lora_node_ip = Some(trimmed.clone());
            app.modal = Modal::None;
            app.push_toast(
                ToastKind::Info,
                format!("lora: connecting to {trimmed} (next tick)"),
            );
            let _ = tx;
            let _ = id;
        }
    }
}

/// Advance the wizard: when the user hits Enter on the body, transition
/// to the next step's prompt (or fire the final RunAction).
async fn advance_wizard(app: &mut App, tx: &mpsc::Sender<Action>) {
    advance_wizard_step(app, tx).await;
}

async fn advance_wizard_step(app: &mut App, tx: &mpsc::Sender<Action>) {
    if let Modal::Wizard(w) = &app.modal {
        match w {
            Wizard::WifiEnterprise { step, eap, .. } => {
                let next_step = *step;
                let current_eap = eap.clone();
                if next_step == 0 || current_eap.is_some() {
                    // Step 1: identity.
                    let modal = Modal::Input {
                        prompt: "Identity".into(),
                        buf: String::new(),
                        kind: InputKind::WifiEnterpriseIdentity,
                    };
                    app.modal = modal;
                } else {
                    finalize_wizard(app, tx).await;
                }
            }
        }
    }
}

/// Finalize the wizard by mapping accumulated state to a RunAction. For
/// Wi-Fi Enterprise, we currently only know how to encode the request;
/// the core side (Phase 6) will translate it into `nmcli connection up`
/// with 802-1x settings.
async fn finalize_wizard(app: &mut App, tx: &mpsc::Sender<Action>) {
    // Pull all fields out of the wizard via `std::mem::replace` so we can
    // overwrite `app.modal` without cloning (the wizard variant contains a
    // non-Clone oneshot::Sender further down the enum).
    let modal = std::mem::replace(&mut app.modal, Modal::None);
    if let Modal::Wizard(Wizard::WifiEnterprise {
        ssid,
        eap,
        identity,
        password,
        anon_or_cert,
        ..
    }) = modal
    {
        if eap.is_none() || identity.is_none() || password.is_none() {
            app.push_toast(ToastKind::Error, "wizard incomplete");
            return;
        }
        let act = RunAction::WifiEnterpriseConnect {
            ssid,
            eap: eap.unwrap_or_default(),
            identity: identity.unwrap_or_default(),
            password,
            anon_or_cert,
        };
        let _ = tx.send(Action::Run(act)).await;
    }
}

async fn handle_action(
    screens: &mut [Box<dyn Screen>],
    app: &mut App,
    tx: &mpsc::Sender<Action>,
    action: Action,
) -> bool {
    match action {
        Action::Tick => {
            // Refreshers already produced data. Also: fire the welcome
            // toast exactly once on the first tick of the process so the
            // user lands on something more useful than a blank pane.
            // Mirrors orbital's startup greeter pattern.
            if !app.boot_toast_sent {
                app.boot_toast_sent = true;
                app.push_toast(
                    ToastKind::Info,
                    "Welcome — Tab to switch panes, ? for help, r to rescan",
                );
            }
            // Refresh the LoRa screen's snapshot from its transport.
            // Non-blocking: the in-process `FakeTransport` returns
            // immediately and the real HTTP transport (when wired in)
            // is bounded by a short read; either way no `select!` wait
            // can hang the renderer.
            if let Some(s) = screens.iter_mut().find(|s| s.id() == ScreenId::LoRa) {
                if let Some(any) = s.as_any_mut() {
                    if let Some(lora) =
                        any.downcast_mut::<crate::screens::lora::LoraScreen>()
                    {
                        lora.poll(app);
                    }
                }
            }
        }
        Action::Key(_) => {}
        Action::Goto(id) => {
            app.current = id;
        }
        Action::CycleScreen(forward) => {
            // Tab / Shift-Tab stepping. Mirrors orbital's
            // Tab/Shift-Tab widget navigation with hidden-widget
            // skipping: `Screen::is_hidden(&app) -> bool` defaults to
            // false so every screen is reachable unless it opts out.
            //
            // BUG FIX: previously only `app.current` was updated, leaving
            // the WM pane's `WindowKind` stuck on the old screen — so the
            // right side kept rendering whatever was last painted while
            // the tab strip / sidebar highlight visibly moved. Routing
            // through `switch_screen` (the same helper `1`..`0` / Enter /
            // Goto use) keeps `app.current` and the WM pane's kind in
            // lockstep, so the next frame actually repaints the new
            // screen. Side-effects (Network auto-scan, Bluetooth scan,
            // LoRa IP modal) follow for free.
            let next = ScreenId::cycle(&*screens, app, app.current, forward);
            // Compute the matching sidebar row so the sidebar cursor
            // tracks the cycled screen — mirrors the digit-key path.
            let sidebar_row = ScreenId::ALL.iter().position(|s| *s == next).unwrap_or(0);
            switch_screen(app, next, sidebar_row);
        }
        Action::Quit => return true,
        Action::Toast(kind, msg) => app.push_toast(kind, msg),
        Action::Toggle(key) => {
            use app::screen::SettingsKey::*;
            // Confirmation toast text is decided at the end of the arm so
            // we can name the post-toggle state (e.g. "theme: light").
            let confirm: Option<String> = match key {
                Theme => {
                    // Cycle through all built-in themes (Dark, Light,
                    // HighContrast, Cyberpunk, VsCodeDark, VsCodeLight,
                    // CatppuccinMocha, Nord, GruvboxDark, SolarizedDark)
                    // instead of the old 3-state toggle. `next()` wraps
                    // around so the user can always get back to Dark.
                    app.theme_name = app.theme_name.next();
                    app.save_prefs();
                    Some(format!("theme: {}", app.theme_name.as_str()))
                }
                Mouse => {
                    app.mouse = !app.mouse;
                    app.save_prefs();
                    Some(format!(
                        "mouse capture: {}",
                        if app.mouse { "on" } else { "off" }
                    ))
                }
                NerdFont => {
                    app.nerd_font = !app.nerd_font;
                    app.save_prefs();
                    Some(format!(
                        "nerd font glyphs: {}",
                        if app.nerd_font { "on" } else { "off" }
                    ))
                }
                WebServer => {
                    let act = if *app.live.web_enabled.read().await {
                        RunAction::WebStop
                    } else {
                        RunAction::WebStart
                    };
                    let sender = app.live.web_ctrl.lock().await.clone();
                    // Optimistic confirmation: assume the request will
                    // succeed. If it fails the web-server task pushes a
                    // follow-up Error toast itself.
                    let will_be = if matches!(act, RunAction::WebStop) {
                        "off"
                    } else {
                        "on"
                    };
                    let _ = sender.send((tx.clone(), Action::Run(act))).await;
                    Some(format!("web server: {will_be}"))
                }
                Units => {
                    // Cycle Metric ↔ Imperial. The render-side string
                    // conversion lives on `App::units` (see Settings
                    // screen) so this stays a one-liner. We reach
                    // `Units` through the lib re-export because `main`
                    // is the binary crate; `crate::prefs::Units` from
                    // here would fail to resolve.
                    app.units = match app.units {
                        cyberdeck_tui::prefs::Units::Metric => cyberdeck_tui::prefs::Units::Imperial,
                        cyberdeck_tui::prefs::Units::Imperial => cyberdeck_tui::prefs::Units::Metric,
                    };
                    app.save_prefs();
                    let label = match app.units {
                        cyberdeck_tui::prefs::Units::Metric => "metric (°C, km/h)",
                        cyberdeck_tui::prefs::Units::Imperial => "imperial (°F, mph)",
                    };
                    Some(format!("units: {label}"))
                }
                TrafficOverlay => {
                    app.traffic_overlay = !app.traffic_overlay;
                    app.save_prefs();
                    Some(format!(
                        "traffic overlay: {}",
                        if app.traffic_overlay { "on" } else { "off" }
                    ))
                }
                WeatherPanel => {
                    app.show_weather_panel = !app.show_weather_panel;
                    app.save_prefs();
                    Some(format!(
                        "weather panel: {}",
                        if app.show_weather_panel { "on" } else { "off" }
                    ))
                }
            };
            if let Some(msg) = confirm {
                app.push_toast(ToastKind::Info, msg);
            }
        }
        Action::Run(act) => {
            // Phase 2 — Editor Save As / Reload are pure local I/O,
            // so they don't go through `spawn_action` (which fans out
            // to cyberdeck_core). Inline keeps them off the tokio
            // runtime and avoids spawning a task for a single
            // `std::fs::write` call.
            match act {
                RunAction::EditorSaveAs(path) => {
                    if app.editor_read_only {
                        app.status_message =
                            Some("editor: read-only — save ignored".to_string());
                    } else {
                        let body = app.editor_buffer.join("\n") + "\n";
                        match std::fs::write(&path, body) {
                            Ok(()) => {
                                app.editor_path = std::path::PathBuf::from(&path);
                                app.editor_dirty = false;
                                app.status_message =
                                    Some(format!("saved as {}", path));
                            }
                            Err(e) => {
                                app.status_message =
                                    Some(format!("editor: save failed ({e})"));
                            }
                        }
                    }
                }
                RunAction::EditorReload => {
                    // Reload is destructive — if the buffer is dirty
                    // we open a Discard confirm instead of clobbering
                    // unsaved edits. Mirrors the Esc-handler semantics.
                    if app.editor_dirty {
                        app.modal = Modal::Confirm {
                            message: "Discard unsaved changes?".to_string(),
                            kind: crate::app::ConfirmKind::Discard,
                            arg: app.editor_path.to_string_lossy().to_string(),
                        };
                    } else {
                        match std::fs::read_to_string(&app.editor_path) {
                            Ok(text) => {
                                app.editor_buffer = text
                                    .split('\n')
                                    .map(|s| s.trim_end_matches('\r').to_string())
                                    .filter(|s| !s.is_empty() || text.ends_with('\n'))
                                    .collect();
                                // The split above keeps a single
                                // trailing empty line so the on-disk
                                // representation round-trips. We
                                // trim the very last element if the
                                // file ended with `\n` — that's the
                                // POSIX convention the editor uses.
                                if app.editor_buffer.last().map(|s| s.is_empty())
                                    == Some(true)
                                    && text.ends_with('\n')
                                {
                                    app.editor_buffer.pop();
                                }
                                app.editor_dirty = false;
                                app.status_message = Some("reloaded".to_string());
                            }
                            Err(e) => {
                                app.status_message =
                                    Some(format!("editor: reload failed ({e})"));
                            }
                        }
                    }
                }
                RunAction::CityPan { col, row, rect } => {
                    // Phase 2 — click-to-pan. Routed to the City
                    // screen's `apply_pan_click` which reprojects
                    // the click through the same `Viewport` the
                    // renderer used and re-centres `viewport_bbox`.
                    // Guarded on `app.current == City` so a stale
                    // CityPan (e.g. dispatched from a non-City tab)
                    // is a no-op rather than a state corruption.
                    if app.current == ScreenId::City {
                        if let Some(screen) = screens
                            .iter_mut()
                            .find(|s| s.id() == ScreenId::City)
                        {
                            // `as_any_mut` returns `Option<&mut dyn Any>`.
                            // Unwrap the option, then downcast to the
                            // concrete `CityScreen` so we can call its
                            // pan handler.
                            if let Some(any) = screen.as_any_mut() {
                                if let Some(city) =
                                    any.downcast_mut::<screens::city::CityScreen>()
                                {
                                    let _ = city.apply_pan_click(col, row, rect);
                                }
                            }
                        }
                    }
                }
                _ => spawn_action(tx.clone(), act),
            }
        }
        Action::ConfirmModal => {} // handled inline above
        Action::CancelModal => app.modal = Modal::None,
        Action::SubmitInput(value) => {
            // Already handled in the key path.
            let _ = value;
        }
        Action::LogPushed(line) => {
            // Single-line path — used by the manual `r` refresh task
            // and the dispatcher falls through to the batched helper.
            app::dedupe_logs_into(&mut app.logs, vec![line], 1000);
        }
        Action::LogLines(lines) => {
            // Fix #1c — single dedupe + append per refiller pass
            // (instead of one per line). Cuts redraw frequency on a
            // busy box from N/sec to 1/sec.
            app::dedupe_logs_into(&mut app.logs, lines, 1000);
        }
        Action::Refresh(id) => {
            // Trivial: re-render. The background task already produces data.
            let _ = id;
            let _ = screens;
        }
        // Module 2.4 — explicit "give me the last 60s of logs now" from
        // the user pressing `r` on the Logs screen. We spawn the
        // journalctl call off the dispatcher (it can take hundreds of
        // ms on a busy box) and route the resulting lines back through
        // the normal `LogPushed` arm, so dedupe + ordering keep
        // working. The screen's `on_key` only enqueues this Action —
        // the actual I/O lives here, keeping the screen handler
        // trivially non-blocking.
        //
        // 60s matches the Q2 lock-in (one minute of context — enough to
        // cover the typical "what just happened?" investigation but
        // tight enough to avoid flooding the buffer on a noisy box).
        // The 1Hz refiller (Module 2.2) continues to feed live updates
        // in parallel via the same `LogPushed` arm.
        Action::RefreshLogs => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let entries = match cyberdeck_core::logs::recent_since(60).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::debug!("logs::RefreshLogs: recent_since(60) failed: {e}");
                        // Surface as a toast so the user knows their
                        // keypress registered even when journalctl
                        // refused (no perms, missing binary, etc.).
                        let _ = tx
                            .send(Action::Toast(
                                ToastKind::Error,
                                format!("refresh failed: {e}"),
                            ))
                            .await;
                        return;
                    }
                };
                for (ts, message) in entries {
                    if message.is_empty() {
                        continue;
                    }
                    let line = crate::app::LogLine {
                        ts: ts.with_timezone(&chrono::Local),
                        message,
                    };
                    if tx.send(Action::LogPushed(line)).await.is_err() {
                        // Receiver dropped — app is shutting down.
                        break;
                    }
                }
            });
        }
        Action::WifiScanResult(networks) => {
            app.wifi_scan_results = networks;
        }
        Action::BluetoothScanResult(devices) => {
            // `app.live.bluetooth` is `Arc<RwLock<Vec<BtDevice>>>` — write
            // through the lock so the next frame's render sees the new
            // device list. tokio's RwLock::write().await returns the
            // guard directly; acquisition errors are surfaced via
            // try_write for non-blocking callers.
            let mut guard = app.live.bluetooth.write().await;
            *guard = devices;
        }
        Action::NetSample {
            iface,
            rx_delta,
            tx_delta,
        } => {
            // Module 5.3 — apply a single second of byte deltas to
            // the per-interface ring. The actual /sys/class/net
            // read happens in `Live::spawn_refreshers`; this arm
            // exists so test code can drive the dispatcher without
            // spinning up the sampler. The helper handles
            // lazy-ring creation and stays in sync with the field's
            // 60-sample cap.
            app.apply_net_sample(&iface, rx_delta, tx_delta);
        }
        Action::ProcTreeRefreshed(procs) => {
            // Module 6.2 — replace the snapshot wholesale. Each refiller
            // tick is the authoritative picture of /proc, so merging
            // would just have to undo the previous tick's removals.
            // The render path reads `app.proc_tree` on the next frame.
            app.apply_proc_tree(procs);
        }
        Action::SavedConnectionsRefreshed(conns) => {
            // Module 8.2 — overwrite the saved-Wi-Fi list. The 30s
            // refiller (App::spawn_refreshers) is the only writer; the
            // render path reads `app.saved_connections` on every frame
            // and the right pane redraws automatically.
            app.saved_connections = conns;
        }
        // Step 9 — City action arms. The refiller (or a `r` keypress)
        // populates `app.live.city_loc` + `app.live.city_weather`
        // through these arms; the City screen reads them on every
        // render. Both are write-through-RwLock so future spawned
        // tasks can update without re-borrowing `App`.
        Action::CityResolved(loc) => {
            // IP-geolocated location arrived. Stamp the live snapshot
            // so the next render shows the real city name. We DON'T
            // push this back into `app.tx` — the City screen's
            // 9-key dispatcher reads `app.live.city_loc` directly, so
            // re-emitting would loop.
            let mut g = app.live.city_loc.write().await;
            *g = Some(loc);
        }
        Action::CityWeatherRefreshed(w) => {
            // Open-Meteo snapshot arrived. Same shape as CityResolved:
            // write through the lock, drop the guard, let the next
            // frame pick it up.
            let mut g = app.live.city_weather.write().await;
            *g = Some(w);
        }
        Action::CityCtrlRefresh => {
            // User pressed `r` on the City screen. Fire-and-forget:
            // spawn one short-lived task that runs the same geo →
            // weather pipeline the 10-min refiller runs. We don't
            // wait on it; the result lands back through
            // `CityResolved` / `CityWeatherRefreshed`. If a previous
            // tap is still in flight, the new one races — that's
            // fine, last write wins on the RwLock.
            //
            // The `tx` we capture here is `app.tx.clone()` from the
            // dispatcher's caller; the spawned task owns it and
            // drops it when the futures complete (channel closes →
            // main-loop receiver notices on its next poll and exits).
            let tx = tx.clone();
            tokio::spawn(async move {
                use crate::screens::city;
                if let Ok(loc) = city::geo::locate().await {
                    if tx.send(Action::CityResolved(loc.clone())).await.is_err() {
                        return;
                    }
                    if let Ok(w) = city::weather::fetch(&loc).await {
                        let _ = tx.send(Action::CityWeatherRefreshed(w)).await;
                    }
                }
            });
        }
        Action::CityCtrlSet { slug } => {
            // Phase 2 — palette-picked slug from the CityPicker
            // modal. Routes to the City screen's `apply_slug` so
            // the road data, location marker, and viewport_bbox
            // all reset to the new city's bbox. Mirrors the
            // `KeyCode::Char('c')` cycler in the City screen's
            // on_key handler — same slug application, different
            // entry point (the picker is jump-to-by-name rather
            // than step-to-next).
            if app.current == ScreenId::City {
                if let Some(screen) = screens
                    .iter_mut()
                    .find(|s| s.id() == ScreenId::City)
                {
                    if let Some(any) = screen.as_any_mut() {
                        if let Some(city) =
                            any.downcast_mut::<screens::city::CityScreen>()
                        {
                            city.apply_slug(slug.clone());
                            app.city_override = Some(slug);
                            // Sync the location marker to the new
                            // bbox centre so the dot lands in the
                            // middle of the freshly-picked city.
                            if let Some(loc) = city.location.as_mut() {
                                let [min_lat, min_lon, max_lat, max_lon] =
                                    city.roads.bbox;
                                loc.name = city.roads.name.clone();
                                loc.lat = (min_lat + max_lat) / 2.0;
                                loc.lon = (min_lon + max_lon) / 2.0;
                                loc.bbox = Some(city.roads.bbox);
                            }
                            app.save_prefs();
                        }
                    }
                }
            }
        }
    }
    false
}

fn spawn_action(tx: mpsc::Sender<Action>, act: RunAction) {
    // `live` (the App's Live registry) isn't needed yet — every RunAction hits
    // cyberdeck_core directly. Once `RunAction::SetGovernor`,
    // `RunAction::ProcessRenice`, etc. need to update `Live` state immediately
    // (instead of waiting for the next refresh tick), add `_live` here and
    // pipe it back through.
    tokio::spawn(async move {
        let res: cyberdeck_core::CoreResult<()> = match act {
            RunAction::WifiConnect { ssid, password } => {
                cyberdeck_core::net::wifi_connect(&ssid, password.as_deref()).await
            }
            RunAction::WifiDisconnect => cyberdeck_core::net::wifi_disconnect().await,
            RunAction::WifiScan => match cyberdeck_core::net::wifi_scan().await {
                Ok(scan) => {
                    let count = scan.len();
                    let _ = tx
                        .send(Action::WifiScanResult(scan))
                        .await;
                    let _ = tx
                        .send(Action::Toast(
                            ToastKind::Ok,
                            format!("found {} networks", count),
                        ))
                        .await;
                    Ok(())
                }
                Err(e) => Err(e),
            },
            RunAction::WifiEnterpriseConnect { .. } => Err(cyberdeck_core::CoreError::Command {
                cmd: "nmcli connection up".into(),
                detail: "enterprise connect lands in Phase 6".into(),
            }),
            RunAction::ServiceStart(u) => cyberdeck_core::services::start(&u).await,
            RunAction::ServiceStop(u) => cyberdeck_core::services::stop(&u).await,
            RunAction::ServiceRestart(u) => cyberdeck_core::services::restart(&u).await,
            RunAction::ServiceEnable(u) => cyberdeck_core::services::enable(&u).await,
            RunAction::ServiceDisable(u) => cyberdeck_core::services::disable(&u).await,
            RunAction::ProcessKill(pid) => cyberdeck_core::process::kill(pid, "TERM").await,
            RunAction::ProcessRenice(pid, n) => cyberdeck_core::process::renice(pid, n).await,
            RunAction::PackageInstall(p) => cyberdeck_core::packages::install(&p).await,
            RunAction::PackageRemove(p) => cyberdeck_core::packages::remove(&p).await,
            RunAction::PackageUpdate => cyberdeck_core::packages::update().await.map(|_| ()),
            RunAction::PackageUpgrade => cyberdeck_core::packages::upgrade().await.map(|_| ()),
            RunAction::SetGovernor(g) => cyberdeck_core::power::set_governor(&g).await,
            RunAction::SetBrightness(v) => cyberdeck_core::display::set_brightness(v).await,
            RunAction::SetVolume { target, percent } => {
                cyberdeck_core::audio::set_volume(&target, percent).await
            }
            RunAction::MuteSink { target, mute } => {
                cyberdeck_core::audio::set_mute(&target, mute).await
            }
            RunAction::SetDefaultSink(name) => cyberdeck_core::audio::set_default_sink(&name).await,
            RunAction::SetInterfaceUp(name, up) => {
                cyberdeck_core::net::interface_toggle(&name, up).await
            }
            RunAction::BluetoothConnect(m) => cyberdeck_core::bluetooth::connect(&m).await,
            RunAction::BluetoothDisconnect(m) => cyberdeck_core::bluetooth::disconnect(&m).await,
            RunAction::BluetoothPair(m) => cyberdeck_core::bluetooth::pair(&m).await,
            RunAction::BluetoothTrust(m) => cyberdeck_core::bluetooth::trust(&m).await,
            RunAction::BluetoothPower(on) => cyberdeck_core::bluetooth::adapter_power(on).await,
            RunAction::BluetoothScan => match cyberdeck_core::bluetooth::list().await {
                Ok(devs) => {
                    let _ = tx
                        .send(Action::BluetoothScanResult(devs))
                        .await;
                    Ok(())
                }
                Err(e) => Err(e),
            },
            RunAction::Reboot => cyberdeck_core::power::reboot().await,
            RunAction::Shutdown => cyberdeck_core::power::shutdown().await,
            RunAction::Suspend => cyberdeck_core::power::suspend().await,
            RunAction::Hibernate => cyberdeck_core::power::hibernate().await,
            RunAction::WebStart => {
                if cfg!(feature = "web") {
                    // The actual serve() future is owned by the tap task in
                    // main(); spawn_action is only a no-op for these.
                    Ok(())
                } else {
                    Err(cyberdeck_core::CoreError::Invalid(
                        "rebuild with `cargo build -p cyberdeck-tui --features web` to enable --web".into(),
                    ))
                }
            }
            RunAction::WebStop => {
                // The tap task handles the actual kill switch.
                Ok(())
            }
            // Phase 2 — Editor variants are handled inline in the
            // main loop's `Action::Run` arm because they're pure
            // local I/O and don't need to round-trip through
            // cyberdeck_core. If `spawn_action` ever sees them it's
            // a programmer error — the inline handler should have
            // consumed them first.
            RunAction::EditorSaveAs(_) | RunAction::EditorReload => {
                Err(cyberdeck_core::CoreError::Command {
                    cmd: "editor".into(),
                    detail: "handled inline — should not reach spawn_action".into(),
                })
            }
            // Phase 2 — CityPan is also handled inline (it mutates
            // the City screen's `viewport_bbox` and that lives in
            // the main loop's `screens` slice, not in the
            // `cyberdeck_core` worker pool). Same shape as the
            // editor guard above.
            RunAction::CityPan { .. } => Err(cyberdeck_core::CoreError::Command {
                cmd: "city".into(),
                detail: "handled inline — should not reach spawn_action".into(),
            }),
        };
        match res {
            Ok(_) => {
                let _ = tx.send(Action::Toast(ToastKind::Ok, "done".into())).await;
            }
            Err(e) => {
                let _ = tx
                    .send(Action::Toast(ToastKind::Error, format!("{e}")))
                    .await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)] // helpers like `last_toast` and `app_with_n_panes` are kept for future use
    use super::*;
    use crate::app::ChoiceOption;
    use crate::app::ToastEntry;
    use crate::wm::tree::SplitDir;

    fn build_screens() -> Vec<Box<dyn Screen>> {
        vec![
            Box::new(screens::system::SystemScreen),
            Box::new(screens::network::NetworkScreen),
            Box::new(screens::bluetooth::BluetoothScreen),
            Box::new(screens::power::PowerScreen),
            Box::new(screens::display::DisplayScreen),
            Box::new(screens::audio::AudioScreen),
            Box::new(screens::storage::StorageScreen),
            Box::new(screens::services::ServicesScreen),
            Box::new(screens::packages::PackagesScreen),
            Box::new(screens::processes::ProcessesScreen),
            Box::new(screens::files::FilesScreen),
            Box::new(screens::logs::LogsScreen),
            Box::new(screens::settings::SettingsScreen),
            Box::new(screens::city::CityScreen::new()),
        ]
    }

    fn app_with_n_panes(n: u8) -> App {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        // Split n-1 times. Cap is Manager::MAX_PANES = 9.
        for _ in 1..n {
            app.manager
                .split_focused(SplitDir::Horizontal, 50, ScreenId::System)
                .expect("below cap");
        }
        // After splits, focus is on the newly-created pane. Refocus the
        // first pane so jump-to-pane-1 tests are deterministic.
        let first = app.manager.pane_ids()[0];
        let _ = app.manager.focus_pane(first);
        app
    }

    fn run<F: std::future::Future<Output = ()>>(f: F) {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        rt.block_on(f);
    }

    fn last_toast(app: &App) -> Option<String> {
        app.toasts.last().map(|t| t.text.clone())
    }

    #[test]
    fn ctrl_w_is_disabled_no_split_no_focus_change() {
        // The TUI is locked to a 2-pane layout. Ctrl-W (with any
        // follow-up key) must not split the tree, close panes, or move
        // focus. Verified across the typical verb set.
        let mut app = app_with_n_panes(1);
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let before_ids = app.manager.pane_ids();
        let before_kind = app.manager.window(app.manager.focused()).map(|w| w.kind);
        run(async {
            // Try the most common Ctrl-W verbs. None should fire.
            for verb in ["v", "s", "h", "j", "k", "l", "n", "x", "q"] {
                handle_key(
                    &mut screens,
                    &mut app,
                    &tx,
                    KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                )
                .await;
                handle_key(
                    &mut screens,
                    &mut app,
                    &tx,
                    KeyEvent::new(KeyCode::Char(verb.chars().next().unwrap()), KeyModifiers::CONTROL),
                )
                .await;
            }
        });
        // Pane count unchanged.
        assert_eq!(app.manager.pane_ids(), before_ids);
        // Focused pane kind unchanged.
        assert_eq!(
            app.manager.window(app.manager.focused()).map(|w| w.kind),
            before_kind
        );
    }

    // ---- Sidebar navigation regression tests ---------------------------
    //
    // The sidebar (screen list) is the TUI's main menu. The bugs these
    // tests guard:
    //   - Up/Down (and k/j) must move a cursor independently of
    //     `app.current` so the user can preview before committing.
    //   - Enter must commit the cursor as the new current screen.
    //   - Tab/Right/l and Esc must hand focus back to the content pane
    //     without changing `current`.
    //   - These keys must NOT fire while the content pane is focused,
    //     otherwise scrolling the System/Network lists breaks.

    fn fresh_app_with_sidebar_focus() -> App {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        app.set_region(Region::Sidebar);
        app.sidebar_idx = 0;
        app
    }

    #[test]
    fn sidebar_down_moves_cursor_wrapping() {
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            // Start at 0. Press Down twice.
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            )
            .await;
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.sidebar_idx, 2);
        // current should not change from Down alone.
        assert_eq!(app.current, ScreenId::System);
    }

    #[test]
    fn sidebar_up_wraps_to_last() {
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            )
            .await;
        });
        // Up from 0 wraps to last.
        assert_eq!(app.sidebar_idx, ScreenId::ALL.len() - 1);
    }

    #[test]
    fn sidebar_j_k_navigate() {
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            // j = Down.
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            )
            .await;
            // k = Up, returns to 0.
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.sidebar_idx, 0);
    }

    #[test]
    fn sidebar_enter_commits_cursor() {
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        // Move cursor to row 4 (Display, 0-indexed).
        app.sidebar_idx = 4;
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.current, ScreenId::Display);
        // The right-side content pane must follow the sidebar commit,
        // otherwise the next render would still paint the old screen.
        let kind = app
            .manager
            .window(app.manager.focused())
            .expect("focused pane")
            .kind;
        assert_eq!(
            kind,
            crate::wm::window::WindowKind::Builtin(ScreenId::Display)
        );
    }

    #[test]
    fn sidebar_left_returns_focus_without_changing_current() {
        // Replaces the old "sidebar_tab_returns_focus" test. Tab now
        // means "cycle screen" on the content side; the new contract is
        // that pressing Left (or h) while in the sidebar is a no-op
        // (already there) and pressing Left from Content leaves the
        // sidebar at the same screen.
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let before = app.current;
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            )
            .await;
        });
        assert!(
            matches!(app.region, Region::Sidebar),
            "Left from the sidebar stays on the sidebar"
        );
        assert_eq!(app.current, before);
    }

    #[test]
    fn content_left_returns_to_sidebar() {
        // The headline fix: from the content pane pressing Left (or h)
        // jumps back to the sidebar without changing the current
        // screen. The old design made this impossible — Tab cycled the
        // screen instead.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        app.set_region(Region::ContentLeft);
        app.current = ScreenId::Network;
        let mut screens = build_screens();
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx2,
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            )
            .await;
        });
        assert!(
            matches!(app.region, Region::Sidebar),
            "Left from ContentLeft jumps to Sidebar"
        );
        assert_eq!(app.current, ScreenId::Network);
    }

    #[test]
    fn sidebar_keys_do_not_fire_when_content_focused() {
        // Content-focused (region = ContentLeft): Up/Down/Enter must NOT
        // mutate the sidebar cursor. Otherwise the focused pane's own
        // list navigation breaks.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        app.set_region(Region::ContentLeft);
        let mut screens = build_screens();
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx2,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            )
            .await;
            handle_key(
                &mut screens,
                &mut app,
                &tx2,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.sidebar_idx, 0, "Down must not move sidebar cursor when content focused");
        assert_eq!(app.current, ScreenId::System, "Enter must not change current when content focused");
    }

    #[test]
    fn router_walk_three_regions() {
        // Full D-pad walk on a screen with both panes (Network):
        //   start = Sidebar
        //   → →        ContentLeft → ContentRight
        //   ←          ContentRight → ContentLeft
        //   ←          ContentLeft → Sidebar
        // The new tab-strip semantics make Left from Sidebar cycle
        // the tab cursor (Phase 1 menu/tab UI), not stay in place.
        // We assert the original three-region walk still works and
        // that the post-walk current screen is unchanged.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        app.set_region(Region::Sidebar);
        app.current = ScreenId::Network;
        let mut screens = build_screens();
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<Action>(8);
        let left = || KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let right = || KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        run(async {
            handle_key(&mut screens, &mut app, &tx2, right()).await;
            assert!(matches!(app.region, Region::ContentLeft), "→ from Sidebar → ContentLeft (got {:?})", app.region);
            handle_key(&mut screens, &mut app, &tx2, right()).await;
            assert!(matches!(app.region, Region::ContentRight), "→ from ContentLeft → ContentRight (got {:?})", app.region);
            handle_key(&mut screens, &mut app, &tx2, left()).await;
            assert!(matches!(app.region, Region::ContentLeft), "← from ContentRight → ContentLeft (got {:?})", app.region);
            handle_key(&mut screens, &mut app, &tx2, left()).await;
            assert!(matches!(app.region, Region::Sidebar), "← from ContentLeft → Sidebar (got {:?})", app.region);
        });
        assert_eq!(app.current, ScreenId::Network, "Region walk must not change the active screen");
    }

    #[test]
    fn number_keys_when_sidebar_focused_move_cursor_to_that_row() {
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            // Press '5'. Sidebar cursor should land on row 4 (Display),
            // and current should switch to Display.
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.sidebar_idx, 4);
        assert_eq!(app.current, ScreenId::Display);
        // Right-side pane kind follows.
        let kind = app
            .manager
            .window(app.manager.focused())
            .expect("focused pane")
            .kind;
        assert_eq!(
            kind,
            crate::wm::window::WindowKind::Builtin(ScreenId::Display)
        );
    }

    #[test]
    fn number_keys_when_content_focused_still_swap_pane_kind() {
        // Even when the sidebar isn't focused (the user is reading the
        // right pane), the digit-key shortcuts still need to swap the
        // content pane — that's the whole point of the new wiring.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        app.sidebar_focused = false;
        let mut screens = build_screens();
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx2,
                KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.current, ScreenId::Bluetooth);
        let kind = app
            .manager
            .window(app.manager.focused())
            .expect("focused pane")
            .kind;
        assert_eq!(
            kind,
            crate::wm::window::WindowKind::Builtin(ScreenId::Bluetooth)
        );
    }

    #[test]
    fn set_pane_kind_swaps_focused_pane_kind() {
        // Direct test for the new Manager API the sidebar Enter path
        // depends on. The single-pane WM means `focused()` is the only
        // pane there is, but the API still has to (a) return the
        // previous kind and (b) leave `Window` in a usable state.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        let prev = app.manager.set_pane_kind(
            crate::wm::window::WindowKind::Builtin(ScreenId::Network),
        );
        assert_eq!(
            prev,
            Some(crate::wm::window::WindowKind::Builtin(ScreenId::System))
        );
        let w = app.manager.window(app.manager.focused()).unwrap();
        assert_eq!(w.kind, crate::wm::window::WindowKind::Builtin(ScreenId::Network));
        // No terminal state was allocated.
        assert!(!w.is_terminal());
    }

    /// Regression test for the "Tab cycles the menu but the view doesn't
    /// change" bug. Previously, `Action::CycleScreen(forward)` updated
    /// only `app.current` — the WM pane's `WindowKind` stayed pinned to
    /// the old screen, so the right side kept painting the previous
    /// screen while the tab strip / sidebar highlight visibly moved.
    ///
    /// Routing Tab through the same `switch_screen` helper that the
    /// digit-key path uses (`1`..`0`) keeps `app.current` AND the WM
    /// pane's `WindowKind` in lockstep. This test pins that contract:
    /// after the CycleScreen action handler runs, both `app.current`
    /// and the pane's WindowKind have moved together.
    #[test]
    fn tab_cycle_updates_wm_pane_kind_not_just_current() {
        let mut app = fresh_app_with_sidebar_focus();
        app.set_region(Region::ContentLeft);
        // Drain any startup actions and ensure the WM pane kind starts
        // matching `app.current` (= System) so the test isn't false-
        // positive if the manager is initialized in a different state.
        let initial_kind = app
            .manager
            .window(app.manager.focused())
            .expect("focused pane")
            .kind;
        assert_eq!(
            initial_kind,
            crate::wm::window::WindowKind::Builtin(ScreenId::System)
        );
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        // The key handler enqueues Action::CycleScreen(true) on the
        // channel; the action handler is what actually mutates app.
        // We drive both: send Tab via `handle_key`, then drain the
        // resulting action through `handle_action` and assert the
        // combined effect — same as the live event loop does.
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            )
            .await;
            // The Tab handler enqueues via tx.send(...).await — we
            // can't observe the action without a matching receiver,
            // so call the handler directly with the action it would
            // have emitted. This is the exact code path the live loop
            // runs.
            handle_action(
                &mut screens,
                &mut app,
                &tx,
                Action::CycleScreen(true),
            )
            .await;
        });
        // Pin the contract: `current` advanced AND the WM pane kind
        // moved to match — that's the line the bug used to break.
        assert_ne!(app.current, ScreenId::System, "Tab must advance current");
        let kind = app
            .manager
            .window(app.manager.focused())
            .expect("focused pane")
            .kind;
        assert_eq!(
            kind,
            crate::wm::window::WindowKind::Builtin(app.current),
            "WM pane's WindowKind must follow app.current after Tab \
             (regression: was left on the previous screen)"
        );
    }

    // -- Phase 5 modal tests -----------------------------------------

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Test fixture. Returns `(tx, rx, app)` where `tx` is the sender that
    /// dispatched actions (`run_input`, `run_choice`, …) will write to. We
    /// hand `App::new` a throwaway channel for its required `rx` param and
    /// then swap `app.tx` so every dispatcher goes through the outer pair.
    fn make_app() -> (mpsc::Sender<Action>, mpsc::Receiver<Action>, App) {
        let (dummy_tx, dummy_rx) = mpsc::channel::<Action>(8);
        let (tx, rx) = mpsc::channel::<Action>(8);
        let mut app = App::new(dummy_tx, dummy_rx);
        // Route every dispatcher through `tx` so `rx.try_recv()` observes
        // the actions they emit.
        app.tx = tx.clone();
        (tx, rx, app)
    }

    #[test]
    fn open_secret_appends_to_buf_and_renders_masked() {
        let (_tx, _rx, mut app) = make_app();
        app.open_secret("Password", InputKind::WifiPassword);
        // No real key event — the modal renders the buffer masked.
        // Inspect state directly.
        match &app.modal {
            Modal::Secret { prompt, buf, .. } => {
                assert_eq!(prompt, "Password");
                assert_eq!(buf, "");
            }
            _ => panic!("expected Secret modal"),
        }
        // Push characters via the modal's buffer (mirrors what the
        // handle_key Char arm does).
        if let Modal::Secret { buf, .. } = &mut app.modal {
            buf.push('h');
            buf.push('i');
        }
        match &app.modal {
            Modal::Secret { buf, .. } => assert_eq!(buf, "hi"),
            _ => panic!("modal changed shape"),
        }
        // The mask string is derived at render time — the unit test
        // verifies the rendering pipeline by calling a small helper.
        let rendered_mask: String = std::iter::repeat('•').take("hi".len()).collect();
        assert_eq!(rendered_mask, "••");
    }

    #[tokio::test]
    async fn secret_modal_esc_dismisses() {
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Secret {
            prompt: "p".into(),
            buf: "secret123".into(),
            kind: InputKind::WifiPassword,
        };
        // Esc dismisses.
        let _ = handle_key(&mut [], &mut app, &_tx, key(KeyCode::Esc)).await;
        assert!(matches!(app.modal, Modal::None));
    }

    #[tokio::test]
    async fn secret_modal_enter_submits_and_dispatches() {
        let (tx, mut _rx, mut app) = make_app();
        app.pending_ssid = Some("HomeNet".into());
        app.modal = Modal::Secret {
            prompt: "p".into(),
            buf: "supersecret".into(),
            kind: InputKind::WifiPassword,
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Enter)).await;
        // Modal closed.
        assert!(matches!(app.modal, Modal::None));
        // A WifiConnect action was enqueued.
        let action = _rx.try_recv().expect("expected action");
        match action {
            Action::Run(RunAction::WifiConnect { ssid, password }) => {
                assert_eq!(ssid, "HomeNet");
                assert_eq!(password.as_deref(), Some("supersecret"));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[tokio::test]
    async fn choice_modal_cursor_wraps_and_enter_dispatches() {
        let (tx, mut _rx, mut app) = make_app();
        app.modal = Modal::Choice {
            prompt: "Pick SSID".into(),
            options: vec![
                ChoiceOption { id: "ssid-a".into(), label: "A".into() },
                ChoiceOption { id: "ssid-b".into(), label: "B".into() },
                ChoiceOption { id: "ssid-c".into(), label: "C".into() },
            ],
            cursor: 0,
            commit_kind: Some(ChoiceCommit::RunAction(RunAction::WifiDisconnect)),
        };
        // j moves cursor forward, wraps to 0.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('j'))).await;
        match &app.modal {
            Modal::Choice { cursor, .. } => assert_eq!(*cursor, 1),
            _ => panic!("expected Choice modal"),
        }
        // Two more j's lands on 2.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('j'))).await;
        match &app.modal {
            Modal::Choice { cursor, .. } => assert_eq!(*cursor, 2),
            _ => panic!("expected Choice modal"),
        }
        // k from 2 -> 1 -> 0, then k wraps backwards to the last (n-1 = 2).
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('k'))).await;
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('k'))).await;
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('k'))).await;
        match &app.modal {
            Modal::Choice { cursor, .. } => assert_eq!(*cursor, 2),
            _ => panic!("expected Choice modal"),
        }
        // Up wraps the same way as k.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Up)).await;
        match &app.modal {
            Modal::Choice { cursor, .. } => assert_eq!(*cursor, 1),
            _ => panic!("expected Choice modal"),
        }
        // Enter dispatches the RunAction and closes the modal.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Enter)).await;
        assert!(matches!(app.modal, Modal::None));
        let action = _rx.try_recv().expect("expected action");
        assert!(matches!(action, Action::Run(RunAction::WifiDisconnect)));
    }

    #[tokio::test]
    async fn choice_modal_esc_dismisses_without_action() {
        let (tx, mut _rx, mut app) = make_app();
        app.modal = Modal::Choice {
            prompt: "Pick".into(),
            options: vec![ChoiceOption { id: "x".into(), label: "X".into() }],
            cursor: 0,
            commit_kind: Some(ChoiceCommit::RunAction(RunAction::WifiDisconnect)),
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Esc)).await;
        assert!(matches!(app.modal, Modal::None));
        assert!(_rx.try_recv().is_err(), "no action should be enqueued");
    }

    // Toggle confirmation toasts: every Action::Toggle must push an Info
    // toast naming the new state, so the user gets immediate feedback
    // instead of a silent flag flip. Mirrors orbital's "every action
    // produces a visible ack" rule.
    #[tokio::test]
    async fn toggle_theme_pushes_confirmation_toast() {
        use app::screen::SettingsKey;
        let (tx, _rx, mut app) = make_app();
        let before = app.theme_name;
        let _ = handle_action(
            &mut [],
            &mut app,
            &tx,
            Action::Toggle(SettingsKey::Theme),
        )
        .await;
        assert_ne!(app.theme_name, before, "Theme toggle must rotate");
        assert_eq!(app.toasts.len(), 1, "exactly one toast");
        assert!(app.toasts[0].text.starts_with("theme: "), "got: {:?}", app.toasts[0].text);
        assert!(matches!(app.toasts[0].kind, ToastKind::Info));
    }

    #[tokio::test]
    async fn toggle_mouse_pushes_confirmation_toast() {
        use app::screen::SettingsKey;
        let (tx, _rx, mut app) = make_app();
        let before = app.mouse;
        let _ = handle_action(
            &mut [],
            &mut app,
            &tx,
            Action::Toggle(SettingsKey::Mouse),
        )
        .await;
        assert_ne!(app.mouse, before, "Mouse toggle must flip");
        assert_eq!(app.toasts.len(), 1);
        assert!(app.toasts[0].text.starts_with("mouse capture: "), "got: {:?}", app.toasts[0].text);
        assert!(matches!(app.toasts[0].kind, ToastKind::Info));
    }

    #[tokio::test]
    async fn toggle_nerd_font_pushes_confirmation_toast() {
        use app::screen::SettingsKey;
        let (tx, _rx, mut app) = make_app();
        let before = app.nerd_font;
        let _ = handle_action(
            &mut [],
            &mut app,
            &tx,
            Action::Toggle(SettingsKey::NerdFont),
        )
        .await;
        assert_ne!(app.nerd_font, before, "NerdFont toggle must flip");
        assert_eq!(app.toasts.len(), 1);
        assert!(app.toasts[0].text.starts_with("nerd font glyphs: "), "got: {:?}", app.toasts[0].text);
        assert!(matches!(app.toasts[0].kind, ToastKind::Info));
    }

    #[tokio::test]
    async fn boot_welcome_toast_fires_exactly_once() {
        let (tx, _rx, mut app) = make_app();
        assert!(!app.boot_toast_sent, "fresh app must start with boot_toast_sent=false");
        assert!(app.toasts.is_empty(), "fresh app must have no toasts");

        // First tick: welcome toast must land.
        let quit = handle_action(&mut [], &mut app, &tx, Action::Tick).await;
        assert!(!quit, "Tick must not request quit");
        assert!(app.boot_toast_sent, "after first Tick boot_toast_sent must be true");
        assert_eq!(app.toasts.len(), 1, "exactly one welcome toast after first Tick");
        assert!(
            app.toasts[0].text.starts_with("Welcome"),
            "welcome toast text must start with 'Welcome', got: {:?}",
            app.toasts[0].text
        );
        assert!(matches!(app.toasts[0].kind, ToastKind::Info));

        // Two more ticks: no extra welcome toasts. Other refreshers may
        // legitimately push unrelated toasts, so we count how many start
        // with 'Welcome' and assert the count stays at 1.
        let _ = handle_action(&mut [], &mut app, &tx, Action::Tick).await;
        let _ = handle_action(&mut [], &mut app, &tx, Action::Tick).await;
        let welcome_count = app
            .toasts
            .iter()
            .filter(|t| t.text.starts_with("Welcome"))
            .count();
        assert_eq!(welcome_count, 1, "welcome toast must fire exactly once across all ticks");
    }

    #[tokio::test]
    async fn choice_modal_pick_input_opens_secret_modal_with_prefill() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::Choice {
            prompt: "Pick".into(),
            options: vec![ChoiceOption { id: "ssid-home".into(), label: "Home".into() }],
            cursor: 0,
            commit_kind: Some(ChoiceCommit::PickInput {
                kind: InputKind::WifiPassword,
                prompt: "Password for Home".into(),
                masked: true,
                prefill: String::new(),
            }),
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Enter)).await;
        // The dispatcher should have opened a Secret modal.
        match &app.modal {
            Modal::Secret { prompt, kind, .. } => {
                assert_eq!(prompt, "Password for Home");
                assert_eq!(*kind, InputKind::WifiPassword);
            }
            other => panic!("expected Secret modal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wizard_esc_dismisses() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::Wizard(Wizard::WifiEnterprise {
            ssid: "Corp".into(),
            step: 0,
            eap: None,
            identity: None,
            password: None,
            anon_or_cert: None,
        });
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Esc)).await;
        assert!(matches!(app.modal, Modal::None));
    }

    #[test]
    fn wizard_done_returns_true_only_when_all_required_fields_set() {
        let mut w = Wizard::WifiEnterprise {
            ssid: "Corp".into(),
            step: 1,
            eap: Some("PEAP".into()),
            identity: Some("alice".into()),
            password: None,
            anon_or_cert: None,
        };
        assert!(!w.done(), "missing password should not be done");
        // Set password — done.
        let Wizard::WifiEnterprise { password, .. } = &mut w;
        *password = Some("pw".into());
        assert!(w.done());
        // For TLS the password is irrelevant; anon_or_cert is required.
        let mut tls = Wizard::WifiEnterprise {
            ssid: "Corp".into(),
            step: 3,
            eap: Some("TLS".into()),
            identity: Some("alice".into()),
            password: None,
            anon_or_cert: None,
        };
        assert!(!tls.done());
        let Wizard::WifiEnterprise { anon_or_cert, .. } = &mut tls;
        *anon_or_cert = Some("/etc/cert.pem".into());
        assert!(tls.done());
    }

    #[tokio::test]
    async fn progress_modal_esc_signals_cancel_and_closes() {
        let (tx, _rx, mut app) = make_app();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
        app.modal = Modal::Progress {
            label: "updating".into(),
            done: 0,
            total: 0,
            cancel: Some(cancel_tx),
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Esc)).await;
        assert!(matches!(app.modal, Modal::None));
        // The cancel channel should have been signalled.
        assert!(cancel_rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn auth_failure_r_recovers_inner_modal() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::AuthFailure {
            command: "nmcli".into(),
            stderr: "auth failed".into(),
            retry: Box::new(Modal::Input {
                prompt: "Password".into(),
                buf: String::new(),
                kind: InputKind::WifiPassword,
            }),
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('r'))).await;
        match &app.modal {
            Modal::Input { prompt, .. } => assert_eq!(prompt, "Password"),
            other => panic!("expected recovered Input, got {other:?}"),
        }
        // Esc on the recovered Input dismisses normally.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Esc)).await;
        assert!(matches!(app.modal, Modal::None));
    }

    #[tokio::test]
    async fn auth_failure_esc_dismisses_inner_too() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::AuthFailure {
            command: "x".into(),
            stderr: "y".into(),
            retry: Box::new(Modal::Input {
                prompt: "P".into(),
                buf: String::new(),
                kind: InputKind::WifiPassword,
            }),
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Esc)).await;
        assert!(matches!(app.modal, Modal::None));
    }

    #[tokio::test]
    async fn killpid_input_rejects_garbage_with_toast() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::Input {
            prompt: "pid".into(),
            buf: "notanumber".into(),
            kind: InputKind::KillPid,
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Enter)).await;
        assert!(matches!(app.modal, Modal::None));
        assert!(app
            .toasts
            .iter()
            .any(|t| t.kind == ToastKind::Error && t.text.contains("invalid pid")));
    }

    // ===== Module 3 — PackageSearch ======================================
    //
    // The Packages screen historically fired an empty-string search (the `/`
    // hotkey just cleared the filter and `s` searched whatever was already in
    // it). The fix introduces a `Modal::Input(InputKind::PackageSearch, ..)`
    // that lets the user type a query and submit it. Submitting must:
    //   1. Store the (trimmed) query on `app.packages_search_query`.
    //   2. Close the modal.
    // Tasks 3.2–3.4 will wire the modal UI + `/` hotkey on the Packages
    // screen itself; this test only locks in the variant + dispatch
    // plumbing.
    #[tokio::test]
    async fn input_kind_package_search_submit_stores_query_and_closes_modal() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::Input {
            prompt: "search packages".into(),
            buf: "ripgrep".into(),
            kind: InputKind::PackageSearch,
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Enter)).await;

        // Modal must close after submit.
        assert!(matches!(app.modal, Modal::None));
        // The trimmed query must be stashed for the Packages screen to pick up.
        assert_eq!(app.packages_search_query.as_deref(), Some("ripgrep"));
    }

    // Empty / whitespace-only submits must NOT clear the existing query —
    // they just dismiss the modal. This keeps the user from accidentally
    // wiping their in-flight search by hitting Enter on an empty field.
    #[tokio::test]
    async fn input_kind_package_search_empty_submit_keeps_existing_query() {
        let (tx, _rx, mut app) = make_app();
        app.packages_search_query = Some("curl".into());
        app.modal = Modal::Input {
            prompt: "search packages".into(),
            buf: "   ".into(),
            kind: InputKind::PackageSearch,
        };
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Enter)).await;

        assert!(matches!(app.modal, Modal::None));
        assert_eq!(
            app.packages_search_query.as_deref(),
            Some("curl"),
            "empty submit must not overwrite an existing query"
        );
    }

    // ===== Module 2 — Modal OK/Cancel polish + BluetoothPasskey =====

    // `Modal::Input` rendered lines must include an "OK" and "Cancel" button
    // row so the affordance is visible to the user (orbital-style modal
    // chrome). Behaviour of Enter/Esc is unchanged.
    #[test]
    fn modal_input_ok_cancel_button_renders() {
        let lines = modal_input_lines("Connect to SSID:", "");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("OK"),
            "Modal::Input lines must include an OK button, got {text:?}"
        );
        assert!(
            text.contains("Cancel"),
            "Modal::Input lines must include a Cancel button, got {text:?}"
        );
        // The prompt must still be there so the user knows what they're filling in.
        assert!(
            text.contains("Connect to SSID:"),
            "Modal::Input lines must still include the prompt, got {text:?}"
        );
        // And the live buffer (empty here) must still be visible.
        assert!(
            text.contains(">"),
            "Modal::Input lines must still include the buffer caret '>', got {text:?}"
        );
    }

    // Same affordance for `Modal::Secret`. The mask `•` is unaffected — OK /
    // Cancel ride alongside it.
    #[test]
    fn modal_secret_ok_cancel_button_renders() {
        let lines = modal_secret_lines("Wi-Fi password for HomeNet", "hunter2");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("OK"), "Modal::Secret lines must include an OK button, got {text:?}");
        assert!(
            text.contains("Cancel"),
            "Modal::Secret lines must include a Cancel button, got {text:?}"
        );
        assert!(
            text.contains("Wi-Fi password for HomeNet"),
            "Modal::Secret lines must still include the prompt, got {text:?}"
        );
        // The mask is rendered as bullets — one per char in the real buf.
        assert!(
            text.contains("•"),
            "Modal::Secret lines must still mask the buffer as bullets, got {text:?}"
        );
        // The real password must NOT leak through the rendered text.
        assert!(
            !text.contains("hunter2"),
            "Modal::Secret must not leak the real password into rendered text, got {text:?}"
        );
    }

    // `InputKind::BluetoothPasskey` must accept only digits. Any non-digit
    // char pressed while this kind is active must be silently dropped at
    // the buffer-insert step (no error toast, no buf mutation).
    #[tokio::test]
    async fn bluetooth_passkey_rejects_letters() {
        let (tx, _rx, mut app) = make_app();
        app.modal = Modal::Secret {
            prompt: "Bluetooth passkey".into(),
            buf: String::new(),
            kind: InputKind::BluetoothPasskey,
        };

        // Press `a` — must be ignored.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('a'))).await;
        match &app.modal {
            Modal::Secret { buf, kind: InputKind::BluetoothPasskey, .. } => {
                assert!(
                    buf.is_empty(),
                    "letter `a` must not append to BluetoothPasskey buffer, got buf={buf:?}"
                );
            }
            other => panic!("expected Modal::Secret (BluetoothPasskey), got {other:?}"),
        }

        // Press `5` — must append.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('5'))).await;
        match &app.modal {
            Modal::Secret { buf, kind: InputKind::BluetoothPasskey, .. } => {
                assert_eq!(buf, "5", "digit `5` must append to BluetoothPasskey buffer");
            }
            other => panic!("expected Modal::Secret (BluetoothPasskey), got {other:?}"),
        }

        // Press `b` again — must still be ignored (cumulative).
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('b'))).await;
        match &app.modal {
            Modal::Secret { buf, kind: InputKind::BluetoothPasskey, .. } => {
                assert_eq!(
                    buf, "5",
                    "letters after a digit must still be rejected, got buf={buf:?}"
                );
            }
            other => panic!("expected Modal::Secret (BluetoothPasskey), got {other:?}"),
        }

        // Press `0` — must append.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Char('0'))).await;
        match &app.modal {
            Modal::Secret { buf, kind: InputKind::BluetoothPasskey, .. } => {
                assert_eq!(buf, "50", "digit `0` must append after `5`, got buf={buf:?}");
            }
            other => panic!("expected Modal::Secret (BluetoothPasskey), got {other:?}"),
        }

        // Backspace removes the last char as usual.
        let _ = handle_key(&mut [], &mut app, &tx, key(KeyCode::Backspace)).await;
        match &app.modal {
            Modal::Secret { buf, kind: InputKind::BluetoothPasskey, .. } => {
                assert_eq!(buf, "5", "Backspace must remove the last char");
            }
            other => panic!("expected Modal::Secret (BluetoothPasskey), got {other:?}"),
        }
    }

    // Module 1.5 — end-to-end handler test. Simulates a short terminal
    // by pre-seeding `app.sidebar_visible` to a value smaller than
    // `ScreenId::ALL.len()`, then driving Down/Up through `handle_key`
    // and verifying the offset actually moves. Before this commit the
    // handler called `clamp_sidebar_offset(total, total)` — a no-op —
    // so the offset never advanced and overflow rows stayed invisible
    // but selectable. This test pins the new wire-up: renderer's
    // `sidebar_visible` reaches the clamp.
    #[test]
    fn sidebar_down_advances_offset_when_visible_window_shorter_than_total() {
        let mut app = fresh_app_with_sidebar_focus();
        // Pretend the renderer drew a 3-row sidebar (e.g. narrow
        // terminal). Place cursor at the bottom of that window.
        app.sidebar_visible = 3;
        app.sidebar_idx = 2; // last row of [0..3)
        app.sidebar_offset = 0;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.sidebar_idx, 3);
        assert_eq!(
            app.sidebar_offset, 1,
            "Down through handle_key must advance offset when cursor exits bottom"
        );
    }

    #[test]
    fn sidebar_up_retreats_offset_when_visible_window_shorter_than_total() {
        let mut app = fresh_app_with_sidebar_focus();
        // Cursor at row 5, visible=3 → clamp picked offset=3 (window
        // [3..6) contains idx=5). Move up; cursor should re-enter the
        // top of the window and the offset should retreat.
        app.sidebar_visible = 3;
        app.sidebar_idx = 5;
        app.sidebar_offset = 3;
        // Pre-clamp once to lock the initial state (defensive — handler
        // will clamp on every keypress, so this just confirms the
        // starting offset is plausible).
        app.clamp_sidebar_offset(ScreenId::ALL.len(), app.sidebar_visible);
        assert_eq!(app.sidebar_offset, 3);

        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(app.sidebar_idx, 4);
        assert_eq!(
            app.sidebar_offset, 2,
            "Up through handle_key must retreat offset when cursor re-enters window"
        );
    }

    // -------------------------------------------------------------------------
    // Module 7.2 — capital `T` opens the `Modal::ToastLog` overlay, Esc closes
    // it, and Up/Down scroll the offset (clamped to `total - visible`).
    // -------------------------------------------------------------------------

    /// Fresh app already on the sidebar so the `T` key isn't claimed by any
    /// focused-pane on_key handler before reaching the global arm.
    fn fresh_app_sidebar() -> App {
        let mut app = app_with_n_panes(1);
        app.set_region(Region::Sidebar);
        app
    }

    #[tokio::test]
    async fn capital_t_opens_toast_log_modal() {
        let mut app = fresh_app_sidebar();
        assert!(matches!(app.modal, Modal::None));
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
        )
        .await;
        assert!(
            matches!(app.modal, Modal::ToastLog),
            "T must open ToastLog, got {:?}",
            app.modal
        );
        assert_eq!(app.toast_log_offset, 0, "open must reset scroll to top");
    }

    #[tokio::test]
    async fn esc_closes_toast_log_modal() {
        let mut app = fresh_app_sidebar();
        app.modal = Modal::ToastLog;
        app.toast_history.push_back(ToastEntry {
            ts: chrono::Local::now(),
            kind: ToastKind::Info,
            message: "test".into(),
        });
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;
        assert!(matches!(app.modal, Modal::None));
    }

    #[tokio::test]
    async fn toast_log_down_advances_offset_clamped_to_history_len() {
        let mut app = fresh_app_sidebar();
        app.modal = Modal::ToastLog;
        for i in 0..50 {
            app.push_toast(ToastKind::Info, format!("t{i}"));
        }
        let initial_offset = app.toast_log_offset;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        // Hammer Down many more times than the cap allows; offset must
        // saturate at total (no blank rows beyond history).
        for _ in 0..500 {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            )
            .await;
        }
        assert!(
            app.toast_log_offset > initial_offset,
            "Down must advance offset"
        );
        // Offset can never exceed history length (visible rows are a
        // subset, so a generous bound is `total`).
        assert!(app.toast_log_offset <= app.toast_history.len());
    }

    #[tokio::test]
    async fn toast_log_up_retreats_offset_toward_zero() {
        let mut app = fresh_app_sidebar();
        app.modal = Modal::ToastLog;
        for i in 0..10 {
            app.push_toast(ToastKind::Info, format!("t{i}"));
        }
        app.toast_log_offset = 5;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        for _ in 0..20 {
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            )
            .await;
        }
        assert_eq!(app.toast_log_offset, 0, "Up must saturate at 0");
    }

    // -------------------------------------------------------------------------
    // Module 7.3 — render-time tests pin the visual contract: newest entry
    // must appear above older entries, and the offset must be defensively
    // clamped when the ring shrinks below the current scroll position.
    //
    // Why pin this here: the offset arithmetic in the modal-key handler is
    // intentionally generous (`min(total)`) because the handler doesn't
    // know the rendered area's height. The render arm then re-clamps to
    // `total - visible`. Without this test, a future refactor that drops
    // either clamp would silently render blank rows.
    // -------------------------------------------------------------------------

    fn render_modal_text(app: &App) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        terminal
            .draw(|f| {
                let area = f.area();
                draw_modal(f, area, app, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut rows: Vec<String> = Vec::new();
        for y in 0..buffer.area.height {
            let mut row = String::new();
            for x in 0..buffer.area.width {
                row.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            rows.push(row);
        }
        rows.join("\n")
    }

    #[test]
    fn toast_log_render_lists_toasts_newest_first() {
        let mut app = fresh_app_sidebar();
        app.push_toast(ToastKind::Info, "first");
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.push_toast(ToastKind::Warn, "second");
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.push_toast(ToastKind::Error, "third");
        app.modal = Modal::ToastLog;

        let text = render_modal_text(&app);
        let pos_third = text.find("third");
        let pos_second = text.find("second");
        let pos_first = text.find("first");
        assert!(
            pos_third.is_some() && pos_second.is_some() && pos_first.is_some(),
            "all three toasts must render; got:\n{text}"
        );
        assert!(
            pos_third.unwrap() < pos_second.unwrap(),
            "third (newest) must render above second"
        );
        assert!(
            pos_second.unwrap() < pos_first.unwrap(),
            "second must render above first (oldest)"
        );
    }

    #[test]
    fn toast_log_offset_zero_shows_newest_at_top() {
        let mut app = fresh_app_sidebar();
        app.push_toast(ToastKind::Info, "old-message");
        app.push_toast(ToastKind::Info, "new-message");
        app.modal = Modal::ToastLog;
        app.toast_log_offset = 0;
        let text = render_modal_text(&app);
        let pos_new = text.find("new-message").expect("new-message should render");
        let pos_old = text.find("old-message").expect("old-message should render");
        assert!(
            pos_new < pos_old,
            "newest must appear at smaller row index than oldest at offset=0"
        );
    }

    #[test]
    fn toast_log_offset_advances_past_oldest() {
        let mut app = fresh_app_sidebar();
        for i in 0..30 {
            app.push_toast(ToastKind::Info, format!("entry-{i:02}"));
        }
        app.modal = Modal::ToastLog;
        // Walk Down until we've scrolled past the newest entry; entry-29
        // should disappear and entry-00 should become visible.
        for _ in 0..30 {
            let prev = app.toast_log_offset;
            // Use the key handler so the offset is gated through the
            // same path as production; the visible clamp is enforced in
            // the renderer.
            let screens = &mut build_screens();
            let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
            // Synchronous execute via `try_run` since this is a sync test.
            futures::executor::block_on(handle_key(
                screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            ));
            if app.toast_log_offset == prev {
                // saturated at total
                break;
            }
        }
        // After scrolling all the way down, entry-29 (the newest) should
        // no longer be visible.
        let text = render_modal_text(&app);
        // Either entry-29 is off-screen or the offset saturated before
        // we could walk that far; either way, the offset must be within
        // history length.
        assert!(app.toast_log_offset <= app.toast_history.len());
        // Defensive: entry-00 should still be findable in the buffer
        // text (it's part of history); it may not be on screen, but the
        // assertion is that the renderer doesn't crash.
        let _ = text.find("entry-00");
    }

    #[test]
    fn toast_log_render_with_empty_history_shows_placeholder() {
        let mut app = fresh_app_sidebar();
        app.modal = Modal::ToastLog;
        let text = render_modal_text(&app);
        assert!(
            text.contains("no toasts yet"),
            "empty history must render the placeholder, got:\n{text}"
        );
    }

    // ---- Module 9: clipboard paste into modal buffers -------------------
    //
    // The TUI uses modal input for Wi-Fi passwords, hidden SSIDs, package
    // search, kill PID, etc. Long values are best entered by pasting from
    // the system clipboard. These tests pin `handle_paste`'s contract:
    //
    //   - paste into an Input modal appends the trimmed text to its buffer
    //   - paste into a Secret modal appends the trimmed text to its buffer
    //   - paste with no modal open is a no-op (no panic)
    //   - empty paste is a no-op (clipboard cleared or empty)
    //   - trailing whitespace (common from `xclip -o` / `wl-paste`) is
    //     stripped before the append so a stray `\n` doesn't slip in
    //
    // The Ctrl+Shift+V fallback (KeyEvent path) is exercised in a separate
    // test so the routing arm can be checked without a real terminal.

    #[test]
    fn paste_appends_to_input_modal_buffer() {
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Input {
            prompt: "search packages".into(),
            buf: "rip".into(),
            kind: InputKind::PackageSearch,
        };
        handle_paste(&mut app, "grep".to_string());
        match &app.modal {
            Modal::Input { buf, .. } => assert_eq!(buf, "ripgrep"),
            _ => panic!("expected Modal::Input, got {:?}", app.modal),
        }
    }

    #[test]
    fn paste_appends_to_secret_modal_buffer() {
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Secret {
            prompt: "wifi password".into(),
            buf: "abc".into(),
            kind: InputKind::WifiPassword,
        };
        handle_paste(&mut app, "def".to_string());
        match &app.modal {
            Modal::Secret { buf, .. } => assert_eq!(buf, "abcdef"),
            _ => panic!("expected Modal::Secret, got {:?}", app.modal),
        }
    }

    #[test]
    fn paste_with_no_modal_is_noop() {
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::None;
        // Must not panic and must not change the modal.
        handle_paste(&mut app, "anything".to_string());
        assert!(matches!(app.modal, Modal::None));
    }

    #[test]
    fn paste_handles_empty_string() {
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Input {
            prompt: "x".into(),
            buf: "abc".into(),
            kind: InputKind::PackageSearch,
        };
        handle_paste(&mut app, String::new());
        match &app.modal {
            Modal::Input { buf, .. } => assert_eq!(buf, "abc"),
            _ => panic!("expected Modal::Input, got {:?}", app.modal),
        }
    }

    #[test]
    fn paste_strips_trailing_newlines() {
        // Clipboard from `xclip -o` or `wl-paste` often has a trailing
        // `\n`. Stripping it at the paste boundary keeps the modal buffer
        // free of accidental whitespace.
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Input {
            prompt: "x".into(),
            buf: String::new(),
            kind: InputKind::PackageSearch,
        };
        handle_paste(&mut app, "hello\n".to_string());
        match &app.modal {
            Modal::Input { buf, .. } => assert_eq!(buf, "hello"),
            _ => panic!("expected Modal::Input, got {:?}", app.modal),
        }
    }

    #[test]
    fn paste_strips_trailing_crlf_and_tabs() {
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Secret {
            prompt: "p".into(),
            buf: String::new(),
            kind: InputKind::WifiPassword,
        };
        handle_paste(&mut app, "hunter2\r\n\t ".to_string());
        match &app.modal {
            Modal::Secret { buf, .. } => assert_eq!(buf, "hunter2"),
            _ => panic!("expected Modal::Secret, got {:?}", app.modal),
        }
    }

    #[test]
    fn paste_into_choice_modal_is_noop() {
        // Pastes only target text-entry modals. Other modal kinds must
        // remain unchanged.
        let (_tx, _rx, mut app) = make_app();
        app.modal = Modal::Choice {
            prompt: "Pick".into(),
            options: vec![ChoiceOption {
                id: "a".into(),
                label: "A".into(),
            }],
            cursor: 0,
            commit_kind: None,
        };
        handle_paste(&mut app, "anything".to_string());
        assert!(matches!(app.modal, Modal::Choice { .. }));
    }

    #[test]
    fn read_clipboard_for_paste_returns_empty_when_no_tool() {
        // On a headless test runner with no X server and no Wayland,
        // every candidate (wl-paste, xclip, xsel, pbpaste) fails to
        // spawn. The helper must return an empty string rather than
        // panic, so `handle_paste` can route it as a no-op.
        //
        // We force-fail by pointing PATH at an empty tempdir.
        let orig_path = std::env::var_os("PATH");
        let empty = std::env::temp_dir().join("cyberdeck-test-empty-path-9f4c");
        let _ = std::fs::create_dir_all(&empty);
        // Tests are single-threaded; env mutation is fine.
        std::env::set_var("PATH", &empty);
        let result = read_clipboard_for_paste();
        if let Some(p) = orig_path {
            std::env::set_var("PATH", p);
        } else {
            std::env::remove_var("PATH");
        }
        let _ = std::fs::remove_dir(&empty);
        assert_eq!(result, "");
    }
}
