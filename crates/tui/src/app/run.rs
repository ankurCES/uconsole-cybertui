use std::io::Stdout;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Local;
use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tokio::sync::mpsc;

use crate::app::action::{Action, RunAction};
use crate::app::live_data::LiveData;
use crate::app::screen::{ScreenId, ScreenRegistry};
use crate::app::state::AppState;
use crate::app::toast::ToastKind;
use crate::modal::render_modal_overlay;
use crate::nav::dispatch::dispatch_key;
use crate::nav::menu_stack::MenuStack;
use crate::nav::UiContext;
use crate::prefs::Prefs;
use crate::screens;
use crate::theme::Theme;

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

    // S19: spawn llama-server sidecar if a model is available.
    #[cfg(feature = "http")]
    let llama_child: Option<crate::llm::LlamaSidecar> = {
        match crate::llm::spawn_sidecar(&prefs).await {
            Some(sidecar) => {
                crate::llm::spawn_health_poll(tx.clone(), &sidecar);
                Some(sidecar)
            }
            None => {
                // No binary or no model — LlamaDown notifies the screen.
                let _ = tx.try_send(Action::LlamaDown);
                None
            }
        }
    };

    let mut state = AppState::new(prefs, live, tx, rx);
    // v2 root is MainMenu, not Overworld.
    state.nav.stack = MenuStack::with_root(ScreenId::MainMenu);

    let mut screens = build_registry();
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            state.ui.clock = Local::now();
            terminal
                .draw(|f| draw_v2(f, &mut state, &mut screens, &dispatch_tx))
                .context("terminal draw")?;
            needs_redraw = false;
        }

        tokio::select! {
            maybe = state.rx.recv() => {
                match maybe {
                    Some(Action::Quit) | None => {
                        #[cfg(feature = "http")]
                        if let Some(mut child) = llama_child {
                            crate::llm::kill_sidecar(&mut child).await;
                        }
                        return Ok(());
                    }
                    Some(a) => apply_action(a, &mut state, &dispatch_tx),
                }
                // Drain burst: coalesce multiple refresher ticks into one redraw.
                while let Ok(a) = state.rx.try_recv() {
                    if matches!(a, Action::Quit) { return Ok(()); }
                    apply_action(a, &mut state, &dispatch_tx);
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
            Constraint::Length(1), // top menu bar (row 0)
            Constraint::Min(0),    // content      (rows 1-21)
            Constraint::Length(1), // hint bar      (row 22)
            Constraint::Length(1), // status bar    (row 23)
        ])
        .split(area);
    let (menu_row, content, hint_row, status_row) = (chunks[0], chunks[1], chunks[2], chunks[3]);

    // Top menu bar — always rendered on row 0.
    state.ui.top_menu.render_bar(f, menu_row, &state.ui.theme);

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

    // Status/clock bar — metrics left, clock right
    {
        let clock_str = state.ui.clock.format("%H:%M:%S").to_string();
        let mut spans: Vec<Span> = Vec::new();

        if let Ok(ssid) = state.live.active_ssid.try_read() {
            if let Some(name) = ssid.as_ref() {
                if !spans.is_empty() { spans.push(Span::raw("  ")); }
                spans.push(Span::styled(format!("▂▄▆█ {name}"), Style::default().fg(Color::Green)));
            }
        }

        if let Ok(batt) = state.live.battery.try_read() {
            if let Some(b) = batt.as_ref() {
                if !spans.is_empty() { spans.push(Span::raw("  ")); }
                let c = if b.capacity > 50 { Color::Green } else if b.capacity > 20 { Color::Yellow } else { Color::Red };
                spans.push(Span::styled(format!("⚡ {}%", b.capacity), Style::default().fg(c)));
            }
        }

        if let Ok(info) = state.live.info.try_read() {
            if !spans.is_empty() { spans.push(Span::raw("  ")); }
            let cpu_pct = ((info.loadavg.0 / info.cpu_count.max(1) as f64) * 100.0).min(100.0) as u8;
            let cc = if cpu_pct < 50 { Color::Green } else if cpu_pct < 80 { Color::Yellow } else { Color::Red };
            spans.push(Span::styled(format!("CPU {cpu_pct}%"), Style::default().fg(cc)));

            let used_kb = info.memory.total_kb.saturating_sub(info.memory.available_kb);
            let mem_str = if used_kb >= 1_048_576 {
                format!("{:.1}G", used_kb as f64 / 1_048_576.0)
            } else {
                format!("{}M", used_kb / 1024)
            };
            let mc = if info.memory.used_pct < 60.0 { Color::Green } else if info.memory.used_pct < 85.0 { Color::Yellow } else { Color::Red };
            spans.push(Span::raw("  "));
            spans.push(Span::styled(format!("MEM {mem_str}"), Style::default().fg(mc)));
        }

        if let Some(msg) = state.ui.status_msg.as_deref() {
            if !spans.is_empty() { spans.push(Span::raw("  ")); }
            spans.push(Span::styled(msg.to_owned(), Style::default().fg(dim)));
        }

        let status_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(8)])
            .split(status_row);

        f.render_widget(Paragraph::new(Line::from(spans)), status_chunks[0]);
        f.render_widget(
            Paragraph::new(Span::raw(clock_str)).style(Style::default().fg(dim)),
            status_chunks[1],
        );
    }

    // Modal overlay — rendered last so it sits on top.
    if let Some(modal) = state.ui.modal.as_ref() {
        render_modal_overlay(f, area, modal.as_ref(), &state.ui.theme);
    }

    // Top menu dropdown — rendered after modal so it appears over content,
    // but a modal takes full priority when open.
    if state.ui.modal.is_none() {
        state.ui.top_menu.render_dropdown(f, menu_row, &state.ui.theme);
    }
}

