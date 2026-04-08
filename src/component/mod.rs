use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::prelude::Rect;

use crate::store::{DataRequest, DataStore};
use crate::theme::Theme;

/// Navigation targets carrying transition-specific data
#[derive(Debug, Clone)]
pub enum NavigationTarget {
    Overview,
    Branches,
    Tags,
    Log { branch: String },
    NodeTree { branch: String, path: String },
    OpsLog,
    Help,
}

/// Actions that components can return from key handling
pub enum Action {
    /// Key was not consumed by this component
    None,
    /// Navigate to a new view
    Navigate(NavigationTarget),
    /// Request data from the store
    RequestData(DataRequest),
    /// Go back in navigation stack
    Back,
    /// Quit the application
    Quit,
}

/// Trait implemented by all standalone UI components.
///
/// Components own their view state (selection index, scroll offset, etc.)
/// but never access icechunk directly. All data comes from the DataStore.
pub trait Component {
    /// Handle a key event. Return an Action for the app to process.
    fn handle_key(&mut self, key: KeyEvent) -> Action;

    /// Render into the given area using data from the store.
    fn render(&self, store: &DataStore, theme: &Theme, frame: &mut Frame, area: Rect);

    /// Called when the component becomes the active view.
    /// Return data requests needed to populate this component.
    fn on_enter(&mut self, store: &DataStore) -> Vec<DataRequest>;

    /// Called when the store has new data.
    /// Update internal state (e.g., clamp selection to new list length).
    fn on_data_changed(&mut self, store: &DataStore);
}

/// Which view is currently active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Branches,
    Tags,
    Log,
    NodeTree,
    OpsLog,
    Help,
}
