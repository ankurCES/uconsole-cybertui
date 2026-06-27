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

mod app;
mod screens;
mod theme;
mod ui;
mod wm;

#[cfg(feature = "web")]
mod web_bridge;

use std::io::{stdout, Stdout};
use std::path::PathBuf;
use std::time::Duration;
#[cfg(feature = "web")]
use std::sync::Arc;

use anyhow::Context;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
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
use app::{App, ConfirmKind, Focus, InputKind, Modal};
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
    let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
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
    execute!(out, EnterAlternateScreen, EnableMouseCapture).context("enter alt screen")?;
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
    ];

    let mut redraw = true;
    let mut last_tick = std::time::Instant::now();
    let tick_rate = Duration::from_millis(250);

    loop {
        if redraw {
            let theme = Theme::by_name(match app.theme_name {
                app::screen::ThemeNameReexport::Dark => theme::ThemeName::Dark,
                app::screen::ThemeNameReexport::Light => theme::ThemeName::Light,
                app::screen::ThemeNameReexport::HighContrast => theme::ThemeName::HighContrast,
            });
            terminal
                .draw(|f| draw(f, app, &mut screens, &theme))
                .context("terminal draw")?;
            redraw = false;
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));

        tokio::select! {
            // Drain queued actions.
            maybe = app.rx.recv() => {
                match maybe {
                    Some(action) => {
                        if handle_action(&mut screens, app, tx, action.clone()).await {
                            return Ok(());
                        }
                        redraw = true;
                    }
                    None => return Ok(()),
                }
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
                    if let Event::Key(k) = event::read()? {
                        if k.kind == KeyEventKind::Press {
                            if handle_key(&mut screens, app, tx, k).await {
                                return Ok(());
                            }
                            redraw = true;
                        }
                    }
                }
            }
        }
        app.cleanup_toasts();
    }
}

fn draw(f: &mut Frame, app: &mut App, screens: &mut [Box<dyn Screen>], theme: &Theme) {
    let (header, sidebar, content) = ui::chunks(f.area());
    ui::draw_header(f, header, app, theme);
    ui::draw_sidebar(f, sidebar, app, theme);
    ui::draw_status(f, content, app, theme);
    // content height = full content area minus status bar at bottom
    let content_inner = rect(
        content.x,
        content.y,
        content.width,
        content.height.saturating_sub(2),
    );
    let id = app.current;
    if let Some(s) = screens.iter_mut().find(|s| s.id() == id) {
        s.render(
            f,
            content_inner,
            app,
            theme,
            matches!(app.focus, Focus::Content),
        );
    }
    ui::draw_toasts(f, f.area(), app, theme);
    draw_modal(f, f.area(), app, theme);
}

fn rect(x: u16, y: u16, w: u16, h: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect::new(x, y, w, h)
}

use ratatui::Frame;