fn build_registry() -> ScreenRegistry {
    use screens::{
        ai_v2::AiScreenV2,
        ai_logs_v2::AiLogsScreen,
        audio_v2::AudioScreenV2,
        bluetooth_v2::BluetoothScreenV2,
        city_v2::CityScreenV2,
        display_v2::DisplayScreenV2,
        editor_v2::EditorScreenV2,
        files_v2::FilesScreenV2,
        intel_v2::IntelScreenV2,
        logs_v2::LogsScreenV2,
        lora_v2::LoraScreenV2,
        main_menu::MainMenuScreen,
        network_v2::NetworkScreenV2,
        packages_v2::PackagesScreenV2,
        power_v2::PowerScreenV2,
        processes_v2::ProcessesScreenV2,
        recon_v2::ReconScreenV2,
        screensaver_v2::ScreensaverScreen,
        services_v2::ServicesScreenV2,
        settings_v2::SettingsScreenV2,
        storage_v2::StorageScreenV2,
        submenu::SubMenuScreen,
        system_v2::SystemScreenV2,
    };

    let mut r = ScreenRegistry::new();
    r.register(Box::new(MainMenuScreen::default()));
    r.register(Box::new(ScreensaverScreen::default()));
    r.register(Box::new(SubMenuScreen::default()));
    r.register(Box::new(SystemScreenV2::default()));
    r.register(Box::new(NetworkScreenV2::default()));
    r.register(Box::new(LoraScreenV2::default()));
    r.register(Box::new(IntelScreenV2::default()));
    r.register(Box::new(ReconScreenV2::default()));
    r.register(Box::new(CityScreenV2::default()));
    r.register(Box::new(BluetoothScreenV2::default()));
    r.register(Box::new(PowerScreenV2::default()));
    r.register(Box::new(DisplayScreenV2::default()));
    r.register(Box::new(AudioScreenV2::default()));
    r.register(Box::new(StorageScreenV2::default()));
    r.register(Box::new(PackagesScreenV2::default()));
    r.register(Box::new(ProcessesScreenV2::default()));
    r.register(Box::new(ServicesScreenV2::default()));
    r.register(Box::new(FilesScreenV2::default()));
    r.register(Box::new(LogsScreenV2::default()));
    r.register(Box::new(SettingsScreenV2::default()));
    r.register(Box::new(EditorScreenV2::default()));
    r.register(Box::new(AiScreenV2::default()));
    r.register(Box::new(AiLogsScreen::default()));
    r
}

