//! Centralised theme: one accent color, one warn, one error. Used by every
//! screen and widget. Defined as a struct of `Color`s so the theme can be
//! swapped at runtime (Settings → Theme) without re-rendering logic.
//!
//! Some `Glyphs` fields are unused while their screens are still stubs —
//! they are referenced by `glyphs()` consumers as Phase-3 wires up.
//! See ROADMAP.md.
#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Dark,
    Light,
    HighContrast,
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
