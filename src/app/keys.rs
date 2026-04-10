use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use crate::component::{Action, BottomTab, DetailMode, Pane};
use crate::search::SearchState;

use super::App;

impl App {
    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Help overlay: ? or Esc closes it, all other keys ignored
        if self.show_help {
            if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
                self.show_help = false;
            }
            return;
        }

        // Search mode intercepts all keys
        if self.search.is_some() {
            self.handle_search_key(key);
            return;
        }
        let action = self.map_key(key);
        self.process_action(action);
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let candidates: Vec<String> = self.search_candidates().to_vec();
        let candidate_refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();

        match key.code {
            KeyCode::Esc => {
                self.search = None;
            }
            KeyCode::Enter => {
                if let Some(ref search) = self.search
                    && let Some(idx) = search.selected_index()
                {
                    // Apply the selection through the normal state setters
                    match self.focused_pane {
                        Pane::Sidebar => {
                            if idx < candidates.len() {
                                self.select_tree_node(&candidates[idx].clone());
                                self.on_tree_selection_changed();
                            }
                        }
                        Pane::Bottom => {
                            self.set_bottom_selected(idx);
                            self.on_bottom_selection_changed();
                        }
                        Pane::Detail => {}
                    }
                }
                self.search = None;
            }
            KeyCode::Backspace => {
                if let Some(ref mut search) = self.search {
                    if search.query.is_empty() {
                        self.search = None;
                    } else {
                        search.pop_char(&candidate_refs);
                        self.sync_search_selection_reactive(&candidates);
                    }
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if let Some(ref mut search) = self.search {
                    search.next();
                    self.sync_search_selection_reactive(&candidates);
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(ref mut search) = self.search {
                    search.prev();
                    self.sync_search_selection_reactive(&candidates);
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut search) = self.search {
                    search.push_char(c, &candidate_refs);
                    self.sync_search_selection_reactive(&candidates);
                }
            }
            _ => {}
        }
    }

    fn map_key(&mut self, key: KeyEvent) -> Action {
        // Handle pending `z` prefix for vim fold commands
        if self.pending_z {
            self.pending_z = false;
            if self.focused_pane == Pane::Sidebar {
                match key.code {
                    KeyCode::Char('o') => {
                        // zo — open selected node
                        let selected = self.tree_state.selected().to_vec();
                        if !selected.is_empty() {
                            self.tree_state.open(selected);
                        }
                    }
                    KeyCode::Char('c') => {
                        // zc — close selected node, or if it's a leaf / already closed,
                        // close and focus the parent group (vim fold behavior)
                        let selected = self.tree_state.selected().to_vec();
                        if !selected.is_empty() {
                            let closed = self.tree_state.close(&selected);
                            if !closed && selected.len() > 1 {
                                // Node was already closed or is a leaf — go to parent
                                let parent = selected[..selected.len() - 1].to_vec();
                                self.tree_state.close(&parent);
                                self.tree_state.select(parent);
                                self.on_tree_selection_changed();
                            }
                        }
                    }
                    KeyCode::Char('O') => {
                        // zO — open selected node and all descendants recursively
                        self.open_tree_deep();
                    }
                    KeyCode::Char('C') => {
                        // zC — close selected node and all descendants
                        self.close_tree_deep();
                    }
                    KeyCode::Char('R') => {
                        // zR — open entire tree
                        self.open_all_tree_nodes();
                    }
                    KeyCode::Char('M') => {
                        // zM — close entire tree
                        self.tree_state.close_all();
                    }
                    _ => {} // Unknown z-command, ignore
                }
            }
            return Action::None;
        }

        // Global keys
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('/') if self.focused_pane != Pane::Detail => {
                self.search = Some(SearchState::new(self.focused_pane, self.bottom_tab));
                return Action::None;
            }
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                return Action::None;
            }
            KeyCode::Char('t') => return Action::ToggleBottom,
            KeyCode::Char('1') => return Action::FocusPane(Pane::Sidebar),
            KeyCode::Char('2') => return Action::FocusPane(Pane::Detail),
            KeyCode::Char('3') => {
                if !self.bottom_visible {
                    self.bottom_visible = true;
                }
                return Action::FocusPane(Pane::Bottom);
            }
            KeyCode::Char('d') => {
                self.detail_scroll = self.detail_scroll.saturating_add(3);
                return Action::None;
            }
            KeyCode::Char('u') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(3);
                return Action::None;
            }
            KeyCode::Char('R') => {
                self.retry_failed();
                return Action::None;
            }
            _ => {}
        }

        // Ctrl+hjkl: move between panes, or pass through to zellij at edges
        if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
        {
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
                    let next = match self.bottom_tab {
                        BottomTab::Snapshots => BottomTab::Branches,
                        BottomTab::Branches => BottomTab::Tags,
                        BottomTab::Tags => BottomTab::Snapshots,
                    };
                    self.switch_bottom_tab(next);
                } else if self.focused_pane == Pane::Detail {
                    let next = match self.detail_mode {
                        DetailMode::Node => DetailMode::Repo,
                        DetailMode::Repo => DetailMode::Branch,
                        DetailMode::Branch => DetailMode::Snapshot,
                        DetailMode::Snapshot => DetailMode::OpsLog,
                        DetailMode::OpsLog => DetailMode::Node,
                    };
                    self.set_detail_mode(next);
                } else {
                    let next = match self.focused_pane {
                        Pane::Sidebar => Pane::Detail,
                        Pane::Detail => {
                            if self.bottom_visible {
                                Pane::Bottom
                            } else {
                                Pane::Sidebar
                            }
                        }
                        Pane::Bottom => Pane::Sidebar,
                    };
                    return Action::FocusPane(next);
                }
                return Action::None;
            }
            KeyCode::BackTab => {
                if self.focused_pane == Pane::Bottom {
                    let prev = match self.bottom_tab {
                        BottomTab::Snapshots => BottomTab::Tags,
                        BottomTab::Branches => BottomTab::Snapshots,
                        BottomTab::Tags => BottomTab::Branches,
                    };
                    self.switch_bottom_tab(prev);
                } else if self.focused_pane == Pane::Detail {
                    let prev = match self.detail_mode {
                        DetailMode::Node => DetailMode::OpsLog,
                        DetailMode::Repo => DetailMode::Node,
                        DetailMode::Branch => DetailMode::Repo,
                        DetailMode::Snapshot => DetailMode::Branch,
                        DetailMode::OpsLog => DetailMode::Snapshot,
                    };
                    self.set_detail_mode(prev);
                } else {
                    // Cycle panes backward: Sidebar -> Bottom -> Detail -> Sidebar
                    let prev = match self.focused_pane {
                        Pane::Sidebar => {
                            if self.bottom_visible {
                                Pane::Bottom
                            } else {
                                Pane::Detail
                            }
                        }
                        Pane::Detail => Pane::Sidebar,
                        Pane::Bottom => Pane::Detail,
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
                let prev = match self.bottom_tab {
                    BottomTab::Snapshots => BottomTab::Tags,
                    BottomTab::Branches => BottomTab::Snapshots,
                    BottomTab::Tags => BottomTab::Branches,
                };
                self.switch_bottom_tab(prev);
                return Action::None;
            }
            KeyCode::Char('l') | KeyCode::Right if self.focused_pane == Pane::Bottom => {
                let next = match self.bottom_tab {
                    BottomTab::Snapshots => BottomTab::Branches,
                    BottomTab::Branches => BottomTab::Tags,
                    BottomTab::Tags => BottomTab::Snapshots,
                };
                self.switch_bottom_tab(next);
                return Action::None;
            }
            KeyCode::Char('l') | KeyCode::Right if self.focused_pane == Pane::Sidebar => {
                return Action::FocusPane(Pane::Detail);
            }
            KeyCode::Char('h') | KeyCode::Left if self.focused_pane == Pane::Detail => {
                if let Some(mode) = self.detail_mode.prev() {
                    self.set_detail_mode(mode);
                } else {
                    return Action::FocusPane(Pane::Sidebar);
                }
            }
            KeyCode::Char('l') | KeyCode::Right if self.focused_pane == Pane::Detail => {
                if let Some(mode) = self.detail_mode.next() {
                    self.set_detail_mode(mode);
                }
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
            KeyCode::Char('z') if self.focused_pane == Pane::Sidebar => {
                self.pending_z = true;
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
            Action::Quit => {
                self.should_quit = true;
            }
        }
    }

    fn select_next(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                let moved = self.tree_state.key_down();
                self.on_tree_selection_changed();
                if !moved && self.bottom_visible {
                    self.focused_pane = Pane::Bottom;
                }
            }
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_add(1),
            Pane::Bottom => {
                let max = self.bottom_list_len().saturating_sub(1);
                if self.bottom_selected() < max {
                    self.set_bottom_selected(self.bottom_selected() + 1);
                }
                self.on_bottom_selection_changed();
            }
        }
    }

    fn select_prev(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                self.tree_state.key_up();
                self.on_tree_selection_changed();
            }
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            Pane::Bottom => {
                if self.bottom_selected() == 0 {
                    self.focused_pane = Pane::Sidebar;
                } else {
                    self.set_bottom_selected(self.bottom_selected() - 1);
                    self.on_bottom_selection_changed();
                }
            }
        }
    }

    fn handle_enter(&mut self) {
        // Set detail mode based on context
        match self.focused_pane {
            Pane::Sidebar | Pane::Detail => self.detail_mode = DetailMode::Node,
            Pane::Bottom => match self.bottom_tab {
                BottomTab::Snapshots => self.detail_mode = DetailMode::Snapshot,
                BottomTab::Branches => self.detail_mode = DetailMode::Branch,
                BottomTab::Tags => {}
            },
        }
        match self.focused_pane {
            Pane::Sidebar => {
                // Toggle open/close on the selected tree node
                self.tree_state.toggle_selected();
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
                self.on_bottom_selection_changed();
            }
        }
    }

    /// Handle a mouse event (click to focus pane, select item)
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        // Handle scroll events on the detail area
        let mouse_pos = ratatui::prelude::Position {
            x: mouse.column,
            y: mouse.row,
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if self.detail_area.contains(mouse_pos) {
                    self.detail_scroll = self.detail_scroll.saturating_add(2);
                }
                return;
            }
            MouseEventKind::ScrollUp => {
                if self.detail_area.contains(mouse_pos) {
                    self.detail_scroll = self.detail_scroll.saturating_sub(2);
                }
                return;
            }
            _ => {}
        }

        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }

        // Clear search when clicking — the click changes focus which breaks the search context
        if self.search.is_some() {
            self.search = None;
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
                self.tree_state.select_first();
                for _ in 0..offset {
                    self.tree_state.key_down();
                }
                self.on_tree_selection_changed();
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
                let new_sel = (row - content_top - header_rows) as usize + self.bottom_offset();
                let max = self.bottom_list_len().saturating_sub(1);
                self.set_bottom_selected(new_sel.min(max));
                self.on_bottom_selection_changed();
            }
        }
    }

    /// Sync the search cursor's selected item into the actual pane selection
    /// and trigger reactive updates (branch switch, diff load, chunk stats, etc.).
    fn sync_search_selection_reactive(&mut self, candidates: &[String]) {
        let Some(ref search) = self.search else {
            return;
        };
        let Some(idx) = search.selected_index() else {
            return;
        };
        match search.target {
            crate::search::SearchTarget::Tree => {
                if let Some(path) = candidates.get(idx) {
                    self.select_tree_node(path);
                    self.on_tree_selection_changed();
                }
            }
            _ => {
                self.set_bottom_selected(idx);
                self.on_bottom_selection_changed();
            }
        }
    }
}