fn apply_action(action: Action, state: &mut AppState, tx: &mpsc::Sender<Action>) {
    match action {
        Action::SetTheme(name) => {
            state.ui.theme = Theme::by_name(name);
            state.prefs.theme = name;
            state.prefs.save();
        }
        Action::LoraNodeAdd(raw) => {
            let mut parts = raw.trim().splitn(2, ' ');
            let ip = parts.next().unwrap_or("").trim().to_string();
            if ip.is_empty() { return; }
            let label = parts.next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            state.prefs.lora_nodes.push(crate::prefs::SavedLoraNode { ip, label });
            state.prefs.save();
        }
        Action::LoraNodeDelete(idx) => {
            if idx < state.prefs.lora_nodes.len() {
                state.prefs.lora_nodes.remove(idx);
                state.prefs.save();
            }
        }
        Action::Run(ra) => {
            let tx2 = tx.clone();
            tokio::spawn(async move { handle_run_action(ra, tx2).await; });
        }
        #[cfg(feature = "http")]
        Action::AiSubmit(text) => {
            use crate::app::live_data::{AiMessage, AiRole};
            let mut history: Vec<AiMessage> = Vec::new();
            if let Ok(mut msgs) = state.live.ai_messages.try_write() {
                msgs.push(AiMessage {
                    role: AiRole::User,
                    content: text,
                    ..Default::default()
                });
                msgs.push(AiMessage {
                    role: AiRole::Assistant,
                    streaming: true,
                    ..Default::default()
                });
                // snapshot all but the trailing streaming placeholder
                let len = msgs.len();
                history = msgs[..len.saturating_sub(1)].to_vec();
            }
            let tx2 = tx.clone();
            tokio::spawn(async move {
                crate::llm::stream_chat(history, tx2).await;
            });
        }
        #[cfg(feature = "http")]
        Action::AiToken(tok) => {
            if let Ok(mut msgs) = state.live.ai_messages.try_write() {
                if let Some(last) = msgs.last_mut() {
                    last.content.push_str(&tok);
                }
            }
        }
        #[cfg(feature = "http")]
        Action::AiThinkToken(tok) => {
            if let Ok(mut msgs) = state.live.ai_messages.try_write() {
                if let Some(last) = msgs.last_mut() {
                    last.thinking.push_str(&tok);
                }
            }
        }
        #[cfg(feature = "http")]
        Action::AiDone => {
            if let Ok(mut msgs) = state.live.ai_messages.try_write() {
                if let Some(last) = msgs.last_mut() {
                    last.streaming = false;
                }
            }
        }
        #[cfg(feature = "http")]
        Action::LlamaReady => {
            if let Ok(mut r) = state.live.llama_ready.try_write() {
                *r = true;
            }
        }
        #[cfg(feature = "http")]
        Action::LlamaDown => {
            let msg = "no model found — place a .gguf in ~/.cyberdeck/models/";
            if let Ok(mut e) = state.live.llama_error.try_write() {
                *e = Some(msg.into());
            }
            state.ui.push_toast(crate::app::toast::ToastKind::Error, &format!("AI: {msg}"));
        }
        #[cfg(feature = "http")]
        Action::LlamaFailed(detail) => {
            if let Ok(mut e) = state.live.llama_error.try_write() {
                *e = Some(detail.clone());
            }
            state.ui.push_toast(
                crate::app::toast::ToastKind::Error,
                &format!("AI model failed: {detail}"),
            );
        }
        _ => {}
    }
}

async fn handle_run_action(action: RunAction, tx: mpsc::Sender<Action>) {
    let result: Result<String, String> = tokio::task::spawn_blocking(move || {
        run_system_action(&action)
    }).await.unwrap_or(Err("task panicked".to_string()));

    match result {
        Ok(msg) if !msg.is_empty() => { tx.try_send(Action::Toast(ToastKind::Info, msg)).ok(); }
        Err(e) => { tx.try_send(Action::Toast(ToastKind::Error, e)).ok(); }
        _ => {}
    }
}

