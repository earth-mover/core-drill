use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;

use crate::component::{Action, BottomTab, Pane};
use crate::store::{DataRequest, DataStore, LoadState, TreeNodeType};
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
    pub tree_state: tui_tree_widget::TreeState<String>,
    pub detail_scroll: usize,
    pub bottom_selected: usize,
    /// Whether we've already auto-expanded the tree after initial load
    tree_auto_expanded: bool,
    /// The snapshot ID we last requested a diff for (to avoid re-requesting)
    pub last_diff_requested: Option<String>,

    // Layout areas (updated each render for mouse hit-testing)
    pub sidebar_area: Rect,
    pub detail_area: Rect,
    pub bottom_area: Option<Rect>,
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
            tree_state: tui_tree_widget::TreeState::<String>::default(),
            detail_scroll: 0,
            bottom_selected: 0,
            tree_auto_expanded: false,
            last_diff_requested: None,
            sidebar_area: Rect::default(),
            detail_area: Rect::default(),
            bottom_area: None,
        }
    }

    /// Kick off initial data loads
    pub fn load_initial_data(&mut self) {
        self.store.submit(DataRequest::Branches);
        self.store.submit(DataRequest::Tags);
        self.store.submit(DataRequest::AllNodes {
            branch: self.current_branch.clone(),
        });
        self.store.submit(DataRequest::Ancestry {
            branch: self.current_branch.clone(),
        });
    }

    /// Drain all pending responses from background worker
    pub fn drain_responses(&mut self) {
        self.store.drain_responses();

        // After AllNodes data arrives, auto-expand groups so the user sees
        // meaningful content immediately instead of a collapsed root.
        if !self.tree_auto_expanded {
            if let Some(LoadState::Loaded(_)) = self.store.node_children.get("/") {
                self.auto_expand_tree();
                self.tree_auto_expanded = true;
            }
        }

        // Auto-request diff when bottom pane is focused on Snapshots tab
        self.maybe_request_snapshot_diff();
    }

    /// If the bottom pane is focused on the Snapshots tab and we have a selected
    /// snapshot that we haven't yet requested a diff for, submit the request.
    fn maybe_request_snapshot_diff(&mut self) {
        if self.focused_pane != Pane::Bottom || self.bottom_tab != BottomTab::Snapshots {
            return;
        }

        let snapshot_id = self.selected_snapshot_id();
        let Some(sid) = snapshot_id else { return };

        // Don't re-request if we already have it or are loading it
        if self.last_diff_requested.as_deref() == Some(&sid) {
            return;
        }

        // Don't re-request if already cached
        if self.store.diffs.contains_key(&sid) {
            self.last_diff_requested = Some(sid);
            return;
        }

        self.last_diff_requested = Some(sid.clone());
        self.store.submit(DataRequest::SnapshotDiff {
            branch: self.current_branch.clone(),
            snapshot_id: sid,
        });
    }

    /// Get the snapshot ID for the currently selected row in the bottom panel.
    pub fn selected_snapshot_id(&self) -> Option<String> {
        let ancestry = self
            .store
            .ancestry
            .get(&self.current_branch)?
            .as_loaded()?;
        ancestry.get(self.bottom_selected).map(|e| e.id.clone())
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
            KeyCode::Char('1') => return Action::FocusPane(Pane::Sidebar),
            KeyCode::Char('3') => {
                if !self.bottom_visible {
                    self.bottom_visible = true;
                }
                return Action::FocusPane(Pane::Bottom);
            }
            KeyCode::PageDown => {
                self.detail_scroll = self.detail_scroll.saturating_add(3);
                return Action::None;
            }
            KeyCode::PageUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(3);
                return Action::None;
            }
            _ => {}
        }

        // Ctrl+hjkl: move between panes, or pass through to zellij at edges
        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    if self.focused_pane != Pane::Sidebar {
                        return Action::FocusPane(Pane::Sidebar);
                    }
                    crate::multiplexer::move_focus("left");
                    return Action::None;
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    crate::multiplexer::move_focus("right");
                    return Action::None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.bottom_visible && self.focused_pane != Pane::Bottom {
                        return Action::FocusPane(Pane::Bottom);
                    }
                    crate::multiplexer::move_focus("down");
                    return Action::None;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if self.focused_pane == Pane::Bottom {
                        return Action::FocusPane(Pane::Sidebar);
                    }
                    crate::multiplexer::move_focus("up");
                    return Action::None;
                }
                _ => {}
            }
        }

        // Tab/Shift+Tab: cycle bottom tabs when bottom focused, else cycle panes
        match key.code {
            KeyCode::Tab => {
                if self.focused_pane == Pane::Bottom {
                    // Cycle bottom tabs forward
                    self.bottom_tab = match self.bottom_tab {
                        BottomTab::Snapshots => BottomTab::Branches,
                        BottomTab::Branches => BottomTab::Tags,
                        BottomTab::Tags => BottomTab::Snapshots,
                    };
                } else {
                    // Cycle panes forward: Sidebar -> Bottom -> Sidebar
                    let next = match self.focused_pane {
                        Pane::Sidebar => {
                            if self.bottom_visible { Pane::Bottom } else { Pane::Sidebar }
                        }
                        Pane::Detail => Pane::Sidebar,
                        Pane::Bottom => Pane::Sidebar,
                    };
                    return Action::FocusPane(next);
                }
                return Action::None;
            }
            KeyCode::BackTab => {
                if self.focused_pane == Pane::Bottom {
                    // Cycle bottom tabs backward
                    self.bottom_tab = match self.bottom_tab {
                        BottomTab::Snapshots => BottomTab::Tags,
                        BottomTab::Branches => BottomTab::Snapshots,
                        BottomTab::Tags => BottomTab::Branches,
                    };
                } else {
                    // Cycle panes backward: Sidebar -> Bottom -> Sidebar
                    let prev = match self.focused_pane {
                        Pane::Sidebar => {
                            if self.bottom_visible { Pane::Bottom } else { Pane::Sidebar }
                        }
                        Pane::Detail => Pane::Sidebar,
                        Pane::Bottom => Pane::Sidebar,
                    };
                    return Action::FocusPane(prev);
                }
                return Action::None;
            }
            _ => {}
        }

        // Directional edge navigation (non-Ctrl)
        match key.code {
            KeyCode::Char('h') | KeyCode::Left if self.focused_pane == Pane::Bottom => {
                // Cycle bottom tabs backward
                self.bottom_tab = match self.bottom_tab {
                    BottomTab::Snapshots => BottomTab::Tags,
                    BottomTab::Branches => BottomTab::Snapshots,
                    BottomTab::Tags => BottomTab::Branches,
                };
                return Action::None;
            }
            KeyCode::Char('l') | KeyCode::Right if self.focused_pane == Pane::Bottom => {
                // Cycle bottom tabs forward
                self.bottom_tab = match self.bottom_tab {
                    BottomTab::Snapshots => BottomTab::Branches,
                    BottomTab::Branches => BottomTab::Tags,
                    BottomTab::Tags => BottomTab::Snapshots,
                };
                return Action::None;
            }
            _ => {}
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
            Pane::Sidebar => {
                let moved = self.tree_state.key_down();
                self.detail_scroll = 0; // reset when changing selection
                if !moved && self.bottom_visible {
                    self.focused_pane = Pane::Bottom;
                }
            }
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_add(1),
            Pane::Bottom => {
                self.bottom_selected = self.bottom_selected.saturating_add(1);
                self.detail_scroll = 0; // reset when changing selection
                self.maybe_request_snapshot_diff();
            }
        }
    }

    fn select_prev(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                self.tree_state.key_up();
                self.detail_scroll = 0; // reset when changing selection
            }
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            Pane::Bottom => {
                if self.bottom_selected == 0 {
                    self.focused_pane = Pane::Sidebar;
                } else {
                    self.bottom_selected -= 1;
                    self.detail_scroll = 0; // reset when changing selection
                    self.maybe_request_snapshot_diff();
                }
            }
        }
    }

    /// Handle a mouse event (click to focus pane, select item)
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }

        let col = mouse.column;
        let row = mouse.row;

        // Determine which pane was clicked and focus it
        if self.sidebar_area.contains((col, row).into()) {
            self.focused_pane = Pane::Sidebar;

            // Calculate which tree row was clicked relative to the sidebar inner area.
            // The sidebar has a border (1 row) + branch selector (1 row), so content
            // starts 2 rows below the sidebar_area top.
            let content_top = self.sidebar_area.y + 2;
            if row >= content_top {
                let offset = (row - content_top) as usize;
                // Navigate to the clicked row by selecting first and moving down
                self.tree_state.select_first();
                for _ in 0..offset {
                    self.tree_state.key_down();
                }
            }
        } else if let Some(bottom) = self.bottom_area
            && bottom.contains((col, row).into())
        {
            if !self.bottom_visible {
                self.bottom_visible = true;
            }
            self.focused_pane = Pane::Bottom;

            // The bottom panel has a 2-row tab bar, then content rows
            let content_top = bottom.y + 2;
            // Skip header row in table (1 row for snapshots table header)
            let header_rows: u16 = match self.bottom_tab {
                BottomTab::Snapshots => 1,
                _ => 0,
            };
            if row >= content_top + header_rows {
                self.bottom_selected = (row - content_top - header_rows) as usize;
            }
        }
    }

    /// Auto-expand the tree when root children are all groups.
    /// Drills down through single-child groups so the user lands on
    /// the first meaningful level of the hierarchy.
    fn auto_expand_tree(&mut self) {
        let mut current_path = "/".to_string();
        let mut identifier_path: Vec<String> = Vec::new();

        loop {
            let children = match self.store.node_children.get(&current_path) {
                Some(LoadState::Loaded(nodes)) => nodes,
                _ => break,
            };

            if children.is_empty() {
                break;
            }

            let all_groups = children
                .iter()
                .all(|n| matches!(n.node_type, TreeNodeType::Group));

            if all_groups && children.len() == 1 {
                // Single group child — open it and keep drilling
                let child = &children[0];
                identifier_path.push(child.path.clone());
                self.tree_state.open(identifier_path.clone());
                current_path = child.path.clone();
            } else if all_groups {
                // Multiple group children — open them all, then select the first
                for child in children {
                    let mut id = identifier_path.clone();
                    id.push(child.path.clone());
                    self.tree_state.open(id);
                }
                // Select the first child
                let first = &children[0];
                let mut select_id = identifier_path.clone();
                select_id.push(first.path.clone());
                self.tree_state.select(select_id);
                return;
            } else {
                // Mixed or all arrays — select the first leaf (array) or first node
                if let Some(first_leaf) = children
                    .iter()
                    .find(|n| matches!(n.node_type, TreeNodeType::Array(_)))
                {
                    let mut select_id = identifier_path.clone();
                    select_id.push(first_leaf.path.clone());
                    self.tree_state.select(select_id);
                } else {
                    let mut select_id = identifier_path.clone();
                    select_id.push(children[0].path.clone());
                    self.tree_state.select(select_id);
                }
                return;
            }
        }

        // Fallback: just select the first visible node
        self.tree_state.select_first();
    }

    fn handle_enter(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                // Toggle open/close on the selected tree node
                self.tree_state.toggle_selected();

                // If a group just opened, trigger loading its children
                let selected = self.tree_state.selected().to_vec();
                if let Some(path) = selected.last()
                    && self.tree_state.opened().contains(&selected)
                {
                    // All nodes are loaded upfront via AllNodes, but if somehow
                    // children aren't cached yet, request them.
                    if !self.store.node_children.contains_key(path) {
                        self.store.submit(DataRequest::AllNodes {
                            branch: self.current_branch.clone(),
                        });
                    }
                }
            }
            Pane::Detail => {
                // If a group is selected in the sidebar, expand it and focus sidebar
                let selected = self.tree_state.selected().to_vec();
                if !selected.is_empty() {
                    self.tree_state.open(selected);
                    self.tree_state.key_right();
                    self.focused_pane = Pane::Sidebar;
                }
            }
            Pane::Bottom => {
                // TODO: handle enter in bottom pane
            }
        }
    }
}
