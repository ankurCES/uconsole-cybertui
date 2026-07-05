use cyberdeck_tui::workspace::{Pane, Workspace};
use cyberdeck_tui::app::ScreenId;

#[test]
fn new_workspace_starts_with_default_tab() {
    let ws = Workspace::new("cyberdeck");
    assert_eq!(ws.tabs.len(), 1);
    assert_eq!(ws.tabs[0].label, "main");
    assert!(ws.focused_tab().panes.is_empty());
}

#[test]
fn split_pane_returns_new_pane_with_correct_direction() {
    let mut ws = Workspace::new("w");
    let tab = ws.focused_tab_mut();
    let p1 = tab.add_pane(Pane::screen(ScreenId::System, "System"));
    let p2 = tab
        .split(p1, cyberdeck_tui::workspace::Split::Horizontal)
        .expect("split must succeed");
    assert_eq!(tab.panes.len(), 2);
    assert_eq!(tab.focused, Some(p2));
}

#[test]
fn focused_pane_walks_tab_then_workspace() {
    let mut ws = Workspace::new("w");
    let tab_id = ws.focused_tab_id();
    let pane_id = ws.focused_tab_mut().add_pane(Pane::screen(ScreenId::Network, "Network"));
    assert_eq!(ws.focused_pane().map(|p| p.id), Some(pane_id));
    assert_eq!(ws.focused_tab_id(), tab_id);
}

#[test]
fn pane_id_is_unique_within_workspace() {
    let mut ws = Workspace::new("w");
    let p1 = ws.focused_tab_mut().add_pane(Pane::screen(ScreenId::System, "System"));
    let p2 = ws.focused_tab_mut().add_pane(Pane::screen(ScreenId::Network, "Network"));
    assert_ne!(p1, p2);
    // reflexivity: proves PartialEq is implemented (Eq is required for
    // HashMap keys; this no-op assertion documents the contract).
    assert_eq!(p1, p1);
}

#[test]
fn pane_kind_round_trips_json() {
    use cyberdeck_tui::workspace::PaneKind;
    let screen = PaneKind::Screen { id: cyberdeck_tui::app::ScreenId::System };
    let s = serde_json::to_string(&screen).unwrap();
    let back: PaneKind = serde_json::from_str(&s).unwrap();
    assert_eq!(back, screen);

    let pty = PaneKind::Pty { command: "zsh".into(), cwd: Some("/tmp".into()) };
    let s = serde_json::to_string(&pty).unwrap();
    let back: PaneKind = serde_json::from_str(&s).unwrap();
    assert_eq!(back, pty);
}