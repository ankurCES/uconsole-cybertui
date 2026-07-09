//! One entry per screen, plus the global registry the command palette uses.

use crate::theme::ThemeName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenId {
    /// Phase 7 — Overworld: Bruce-firmware-style carousel menu.
    /// Lives at index 0 of `ScreenId::ALL` so the Tab-cycle always
    /// starts here. Single-pane screen with a 3×N grid of all
    /// visible menu names; Enter enters the focused screen; Esc
    /// opens a Quit confirmation modal (so a stray Esc from a
    /// child screen can't kill the app). Does NOT have a right
    /// pane (`has_right_pane == false`) so the region model
    /// never tries to step into it.
    Overworld,
    System,
    Network,
    Bluetooth,
    Power,
    Display,
    Audio,
    Storage,
    Services,
    Packages,
    Processes,
    Files,
    Logs,
    Settings,
    /// Module 4 — Files: in-TUI editor. Reachable only from the Files
    /// screen via `e` on a selected file. `is_hidden` returns `true`
    /// for the EditorScreen so Tab/Shift-Tab cycling (which uses
    /// `ScreenId::cycle` in this module) skips it.
    Editor,
    /// Meshtastic over LAN HTTP: longfast channel chat (left pane) + nodes with
    /// hops_away (right pane). The IP for the on-LAN node is supplied at
    /// runtime via the `i` modal — see `screens::lora` + `InputKind::LoraNodeIp`.
    LoRa,
    /// Step 3 — City screen: IP-geolocated road map rendered in braille
    /// (left pane) + live weather + wind data (right pane). The actual
    /// renderer lands in Step 8; this stub exists so the sidebar can
    /// resolve the variant and the layout-audit test can pin its
    /// multi-pane bucket before the real implementation is wired in.
    City,
    /// Phase 7 M4 — Intel screen: OSINT-feed aggregator with a 9-row
    /// layer grid (left pane) and a per-layer snapshot detail (right
    /// pane). Each layer renders a hardcoded `cyberdeck_intel::Snapshot`
    /// from the snapshot module — the M5 refiller lands later. Mirrors
    /// the City screen's two-pane shape: left = layer grid + status,
    /// right = selected layer's summary + raw payload preview.
    Intel,
    /// Phase 7 M7 — Recon screen: 7-tab OSINT action console. Each
    /// tab drives one `cyberdeck_intel::recon` primitive (DNS / WHOIS
    /// / IP / SSL / CVE / CRYPTO / SANCTIONS) and renders its output
    /// in a scrollable tinted area. Single-pane (no right pane) —
    /// the output IS the screen. Reachable via the overworld
    /// carousel and through the Tab cycle; SSRF-gated at the
    /// primitive layer (Osiris-derived MIT note).
    Recon,
}