fn run_system_action(action: &RunAction) -> Result<String, String> {
    use std::process::Command;
    fn sh(args: &[&str]) -> Result<String, String> {
        let out = Command::new(args[0]).args(&args[1..]).output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
    match action {
        RunAction::ProcessKill(pid) => {
            sh(&["kill", "-9", &pid.to_string()])?;
            Ok(format!("killed PID {pid}"))
        }
        RunAction::ServiceStart(unit) => {
            sh(&["systemctl", "start", unit])?;
            Ok(format!("started {unit}"))
        }
        RunAction::ServiceStop(unit) => {
            sh(&["systemctl", "stop", unit])?;
            Ok(format!("stopped {unit}"))
        }
        RunAction::ServiceRestart(unit) => {
            sh(&["systemctl", "restart", unit])?;
            Ok(format!("restarted {unit}"))
        }
        RunAction::SetGovernor(gov) => {
            // Write to every online CPU core's scaling_governor.
            let glob_path = "/sys/devices/system/cpu";
            if let Ok(rd) = std::fs::read_dir(glob_path) {
                for entry in rd.flatten() {
                    let p = entry.path().join("cpufreq/scaling_governor");
                    std::fs::write(&p, gov.as_bytes()).ok();
                }
            }
            Ok(format!("governor → {gov}"))
        }
        RunAction::SetBrightness(pct) => {
            // Find first backlight device.
            let bl = "/sys/class/backlight";
            if let Ok(rd) = std::fs::read_dir(bl) {
                for entry in rd.flatten() {
                    let max_path = entry.path().join("max_brightness");
                    if let Ok(s) = std::fs::read_to_string(&max_path) {
                        if let Ok(max) = s.trim().parse::<u64>() {
                            let val = (max * *pct as u64 / 100).min(max);
                            std::fs::write(entry.path().join("brightness"), val.to_string()).ok();
                            return Ok(format!("brightness → {pct}%"));
                        }
                    }
                }
            }
            Err("no backlight device found".to_string())
        }
        RunAction::SetVolume { target, percent } => {
            sh(&["pactl", "set-sink-volume", target, &format!("{percent}%")])?;
            Ok(format!("volume → {percent}%"))
        }
        RunAction::MuteSink { target, mute } => {
            sh(&["pactl", "set-sink-mute", target, if *mute { "1" } else { "0" }])?;
            Ok(if *mute { "muted".to_string() } else { "unmuted".to_string() })
        }
        RunAction::SetDefaultSink(id) => {
            sh(&["pactl", "set-default-sink", id])?;
            Ok(format!("default sink → {id}"))
        }
        RunAction::BluetoothConnect(mac) => {
            sh(&["bluetoothctl", "connect", mac])?;
            Ok(format!("connected {mac}"))
        }
        RunAction::BluetoothDisconnect(mac) => {
            sh(&["bluetoothctl", "disconnect", mac])?;
            Ok(format!("disconnected {mac}"))
        }
        RunAction::BluetoothPair(mac) => {
            sh(&["bluetoothctl", "pair", mac])?;
            Ok(format!("paired {mac}"))
        }
        RunAction::BluetoothTrust(mac) => {
            sh(&["bluetoothctl", "trust", mac])?;
            Ok(format!("trusted {mac}"))
        }
        RunAction::BluetoothScan => {
            // Brief scan — non-blocking fire-and-forget.
            Command::new("bluetoothctl").args(["scan", "on"]).spawn().ok();
            Ok(String::new())
        }
        RunAction::Reboot => { sh(&["systemctl", "reboot"])?; Ok(String::new()) }
        RunAction::Shutdown => { sh(&["systemctl", "poweroff"])?; Ok(String::new()) }
        RunAction::Suspend => { sh(&["systemctl", "suspend"])?; Ok(String::new()) }
        RunAction::Hibernate => { sh(&["systemctl", "hibernate"])?; Ok(String::new()) }
        _ => Ok(String::new()), // other actions handled elsewhere
    }
}
