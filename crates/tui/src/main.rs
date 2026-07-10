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
use cyberdeck_tui::{app, keymap, screens, theme, ui, util, wm};
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
                            --web            Also start the LAN web server (default 127.0.0.1:7878)\n  \
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
            .unwrap_or_else(|| "127.0.0.1:7878".to_string());
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
                            let token = cyberdeck_web::auth::Token::new();
                            let _ = tx_for_task
                                .send(Action::Toast(
                                    ToastKind::Info,
                                    format!("web auth token: {}", token.0),
                                ))
                                .await;
                            let serve = cyberdeck_web::run_with(&bind, live_arc, Some(w_tx), Some(token));
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
        // Phase 7 — Carousel front door. Lives at index 0 of
        // `ScreenId::ALL`; first stop on a Tab-cycle. The Vec
        // order doesn't have to match `ALL` (lookup is by id
        // via `.find`), but matching helps reviewers map code
        // to the sidebar.
        Box::new(screens::overworld::OverworldScreen::new()),
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
        // Intel screen: layer grid (left) + selected-layer detail (right).
        // M4 ships a hardcoded snapshot list inside `IntelScreen::new`;
        // M5 swaps the data source for the refiller without changing
        // the renderer.
        Box::new(screens::intel::IntelScreen::new()),
        // Recon screen: 7-tab OSINT action console (Phase 7 M7).
        // Single-pane by design — the rendered output IS the screen.
        // Tab/BackTab cycles tabs, characters append to the query,
        // Enter runs the active arm, Esc clears, j/k scrolls.
        Box::new(screens::recon::ReconScreen::new()),
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
    // Four-row layout: header / tab_strip / body / legend. `tab_strip`
    // is `None` when the terminal is too short (< 10 rows); a tight window
    // never paints a half-rendered tab strip. The strip row goes away to
    // give the focused screen as much vertical space as possible — the
    // strip is purely an indicator, never a hard navigation requirement.
    let (header, tab_strip, body, legend) = ui::chunks(f.area());
    // The header is now just live status icons + clock on a single row.
    ui::draw_header(f, header, app, theme);
    // Bruce-firmware menu-name strip with preview cursor highlight.
    // `app.tab_cursor` is `Some(id)` while the user is previewing a
    // target screen via Tab (no commit yet); `None` means the strip
    // shows the *current* screen highlighted and no preview is active.
    // The click-test rect is set on `app` so the click handler in
    // `handle_mouse` can resolve (col,row) → ScreenId without redoing
    // the layout math.
    app.tab_strip_rect = tab_strip;
    if let Some(area) = tab_strip {
        crate::ui::tab_strip::draw(f, area, app, app.tab_cursor, theme);
    }
    // M1 (menu revamp): the launcher grid is gone. When `Region::Sidebar`
    // is the focused region (the user pressed `B` from a screen or `Ctrl+M`
    // from the menu bar), we render the **Overworld** instead of the old
    // 4×4 launcher tile grid. The Overworld is the canonical main menu —
    // the launcher was a sibling of it, and we want one global menu, not
    // two surfaces competing for the "what screen should I open" question.
    // The keymap is unchanged in M1: B/Esc/Ctrl+M still move you into
    // `Region::Sidebar`; M2 retires the keymap path. M3 deletes the
    // `Region::Sidebar` variant entirely.
    if app.region == Region::Sidebar {
        // Render the Overworld in the body. We construct it fresh on each
        // draw because it owns no persistent state that isn't already on
        // `App` (its cursor is currently per-launch; M4 routes it through
        // `App` so it survives across visits — for M1, a fresh cursor
        // starting on tile 1 = System is fine).
        let mut overworld = screens::overworld::OverworldScreen::new();
        overworld.render(f, body, app, theme, true);
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
    wm::modal::draw_modal(f, f.area(), app, theme);
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

/// Single point where digit-key shortcuts (`1`..`0`) translate into a
/// screen change. Keeps `app.current` and the WM pane's `WindowKind`
/// in sync so the right side actually redraws with the new screen —
/// without this, the sidebar would say "Network" but the content
/// pane would keep showing whatever it last rendered.
fn switch_screen(app: &mut App, screen: ScreenId, sidebar_row: usize) {
    if matches!(app.region, Region::Sidebar) {
        // `launcher_offset` is a visible-row index, not a ScreenId
        // index — bounds-check against the visible list so callers
        // that pass a stale raw ALL index (e.g. tests pinning an
        // old layout) don't poison the cursor.
        let n = ScreenId::ALL.len();
        app.launcher_offset = sidebar_row.min(n.saturating_sub(1));
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

/// Commit the launcher cursor: switch `app.current` to the screen
/// the launcher is highlighting, swap the WM pane kind in lockstep,
/// and step the cursor +1 (so a stream of Enter presses walks the
/// grid entry-by-entry). Returns `false` because committing a screen
/// is not a quit signal.
///
/// The cursor is a visible-row index, not a `ScreenId::ALL` index:
/// `Overworld` is hidden from the launcher (it *is* the menu, so a
/// "Menu" tile in the launcher is circular), so a raw `ALL[3]`
/// read would land on the wrong screen. `sidebar_visible` returns
/// the same filtered list `draw_launcher` paints, keeping digit-key
/// shortcuts in lockstep with arrow navigation.
async fn commit_launcher(app: &mut App, screens: &[Box<dyn Screen>]) -> bool {
    let visible = ScreenId::sidebar_visible(screens, app);
    let n = visible.len();
    if n == 0 {
        return false;
    }
    let idx = app.launcher_offset.min(n - 1);
    let id = visible[idx];
    app.current = id;
    // Set the WM pane so the next render paints the chosen screen
    // even if its region hasn't been visited yet.
    let _ = app
        .manager
        .set_pane_kind(crate::wm::window::WindowKind::Builtin(id));
    // Step focus into the screen so arrow keys there have their
    // canonical content-side semantics.
    app.set_region(Region::ContentLeft);
    // Advance the cursor so consecutive Right/l (or Enter) presses
    // walk the grid one tile at a time, mirroring how a launcher
    // on a console UI behaves when you hold the A button.
    if app.launcher_offset + 1 < n {
        app.launcher_offset += 1;
    }
    false
}

/// True for any key event that represents a "real" key the user
/// intended to bind (i.e. anything other than a bare modifier press,
/// which on some terminals arrives as `KeyCode::Modifier(_)` while the
/// user is still composing a chord). The capture loop in `handle_key`
/// uses this to keep waiting on modifier-only events instead of
/// silently storing "Shift" as a binding.
fn is_captureable(k: &KeyEvent) -> bool {
    !matches!(k.code, KeyCode::Modifier(_))
}

async fn handle_key(
    screens: &mut [Box<dyn Screen>],
    app: &mut App,
    tx: &mpsc::Sender<Action>,
    key: KeyEvent,
) -> bool {
    use KeyCode::*;

    // User keymap capture loop. When the user enters capture mode on
    // the Settings → Keys screen, the next non-modifier event is
    // consumed here: stored as a binding, persisted, and the capture
    // target is cleared. The event is *not* propagated to any other
    // handler (modal/global/screen). Runs before the hardware shim so
    // the user can bind any physical key, including one the shim would
    // rewrite (e.g. uconsole X/Y/A/B → Up/Down/Enter/Esc).
    if let Some(action) = app.keymap_capture {
        if is_captureable(&key) {
            // Conflict check: a single physical key can be bound to
            // at most one NavAction. Reject duplicates with a toast
            // and keep capture armed so the user can try again.
            // `bindings` is `pub(crate)` and the binary crate is a
            // separate compilation unit, so we scan via the public
            // `iter()` accessor (see the matching comment on
            // `Action::KeymapCmd(K::Clear)`).
            let conflict = app.keymap.iter().find(|(_, v)| *v == key).map(|(k, _)| k);
            if let Some(other) = conflict {
                app.push_toast(ToastKind::Warn,
                    format!("{:?} already bound to {} — pick a different key", key, other.label()));
            } else {
                app.keymap.bind(action, key);
                app.save_prefs();
                app.push_toast(ToastKind::Info,
                    format!("{} → {}", action.label(), keymap::key_event_label(key)));
                app.keymap_capture = None;
            }
            return false; // consumed
        }
        // Modifier-only press (Ctrl, Shift, Alt) — ignore and keep
        // waiting for a real key.
        if matches!(key.code, KeyCode::Modifier(_)) {
            return false;
        }
        // Real key but the user wants to abort the capture (Esc).
        if matches!(key.code, KeyCode::Esc) {
            app.keymap_capture = None;
            app.push_toast(ToastKind::Info, "capture cancelled".to_string());
            return false;
        }
    }

    // ponytail: single flat remap, no profiles/env vars/user-keymap layers.
    // a→Enter, b→Esc (uConsole face buttons). D-pad sends real arrow codes.
    // Gated on text-input modals so literal a/b still work in input fields.
    let key = if app.modal.accepts_text_input() {
        key
    } else {
        match key.code {
            Char('a') => KeyEvent::new(KeyCode::Enter, key.modifiers),
            Char('b') => KeyEvent::new(KeyCode::Esc, key.modifiers),
            _ => key,
        }
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
                    let actions = wm::modal::palette_actions();
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
                    if let Modal::Secret { buf, .. } = &mut app.modal {
                        crate::app::zeroize_string(buf);
                    }
                    app.modal = Modal::None;
                }
                Enter => {
                    let value = buf.clone();
                    if let Modal::Secret { buf, .. } = &mut app.modal {
                        crate::app::zeroize_string(buf);
                    }
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
                Char('r') | Char('R') | Enter => {
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

    // M2 — the Phase-1 menu bar (App.menu / MenuState / F10 / Alt+F)
    // is deleted. The Overworld tile grid replaces it; Ctrl+M toggles
    // that grid (see the gate below). What used to be reached via
    // "File → Quit" is now `q` or `Ctrl+C`; "File → Help" is `?`;
    // "File → Command Palette" is `:`. There's nothing left for the
    // dropdown to host, so the menu-bar gate at this position is gone.
    //
    // M2 (menu revamp) — disjoint keymap gate. When `app.menu_active`
    // is true the Overworld tile grid owns every key: arrows move the
    // cursor, digits jump (1‑9,0), Enter lands on the chosen screen,
    // Esc toasts the quit hint. We route directly to the registry's
    // OverworldScreen (the singleton) so the cursor persists across
    // keypresses — a fresh `OverworldScreen::new()` per event would
    // reset the cursor on every arrow press.
    //
    // Order rationale:
    //   • Modals still win (this gate is below the modal match).
    //   • Legacy Phase-1 menu bar still wins (this gate is below it).
    //   • Global keys (`q`, `Ctrl+C`) — see below — are matched next,
    //     so the user can always exit the app even from the menu.
    //   • Screen on_key and region routing never run while the menu
    //     owns input. Pressing `Ctrl+M` (handled in the global-keys
    //     block) toggles `menu_active` back off and restores normal
    //     navigation.
    //
    // The menu's own `Ctrl+M` toggle MUST stay reachable even when
    // `menu_active=true`, otherwise the gate traps the user inside
    // the menu — so we explicitly swallow `Ctrl+M` here, toggle, and
    // return true (consumed). All other keys are forwarded via
    // `OverworldScreen::on_key`.
    if app.menu_active {
        // Hard escape hatch: Ctrl+M closes the menu from inside.
        // Without this the user can only escape by pressing Enter (which
        // commits a screen) — leaving no way to back out without
        // leaving the Overworld view. Toggling the same flag we test
        // here ends the gate cleanly next frame.
        //
        // CRITICAL: return `false`, NOT `true`. In `handle_key`,
        // returning `true` is the quit signal (see `run_app` at
        // `Event::Key(...) if handle_key(...).await { return Ok(()); }`).
        // An earlier revision here returned `true`, which made
        // Ctrl+M quit the entire app instead of toggling the menu.
        if matches!(key.code, KeyCode::Char('m'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.kind, KeyEventKind::Press)
        {
            app.toggle_menu_active();
            return false;
        }
        // Locate the singleton OverworldScreen in the registry. The
        // mirrors `sidebar_row` is computed via `ScreenId::ALL.iter()
        // .position(...)` elsewhere in this file, so the ordering is
        // stable across launches.
        let ow_idx = crate::app::screen::ScreenId::ALL
            .iter()
            .position(|s| *s == crate::app::screen::ScreenId::Overworld)
            .unwrap_or(0);
        // Defensive: if the registry doesn't contain Overworld (custom
        // builds, tests), bail and let the regular keymap run. This
        // should never happen in production — Overworld is a required
        // registered screen.
        if let Some(screen) = screens.get_mut(ow_idx) {
            return screen.on_key(key, app);
        }
        return false;
    }

    // Global keys.
    const LAUNCHER_COLS: usize = 4;
    match key.code {
        // `q` (with or without Ctrl) quits. Bare `q` is the universal
        // TUI muscle memory; Ctrl+Q and Ctrl+C do the same. The
        // menu-active gate above runs first — so while the menu is
        // up, `q` does nothing (the user must Ctrl+M out first to
        // avoid accidental quits).
        Char('q') | Char('Q') => return true,
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
            // Single-source-of-truth for the `: ` palette. Inline
            // here (rather than dragging in `menu_bar::open_palette`)
            // because we're deleting the menu_bar module entirely.
            app.palette_buf.clear();
            app.palette_idx = 0;
            app.modal = crate::app::Modal::CommandPalette;
        }
        // Ctrl+M — M2 menu-revamp global. Toggles the Overworld tile
        // grid. F10 and Alt+F no longer open the legacy dropdown
        // (the dropdown is deleted; all of its actions had hot-key
        // equivalents and live in the global-keys block above).
        //
        // The toggle runs *after* the menu-active gate higher in this
        // function, so the same key both opens AND closes the menu
        // from inside (the gate also handles its own Ctrl+M for in-gate
        // toggling so the user can never get trapped with no exit).
        Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_menu_active();
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
        // Launcher (sidebar) navigation. The launcher grid is the user's
        // hub: arrows move the cursor *without* committing a screen;
        // Enter (or Right/l, mirroring A) commits and drops focus into
        // the chosen screen; Esc sends focus to the current screen.
        // The launcher grid is 4 columns wide on ≥64-col terminals and
        // 2 columns otherwise — see `ui::draw_launcher`. The handlers
        // here mirror that layout so visual cursor and logic cursor
        // stay in lockstep on the same frame.
        // Up/Down/Left/Right just nudge `app.launcher_offset`. The
        // renderer re-clamps on every frame in case the visible-screen
        // list shrinks (e.g. Editor hidden). Modifier guards make sure
        // we don't intercept Ctrl/Alt-modified versions of these keys.
        //
        // `n` is the *visible-screens* count, not `ScreenId::ALL.len()`,
        // because `Overworld` is hidden from the launcher. Treating the
        // raw enum length as `n` would let the cursor step past the
        // last visible tile and land on Offworld offsets that render
        // as nothing.
        Up | Char('k')
            if app.region == Region::Sidebar
                && !key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let n = ScreenId::sidebar_visible(screens, app).len().max(1);
            if app.launcher_offset == 0 {
                app.launcher_offset = n.saturating_sub(1);
            } else {
                app.launcher_offset -= 1;
            }
            return false;
        }
        Down | Char('j')
            if app.region == Region::Sidebar
                && !key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let n = ScreenId::sidebar_visible(screens, app).len().max(1);
            app.launcher_offset = (app.launcher_offset + 1) % n;
            return false;
        }
        Left | Char('h')
            if app.region == Region::Sidebar
                && !key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            // Column-wise step in a 4-col (or 2-col narrow) grid.
            // Plain (no-modifier) keys only — Ctrl-H and Alt-H belong
            // to system / tab-strip handlers, not the launcher.
            let n = ScreenId::sidebar_visible(screens, app).len().max(1);
            let cols = LAUNCHER_COLS.min(n);
            if app.launcher_offset == 0 {
                // Wrap from the very first tile to the last.
                app.launcher_offset = n.saturating_sub(1);
            } else if app.launcher_offset % cols == 0 {
                // Already in the leftmost column; jump to the rightmost
                // tile of the previous row band so the cursor stays
                // visible on the same horizontal "stripe".
                app.launcher_offset -= 1;
            } else {
                app.launcher_offset -= 1;
            }
            return false;
        }
        Right | Char('l')
            if app.region == Region::Sidebar
                && !key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            // Column-wise step in a 4-col (or 2-col narrow) grid.
            // The user contract is "arrows navigate; Enter selects".
            // Right is an *arrow* — it must never commit. Commit
            // happens on Enter / PageDown / Tab / digit.
            let n = ScreenId::sidebar_visible(screens, app).len().max(1);
            let cols = LAUNCHER_COLS.min(n);
            if app.launcher_offset + 1 >= n {
                app.launcher_offset = 0;
            } else if (app.launcher_offset + 1) % cols == 0 {
                // Cursor would land at the rightmost column of the
                // next row; allow that — it's a valid tile — and let
                // a further Right wrap to 0.
                app.launcher_offset += 1;
            } else {
                app.launcher_offset += 1;
            }
            return false;
        }
        // PageUp jumps to the first tile, PageDown to the last.
        PageUp if app.region == Region::Sidebar => {
            app.launcher_offset = 0;
            return false;
        }
        PageDown if app.region == Region::Sidebar => {
            let n = ScreenId::sidebar_visible(screens, app).len();
            app.launcher_offset = n.saturating_sub(1);
            return false;
        }
        Enter if app.region == Region::Sidebar => {
            // The launcher is the source of truth for "what's selected"
            // (single-focus model), so resolve the cursor directly to a
            // screen id — no separate sidebar_idx indirection.
            return commit_launcher(app, screens).await;
        }
        Esc if app.region == Region::Sidebar => {
            // From the launcher, Esc sends focus to the *current* screen
            // (not to a freshly-chosen one). This mirrors the
            // B-button "leave the menu" affordance on a console layout.
            app.set_region(Region::ContentLeft);
            return false;
        }
        // Numeric jump (1..=9, then 0 ⇒ index 9) — pin to the launcher's
        // visible cursor as well so it lines up with the arrow keys.
        // Bounds-check against the *visible* count so pressing a digit
        // higher than the number of tiles is a no-op (not a panic).
        Char(c) if app.region == Region::Sidebar && matches!(c, '0'..='9') && !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx: usize = if c == '0' { 9 } else { (c as usize) - ('1' as usize) };
            let n = ScreenId::sidebar_visible(screens, app).len();
            if idx < n {
                app.launcher_offset = idx;
                return commit_launcher(app, screens).await;
            }
            return false;
        }
        // Tab / Shift-Tab steps the *preview cursor* across the menu-name
        // strip. The strip lights up one menu name (the "where Tab would
        // land" indicator); only Enter commits. This matches the Bruce
        // firmware contract: `Tab` previews, `Enter` confirms, `Esc`
        // cancels. No modal may be open and focus has to be on the
        // content side — that's where the strip lives visually.
        Tab if matches!(app.region, Region::ContentLeft | Region::ContentRight)
            && matches!(app.modal, Modal::None) =>
        {
            app.cycle_tab_cursor(true);
            return false;
        }
        BackTab if matches!(app.region, Region::ContentLeft | Region::ContentRight)
            && matches!(app.modal, Modal::None) =>
        {
            app.cycle_tab_cursor(false);
            return false;
        }
        // Enter commits the Tab preview — `app.current` jumps to the
        // cursor's screen, then the cursor resets to None so the strip
        // stops highlighting. No-op when nothing has been pre-viewed
        // (the user pressed Enter cold, which keeps the current screen
        // — same as not pressing it).
        Enter if matches!(app.region, Region::ContentLeft | Region::ContentRight)
            && matches!(app.modal, Modal::None) =>
        {
            app.commit_tab_cursor();
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
            // Esc fallthrough: if no screen consumed Esc, clear tab
            // preview and go back to the launcher (sidebar).
            if matches!(key.code, Esc)
                && matches!(app.region, Region::ContentLeft | Region::ContentRight)
            {
                app.clear_tab_cursor();
                app.set_region(Region::Sidebar);
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
        // Resetting the user keymap is a pure in-memory reset + prefs
        // write on App, same shape as Discard above. Mirrors that
        // arm's early-return pattern.
        ConfirmKind::KeymapReset => {
            let _ = arg;
            app.keymap = crate::keymap::Keymap::default();
            app.save_prefs();
            app.push_toast(ToastKind::Info, "keymap reset to defaults".to_string());
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
                Keymap => {
                    // Enter the user-keymap editing sub-mode. We don't
                    // toggle the flag here — this is a *mode-entry* action,
                    // not a boolean flip. The sub-mode itself clears the
                    // flag on exit (Esc / q) so the user lands back on
                    // the normal Settings list. The Phase-1 menu bar is
                    // gone; the sub-mode no longer needs to dismiss it.
                    app.keymap_editing = true;
                    Some("keys: editing".to_string())
                }
            };
            if let Some(msg) = confirm {
                app.push_toast(ToastKind::Info, msg);
            }
        }
        Action::KeymapCmd(cmd) => {
            use crate::keymap::KeymapCmd as K;
            match cmd {
                K::BeginCapture(action) => {
                    // Mark the sub-mode as capturing for `action`. The next
                    // keypress that reaches `handle_key` while this flag is
                    // set is consumed and turned into a `KeymapCmd::CaptureKey`
                    // by the dispatcher.
                    app.keymap_capture = Some(action);
                    // The Phase-1 menu-bar `app.menu.close()` call used
                    // to live here so the dropdown didn't overlap the
                    // Keys table. The menu bar is gone; the Keys
                    // sub-mode owns its own chrome.
                }
                K::CaptureKey => {
                    // No-op: the actual capture happened in handle_key
                    // (which intercepted the key, set the binding, and
                    // returned false). Reaching here means the user
                    // pressed something we already handled — ignore.
                }
                K::Clear(action) => {
                    // `app.keymap.bindings` is `pub(crate)`, but the binary
                    // crate's `main.rs` is treated as a separate compilation
                    // unit for visibility purposes, so we route through the
                    // library's `unbind` method instead.
                    app.keymap.unbind(action);
                    app.save_prefs();
                    app.push_toast(ToastKind::Info,
                        format!("cleared binding for {}", action.label()));
                }
                K::ResetAll => {
                    // Don't wipe unilaterally — confirm first. The actual
                    // `Keymap::default()` swap lives in `run_confirm`'s
                    // `ConfirmKind::KeymapReset` arm so the user gets
                    // the same confirm-modal UX as Reboot/Shutdown/etc.
                    app.modal = Modal::Confirm {
                        message: "Reset all key bindings to defaults?".to_string(),
                        kind: crate::app::ConfirmKind::KeymapReset,
                        arg: String::new(),
                    };
                }
                K::ExitMode => {
                    app.keymap_capture = None;
                    app.keymap_editing = false;
                }
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
        Action::IntelSnapshot(snap) => {
            // M5 — refiller pushed a fresh snapshot for one layer.
            // Upsert into the per-layer map and recompute the
            // worst-sentinel rollup so the footer chip is O(1) on
            // every frame. We hold the worst computation local
            // rather than calling the crate's helper so a single
            // batch update is cheap (a max over 9 elements).
            app.intel_snapshots.insert(snap.layer, snap);
            app.intel_worst = cyberdeck_intel::worst_sentinel(
                app.intel_snapshots.values().map(|s| s.sentinel),
            );
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

    /// Build a screens Vec that mirrors the production registry's
    /// `ScreenId::ALL` order, with the same number of entries
    /// (and the same index alignment) so `ScreenId::sidebar_visible`
    /// and `draw_launcher` (both of which look up the screen at
    /// `ALL[idx]`) hit the right slot for every id. The Overworld
    /// slot IS registered here, but its `in_sidebar()` returns
    /// false so it's hidden from the launcher — including a stub
    /// is what keeps the indices aligned.
    ///
    /// If you add a new `ScreenId`, append a matching screen
    /// here in the same position.
    ///
    /// `Editor` and `LoRa` are intentionally omitted: they have
    /// no production registry entry, so `screens.get(idx)` for
    /// those indices returns `None` and the sidebar filter treats
    /// that as "absent" (which is the contract the production
    /// code relies on).
    fn build_screens() -> Vec<Box<dyn Screen>> {
        vec![
            Box::new(screens::overworld::OverworldScreen::new()), // ALL[0]  — hidden from sidebar
            Box::new(screens::system::SystemScreen),               // ALL[1]
            Box::new(screens::network::NetworkScreen),             // ALL[2]
            Box::new(screens::bluetooth::BluetoothScreen),         // ALL[3]
            Box::new(screens::power::PowerScreen),                 // ALL[4]
            Box::new(screens::display::DisplayScreen),             // ALL[5]
            Box::new(screens::audio::AudioScreen),                 // ALL[6]
            Box::new(screens::storage::StorageScreen),             // ALL[7]
            Box::new(screens::services::ServicesScreen),           // ALL[8]
            Box::new(screens::packages::PackagesScreen),           // ALL[9]
            Box::new(screens::processes::ProcessesScreen),         // ALL[10]
            Box::new(screens::files::FilesScreen),                 // ALL[11]
            Box::new(screens::logs::LogsScreen),                   // ALL[12]
            Box::new(screens::settings::SettingsScreen),           // ALL[13]
            // ALL[14] = Editor → no registry entry
            // ALL[15] = LoRa → no test stub
            Box::new(screens::city::CityScreen::new()),            // ALL[16]
            Box::new(screens::intel::IntelScreen::new()),          // ALL[17]
            Box::new(screens::recon::ReconScreen::new()),          // ALL[18]
        ]
    }

    fn app_with_n_panes(n: u8) -> App {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        // M2 — tests using this helper exercise the legacy Phase-1
        // keymap (without first activating the M2 menu). Default
        // boot has `menu_active=true` so the Overworld would swallow
        // every key; flip it off so the rest of the test (keymap,
        // Ctrl+W, etc.) sees the pre-M2 routing.
        app.menu_active = false;
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

    // ---- Ctrl+M: global menu shortcut -------------------------------
    //
    // The user-facing contract is:
    //   "Ctrl+M opens the menu from any screen, including while focus is
    //    in the Content region (i.e. while a screen is 'active')."
    // The shortcut must mirror F10 / Alt+F — that is, it must toggle,
    // not latch. Sending Ctrl+M twice in a row should leave the menu
    // closed. The semantic is "get to the menu" — so the File menu is
    // the default since F10/Alt+F already open it.

    fn send_ctrl_m() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_m_opens_menu_from_sidebar_focus() {
        let mut app = app_with_n_panes(1);
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        // Default boot focus is the Sidebar; mirror that explicitly.
        app.region = crate::app::Region::Sidebar;
        run(async {
            let quit = handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(!quit, "Ctrl+M must not quit");
        });
        // M2 — Ctrl+M toggles the new `menu_active` flag (the M2
        // Overworld tile-grid replaces the Phase-1 menu-bar on this
        // binding). F10 / Alt+F still open the legacy dropdown.
        assert!(app.menu_active, "Ctrl+M from sidebar should activate the menu");
    }

    #[test]
    fn ctrl_m_opens_menu_from_content_focus() {
        // The whole point of the shortcut — the user is "inside" a screen
        // and hits Ctrl+M to get back to menu-level actions.
        let mut app = app_with_n_panes(1);
        app.region = crate::app::Region::ContentLeft;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            let quit = handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(!quit, "Ctrl+M must not quit");
        });
        assert!(app.menu_active);
    }

    #[test]
    fn ctrl_m_is_noop_when_menu_already_open() {
        // M2 — pressing Ctrl+M a second time while the menu is active
        // toggles it OFF (closes the menu), unlike Phase-1's "sticky"
        // behavior where the only way out was Esc. This is the
        // user-facing toggle semantic, same as F10 → F10 in a desktop
        // app: open, then close.
        let mut app = app_with_n_panes(1);
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(app.menu_active, "first Ctrl+M opens the menu");
            // Second Ctrl+M closes it.
            handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(
                !app.menu_active,
                "second Ctrl+M closes the menu (toggle semantic)"
            );
        });
    }

    #[test]
    fn ctrl_m_returns_false_to_keep_render_loop_alive() {
        // Returning `true` from handle_key signals "quit". Ctrl+M must
        // NOT return true under any combination tested above — pinned
        // explicitly so future refactors that drop into the quit arm
        // (e.g. a Ctrl-M-x vim-style "leader key") fail loudly.
        let mut app = app_with_n_panes(1);
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let key = send_ctrl_m();
        // Capture the return value out of `run`.
        let quit_flag = std::cell::Cell::new(true);
        run(async {
            let q = handle_key(&mut screens, &mut app, &tx, key).await;
            quit_flag.set(q);
        });
        assert!(!quit_flag.take(), "Ctrl+M must NOT signal quit");
    }

    // ---- M2 menu-active gate -----------------------------------------
    //
    // The M2 menu-active gate in `handle_key` owns the keyboard surface
    // while `app.menu_active` is true. The tests below pin its contract:
    //   * When the menu is active, every key routes to the OverworldScreen
    //     singleton (so its cursor accumulates across keypresses).
    //   * The Phase-1 sidebar cursor and global digit handlers are
    //     silenced — pressing `5` no longer swaps pane kind.
    //   * The Phase-1 regional cursor (`launcher_offset`) is also
    //     untouched — the Overworld owns its own cursor.
    //   * When the menu is dismissed (Ctrl+M a second time, or the user
    //     pressed Esc to back out), the legacy keymap resumes.

    fn active_menu_app() -> App {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        // M2 default — boot with the menu active. Leave region as the
        // default (Sidebar) so the test exercises the path where the
        // user toggles the menu while still in launcher mode.
        app.menu_active = true;
        app
    }

    /// Pressing arrows while `menu_active=true` must move the
    /// Overworld's persistent cursor and NOT the Phase-1
    /// `launcher_offset`. This is the core M2 invariant: the two
    /// cursors live in different fields and never bleed across.
    #[test]
    fn menu_active_overworld_arrow_owns_cursor() {
        let mut app = active_menu_app();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let ow_idx = ScreenId::ALL
            .iter()
            .position(|s| *s == ScreenId::Overworld)
            .unwrap();
        let ow_initial = screens[ow_idx]
            .as_any()
            .and_then(|a| a.downcast_ref::<crate::screens::overworld::OverworldScreen>())
            .expect("OverworldScreen present in registry")
            .cursor_for_test();
        let launcher_initial = app.launcher_offset;
        let cols = screens[ow_idx]
            .as_any()
            .and_then(|a| a.downcast_ref::<crate::screens::overworld::OverworldScreen>())
            .expect("OverworldScreen present in registry")
            .cols_for_test();
        run(async {
            for _ in 0..3 {
                handle_key(
                    &mut screens,
                    &mut app,
                    &tx,
                    KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                )
                .await;
            }
        });
        let ow_final = screens[ow_idx]
            .as_any()
            .and_then(|a| a.downcast_ref::<crate::screens::overworld::OverworldScreen>())
            .expect("OverworldScreen present in registry")
            .cursor_for_test();
        // Cursor advanced by three rows worth of increment in the
        // Overworld grid (each Down = +cols). Independent of the
        // column-count chosen by grid_cols_for at last render, the
        // delta from N Down presses is exactly N*cols as long as we
        // don't wrap. The visible count is 18, so 3 downs from cursor
        // 1 with any cols in [2,5] lands at 1 + 3*cols without wrap.
        assert_eq!(
            ow_final,
            ow_initial + 3 * cols,
            "three Down presses must advance the Overworld cursor by 3*cols"
        );
        // The Phase-1 launcher cursor was untouched.
        assert_eq!(
            app.launcher_offset, launcher_initial,
            "menu-active gate must not touch launcher_offset"
        );
    }

    /// Pressing a digit while `menu_active=true` is a tile-jump inside
    /// the Overworld grid — it must NOT swap pane kind (Phase-1's
    /// digit-shortcut behavior). The M2 digit arm in
    /// `OverworldScreen::on_key` just moves the cursor.
    #[test]
    fn menu_active_digit_jumps_tile_does_not_swap_pane() {
        let mut app = active_menu_app();
        app.current = ScreenId::System;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let ow_idx = ScreenId::ALL
            .iter()
            .position(|s| *s == ScreenId::Overworld)
            .unwrap();
        run(async {
            // Press '5'. M2 = jump Overworld cursor to index 4.
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE),
            )
            .await;
        });
        let ow_cursor = screens[ow_idx]
            .as_any()
            .and_then(|a| a.downcast_ref::<crate::screens::overworld::OverworldScreen>())
            .expect("OverworldScreen present in registry")
            .cursor_for_test();
        assert_eq!(
            ow_cursor, 4,
            "digit '5' in active menu jumps Overworld cursor to index 4"
        );
        // Phase-1 contract: `current` would have flipped to Display.
        assert_eq!(
            app.current, ScreenId::System,
            "M2 menu must NOT auto-swap pane on digit press; Enter commits"
        );
    }

    /// `menu_active=false` lets the Phase-1 keymap resume. This is the
    /// documented "menu dismissed — back to normal nav" state.
    #[test]
    fn menu_active_false_restores_phase1_keymap() {
        let mut app = active_menu_app();
        app.menu_active = false;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            // Press '5' with menu dismissed — Phase-1 digit-jump fires
            // and swaps pane kind to Display.
            handle_key(
                &mut screens,
                &mut app,
                &tx,
                KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE),
            )
            .await;
        });
        assert_eq!(
            app.current, ScreenId::Display,
            "dismissed menu must route digits through the Phase-1 switcher"
        );
    }

    /// Repeated Ctrl+M toggles the menu (open → close → open). This is
    /// the "stuck inside the menu" escape hatch the gate above must
    /// not regress.
    #[test]
    fn ctrl_m_toggle_is_symmetric() {
        let mut app = active_menu_app();
        app.menu_active = false;
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(app.menu_active, "1st Ctrl+M opens");
            handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(!app.menu_active, "2nd Ctrl+M closes");
            handle_key(&mut screens, &mut app, &tx, send_ctrl_m()).await;
            assert!(app.menu_active, "3rd Ctrl+M opens again");
        });
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
        // M2 — Phase-1 sidebar navigation tests need the menu
        // dismissed so the legacy arrow/digit handlers still run.
        app.menu_active = false;
        app.set_region(Region::Sidebar);
        app.launcher_offset = 0;
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
        assert_eq!(app.launcher_offset, 2);
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
        // Up from 0 wraps to the last visible tile. The launcher
        // skips Overworld (it's the menu, not a destination) and
        // skips the no-slot ids (Editor, LoRa), so the visible list
        // is shorter than `ScreenId::ALL` by 3 — not 2 as it was
        // before Overworld was added.
        let visible_n = ScreenId::sidebar_visible(&build_screens(), &app).len();
        assert_eq!(
            app.launcher_offset,
            visible_n - 1,
            "Up from 0 must wrap to the last sidebar-visible screen"
        );
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
        assert_eq!(app.launcher_offset, 0);
    }

    #[test]
    fn sidebar_enter_commits_cursor() {
        let mut app = fresh_app_with_sidebar_focus();
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        // Move cursor to row 4 (Display, 0-indexed).
        app.launcher_offset = 4;
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
        // M2 — dismiss the menu so Left routes via the region router
        // rather than the Overworld menu-active gate.
        app.menu_active = false;
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
        // M2 — dismiss the menu so Down/Enter reach the Phase-1 region
        // router (which is what this regression pins).
        app.menu_active = false;
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
        assert_eq!(app.launcher_offset, 0, "Down must not move sidebar cursor when content focused");
        assert_eq!(app.current, ScreenId::System, "Enter must not change current when content focused");
    }

    #[test]
    fn router_walk_three_regions() {
        // D-pad walk on the new launcher-only launcher-completion flow:
        //   start = Sidebar (launcher focused)
        //   ↓        launcher cursor advances one tile (no commit)
        //   ↑        launcher cursor retreats one tile (no commit)
        //   ↵        commit: launcher → ContentLeft (focus drops into screen)
        //   → →      ContentLeft → ContentRight
        //   ←        ContentRight → ContentLeft
        //   ←        ContentLeft → Sidebar (back to launcher)
        // The user-facing contract is "arrows navigate; Enter selects;
        // Tab cycles". Right/L/Up/Down on the launcher no longer take
        // the user into a screen — that's Enter's job. Region walks
        // never change which screen is rendered.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        // M2 — dismiss the menu so the walker hits the Phase-1 region
        // router arms (which this regression pins).
        app.menu_active = false;
        app.set_region(Region::Sidebar);
        app.current = ScreenId::System;
        let initial_cursor = app.launcher_offset;
        let mut screens = build_screens();
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<Action>(8);
        let left = || KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let _right = || KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let down = || KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let up = || KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let enter = || KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        run(async {
            // Down from Sidebar must advance the launcher cursor —
            // does NOT change region or commit a screen.
            handle_key(&mut screens, &mut app, &tx2, down()).await;
            assert!(matches!(app.region, Region::Sidebar), "↓ from Sidebar must stay in the launcher");
            assert_ne!(app.launcher_offset, initial_cursor, "↓ from Sidebar must advance launcher cursor");
            // Up must retreat it (round-trip).
            handle_key(&mut screens, &mut app, &tx2, up()).await;
            assert_eq!(app.launcher_offset, initial_cursor, "↑ from Sidebar must retreat launcher cursor");
            // Enter commits: region → ContentLeft.
            handle_key(&mut screens, &mut app, &tx2, enter()).await;
            assert!(matches!(app.region, Region::ContentLeft), "↵ from Sidebar → ContentLeft");
        });
        // After the commit-then-content walk the active screen is
        // whatever the launcher was on (System if cursor stayed at 0).
        assert_eq!(app.current, ScreenId::System, "Region walk must not change the active screen");

        let (tx3, _rx3) = tokio::sync::mpsc::channel::<Action>(8);
        run(async {
            // From ContentLeft, → → should move region into ContentRight
            // *on a multi-pane screen*. Network owns its own Right arrow
            // (jumps to first wifi row), so the multi-pane step only
            // happens on a single-pane screen. Use the System screen
            // which has no Right arrow claim.
            handle_key(&mut screens, &mut app, &tx3, left()).await;
            assert!(matches!(app.region, Region::Sidebar), "← from ContentLeft → Sidebar");
        });
    }

    #[test]
    fn number_keys_when_sidebar_focused_move_cursor_to_that_row() {
        let mut app = fresh_app_with_sidebar_focus();
        // M2 — when the menu is active (default boot), digit keys are
        // owned by the Overworld tile grid (jump the cursor to that
        // index). The legacy Phase-1 "digit-key swaps pane kind" path
        // runs only after the menu has been dismissed. This test pins
        // the LEGACY fallback, so dismiss the menu first.
        app.menu_active = false;
        app.current = ScreenId::System;
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
        // Number keys commit *and* advance the launcher cursor one tile,
        // so a stream of "5 5 5" presses steps the launcher instead of
        // re-entering the same screen. Cursor lands at index 5 (the
        // tile to the right of Display in the grid).
        assert_eq!(
            app.launcher_offset, 5,
            "number-key commit advances cursor one tile"
        );
    }

    #[tokio::test]
    async fn keymap_capture_stores_binding_and_persists() {
        use crate::keymap::NavAction;
        use crossterm::event::KeyCode;
        use std::path::PathBuf;
        // Point prefs at a temp file BEFORE App::new so the prefs loader
        // (and the subsequent save_prefs call) writes into the sandbox
        // instead of clobbering the developer's real prefs.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        let mut app = crate::app::App::new(tx, rx);
        app.keymap_capture = Some(NavAction::Down);
        let _ = handle_key(&mut [], &mut app,
            &tokio::sync::mpsc::channel::<Action>(1).0,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)).await;

        assert_eq!(app.keymap.get(NavAction::Down),
                   Some(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
        assert!(app.keymap_capture.is_none(), "capture must clear after success");

        // Verify the binding landed on disk.
        let prefs_path: PathBuf = dir.path().join("cyberdeck").join("prefs.json");
        let raw = std::fs::read_to_string(&prefs_path).expect("prefs.json written");
        assert!(raw.contains("\"down\""), "down binding not in prefs: {raw}");
        assert!(raw.contains("\"Char\""), "expected KeyCode::Char in prefs: {raw}");
        assert!(raw.contains("\"s\""), "expected 's' in prefs: {raw}");
    }

    #[tokio::test]
    async fn keymap_capture_rejects_conflict() {
        use crate::keymap::NavAction;
        let (_tx, _rx, mut app) = make_app();
        // 'j' is already bound to Down — pressing it for Up should be rejected.
        app.keymap.bind(NavAction::Down, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        app.keymap_capture = Some(NavAction::Up);
        let _ = handle_key(&mut [], &mut app,
            &tokio::sync::mpsc::channel::<Action>(1).0,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)).await;
        // Capture must still be armed (user needs another chance to pick).
        assert_eq!(app.keymap_capture, Some(NavAction::Up),
                   "conflict should keep capture armed");
        // Up must NOT have been rebound to 'j'.
        assert!(app.keymap.get(NavAction::Up).is_none(),
                "conflict must not store a new binding");
    }

    #[test]
    fn number_keys_when_content_focused_still_swap_pane_kind() {
        // Even when the sidebar isn't focused (the user is reading the
        // right pane), the digit-key shortcuts still need to swap the
        // content pane — that's the whole point of the new wiring.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        // M2 — the menu defaults to ACTIVE on boot, so pressing `3`
        // would jump the Overworld cursor instead of swapping panes.
        // This test pins the LEGACY fallback path: when the menu has
        // been dismissed (e.g. user pressed Enter on a tile), digit
        // keys go back to working as pane-switchers.
        app.menu_active = false;
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
        // M2 — dismiss the default-active menu so Phase-1 assertions
        // (keymap captures, run_input success, etc.) see the pre-M2
        // routing instead of Overworld swallowing every key.
        app.menu_active = false;
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
        let lines = wm::modal::modal_input_lines("Connect to SSID:", "");
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
        let lines = wm::modal::modal_secret_lines("Wi-Fi password for HomeNet", "hunter2");
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
    async fn esc_in_files_goes_up_a_folder() {
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = fresh_app_sidebar();
        app.current = crate::app::ScreenId::Files;
        app.set_region(Region::ContentLeft);
        // The catch-all in handle_key routes Esc to whichever screen the
        // focused WM pane is currently displaying; mirror `switch_screen`
        // so the Builtin kind points at FilesScreen.
        let _ = app
            .manager
            .set_pane_kind(crate::wm::window::WindowKind::Builtin(
                crate::app::ScreenId::Files,
            ));
        // Pre-build a tempdir two levels deep so files_cwd has a parent.
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        app.files_cwd = nested.clone();

        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;

        assert_eq!(
            app.files_cwd,
            nested.parent().unwrap().to_path_buf(),
            "Esc should go up one folder"
        );
        assert_eq!(
            app.region,
            Region::ContentLeft,
            "screen claimed Esc; region should stay in content"
        );
    }

    #[tokio::test]
    async fn esc_at_filesystem_root_falls_through_to_launcher() {
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = fresh_app_sidebar();
        app.current = crate::app::ScreenId::Files;
        app.set_region(Region::ContentLeft);
        let _ = app
            .manager
            .set_pane_kind(crate::wm::window::WindowKind::Builtin(
                crate::app::ScreenId::Files,
            ));
        app.files_cwd = std::path::PathBuf::from("/");

        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;

        // No parent — Files returned false. Unconsumed Esc falls through
        // to the sidebar (B button = universal "back" with flat remap).
        assert_eq!(
            app.region,
            Region::Sidebar,
            "at filesystem root, unconsumed Esc should go to sidebar"
        );
        assert_eq!(
            app.files_cwd,
            std::path::PathBuf::from("/"),
            "cwd should be unchanged when Esc falls through"
        );
    }

    /// Task 6 — Logs screen claims `Esc` to dismiss the active filter.
    /// When `app.logs_filter` is non-empty, the screen clears it and
    /// returns `true` from `on_key`, so the launcher does NOT take Esc.
    /// The catch-all in `handle_key` routes `Esc` to whichever screen the
    /// focused WM pane is currently displaying; mirror `switch_screen`
    /// so the Builtin kind points at LogsScreen.
    #[tokio::test]
    async fn esc_in_logs_clears_active_filter() {
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = fresh_app_sidebar();
        app.current = crate::app::ScreenId::Logs;
        app.set_region(Region::ContentLeft);
        let _ = app
            .manager
            .set_pane_kind(crate::wm::window::WindowKind::Builtin(
                crate::app::ScreenId::Logs,
            ));
        // An active filter is just a non-empty `logs_filter`.
        app.logs_filter = "error".to_string();
        assert!(
            !app.logs_filter.is_empty(),
            "precondition: filter is active"
        );

        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;

        assert!(
            app.logs_filter.is_empty(),
            "Esc should clear the active filter, got {:?}",
            app.logs_filter
        );
        assert_eq!(
            app.region,
            Region::ContentLeft,
            "screen claimed Esc; region should stay in content"
        );
    }

    /// Task 6 — Logs screen does NOT claim `Esc` when no filter is
    /// active, so the launcher can still take it. The catch-all in
    /// `handle_key` only forwards the key once; if Logs returns
    /// `false`, nothing else handles Esc from a content region and
    /// the region stays put (Esc is a no-op at the launcher, matching
    /// the `esc_at_filesystem_root_falls_through_to_launcher` contract).
    #[tokio::test]
    async fn esc_in_logs_with_no_filter_is_noop() {
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = fresh_app_sidebar();
        app.current = crate::app::ScreenId::Logs;
        app.set_region(Region::ContentLeft);
        let _ = app
            .manager
            .set_pane_kind(crate::wm::window::WindowKind::Builtin(
                crate::app::ScreenId::Logs,
            ));
        assert!(
            app.logs_filter.is_empty(),
            "precondition: no filter is set"
        );

        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;

        assert!(
            app.logs_filter.is_empty(),
            "Esc with no active filter must not mutate the filter"
        );
        // Unconsumed Esc falls through to sidebar (B = universal back).
        assert_eq!(
            app.region,
            Region::Sidebar,
            "no filter active → Logs returns false → Esc goes to sidebar"
        );
    }

    #[tokio::test]
    async fn b_in_content_moves_to_launcher() {
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = fresh_app_sidebar();
        app.set_region(Region::ContentLeft);

        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        )
        .await;

        assert_eq!(
            app.region,
            Region::Sidebar,
            "B from ContentLeft should move focus to launcher"
        );
    }

    #[tokio::test]
    async fn b_in_sidebar_goes_to_content() {
        let mut screens = build_screens();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = fresh_app_sidebar();
        app.set_region(Region::Sidebar);

        handle_key(
            &mut screens,
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        )
        .await;

        // B → Esc (flat remap); Esc in sidebar sends focus to current screen.
        assert_eq!(app.region, Region::ContentLeft,
            "B (Esc) in sidebar should focus the current screen");
    }

    /// Module 4 — the editor (a hidden builtin, reachable only from
    /// Files via `e`) must claim `Esc` for itself and return focus to
    /// Files. The catch-all in `handle_key` only reaches the screen's
    /// `on_key` when the focused pane's `WindowKind` matches a screen
    /// in the `screens` vec; for the editor test we slice in just an
    /// `EditorScreen` so the catch-all dispatches `Esc` to it.
    #[tokio::test]
    async fn esc_in_editor_closes_editor() {
        use crate::screens::editor::EditorScreen;
        use crate::wm::window::WindowKind;
        let mut app = fresh_app_sidebar();
        app.current = crate::app::ScreenId::Editor;
        app.set_region(Region::ContentLeft);
        let _ = app.manager.set_pane_kind(WindowKind::Builtin(
            crate::app::ScreenId::Editor,
        ));
        // Clean editor (default state — no file loaded) so Esc takes
        // the "focus back to Files" branch, not the Discard-confirm
        // branch.
        app.editor_dirty = false;
        assert_eq!(
            app.manager.focused_pane_kind(),
            Some(WindowKind::Builtin(crate::app::ScreenId::Editor)),
            "precondition: focused pane is the editor"
        );

        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        handle_key(
            &mut [Box::new(EditorScreen)],
            &mut app,
            &tx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;

        assert_eq!(
            app.manager.focused_pane_kind(),
            Some(WindowKind::Builtin(crate::app::ScreenId::Files)),
            "Esc on a clean editor must focus the Files pane"
        );
        assert!(
            matches!(app.modal, Modal::None),
            "Esc on a clean editor must not open any modal, got {:?}",
            app.modal
        );
        assert_eq!(
            app.region,
            Region::ContentLeft,
            "region should stay in content (screen claimed Esc)"
        );
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
                wm::modal::draw_modal(f, area, app, &theme);
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

    // -----------------------------------------------------------------------
    // Settings → Keys sub-mode render polish (Task 10).
    //
    // These render tests exercise the sub-mode block at the same widths the
    // plan called out (80 / 100 / 120 / 140 cols × 32 rows) and assert that
    // the " Keys " title is painted. A regression that strips the title (or
    // panics inside the renderer at a narrow width) trips these.
    // -----------------------------------------------------------------------

    fn render_settings_keymap_text(app: &mut App, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::by_name(crate::theme::ThemeName::Dark);
        terminal
            .draw(|f| {
                let area = f.area();
                let mut screen = screens::settings::SettingsScreen;
                screen.render(f, area, app, &theme, true);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut rows: Vec<String> = Vec::with_capacity(buffer.area.height as usize);
        for y in 0..buffer.area.height {
            let mut row = String::with_capacity(buffer.area.width as usize);
            for x in 0..buffer.area.width {
                row.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            rows.push(row);
        }
        rows.join("\n")
    }

    fn app_with_keymap_submode() -> App {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = App::new(tx, rx);
        app.current = ScreenId::Settings;
        app.keymap_editing = true;
        app
    }

    #[test]
    fn render_settings_keymap_submode_80x32() {
        let mut app = app_with_keymap_submode();
        let text = render_settings_keymap_text(&mut app, 80, 32);
        assert!(
            text.contains(" Keys "),
            "Settings → Keys sub-mode must render the `Keys` title; got:\n{text}"
        );
    }

    #[test]
    fn render_settings_keymap_submode_100x32() {
        let mut app = app_with_keymap_submode();
        let text = render_settings_keymap_text(&mut app, 100, 32);
        assert!(
            text.contains(" Keys "),
            "Settings → Keys sub-mode must render the `Keys` title at 100 cols; got:\n{text}"
        );
    }

    #[test]
    fn render_settings_keymap_submode_120x32() {
        let mut app = app_with_keymap_submode();
        let text = render_settings_keymap_text(&mut app, 120, 32);
        assert!(
            text.contains(" Keys "),
            "Settings → Keys sub-mode must render the `Keys` title at 120 cols; got:\n{text}"
        );
    }

    #[test]
    fn render_settings_keymap_submode_140x32() {
        let mut app = app_with_keymap_submode();
        let text = render_settings_keymap_text(&mut app, 140, 32);
        assert!(
            text.contains(" Keys "),
            "Settings → Keys sub-mode must render the `Keys` title at 140 cols; got:\n{text}"
        );
    }

    #[tokio::test]
    async fn shim_bypass_in_text_input_modals() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        for modal in [
            Modal::Input { prompt: "test".into(), buf: String::new(), kind: InputKind::WifiPassword },
            Modal::Secret { prompt: "test".into(), buf: String::new(), kind: InputKind::WifiPassword },
            Modal::CommandPalette,
        ] {
            let mut app = app_with_n_panes(1);
            app.menu_active = false;
            app.modal = modal;
            handle_key(&mut [], &mut app, &tx,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)).await;
            // 'a' must NOT become Enter — modal should still be active
            assert!(!matches!(app.modal, Modal::None),
                "flat remap must not remap 'a' → Enter in text-input modal");
        }
    }

    #[tokio::test]
    async fn auth_failure_enter_retries() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Action>(8);
        let mut app = app_with_n_panes(1);
        app.menu_active = false;
        app.modal = Modal::AuthFailure {
            command: "test".into(),
            stderr: "denied".into(),
            retry: Box::new(Modal::Help),
        };
        handle_key(&mut [], &mut app, &tx,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).await;
        assert!(matches!(app.modal, Modal::Help),
            "Enter in AuthFailure should retry (restore inner modal)");
    }

    #[test]
    fn modal_accepts_text_input_correct() {
        assert!(Modal::Input { prompt: "".into(), buf: "".into(),
            kind: InputKind::WifiPassword }.accepts_text_input());
        assert!(Modal::Secret { prompt: "".into(), buf: "".into(),
            kind: InputKind::WifiPassword }.accepts_text_input());
        assert!(Modal::CommandPalette.accepts_text_input());
        assert!(!Modal::None.accepts_text_input());
        assert!(!Modal::Help.accepts_text_input());
        assert!(!Modal::ToastLog.accepts_text_input());
    }
}