impl ScreenId {
    /// The Overworld sits at index 0 so the Tab-cycle always opens
    /// here (the "front door" metaphor from Bruce firmware). After
    /// it comes the existing 15 screens in sidebar order; new
    /// screens land at the tail so `is_hidden`-skipped entries
    /// (currently just `Editor`) sit in a stable position.
    pub const ALL: &'static [ScreenId] = &[
        ScreenId::Overworld,
        ScreenId::System,
        ScreenId::Network,
        ScreenId::Bluetooth,
        ScreenId::Power,
        ScreenId::Display,
        ScreenId::Audio,
        ScreenId::Storage,
        ScreenId::Services,
        ScreenId::Packages,
        ScreenId::Processes,
        ScreenId::Files,
        ScreenId::Logs,
        ScreenId::Settings,
        ScreenId::Editor,
        ScreenId::LoRa,
        ScreenId::City,
        ScreenId::Intel,
        ScreenId::Recon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScreenId::Overworld => "Menu",
            ScreenId::System => "System",
            ScreenId::Network => "Network",
            ScreenId::Bluetooth => "Bluetooth",
            ScreenId::Power => "Power",
            ScreenId::Display => "Display",
            ScreenId::Audio => "Audio",
            ScreenId::Storage => "Storage",
            ScreenId::Services => "Services",
            ScreenId::Packages => "Packages",
            ScreenId::Processes => "Processes",
            ScreenId::Files => "Files",
            ScreenId::Logs => "Logs",
            ScreenId::Settings => "Settings",
            ScreenId::Editor => "Editor",
            ScreenId::LoRa => "LoRa",
            ScreenId::City => "City",
            ScreenId::Intel => "Intel",
            ScreenId::Recon => "Recon",
        }
    }

    /// True when the screen renders a left+right split so the region model
    /// has somewhere to step to. Mirrors the multi-pane screens whose
    /// `Borders::ALL` blocks both read `app.region`.
    ///
    /// The Overworld is intentionally single-pane so pressing `→`
    /// from its right border doesn't dead-end on a "no pane here"
    /// state — pressing Right while on the Overworld does nothing,
    /// matching its content shape.
    pub fn has_right_pane(self) -> bool {
        matches!(
            self,
            ScreenId::System
                | ScreenId::Network
                | ScreenId::Files
                | ScreenId::Power
                | ScreenId::Display
                | ScreenId::Packages
                | ScreenId::LoRa
                | ScreenId::City
                | ScreenId::Intel
        )
    }

    pub fn glyph(self) -> &'static str {
        match self {
            // Overworld = a 3×3 grid icon (the carousel itself).
            ScreenId::Overworld => "▦",
            ScreenId::System => "◉",
            ScreenId::Network => "≋",
            ScreenId::Bluetooth => "⛁",
            ScreenId::Power => "🜲",
            ScreenId::Display => "▣",
            ScreenId::Audio => "♪",
            ScreenId::Storage => "▤",
            ScreenId::Services => "⚙",
            ScreenId::Packages => "▦",
            ScreenId::Processes => "≡",
            ScreenId::Files => "▢",
            ScreenId::Logs => "▥",
            ScreenId::Settings => "✱",
            ScreenId::Editor => "✎",
            // LoRa = stacked nodes glyph (works as Nerd Font + ASCII fallback).
            ScreenId::LoRa => "≣",
            // City = a globe-with-grid glyph. ASCII fallback would be a
            // simple "@"; the braille renderer in Step 7 draws the
            // actual map so this is just a sidebar marker.
            ScreenId::City => "◍",
            // Intel = OSINT/radar "eye" glyph. Mirrors the intel
            // screen's surveillance/recon semantics without leaning
            // on colour or emoji.
            ScreenId::Intel => "◬",
            // Recon = magnifier — same shelf as Intel but flags the
            // action-console semantics (user types a query, hits
            // Enter, reads output) rather than the passive
            // surveillance shape of the Intel layer grid.
            ScreenId::Recon => "⌕",
        }
    }

    /// Step to the next/previous screen, skipping any whose `Screen::is_hidden`
    /// returns `true`. Mirrors orbital's Tab/Shift-Tab widget navigation with
    /// hidden-widget skipping. Wraps around the end of `ScreenId::ALL` in the
    /// cycle direction. If every screen is hidden the current screen is
    /// returned unchanged — cycling must never strand the user on a dead end.
    pub fn cycle(
        screens: &[Box<dyn Screen>],
        app: &crate::app::App,
        current: ScreenId,
        forward: bool,
    ) -> ScreenId {
        let all = ScreenId::ALL;
        let pos = all.iter().position(|s| *s == current).unwrap_or(0);
        let n = all.len();
        // Bound the loop at `n` iterations so a fully-hidden screen set can't
        // spin forever; the early return below handles that case explicitly.
        for step in 1..=n {
            let idx = if forward {
                (pos + step) % n
            } else {
                (pos + n - step % n) % n
            };
            let candidate = all[idx];
            // The slice carries one screen per ScreenId in the registered
            // order; an absent slot is treated as "not hidden" so default
            // screens stay reachable in tests that build partial screens.
            if let Some(s) = screens.get(idx) {
                if s.is_hidden(app) {
                    continue;
                }
            }
            return candidate;
        }
        current
    }

    /// Return the screens that should appear in the sidebar launcher,
    /// preserving `ScreenId::ALL` order. Mirrors the filter that
    /// `ui::draw_launcher` applies when building the tile grid, so
    /// `launcher_offset` (a visible-row index) maps cleanly to a
    /// `ScreenId` from this list.
    ///
    /// Sits on `ScreenId` rather than on a screen instance because the
    /// `launcher_offset` field is a `usize` and the dispatchers in
    /// `main.rs` need a list to bounds-check against; building the
    /// list inline in each handler would duplicate the filter.
    ///
    /// Filter rules:
    /// * If the screens slice has a slot at `ALL[idx]` and that
    ///   screen's `in_sidebar()` returns false, skip it.
    /// * If the screens slice has no slot at `ALL[idx]`, the id
    ///   must be `Editor` (the only intentional no-slot entry) to
    ///   survive — every other no-slot id is treated as absent.
    ///   This keeps the test-helper `build_screens()`, which only
    ///   registers 14 of 17 ids, from accidentally surfacing
    ///   `Overworld` (no slot) and `LoRa` (no slot) as visible
    ///   launcher tiles.
    pub fn sidebar_visible(screens: &[Box<dyn Screen>], app: &crate::app::App) -> Vec<ScreenId> {
        ScreenId::ALL
            .iter()
            .enumerate()
            .filter_map(|(idx, id)| {
                match screens.get(idx) {
                    Some(s) if !s.in_sidebar(app) => None,
                    Some(_) => Some(*id),
                    None if *id == ScreenId::Editor => None,
                    None => None,
                }
            })
            .collect()
    }
}

