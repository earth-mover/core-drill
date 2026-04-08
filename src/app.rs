use std::collections::BTreeSet;

use color_eyre::Result;
use icechunk::Repository;

/// Which view is currently active in the TUI
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

/// Application state
pub struct App {
    pub repo: Repository,
    pub current_view: View,
    pub nav_stack: Vec<View>,
    pub should_quit: bool,

    // Data loaded from repo
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub snapshot_count: usize,
    pub repo_status: String,

    // Selection state
    pub selected_index: usize,

    // Loading state
    pub loading: bool,
    pub status_message: String,
}

impl App {
    pub fn new(repo: Repository) -> Self {
        Self {
            repo,
            current_view: View::Overview,
            nav_stack: Vec::new(),
            should_quit: false,
            branches: Vec::new(),
            tags: Vec::new(),
            snapshot_count: 0,
            repo_status: String::from("Unknown"),
            selected_index: 0,
            loading: false,
            status_message: String::from("Loading..."),
        }
    }

    /// Load initial repository info (cheap — single fetch)
    pub async fn load_repo_info(&mut self) -> Result<()> {
        self.loading = true;

        let branches: BTreeSet<String> = self.repo.list_branches().await?;
        let tags: BTreeSet<String> = self.repo.list_tags().await?;

        self.branches = branches.into_iter().collect();
        self.tags = tags.into_iter().collect();
        self.status_message = String::from("Ready");
        self.loading = false;

        Ok(())
    }

    /// Navigate to a new view, pushing current onto stack
    pub fn push_view(&mut self, view: View) {
        self.nav_stack.push(self.current_view);
        self.current_view = view;
        self.selected_index = 0;
    }

    /// Go back to previous view
    pub fn pop_view(&mut self) {
        if let Some(prev) = self.nav_stack.pop() {
            self.current_view = prev;
            self.selected_index = 0;
        }
    }

    /// Move selection up in current list
    pub fn select_prev(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    /// Move selection down in current list
    pub fn select_next(&mut self, max: usize) {
        if max > 0 && self.selected_index < max - 1 {
            self.selected_index += 1;
        }
    }

    /// Handle a key event
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => {
                if self.current_view == View::Help {
                    self.pop_view();
                } else {
                    self.push_view(View::Help);
                }
            }
            KeyCode::Esc | KeyCode::Backspace => self.pop_view(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.current_list_len();
                self.select_next(max);
            }
            // Number keys to jump to views
            KeyCode::Char('1') => self.switch_view(View::Overview),
            KeyCode::Char('2') => self.switch_view(View::Branches),
            KeyCode::Char('3') => self.switch_view(View::Tags),
            KeyCode::Char('4') => self.switch_view(View::Log),
            KeyCode::Char('5') => self.switch_view(View::NodeTree),
            KeyCode::Char('6') => self.switch_view(View::OpsLog),
            _ => {}
        }
    }

    /// Switch to a view directly (not push — replaces current)
    fn switch_view(&mut self, view: View) {
        self.current_view = view;
        self.selected_index = 0;
    }

    /// Get the length of the currently displayed list
    fn current_list_len(&self) -> usize {
        match self.current_view {
            View::Branches => self.branches.len(),
            View::Tags => self.tags.len(),
            _ => 0,
        }
    }
}
