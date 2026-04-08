use crossterm::event::KeyEvent;

use crate::component::{Action, NavigationTarget, View};
use crate::store::{DataRequest, DataStore};
use crate::theme::Theme;

/// Shared navigation context derived from navigation actions
pub struct NavigationContext {
    pub current_branch: Option<String>,
    pub current_snapshot: Option<String>,
    pub current_path: Option<String>,
}

/// Application coordinator.
/// Owns the data store, theme, navigation state.
/// Routes events and manages view lifecycle.
pub struct App {
    pub store: DataStore,
    pub theme: Theme,
    pub should_quit: bool,

    // Navigation
    pub current_view: View,
    pub nav_stack: Vec<View>,
    pub nav_context: NavigationContext,

    // Per-view selection state (temporary until components exist)
    pub selected_index: usize,
}

impl App {
    pub fn new(store: DataStore) -> Self {
        Self {
            store,
            theme: Theme::default(),
            should_quit: false,
            current_view: View::Overview,
            nav_stack: Vec::new(),
            nav_context: NavigationContext {
                current_branch: None,
                current_snapshot: None,
                current_path: None,
            },
            selected_index: 0,
        }
    }

    /// Load initial data — kicks off branch and tag fetches
    pub fn load_initial_data(&mut self) {
        self.store.submit(DataRequest::Branches);
        self.store.submit(DataRequest::Tags);
    }

    /// Drain all pending responses from background worker
    pub fn drain_responses(&mut self) {
        self.store.drain_responses();
    }

    /// Handle a key event — global keys first, then delegate
    pub fn handle_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        let action = match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('?') => {
                if self.current_view == View::Help {
                    Action::Back
                } else {
                    Action::Navigate(NavigationTarget::Help)
                }
            }
            KeyCode::Esc | KeyCode::Backspace => Action::Back,
            // Number keys to jump to views
            KeyCode::Char('1') => Action::Navigate(NavigationTarget::Overview),
            KeyCode::Char('2') => Action::Navigate(NavigationTarget::Branches),
            KeyCode::Char('3') => Action::Navigate(NavigationTarget::Tags),
            KeyCode::Char('4') => {
                let branch = self
                    .nav_context
                    .current_branch
                    .clone()
                    .unwrap_or_else(|| "main".to_string());
                Action::Navigate(NavigationTarget::Log { branch })
            }
            KeyCode::Char('5') => {
                let branch = self
                    .nav_context
                    .current_branch
                    .clone()
                    .unwrap_or_else(|| "main".to_string());
                Action::Navigate(NavigationTarget::NodeTree {
                    branch,
                    path: "/".to_string(),
                })
            }
            KeyCode::Char('6') => Action::Navigate(NavigationTarget::OpsLog),
            // List navigation
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected_index = self.selected_index.saturating_add(1);
                // Clamp will happen in render
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected_index = self.selected_index.saturating_sub(1);
                Action::None
            }
            _ => Action::None,
        };

        self.process_action(action);
    }

    fn process_action(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Navigate(target) => {
                let view = self.target_to_view(&target);
                // Update navigation context from the target
                self.update_nav_context(&target);
                // Push current view and switch
                if view != self.current_view {
                    self.nav_stack.push(self.current_view);
                    self.current_view = view;
                    self.selected_index = 0;
                    // Trigger data loading for the new view
                    self.request_data_for_view(view);
                }
            }
            Action::RequestData(request) => {
                self.store.submit(request);
            }
            Action::Back => {
                if let Some(prev) = self.nav_stack.pop() {
                    self.current_view = prev;
                    self.selected_index = 0;
                }
            }
            Action::Quit => {
                self.should_quit = true;
            }
        }
    }

    fn target_to_view(&self, target: &NavigationTarget) -> View {
        match target {
            NavigationTarget::Overview => View::Overview,
            NavigationTarget::Branches => View::Branches,
            NavigationTarget::Tags => View::Tags,
            NavigationTarget::Log { .. } => View::Log,
            NavigationTarget::NodeTree { .. } => View::NodeTree,
            NavigationTarget::OpsLog => View::OpsLog,
            NavigationTarget::Help => View::Help,
        }
    }

    fn update_nav_context(&mut self, target: &NavigationTarget) {
        match target {
            NavigationTarget::Log { branch } => {
                self.nav_context.current_branch = Some(branch.clone());
            }
            NavigationTarget::NodeTree { branch, path } => {
                self.nav_context.current_branch = Some(branch.clone());
                self.nav_context.current_path = Some(path.clone());
            }
            _ => {}
        }
    }

    fn request_data_for_view(&mut self, view: View) {
        match view {
            View::Branches => {
                if !self.store.branches.is_loaded() {
                    self.store.submit(DataRequest::Branches);
                }
            }
            View::Tags => {
                if !self.store.tags.is_loaded() {
                    self.store.submit(DataRequest::Tags);
                }
            }
            View::Log => {
                if let Some(branch) = &self.nav_context.current_branch
                    && !self.store.ancestry.contains_key(branch) {
                        self.store.submit(DataRequest::Ancestry {
                            branch: branch.clone(),
                        });
                    }
            }
            View::NodeTree => {
                if let (Some(branch), Some(path)) = (
                    &self.nav_context.current_branch,
                    &self.nav_context.current_path,
                )
                    && !self.store.node_children.contains_key(path) {
                        self.store.submit(DataRequest::NodeChildren {
                            branch: branch.clone(),
                            parent_path: path.clone(),
                        });
                    }
            }
            View::OpsLog => {
                if !self.store.ops_log.is_loaded() {
                    self.store.submit(DataRequest::OpsLog);
                }
            }
            _ => {}
        }
    }
}
