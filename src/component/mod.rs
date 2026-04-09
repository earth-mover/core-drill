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

/// Actions that components can return from key handling
#[allow(dead_code)]
pub enum Action {
    /// Key was not consumed
    None,
    /// Focus a specific pane
    FocusPane(Pane),
    /// Toggle the bottom panel visibility
    ToggleBottom,
    /// Switch the bottom panel tab
    SwitchBottomTab(BottomTab),
    /// Quit the application
    Quit,
}
