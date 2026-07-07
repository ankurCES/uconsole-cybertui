//! Centralised theme: one accent color, one warn, one error. Used by every
//! screen and widget. Defined as a struct of `Color`s so the theme can be
//! swapped at runtime (Settings → Theme) without re-rendering logic.
//!
//! Some `Glyphs` fields are unused while their screens are still stubs —
//! they are referenced by `glyphs()` consumers as Phase-3 wires up.
//! See ROADMAP.md.
#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};

/// All built-in theme names. The order here is the cycling order used by
/// the Settings screen's theme picker and the public `Theme::next` /
/// `Theme::prev` helpers.
///
/// Additive: existing variants (`Dark`, `Light`, `HighContrast`) keep
/// their discriminant so persisted prefs from older builds round-trip
/// cleanly via serde's untagged fallback. New variants are appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    Dark,
    Light,
    HighContrast,
    Cyberpunk,
    VsCodeDark,
    VsCodeLight,
    CatppuccinMocha,
    Nord,
    GruvboxDark,
    SolarizedDark,
}

/// All theme names in cycling order — `Settings` uses this to advance on
/// each press of the theme key without having to keep its own ordering.
pub const ALL_THEME_NAMES: &[ThemeName] = &[
    ThemeName::Dark,
    ThemeName::Light,
    ThemeName::HighContrast,
    ThemeName::Cyberpunk,
    ThemeName::VsCodeDark,
    ThemeName::VsCodeLight,
    ThemeName::CatppuccinMocha,
    ThemeName::Nord,
    ThemeName::GruvboxDark,
    ThemeName::SolarizedDark,
];

impl ThemeName {
    /// Next theme in `ALL_THEME_NAMES`, wrapping at the end. Used by the
    /// Settings screen's `t` shortcut.
    pub fn next(self) -> Self {
        let pos = ALL_THEME_NAMES
            .iter()
            .position(|n| *n == self)
            .unwrap_or(0);
        ALL_THEME_NAMES[(pos + 1) % ALL_THEME_NAMES.len()]
    }

    /// Previous theme, wrapping at the start. Reserved for a future
    /// `Shift+T` shortcut.
    #[allow(dead_code)]
    pub fn prev(self) -> Self {
        let pos = ALL_THEME_NAMES
            .iter()
            .position(|n| *n == self)
            .unwrap_or(0);
        let n = ALL_THEME_NAMES.len();
        ALL_THEME_NAMES[(pos + n - 1) % n]
    }

