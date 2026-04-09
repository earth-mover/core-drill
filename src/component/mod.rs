use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::prelude::Rect;

use crate::store::{DataRequest, DataStore};
use crate::theme::Theme;

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
    /// Request data from the store
    RequestData(DataRequest),
    /// Quit the application
    Quit,
}

/// Trait implemented by all standalone UI components.
///
/// Components own their view state (selection index, scroll offset, etc.)
/// but never access icechunk directly. All data comes from the DataStore.
#[allow(dead_code)]
pub trait Component {
    /// Handle a key event. Return an Action for the app to process.
    fn handle_key(&mut self, key: KeyEvent) -> Action;

    /// Render into the given area using data from the store.
    fn render(
        &self,
        store: &DataStore,
        theme: &Theme,
        focused: bool,
        frame: &mut Frame,
        area: Rect,
    );

    /// Return data requests needed to populate this component.
    fn on_enter(&mut self, store: &DataStore) -> Vec<DataRequest>;

    /// Called when the store has new data.
    fn on_data_changed(&mut self, store: &DataStore);
}
