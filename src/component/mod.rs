/// Which pane is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Detail,
    Bottom,
}

/// What the bottom panel is showing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Snapshots,
    Branches,
    Tags,
}

/// What the detail pane is showing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailMode {
    /// Show node detail (array/group metadata) based on tree selection
    Node,
    /// Show repository overview (config, feature flags, status)
    Repo,
    /// Show repository operations log (mutation history)
    OpsLog,
    /// Show branch detail (commits, storage stats) for selected branch
    Branch,
    /// Show snapshot detail (diff) for selected snapshot
    Snapshot,
}

impl DetailMode {
    /// Returns the next tab to the right, or None if at the rightmost.
    pub fn next(&self) -> Option<Self> {
        match self {
            Self::Node => Some(Self::Repo),
            Self::Repo => Some(Self::Branch),
            Self::Branch => Some(Self::Snapshot),
            Self::Snapshot => Some(Self::OpsLog),
            Self::OpsLog => None,
        }
    }

    /// Returns the next tab to the left, or None if at the leftmost.
    pub fn prev(&self) -> Option<Self> {
        match self {
            Self::Node => None,
            Self::Repo => Some(Self::Node),
            Self::Branch => Some(Self::Repo),
            Self::Snapshot => Some(Self::Branch),
            Self::OpsLog => Some(Self::Snapshot),
        }
    }
}

/// Actions that components can return from key handling
pub enum Action {
    /// Key was not consumed
    None,
    /// Focus a specific pane
    FocusPane(Pane),
    /// Toggle the bottom panel visibility
    ToggleBottom,
    /// Quit the application
    Quit,
}