    /// Stable kebab-case identifier used in persisted prefs (`prefs.json`)
    /// and CLI flags. Stable across builds so renaming a variant in Rust
    /// is a breaking change for user prefs — keep these names frozen.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
            ThemeName::HighContrast => "high-contrast",
            ThemeName::Cyberpunk => "cyberpunk",
            ThemeName::VsCodeDark => "vscode-dark",
            ThemeName::VsCodeLight => "vscode-light",
            ThemeName::CatppuccinMocha => "catppuccin-mocha",
            ThemeName::Nord => "nord",
            ThemeName::GruvboxDark => "gruvbox-dark",
            ThemeName::SolarizedDark => "solarized-dark",
        }
    }

    /// Reverse lookup. Unknown / future-stripped strings map to `Dark`
    /// so a corrupt or older prefs file still renders something.
    pub fn from_str(s: &str) -> Self {
        match s {
            "dark" => ThemeName::Dark,
            "light" => ThemeName::Light,
            "high-contrast" => ThemeName::HighContrast,
            "cyberpunk" => ThemeName::Cyberpunk,
            "vscode-dark" => ThemeName::VsCodeDark,
            "vscode-light" => ThemeName::VsCodeLight,
            "catppuccin-mocha" => ThemeName::CatppuccinMocha,
            "nord" => ThemeName::Nord,
            "gruvbox-dark" => ThemeName::GruvboxDark,
            "solarized-dark" => ThemeName::SolarizedDark,
            // Unknown / legacy — fall back rather than panic. The user
            // sees `dark` and the next save normalises the prefs file.
            _ => ThemeName::Dark,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub accent_2: Color,
    pub warn: Color,
    pub error: Color,
    pub ok: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub border: Color,
    pub border_focus: Color,
}

impl Theme {
    pub fn by_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self {
                bg: Color::Reset,
                fg: Color::Rgb(220, 220, 220),
                dim: Color::Rgb(120, 120, 120),
                accent: Color::Rgb(0, 200, 220),     // cyan
                accent_2: Color::Rgb(170, 120, 255), // violet
                warn: Color::Rgb(240, 180, 60),
                error: Color::Rgb(240, 90, 90),
                ok: Color::Rgb(110, 220, 130),
                selection_bg: Color::Rgb(0, 200, 220),
                selection_fg: Color::Rgb(15, 15, 25),
                border: Color::Rgb(70, 70, 80),
                border_focus: Color::Rgb(0, 200, 220),
            },
            ThemeName::Light => Self {
                bg: Color::Rgb(248, 248, 248),
                fg: Color::Rgb(20, 20, 20),
                dim: Color::Rgb(110, 110, 110),
                accent: Color::Rgb(20, 120, 160),
                accent_2: Color::Rgb(100, 60, 180),
                warn: Color::Rgb(180, 120, 20),
                error: Color::Rgb(190, 40, 40),
                ok: Color::Rgb(30, 140, 60),
                selection_bg: Color::Rgb(20, 120, 160),
                selection_fg: Color::Rgb(248, 248, 248),
                border: Color::Rgb(180, 180, 180),
                border_focus: Color::Rgb(20, 120, 160),
            },
            ThemeName::HighContrast => Self {
                bg: Color::Black,
                fg: Color::White,
                dim: Color::Rgb(180, 180, 180),
                accent: Color::Yellow,
                accent_2: Color::Magenta,
                warn: Color::LightYellow,
                error: Color::LightRed,
                ok: Color::LightGreen,
                selection_bg: Color::Yellow,
                selection_fg: Color::Black,
                border: Color::White,
                border_focus: Color::Yellow,
            },
            // Cyberpunk: the magenta/cyan neon of the install script banner.
            // Deep purple-near-black bg, hot pink accent, cyan secondary,
            // neon green ok. Borders glow magenta.
            ThemeName::Cyberpunk => Self {
                bg: Color::Rgb(15, 10, 25),
                fg: Color::Rgb(230, 220, 255),
                dim: Color::Rgb(120, 100, 150),
                accent: Color::Rgb(255, 60, 180),     // hot magenta
                accent_2: Color::Rgb(0, 240, 255),   // electric cyan
                warn: Color::Rgb(255, 200, 80),
                error: Color::Rgb(255, 80, 120),
                ok: Color::Rgb(110, 255, 160),
                selection_bg: Color::Rgb(255, 60, 180),
                selection_fg: Color::Rgb(15, 10, 25),
                border: Color::Rgb(90, 50, 130),
                border_focus: Color::Rgb(255, 60, 180),
            },
            // VS Code Dark+ (built-in default theme). Source: VSCode's
            // `dark-plus.json` workbench colors, swatches lifted from
            // official Visual Studio Code repo defaults.
            ThemeName::VsCodeDark => Self {
                bg: Color::Rgb(30, 30, 30),
                fg: Color::Rgb(212, 212, 212),
                dim: Color::Rgb(133, 133, 133),
                accent: Color::Rgb(0, 122, 204),      // VSCode blue
                accent_2: Color::Rgb(197, 134, 192),  // mauve
                warn: Color::Rgb(220, 220, 170),
                error: Color::Rgb(244, 71, 71),
                ok: Color::Rgb(73, 201, 176),
                selection_bg: Color::Rgb(38, 79, 120),
                selection_fg: Color::Rgb(255, 255, 255),
                border: Color::Rgb(60, 60, 60),
                border_focus: Color::Rgb(0, 122, 204),
            },
            // VS Code Light+ — same theme, light variant.
            ThemeName::VsCodeLight => Self {
                bg: Color::Rgb(255, 255, 255),
                fg: Color::Rgb(30, 30, 30),
                dim: Color::Rgb(120, 120, 120),
                accent: Color::Rgb(0, 100, 180),
                accent_2: Color::Rgb(128, 50, 150),
                warn: Color::Rgb(170, 100, 0),
                error: Color::Rgb(200, 30, 30),
                ok: Color::Rgb(20, 140, 60),
                selection_bg: Color::Rgb(173, 214, 255),
                selection_fg: Color::Rgb(30, 30, 30),
                border: Color::Rgb(200, 200, 200),
                border_focus: Color::Rgb(0, 100, 180),
            },
            // Catppuccin Mocha — same palette that already lives in
            // `ui::palette::Palette`. Kept inline here rather than
            // routed through `Palette` so the renderer doesn't pay an
            // indirection cost and so a Palette rename can't break the
            // theme system.
            ThemeName::CatppuccinMocha => Self {
                bg: Color::Rgb(30, 30, 46),         // base
                fg: Color::Rgb(205, 214, 244),      // text
                dim: Color::Rgb(108, 112, 134),     // overlay0
                accent: Color::Rgb(137, 180, 250),  // blue
                accent_2: Color::Rgb(203, 166, 247),// mauve
                warn: Color::Rgb(229, 200, 144),    // yellow
                error: Color::Rgb(243, 139, 168),   // red
                ok: Color::Rgb(166, 227, 161),      // green
                selection_bg: Color::Rgb(69, 71, 90),
                selection_fg: Color::Rgb(205, 214, 244),
                border: Color::Rgb(49, 50, 68),     // surface0
                border_focus: Color::Rgb(137, 180, 250),
            },
            // Nord — same palette as `Palette::nord`. Arctic blues and
            // muted greens; the dim is frost-pole so it stays readable
            // on snow-storm bg.
            ThemeName::Nord => Self {
                bg: Color::Rgb(46, 52, 64),         // nord0
                fg: Color::Rgb(236, 239, 244),      // nord6
                dim: Color::Rgb(76, 86, 106),       // nord3
                accent: Color::Rgb(136, 192, 208),  // nord8 (frost)
                accent_2: Color::Rgb(180, 142, 173),// nord15 (aurora purple)
                warn: Color::Rgb(235, 203, 139),    // nord13 (aurora yellow)
                error: Color::Rgb(191, 97, 106),    // nord11 (aurora red)
                ok: Color::Rgb(163, 190, 140),      // nord14 (aurora green)
                selection_bg: Color::Rgb(67, 76, 94),
                selection_fg: Color::Rgb(236, 239, 244),
                border: Color::Rgb(59, 66, 82),     // nord1
                border_focus: Color::Rgb(136, 192, 208),
            },
            // Gruvbox Dark — warm earth tones. Retro/sepia feel.
            ThemeName::GruvboxDark => Self {
                bg: Color::Rgb(40, 40, 40),         // bg0_h
                fg: Color::Rgb(235, 219, 178),      // fg
                dim: Color::Rgb(146, 131, 116),     // gray
                accent: Color::Rgb(131, 165, 152),  // aqua
                accent_2: Color::Rgb(211, 134, 155),// purple
                warn: Color::Rgb(250, 189, 47),     // yellow
                error: Color::Rgb(251, 73, 52),     // red
                ok: Color::Rgb(184, 187, 38),       // green
                selection_bg: Color::Rgb(80, 73, 69),
                selection_fg: Color::Rgb(235, 219, 178),
                border: Color::Rgb(60, 56, 54),     // bg1
                border_focus: Color::Rgb(131, 165, 152),
            },
            // Solarized Dark — classic Ethan Schoonover palette.
            ThemeName::SolarizedDark => Self {
                bg: Color::Rgb(0, 43, 54),          // base03
                fg: Color::Rgb(131, 148, 150),      // base0
                dim: Color::Rgb(88, 110, 117),      // base01
                accent: Color::Rgb(38, 139, 210),   // blue
                accent_2: Color::Rgb(108, 113, 196),// violet
                warn: Color::Rgb(181, 137, 0),      // yellow
                error: Color::Rgb(220, 50, 47),     // red
                ok: Color::Rgb(133, 153, 0),        // green
                selection_bg: Color::Rgb(7, 54, 66),
                selection_fg: Color::Rgb(147, 161, 161),
                border: Color::Rgb(7, 54, 66),      // base02
                border_focus: Color::Rgb(38, 139, 210),
            },
        }
    }

    pub fn title(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }
    pub fn dim(&self) -> Style {
        Style::default().fg(self.dim)
    }
    pub fn warn(&self) -> Style {
        Style::default().fg(self.warn)
    }
    pub fn error(&self) -> Style {
        Style::default().fg(self.error)
    }
    pub fn ok(&self) -> Style {
        Style::default().fg(self.ok)
    }
    pub fn key(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }
    pub fn border(&self, focused: bool) -> Style {
        if focused {
            Style::default().fg(self.border_focus)
        } else {
            Style::default().fg(self.border)
        }
    }
}

