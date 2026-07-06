//! Daemon-side workspace state. Lives independently of the TUI; the
//! TUI subscribes via the event bus and re-renders on every change.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::rpc::RpcError;

/// Mirrors `cyberdeck_tui::workspace` but lives in the daemon so the CLI
/// can mutate it without the TUI being attached. The fields are kept
/// identical — see `from_tui_workspace` / `to_tui_workspace` for the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u64);

static NEXT_WS: AtomicU64 = AtomicU64::new(1);
static NEXT_TAB: AtomicU64 = AtomicU64::new(1);
static NEXT_PANE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Split {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneState {
    Blocked,
    Working,
    Done,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneKind {
    Screen { id: String },
    Pty { command: String, cwd: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub title: String,
    pub state: PaneState,
    pub last_state_change_seq: u64,
    pub seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub label: String,
    pub panes: Vec<Pane>,
    pub focused: Option<PaneId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub focused_tab: usize,
}

impl Workspace {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: WorkspaceId(NEXT_WS.fetch_add(1, Ordering::Relaxed)),
            name: name.into(),
            tabs: vec![Tab {
                id: TabId(NEXT_TAB.fetch_add(1, Ordering::Relaxed)),
                label: "main".into(),
                panes: vec![],
                focused: None,
            }],
            focused_tab: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub workspaces: Vec<Workspace>,
    pub focused_workspace: Option<WorkspaceId>,
    /// Per-PTY last-N-bytes tail — used by the agent-detect matcher.
    pub pty_tail: HashMap<PaneId, String>,
}

impl DaemonState {
    pub fn new() -> Self {
        let main = Workspace::new("cyberdeck");
        let focused = main.id;
        Self {
            workspaces: vec![main],
            focused_workspace: Some(focused),
            pty_tail: HashMap::new(),
        }
    }

    pub fn focused_workspace(&self) -> Option<&Workspace> {
        let id = self.focused_workspace?;
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn focused_workspace_mut(&mut self) -> Option<&mut Workspace> {
        let id = self.focused_workspace?;
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        for ws in &mut self.workspaces {
            for tab in &mut ws.tabs {
                if let Some(p) = tab.panes.iter_mut().find(|p| p.id == id) {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Read-only counterpart of [`Self::pane_mut`]. Used by handlers that
    /// only need to inspect (e.g. `PaneState`) without taking a write lock.
    pub fn pane_mut_for_read(&self, id: PaneId) -> Option<&Pane> {
        for ws in &self.workspaces {
            for tab in &ws.tabs {
                if let Some(p) = tab.panes.iter().find(|p| p.id == id) {
                    return Some(p);
                }
            }
        }
        None
    }

    pub fn focus_pane(&mut self, pane: PaneId) -> Result<(), RpcError> {
        for ws in &mut self.workspaces {
            for (ti, tab) in ws.tabs.iter_mut().enumerate() {
                if tab.panes.iter().any(|p| p.id == pane) {
                    tab.focused = Some(pane);
                    ws.focused_tab = ti;
                    self.focused_workspace = Some(ws.id);
                    return Ok(());
                }
            }
        }
        Err(RpcError::new("not_found", format!("pane {pane:?} not found")))
    }

    pub fn split_pane(&mut self, anchor: PaneId, dir: Split) -> Result<PaneId, RpcError> {
        let ws = self.focused_workspace_mut().ok_or_else(|| RpcError::new("no_workspace", "no focused workspace"))?;
        let tab = &mut ws.tabs[ws.focused_tab];
        if !tab.panes.iter().any(|p| p.id == anchor) {
            return Err(RpcError::new("not_found", "anchor pane not in focused tab"));
        }
        let label = match dir {
            Split::Horizontal => "sh (right)",
            Split::Vertical => "sh (below)",
        };
        let new_id = PaneId(NEXT_PANE.fetch_add(1, Ordering::Relaxed));
        tab.panes.push(Pane {
            id: new_id,
            kind: PaneKind::Pty { command: "sh".into(), cwd: None },
            title: label.into(),
            state: PaneState::Unknown,
            last_state_change_seq: 0,
            seen: false,
        });
        tab.focused = Some(new_id);
        Ok(new_id)
    }
}

/// Shared handle to the daemon's [`DaemonState`]. Cheap to clone (it's
/// just two Arcs) so every RPC handler holds its own reference without
/// worrying about contention. Handlers grab read/write locks per call.
pub type SharedState = std::sync::Arc<tokio::sync::RwLock<DaemonState>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_one_workspace() {
        let s = DaemonState::new();
        assert_eq!(s.workspaces.len(), 1);
        assert_eq!(s.workspaces[0].name, "cyberdeck");
        assert!(s.focused_workspace().is_some());
    }

    #[test]
    fn split_creates_new_pane_focused() {
        let mut s = DaemonState::new();
        let anchor = Pane {
            id: PaneId(99),
            kind: PaneKind::Screen { id: "System".into() },
            title: "system".into(),
            state: PaneState::Unknown,
            last_state_change_seq: 0,
            seen: false,
        };
        let anchor_id = anchor.id;
        s.focused_workspace_mut().unwrap().tabs[0].panes.push(anchor);
        let new_id = s.split_pane(anchor_id, Split::Horizontal).unwrap();
        assert_ne!(new_id, anchor_id);
        let ws = s.focused_workspace().unwrap();
        assert_eq!(ws.tabs[0].focused, Some(new_id));
    }

    #[test]
    fn split_unknown_pane_errors() {
        let mut s = DaemonState::new();
        let err = s.split_pane(PaneId(404), Split::Vertical).unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[test]
    fn focus_unknown_pane_errors() {
        let mut s = DaemonState::new();
        let err = s.focus_pane(PaneId(404)).unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    /// Confirms `PaneKind` emits `kind: "screen"` (lowercase) — matching the
    /// TUI workspace model. If the serde tag style ever drifts back to PascalCase
    /// this test will catch it and prevent silent wire-format breakage.
    #[test]
    fn pane_kind_serializes_lowercase_to_match_tui() {
        let s = PaneKind::Screen { id: "System".into() };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["kind"], "screen", "DaemonPaneKind::Screen must serialize as lowercase \"screen\"");
        assert_eq!(v["id"], "System");

        let p = PaneKind::Pty { command: "sh".into(), cwd: None };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "pty", "DaemonPaneKind::Pty must serialize as lowercase \"pty\"");
        assert_eq!(v["command"], "sh");
        assert!(v["cwd"].is_null());

        // Round-trip back to itself — proves the wire shape is invertible.
        let back: PaneKind = serde_json::from_value(v).unwrap();
        match back {
            PaneKind::Pty { command, cwd } => {
                assert_eq!(command, "sh");
                assert_eq!(cwd, None);
            }
            _ => panic!("expected Pty after round-trip"),
        }
    }

    /// The full Workspace shape that the daemon hands back to the TUI over
    /// the RPC socket must deserialise as `cyberdeck_tui::workspace::Workspace`.
    /// This is the test that would have caught the daemon/tui serde-tag
    /// mismatch had it ever shipped.
    #[test]
    fn daemon_workspace_round_trips_into_tui_workspace() {
        let mut s = DaemonState::new();
        let anchor = Pane {
            id: PaneId(7),
            kind: PaneKind::Screen { id: "System".into() },
            title: "system".into(),
            state: PaneState::Idle,
            last_state_change_seq: 1,
            seen: false,
        };
        s.focused_workspace_mut().unwrap().tabs[0].panes.push(anchor);
        let ws = s.focused_workspace().unwrap().clone();
        let json = serde_json::to_string(&ws).unwrap();
        let tui_ws: cyberdeck_tui::workspace::Workspace =
            serde_json::from_str(&json).expect("daemon Workspace must deserialize into TUI Workspace");
        assert_eq!(tui_ws.tabs[0].panes.len(), 1);
        match &tui_ws.tabs[0].panes[0].kind {
            cyberdeck_tui::workspace::PaneKind::Screen { id } => {
                assert_eq!(*id, cyberdeck_tui::app::ScreenId::System);
            }
            other => panic!("expected Screen, got {other:?}"),
        }
    }
}
