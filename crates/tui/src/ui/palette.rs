//! herd-style palette — one struct, many named looks. Mirrors herdr's
//! `Palette` shape so a future "import herdr theme.toml" can map fields 1:1.
//! See docs/superpowers/plans/2026-07-05-herd-style-ui-and-cli.md.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub panel_bg: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface_dim: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub mauve: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub teal: Color,
}

impl Palette {
    pub fn catppuccin_mocha() -> Self {
        Self {
            accent:     Color::Rgb(137, 180, 250), // blue
            panel_bg:   Color::Rgb(30, 30, 46),
            surface0:   Color::Rgb(49, 50, 68),
            surface1:   Color::Rgb(69, 71, 90),
            surface_dim:Color::Rgb(24, 24, 37),
            overlay0:   Color::Rgb(108, 112, 134),
            overlay1:   Color::Rgb(127, 132, 156),
            text:       Color::Rgb(205, 214, 244),
            subtext0:   Color::Rgb(166, 173, 200),
            mauve:      Color::Rgb(203, 166, 247),
            green:      Color::Rgb(166, 227, 161),
            yellow:     Color::Rgb(229, 200, 144),
            red:        Color::Rgb(243, 139, 168),
            blue:       Color::Rgb(137, 180, 250),
            teal:       Color::Rgb(148, 226, 213),
        }
    }

    pub fn gruvbox_dark() -> Self {
        Self {
            accent:     Color::Rgb(131, 165, 152),
            panel_bg:   Color::Rgb(40, 40, 40),
            surface0:   Color::Rgb(60, 56, 54),
            surface1:   Color::Rgb(80, 73, 69),
            surface_dim:Color::Rgb(29, 32, 33),
            overlay0:   Color::Rgb(146, 131, 116),
            overlay1:   Color::Rgb(189, 174, 147),
            text:       Color::Rgb(235, 219, 178),
            subtext0:   Color::Rgb(213, 196, 161),
            mauve:      Color::Rgb(211, 134, 155),
            green:      Color::Rgb(184, 187, 38),
            yellow:     Color::Rgb(250, 189, 47),
            red:        Color::Rgb(251, 73, 52),
            blue:       Color::Rgb(131, 165, 152),
            teal:       Color::Rgb(142, 192, 124),
        }
    }

    pub fn nord() -> Self {
        Self {
            accent:     Color::Rgb(136, 192, 208),
            panel_bg:   Color::Rgb(46, 52, 64),
            surface0:   Color::Rgb(59, 66, 82),
            surface1:   Color::Rgb(67, 76, 94),
            surface_dim:Color::Rgb(36, 40, 50),
            overlay0:   Color::Rgb(76, 86, 106),
            overlay1:   Color::Rgb(143, 188, 187),
            text:       Color::Rgb(236, 239, 244),
            subtext0:   Color::Rgb(216, 222, 233),
            mauve:      Color::Rgb(180, 142, 173),
            green:      Color::Rgb(163, 190, 140),
            yellow:     Color::Rgb(235, 203, 139),
            red:        Color::Rgb(191, 97, 106),
            blue:       Color::Rgb(129, 161, 193),
            teal:       Color::Rgb(143, 188, 187),
        }
    }

    pub fn legacy_dark() -> Self {
        Self {
            accent:     Color::Rgb(0, 200, 220),
            panel_bg:   Color::Reset,
            surface0:   Color::Rgb(30, 30, 30),
            surface1:   Color::Rgb(50, 50, 50),
            surface_dim:Color::Rgb(15, 15, 15),
            overlay0:   Color::Rgb(120, 120, 120),
            overlay1:   Color::Rgb(160, 160, 160),
            text:       Color::Rgb(220, 220, 220),
            subtext0:   Color::Rgb(180, 180, 180),
            mauve:      Color::Rgb(170, 120, 255),
            green:      Color::Rgb(110, 220, 130),
            yellow:     Color::Rgb(240, 180, 60),
            red:        Color::Rgb(240, 90, 90),
            blue:       Color::Rgb(0, 200, 220),
            teal:       Color::Rgb(110, 220, 220),
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "catppuccin-mocha" => Some(Self::catppuccin_mocha()),
            "gruvbox-dark" => Some(Self::gruvbox_dark()),
            "nord" => Some(Self::nord()),
            "legacy-dark" => Some(Self::legacy_dark()),
            _ => None,
        }
    }
}