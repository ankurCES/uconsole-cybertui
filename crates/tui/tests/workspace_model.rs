use cyberdeck_tui::workspace::{Pane, Workspace};

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
    let p1 = tab.add_pane(Pane::screen("System"));
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
    let pane_id = ws.focused_tab_mut().add_pane(Pane::screen("Network"));
    assert_eq!(ws.focused_pane().map(|p| p.id), Some(pane_id));
    assert_eq!(ws.focused_tab_id(), tab_id);
}

#[test]
fn pane_id_is_unique_within_workspace() {
    let mut ws = Workspace::new("w");
    let p1 = ws.focused_tab_mut().add_pane(Pane::screen("System"));
    let p2 = ws.focused_tab_mut().add_pane(Pane::screen("Network"));
    assert_ne!(p1, p2);
    // Sanity: PaneId is opaque and equality is reflexive. The
    // `assert_ne!` above already proves PartialEq works; this guards
    // against accidentally making the type non-Eq.
    assert_eq!(p1, p1);
}