/// Nerd Font glyphs with ASCII fallbacks (set `NERD_FONT=0` to disable).
pub struct Glyphs {
    pub bullet: &'static str,
    pub arrow: &'static str,
    pub check: &'static str,
    pub cross: &'static str,
    pub wifi: &'static str,
    pub bat: &'static str,
    pub temp: &'static str,
    pub cpu: &'static str,
    pub mem: &'static str,
    pub disk: &'static str,
    pub net: &'static str,
    pub signal_full: &'static str,
    pub signal_mid: &'static str,
    pub signal_low: &'static str,
    pub signal_none: &'static str,
}

pub const GLYPHS_NERD: Glyphs = Glyphs {
    bullet: "●",
    arrow: "▸",
    check: "✓",
    cross: "✗",
    wifi: "󰤨",
    bat: "🜲",
    temp: "▲",
    cpu: "▰",
    mem: "▰",
    disk: "▰",
    net: "󰈀",
    signal_full: "▰▰▰▰",
    signal_mid: "▰▰▰▱",
    signal_low: "▰▱▱▱",
    signal_none: "▱▱▱▱",
};

pub const GLYPHS_ASCII: Glyphs = Glyphs {
    bullet: "*",
    arrow: ">",
    check: "OK",
    cross: "X",
    wifi: "WiFi",
    bat: "BAT",
    temp: "T",
    cpu: "C",
    mem: "M",
    disk: "D",
    net: "NET",
    signal_full: "||||",
    signal_mid: "|||.",
    signal_low: "||..",
    signal_none: "|...",
};

pub fn glyphs() -> &'static Glyphs {
    if std::env::var("NERD_FONT").as_deref() == Ok("0") {
        &GLYPHS_ASCII
    } else {
        &GLYPHS_NERD
    }
}

pub use crate::ui::palette::Palette;
