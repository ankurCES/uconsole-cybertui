//! Fleet data model: Workspace → Tab → Pane tree.
//!
//! Both the daemon and the TUI render from this struct; the CLI mutates a
//! remote copy over RPC. See docs/superpowers/plans/2026-07-05-herd-style-ui-and-cli.md.
//!
//! ## Serialization note
//!
//! `ScreenId` is defined in `crate::app::screen` and predates this module —
//! it deliberately does not derive `Serialize`/`Deserialize` (the task 1
//! constraint forbids modifying other files). To keep one source of truth
//! for screen identity across the daemon/TUI wire we serialize
//! `PaneKind::Screen` using `ScreenId::label()` (e.g. "System", "Network")
//! as the wire form and resolve it back via `ScreenId::ALL` on the
//! receiving side.

use serde::de::{self, Deserializer};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u64);

impl PaneId {
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Split {
    Horizontal, // side-by-side
    Vertical,   // top/bottom
}

/// What kind of pane this is. `Screen` panes are the existing 13 screens
/// (System, Network, ...). `Pty` panes run a real shell or command.
///
/// `PaneKind` deliberately implements serde by hand (not via
/// `#[derive(Serialize, Deserialize)]`) because `ScreenId` does not derive
/// either trait — see the module-level note above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneKind {
    Screen { id: crate::app::screen::ScreenId },
    Pty { command: String, cwd: Option<String> },
}

impl Serialize for PaneKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            PaneKind::Screen { id } => {
                let mut s = serializer.serialize_struct("PaneKind", 2)?;
                s.serialize_field("kind", "Screen")?;
                s.serialize_field("id", id.label())?;
                s.end()
            }
            PaneKind::Pty { command, cwd } => {
                let mut s = serializer.serialize_struct("PaneKind", 3)?;
                s.serialize_field("kind", "Pty")?;
                s.serialize_field("command", command)?;
                s.serialize_field("cwd", cwd)?;
                s.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for PaneKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase")]
        enum Raw {
            Screen { id: String },
            Pty {
                command: String,
                cwd: Option<String>,
            },
        }
        let raw = Raw::deserialize(deserializer)?;
        match raw {
            Raw::Screen { id } => {
                let resolved = crate::app::screen::ScreenId::ALL
                    .iter()
                    .copied()
                    .find(|s| s.label() == id)
                    .ok_or_else(|| {
                        de::Error::custom(format!("unknown ScreenId label: {id}"))
                    })?;
                Ok(PaneKind::Screen { id: resolved })
            }
            Raw::Pty { command, cwd } => Ok(PaneKind::Pty { command, cwd }),
        }
    }
}

/// State pill rendered on the sidebar and the pane title bar.
/// Mirrors herdr's four-state detection model (Blocked / Working / Done / Idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneState {
    Blocked,
    Working,
    Done, // seen, finished
    Idle, // running but quiet, e.g. waiting at a prompt
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub title: String,
    pub state: PaneState,
    pub last_state_change_seq: u64,
    /// true when the user has looked at the pane since its last state change.
    /// herd uses this to decide whether to render the "done" dot in teal or
    /// the "idle" dot in green — we mirror that exactly.
    pub seen: bool,
}

impl Pane {
    pub fn screen(label: &str) -> Self {
        Self {
            id: PaneId::new(),
            kind: PaneKind::Screen {
                id: crate::app::screen::ScreenId::System, // overwritten by caller
            },
            title: label.to_string(),
            state: PaneState::Unknown,
            last_state_change_seq: 0,
            seen: false,
        }
    }

    pub fn pty(command: impl Into<String>) -> Self {
        Self {
            id: PaneId::new(),
            kind: PaneKind::Pty {
                command: command.into(),
                cwd: None,
            },
            title: "sh".to_string(),
            state: PaneState::Unknown,
            last_state_change_seq: 0,
            seen: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub label: String,
    pub panes: Vec<Pane>,
    /// Index into `panes` of the focused pane within this tab.
    pub focused: Option<PaneId>,
}

impl Tab {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: TabId(0),
            label: label.into(),
            panes: vec![],
            focused: None,
        }
    }

    pub fn add_pane(&mut self, pane: Pane) -> PaneId {
        let id = pane.id;
        self.panes.push(pane);
        self.focused = Some(id);
        id
    }

    pub fn split(&mut self, anchor: PaneId, dir: Split) -> Option<PaneId> {
        if !self.panes.iter().any(|p| p.id == anchor) {
            return None;
        }
        let mut new_pane = Pane::pty("sh");
        new_pane.title = match dir {
            Split::Horizontal => "sh (right)",
            Split::Vertical => "sh (below)",
        }
        .to_string();
        let id = new_pane.id;
        self.panes.push(new_pane);
        self.focused = Some(id);
        Some(id)
    }
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
            id: WorkspaceId(0),
            name: name.into(),
            tabs: vec![Tab::new("main")],
            focused_tab: 0,
        }
    }

    pub fn focused_tab(&self) -> &Tab {
        &self.tabs[self.focused_tab]
    }

    pub fn focused_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.focused_tab]
    }

    pub fn focused_tab_id(&self) -> TabId {
        self.tabs[self.focused_tab].id
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        let tab = self.focused_tab();
        let id = tab.focused?;
        tab.panes.iter().find(|p| p.id == id)
    }
}