/// Re-export so the App state doesn't depend on the theme module directly.
pub type ThemeNameReexport = ThemeName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsKey {
    Theme,
    Mouse,
    NerdFont,
    WebServer,
    /// Step 2 — weather units (Metric / Imperial). Toggled with `u`
    /// on the Settings screen. Persisted in prefs.
    Units,
    /// Step 2 — City screen traffic overlay. Toggled with `T` on
    /// the Settings screen. Persisted in prefs.
    TrafficOverlay,
    /// Step 2 — City screen weather panel visibility. Toggled with
    /// `w` (when not on Settings). Persisted in prefs.
    WeatherPanel,
    /// User-editable keymap (Settings → Keys). Toggling enters the
    /// sub-mode rendered by `screens::settings::render` when
    /// `app.keymap_editing == true`.
    Keymap,
}

/// A trait every screen implements. Screens are stateless functions of App
/// + the live data; they don't hold their own state beyond what App exposes.
pub trait Screen {
    fn id(&self) -> ScreenId;
    #[allow(dead_code)] // exposed for future per-screen titles
    fn title(&self) -> &'static str {
        self.id().label()
    }
    /// Render the screen into the given area of the frame. The `focus` flag
    /// indicates whether the content pane is the focused one (for borders).
    fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        app: &mut crate::app::App,
        theme: &crate::theme::Theme,
        focus: bool,
    );
    /// Handle a key event while the screen is focused. Returns true if the
    /// event was consumed. Modal handling lives in the main loop, not here.
    fn on_key(&mut self, _key: crossterm::event::KeyEvent, _app: &mut crate::app::App) -> bool {
        false
    }
    /// Downcast hook for screens that need to be reached through a trait
    /// object. `main.rs` uses this on `LoraScreen` only to call `poll` on
    /// each `Action::Tick`. Default returns `None` for screens that don't
    /// need it; `LoraScreen` overrides it. `None` keeps the trait
    /// default-implementable and avoids forcing `Any` on every screen.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
    /// Immutable counterpart to `as_any_mut`. Used by tests in `main.rs`
    /// that want to introspect the singleton OverworldScreen's cursor
    /// after routing keys through the registry — see
    /// `menu_active_overworld_arrow_owns_cursor`. Same default-empty
    /// pattern so screens that don't need introspection pay no cost.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
    /// Whether this screen should be skipped by `Tab` / `Shift-Tab` screen
    /// cycling. Defaults to `false` so every screen is reachable unless it
    /// explicitly opts out. Mirrors orbital's hidden-widget skip in its
    /// Tab/Shift-Tab navigation.
    fn is_hidden(&self, _app: &crate::app::App) -> bool {
        false
    }
    /// Whether this screen should appear as a tile in the sidebar launcher.
    /// Defaults to `!is_hidden()`. Overworld opts out — the sidebar launcher
    /// *is* the menu, so listing a "Menu" tile alongside the screens it
    /// launches would be circular. The Tab cycle still hits Overworld; only
    /// the sidebar index skips it (so `launcher_offset` indexes line up
    /// with the screens you'd actually want to *jump to* with a digit key).
    fn in_sidebar(&self, app: &crate::app::App) -> bool {
        !self.is_hidden(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;

    /// Test fake: each instance declares its own `ScreenId` and `is_hidden`
    /// answer. Keeps the cycle test independent of the real screen impls.
    struct FakeScreen {
        id: ScreenId,
        hidden: bool,
    }

    impl Screen for FakeScreen {
        fn id(&self) -> ScreenId {
            self.id
        }
        fn render(
            &mut self,
            _frame: &mut ratatui::Frame,
            _area: ratatui::layout::Rect,
            _app: &mut crate::app::App,
            _theme: &crate::theme::Theme,
            _focus: bool,
        ) {
        }
        fn is_hidden(&self, _app: &crate::app::App) -> bool {
            self.hidden
        }
    }

    fn dummy_app() -> crate::app::App {
        // The cycle helper only inspects `is_hidden(&App)`; nothing else is
        // touched, so an unconnected mpsc pair is enough. We never await on
        // the receiver — it exists solely so `App::new` can take ownership
        // of both ends.
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(1);
        crate::app::App::new(tx, rx)
    }

    fn all_visible() -> Vec<Box<dyn Screen>> {
        // Mirror the runtime `Screen::is_hidden` answers here so the
        // cycle tests line up with what the real renderer would do.
        // Editor opts out of sidebar cycling (it's reachable only via
        // `e` from Files and exits via Esc); mark it hidden in the
        // test fake so wrap-around lands on `Settings` (the last
        // truly-visible screen) instead of the Editor sink.
        ScreenId::ALL
            .iter()
            .map(|id| {
                let hidden = matches!(id, ScreenId::Editor);
                Box::new(FakeScreen { id: *id, hidden }) as Box<dyn Screen>
            })
            .collect()
    }

    #[test]
    fn cycle_forward_steps_to_next_screen() {
        let screens = all_visible();
        let app = dummy_app();
        let next = ScreenId::cycle(&screens, &app, ScreenId::Network, true);
        assert_eq!(next, ScreenId::Bluetooth);
    }

    #[test]
    fn cycle_backward_wraps_around() {
        let screens = all_visible();
        let app = dummy_app();
        // Overworld sits at `ALL[0]`, so "going backward from
        // System" lands on Overworld directly — it no longer wraps
        // all the way around. From Overworld itself, backward *does*
        // wrap to the last visible screen. As we append visible
        // screens (City, Intel, …) the wrap target moves; we resolve
        // it dynamically against the current `ALL` tail so the test
        // asserts the wrap-around contract, not a specific screen.
        let prev_from_overworld =
            ScreenId::cycle(&screens, &app, ScreenId::Overworld, false);
        let expected = *ScreenId::ALL.last().expect("ScreenId::ALL is non-empty");
        assert_eq!(
            prev_from_overworld, expected,
            "backward-wrap from Overworld must land on the last screen in ALL"
        );
    }

    #[test]
    fn cycle_forward_wraps_around() {
        let screens = all_visible();
        let app = dummy_app();
        // Overworld sits at the start of the cycle, so the wrap from
        // the *current* last entry of `ALL` lands on Overworld. We
        // resolve "last" dynamically against `ScreenId::ALL` so this
        // test asserts the wrap-around contract rather than a specific
        // screen name, and survives future tail-appends (the next one
        // being whatever M5/M6 adds).
        let last = *ScreenId::ALL.last().expect("ScreenId::ALL is non-empty");
        let next = ScreenId::cycle(&screens, &app, last, true);
        assert_eq!(
            next, ScreenId::Overworld,
            "forward-wrap from the last entry must land on Overworld (the cycle's start)"
        );
    }

    #[test]
    fn cycle_skips_hidden_screens() {
        // Build a screen set where Network and Power are hidden. Starting on
        // System and going forward must land on Bluetooth (skipping Network).
        let mut screens: Vec<Box<dyn Screen>> = ScreenId::ALL
            .iter()
            .map(|id| {
                let hidden = matches!(id, ScreenId::Network | ScreenId::Power | ScreenId::Editor);
                Box::new(FakeScreen { id: *id, hidden }) as Box<dyn Screen>
            })
            .collect();
        let app = dummy_app();
        // First forward step must skip the hidden Network.
        let next = ScreenId::cycle(&screens, &app, ScreenId::System, true);
        assert_eq!(next, ScreenId::Bluetooth);
        // And the backward step from System lands on Overworld (the
        // immediate predecessor in `ALL`). Pre-Overworld the test
        // expected `City` — that wrapped all the way around. With
        // Overworld inserted at `ALL[0]` the wrap now goes one step
        // backward, which is the right behavior: System's left
        // neighbor in the cycle IS Overworld.
        let prev = ScreenId::cycle(&screens, &app, ScreenId::System, false);
        assert_eq!(prev, ScreenId::Overworld);
        // Sanity: with everything visible, the first forward step lands on
        // Network itself, proving the skip is what made the test above pass.
        for s in screens.iter_mut() {
            // No way back through `dyn Screen` to flip `hidden` without a
            // second FakeScreen type, so just rebuild the slice.
            let _ = s;
        }
        let visible = all_visible();
        let sanity = ScreenId::cycle(&visible, &app, ScreenId::System, true);
        assert_eq!(sanity, ScreenId::Network);
    }

    #[test]
    fn cycle_all_hidden_returns_current() {
        // If every screen is hidden the helper must NOT loop forever — it
        // must return the current screen unchanged so the user is never
        // stranded on a dead end.
        let screens: Vec<Box<dyn Screen>> = ScreenId::ALL
            .iter()
            .map(|id| Box::new(FakeScreen { id: *id, hidden: true }) as Box<dyn Screen>)
            .collect();
        let app = dummy_app();
        assert_eq!(
            ScreenId::cycle(&screens, &app, ScreenId::Network, true),
            ScreenId::Network
        );
        assert_eq!(
            ScreenId::cycle(&screens, &app, ScreenId::Network, false),
            ScreenId::Network
        );
    }

    /// `ScreenId::LoRa` resolves to the `LoRa` label/glyph and is listed in
    /// `ScreenId::ALL` so the sidebar can find it.
    #[test]
    fn lora_screen_is_registered() {
        let id = ScreenId::LoRa;
        assert_eq!(id.label(), "LoRa");
        assert_eq!(id.glyph(), "≣");
        assert!(ScreenId::ALL.contains(&ScreenId::LoRa));
        assert!(
            id.has_right_pane(),
            "LoRa is a multi-pane screen (chat | nodes) and must report has_right_pane() == true"
        );
    }

    /// Module 5b layout audit — locks the bucket classification from
    /// `docs/superpowers/specs/2026-06-28-tui-ux-improvements-design.md`.
    ///
    /// Three buckets:
    ///   * `multi` — every multi-pane screen renders a single
    ///     `Layout::default().direction(Direction::Horizontal)` with
    ///     `[Constraint::Percentage(60), Constraint::Percentage(40)]`.
    ///     Left = list/form, right = status block. No nested
    ///     `Layout` calls.
    ///   * `single` — single-pane screens (Storage, Services,
    ///     Processes, Logs, Settings, Bluetooth) render a single
    ///     `Block::default()` outer + a `Block::default().borders(
    ///     Borders::NONE)` inner list. No `Layout::Horizontal`,
    ///     no nested `Layout`.
    ///   * `exempt` — `editor` is off the sidebar (reachable only via
    ///     `e` from Files, exits via Esc). The spec explicitly
    ///     exempts it from the sidebar layout contract.
    ///
    /// Test reads each file as a `&str` via `include_str!` so the
    /// invariants are pinned at the source level — no `TestBackend`
    /// needed, no render cost. If a future edit breaks a bucket's
    /// contract, this test fails before the rendered TUI can.
    #[test]
    fn screen_renders_layout_audit() {
        // Path-relative to the crate root (`crates/tui/`).
        const MULTI: &[(&str, &str)] = &[
            ("system",    include_str!("../screens/system.rs")),
            ("power",     include_str!("../screens/power.rs")),
            ("display",   include_str!("../screens/display.rs")),
            ("audio",     include_str!("../screens/audio.rs")),
            ("packages",  include_str!("../screens/packages.rs")),
            ("files",     include_str!("../screens/files.rs")),
            ("network",   include_str!("../screens/network.rs")),
            // Step 3 — City is multi-pane (braille map | weather). The
            // stub render in screens/city/mod.rs already uses the
            // canonical [Percentage(60), Percentage(40)] Horizontal
            // split so this entry pins the bucket before the real
            // renderer lands.
            ("city",      include_str!("../screens/city/mod.rs")),
        ];
        const SINGLE: &[(&str, &str)] = &[
            ("storage",   include_str!("../screens/storage.rs")),
            ("services",  include_str!("../screens/services.rs")),
            ("processes", include_str!("../screens/processes.rs")),
            ("logs",      include_str!("../screens/logs.rs")),
            ("settings",  include_str!("../screens/settings.rs")),
            ("bluetooth", include_str!("../screens/bluetooth.rs")),
        ];
        // `editor` is exempt — skip.

        // Canonical spec-compliant constraint pair. The audit asserts the
        // file contains a `Layout::default()` chain with a
        // Direction::Horizontal split and this exact constraint array,
        // regardless of indentation. The whole point of pinning it
        // here is to lock the visual split; whitespace coupling to
        // the test source would be a maintenance trap.
        const SPEC_SPLIT: &str = "[Constraint::Percentage(60), Constraint::Percentage(40)]";
        const SPEC_DIR: &str = "Direction::Horizontal";

        for (name, src) in MULTI {
            // Must contain exactly one `Layout::default()` chain, and
            // it must be a Horizontal split with the 60/40 constraint
            // pair.
            let count = src.matches("Layout::default()").count();
            assert_eq!(
                count, 1,
                "{name}: multi-pane screen must have exactly one Layout::default() (got {count})"
            );
            assert!(
                src.contains(SPEC_DIR),
                "{name}: multi-pane split must use Direction::Horizontal"
            );
            assert!(
                src.contains(SPEC_SPLIT),
                "{name}: multi-pane split must be [Percentage(60), Percentage(40)] — spec deviation"
            );
            // No nested Layout:: calls inside the render fn.
            let nested = src
                .split("fn render")
                .nth(1)
                .map(|rest| rest.matches("Layout::default()").count().saturating_sub(1))
                .unwrap_or(0);
            assert_eq!(
                nested, 0,
                "{name}: multi-pane screen must not nest additional Layout::default() inside render"
            );
        }

        for (name, src) in SINGLE {
            // Must not use a horizontal split — these screens are
            // single-pane by design (a single list + a bottom hint
            // strip).
            assert!(
                !src.contains("Direction::Horizontal"),
                "{name}: single-pane screen must not use Direction::Horizontal"
            );
            assert!(
                !src.contains("Layout::default()"),
                "{name}: single-pane screen must not use Layout::default() (it has zero Layout splits)"
            );
            // Sanity: must still render at least one Block + one
            // List/Table — a screen with neither is misclassified.
            assert!(
                src.contains("Block::default()") && src.contains("List::new"),
                "{name}: single-pane screen must render a Block + a List"
            );
        }
    }
}
