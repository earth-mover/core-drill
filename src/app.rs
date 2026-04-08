use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::component::{Action, BottomTab, Pane};
use crate::store::{DataRequest, DataStore};
use crate::theme::Theme;

/// Application coordinator.
/// Owns the data store, theme, pane focus, and layout state.
pub struct App {
    pub store: DataStore,
    pub theme: Theme,
    pub should_quit: bool,

    // Layout
    pub focused_pane: Pane,
    pub bottom_visible: bool,
    pub bottom_tab: BottomTab,
    pub show_help: bool,

    // Navigation context
    pub current_branch: String,
    pub repo_url: String,

    // Per-pane selection state
    pub sidebar_selected: usize,
    pub detail_scroll: usize,
    pub bottom_selected: usize,

    // Sidebar tree state
    pub expanded_paths: std::collections::HashSet<String>,
}

impl App {
    pub fn new(store: DataStore, repo_url: String) -> Self {
        Self {
            store,
            theme: Theme::default(),
            should_quit: false,
            focused_pane: Pane::Sidebar,
            bottom_visible: true,
            bottom_tab: BottomTab::Snapshots,
            show_help: false,
            current_branch: "main".to_string(),
            repo_url,
            sidebar_selected: 0,
            detail_scroll: 0,
            bottom_selected: 0,
            expanded_paths: std::collections::HashSet::new(),
        }
    }

    /// Kick off initial data loads
    pub fn load_initial_data(&mut self) {
        self.store.submit(DataRequest::Branches);
        self.store.submit(DataRequest::Tags);
        self.store.submit(DataRequest::NodeChildren {
            branch: self.current_branch.clone(),
            parent_path: "/".to_string(),
        });
        self.store.submit(DataRequest::Ancestry {
            branch: self.current_branch.clone(),
        });
    }

    /// Drain all pending responses from background worker
    pub fn drain_responses(&mut self) {
        self.store.drain_responses();
    }

    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) {
        let action = self.map_key(key);
        self.process_action(action);
    }

    fn map_key(&mut self, key: KeyEvent) -> Action {
        // Global keys
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                return Action::None;
            }
            KeyCode::Char('t') => return Action::ToggleBottom,
            _ => {}
        }

        // Ctrl+hjkl / Ctrl+arrows for pane navigation
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => return Action::FocusPane(Pane::Sidebar),
                KeyCode::Char('l') | KeyCode::Right => return Action::FocusPane(Pane::Detail),
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.bottom_visible {
                        return Action::FocusPane(Pane::Bottom);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if self.focused_pane == Pane::Bottom {
                        return Action::FocusPane(Pane::Sidebar);
                    }
                }
                _ => {}
            }
        }

        // Bottom tab switching (when bottom is focused)
        if self.focused_pane == Pane::Bottom {
            match key.code {
                KeyCode::Char('1') => return Action::SwitchBottomTab(BottomTab::Snapshots),
                KeyCode::Char('2') => return Action::SwitchBottomTab(BottomTab::Branches),
                KeyCode::Char('3') => return Action::SwitchBottomTab(BottomTab::Tags),
                _ => {}
            }
        }

        // Pane-local navigation
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_next();
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_prev();
                Action::None
            }
            KeyCode::Enter => {
                self.handle_enter();
                Action::None
            }
            _ => Action::None,
        }
    }

    fn process_action(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::FocusPane(pane) => {
                self.focused_pane = pane;
            }
            Action::ToggleBottom => {
                self.bottom_visible = !self.bottom_visible;
                if !self.bottom_visible && self.focused_pane == Pane::Bottom {
                    self.focused_pane = Pane::Sidebar;
                }
            }
            Action::SwitchBottomTab(tab) => {
                self.bottom_tab = tab;
            }
            Action::RequestData(request) => {
                self.store.submit(request);
            }
            Action::Quit => {
                self.should_quit = true;
            }
        }
    }

    fn select_next(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => self.sidebar_selected = self.sidebar_selected.saturating_add(1),
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_add(1),
            Pane::Bottom => self.bottom_selected = self.bottom_selected.saturating_add(1),
        }
    }

    fn select_prev(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => self.sidebar_selected = self.sidebar_selected.saturating_sub(1),
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            Pane::Bottom => self.bottom_selected = self.bottom_selected.saturating_sub(1),
        }
    }

    fn handle_enter(&mut self) {
        // TODO: expand/collapse tree nodes, select snapshots, etc.
    }
}