fn draw_modal(f: &mut Frame, area: ratatui::layout::Rect, app: &App, theme: &Theme) {
    match &app.modal {
        Modal::None => {}
        Modal::Help => {
            use ratatui::text::Line;
            use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
            let lines = vec![
                Line::from("cyberdeck-tui — help"),
                Line::from(""),
                Line::from(" ↑/↓ j/k    navigate lists"),
                Line::from(" ←/→ h/l    switch focus between sidebar and content"),
                Line::from(" enter      open / confirm"),
                Line::from(" esc        back / cancel"),
                Line::from(" 1..9       jump to screen"),
                Line::from(" r          refresh current screen"),
                Line::from(" :          command palette"),
                Line::from(" ?          this help"),
                Line::from(" q          quit"),
                Line::from(""),
                Line::from("Press ? or esc to close."),
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
                        .title(" help ")
                        .borders(Borders::ALL)
                        .border_style(theme.border(true)),
                )
                .wrap(Wrap { trim: false })
                .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
            f.render_widget(p, rect);
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
            use ratatui::text::Line;
            use ratatui::widgets::{Block, Borders, Clear, Paragraph};
            let lines = vec![Line::from(prompt.clone()), Line::from(format!("> {buf}"))];
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

async fn handle_key(
    screens: &mut [Box<dyn Screen>],
    app: &mut App,
    tx: &mpsc::Sender<Action>,
    key: KeyEvent,
) -> bool {
    use KeyCode::*;

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
                    // Push into the live buffer via a re-borrow.
                    if let Modal::Input { buf, .. } = &mut app.modal {
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
        Char(':') => {
            app.modal = Modal::CommandPalette;
            app.palette_buf.clear();
            app.palette_idx = 0;
        }
        Char('1') => app.current = ScreenId::System,
        Char('2') => app.current = ScreenId::Network,
        Char('3') => app.current = ScreenId::Bluetooth,
        Char('4') => app.current = ScreenId::Power,
        Char('5') => app.current = ScreenId::Display,
        Char('6') => app.current = ScreenId::Audio,
        Char('7') => app.current = ScreenId::Storage,
        Char('8') => app.current = ScreenId::Services,
        Char('9') => app.current = ScreenId::Packages,
        Char('0') => app.current = ScreenId::Settings,
        Tab => {
            app.focus = match app.focus {
                Focus::Sidebar => Focus::Content,
                Focus::Content => Focus::Sidebar,
            };
        }
        _ => {
            // Forward to the focused screen.
            if matches!(app.focus, Focus::Content) {
                if let Some(s) = screens.iter_mut().find(|s| s.id() == app.current) {
                    if s.on_key(key, app) {
                        return false;
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

async fn run_confirm(_app: &mut App, tx: &mpsc::Sender<Action>, kind: ConfirmKind, arg: String) {
    let act = match kind {
        ConfirmKind::Reboot => RunAction::Reboot,
        ConfirmKind::Shutdown => RunAction::Shutdown,
        ConfirmKind::Kill => RunAction::ProcessKill(arg.parse().unwrap_or(0)),
        ConfirmKind::Remove => RunAction::PackageRemove(arg),
        ConfirmKind::DisconnectWifi => RunAction::WifiDisconnect,
    };
    let _ = tx.send(Action::Run(act)).await;
}

async fn run_input(app: &mut App, tx: &mpsc::Sender<Action>, kind: InputKind, value: String) {
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
        InputKind::ConnectSSID => RunAction::WifiConnect {
            ssid: value,
            password: None,
        },
        InputKind::KillPid => match value.parse::<i32>() {
            Ok(p) => RunAction::ProcessKill(p),
            Err(_) => {
                app.push_toast(ToastKind::Error, "invalid pid");
                return;
            }
        },
    };
    let _ = tx.send(Action::Run(act)).await;
}

async fn handle_action(
    screens: &mut [Box<dyn Screen>],
    app: &mut App,
    tx: &mpsc::Sender<Action>,
    action: Action,
) -> bool {
    match action {
        Action::Tick => {} // refreshers already produced data
        Action::Key(_) => {}
        Action::Goto(id) => {
            app.current = id;
        }
        Action::Quit => return true,
        Action::Toast(kind, msg) => app.push_toast(kind, msg),
        Action::Toggle(key) => {
            use app::screen::SettingsKey::*;
            match key {
                Theme => {
                    app.theme_name = match app.theme_name {
                        app::screen::ThemeNameReexport::Dark => {
                            app::screen::ThemeNameReexport::Light
                        }
                        app::screen::ThemeNameReexport::Light => {
                            app::screen::ThemeNameReexport::HighContrast
                        }
                        app::screen::ThemeNameReexport::HighContrast => {
                            app::screen::ThemeNameReexport::Dark
                        }
                    };
                }
                Mouse => app.mouse = !app.mouse,
                NerdFont => app.nerd_font = !app.nerd_font,
                WebServer => {
                    let act = if *app.live.web_enabled.read().await {
                        RunAction::WebStop
                    } else {
                        RunAction::WebStart
                    };
                    let sender = app.live.web_ctrl.lock().await.clone();
                    let _ = sender.send((tx.clone(), Action::Run(act))).await;
                }
            }
        }
        Action::Run(act) => {
            spawn_action(tx.clone(), act);
        }
        Action::ConfirmModal => {} // handled inline above
        Action::CancelModal => app.modal = Modal::None,
        Action::SubmitInput(value) => {
            // Already handled in the key path.
            let _ = value;
        }
        Action::LogPushed(line) => {
            app.logs.push(line);
            if app.logs.len() > 1000 {
                let drop = app.logs.len() - 1000;
                app.logs.drain(0..drop);
            }
        }
        Action::Refresh(id) => {
            // Trivial: re-render. The background task already produces data.
            let _ = id;
            let _ = screens;
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
