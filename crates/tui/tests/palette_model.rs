//! Integration tests for the herd-style palette.

use cyberdeck_tui::Palette;

#[test]
fn palette_named_lookups_match() {
    assert!(Palette::by_name("catppuccin-mocha").is_some());
    assert!(Palette::by_name("gruvbox-dark").is_some());
    assert!(Palette::by_name("nord").is_some());
    assert!(Palette::by_name("legacy-dark").is_some());
    assert!(Palette::by_name("does-not-exist").is_none());
}

#[test]
fn palette_state_colors_are_distinct() {
    // Used by the agent-pill rendering; the four state colors must never
    // alias (a "blocked" pill rendered as green would silently tell the
    // user the wrong thing).
    let p = Palette::catppuccin_mocha();
    assert_ne!(p.red, p.yellow);
    assert_ne!(p.red, p.green);
    assert_ne!(p.red, p.teal);
    assert_ne!(p.yellow, p.green);
}

#[test]
fn legacy_dark_keyed_lookup_works() {
    // back-compat: the existing theme.rs Dark palette becomes a
    // first-class named look so Settings → Theme doesn't lose it.
    let p = Palette::by_name("legacy-dark").unwrap();
    // legacy-dark.panel_bg is the only Color::Reset in any palette —
    // this is the "no background" sentinel the existing renderer relies on.
    assert_eq!(p.panel_bg, ratatui::style::Color::Reset);
}

#[test]
fn each_named_palette_has_distinct_accent() {
    // The accent defines the brand; we want different palettes to look
    // visually different in the sidebar header.
    let accents = [
        Palette::by_name("catppuccin-mocha").unwrap().accent,
        Palette::by_name("gruvbox-dark").unwrap().accent,
        Palette::by_name("nord").unwrap().accent,
        Palette::by_name("legacy-dark").unwrap().accent,
    ];
    let unique: std::collections::HashSet<_> = accents.iter().collect();
    assert_eq!(unique.len(), accents.len(), "two palettes share the same accent");
}

#[test]
fn palette_is_copy() {
    // Confirms the struct can be embedded in a hot-render path that copies
    // the palette many times per frame; relies on `#[derive(Copy)]`.
    let p = Palette::catppuccin_mocha();
    let _ = p; // use the value
    fn assert_copy<T: Copy>(_: T) {}
    assert_copy(p);
}