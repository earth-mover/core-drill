use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use crate::component::{Action, BottomTab, DetailMode, Pane};
use crate::search::SearchState;

use super::App;

impl App {
    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Help overlay: ? or Esc closes it, all other keys ignored
        if self.show_help {
            if matches!(key.code, KeyCode::Char('q') | KeyCode::Char('?')) {
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
                // Save for n/N repeat even on Esc
                if let Some(ref search) = self.search
                    && !search.query.is_empty()
                {
                    self.last_search = Some((search.target, search.query.clone()));
                }
                self.search = None;
            }
            KeyCode::Enter => {
                if let Some(ref search) = self.search {
                    // Save for n/N repeat
                    if !search.query.is_empty() {
                        self.last_search = Some((search.target, search.query.clone()));
                    }
                    if let Some(idx) = search.selected_index() {
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
        // Handle pending `g` prefix for vim jump commands (gg = go to top)
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.select_first();
            }
            return Action::None;
        }

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

        // Handle pending `y` prefix for yank commands (yy=selection, yp=Python, yr=Rust)
        if self.pending_y {
            self.pending_y = false;
            match key.code {
                KeyCode::Char('y') => {
                    // yy — yank current selection
                    let text = self.yank_selection_text();
                    if !text.is_empty() {
                        self.yank_text(text, "selection");
                    }
                }
                KeyCode::Char('p') => {
                    let ctx = self.code_context();
                    let (python, _) = crate::codegen::generate(&self.repo_info, &ctx);
                    self.yank_text(python, "Python snippet");
                }
                KeyCode::Char('r') => {
                    let ctx = self.code_context();
                    let (_, rust) = crate::codegen::generate(&self.repo_info, &ctx);
                    self.yank_text(rust, "Rust snippet");
                }
                _ => {} // cancel — unknown y-command
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
            KeyCode::Char('y') => {
                self.pending_y = true;
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
                KeyCode::Char('d') => {
                    let half = (self.pane_visible_height() / 2).max(1);
                    self.move_by(half as isize);
                    return Action::None;
                }
                KeyCode::Char('u') => {
                    let half = (self.pane_visible_height() / 2).max(1);
                    self.move_by(-(half as isize));
                    return Action::None;
                }
                KeyCode::Char('f') => {
                    let page = self.pane_visible_height().max(1);
                    self.move_by(page as isize);
                    return Action::None;
                }
                KeyCode::Char('b') => {
                    let page = self.pane_visible_height().max(1);
                    self.move_by(-(page as isize));
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
                } else {
                    return Action::FocusPane(Pane::Bottom);
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
            KeyCode::Char('G') => {
                self.select_last();
                Action::None
            }
            KeyCode::Char('g') => {
                self.pending_g = true;
                Action::None
            }
            KeyCode::Char('H') => {
                self.select_screen_top();
                Action::None
            }
            KeyCode::Char('M') => {
                self.select_screen_middle();
                Action::None
            }
            KeyCode::Char('L') => {
                self.select_screen_bottom();
                Action::None
            }
            KeyCode::Char('}') => {
                self.move_by(10);
                Action::None
            }
            KeyCode::Char('{') => {
                self.move_by(-10);
                Action::None
            }
            KeyCode::Char('n') => {
                self.search_jump(true);
                Action::None
            }
            KeyCode::Char('N') => {
                self.search_jump(false);
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

    fn select_first(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                self.tree_state.select_first();
                self.on_tree_selection_changed();
            }
            Pane::Detail => self.detail_scroll = 0,
            Pane::Bottom => {
                self.set_bottom_selected(0);
                self.on_bottom_selection_changed();
            }
        }
    }

    fn select_last(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                self.tree_state.select_last();
                self.on_tree_selection_changed();
            }
            Pane::Detail => self.detail_scroll = usize::MAX / 2,
            Pane::Bottom => {
                let max = self.bottom_list_len().saturating_sub(1);
                self.set_bottom_selected(max);
                self.on_bottom_selection_changed();
            }
        }
    }

    /// Visible row count for the currently focused pane.
    fn pane_visible_height(&self) -> usize {
        match self.focused_pane {
            Pane::Sidebar => self.sidebar_area.height.saturating_sub(2) as usize,
            Pane::Detail => self.detail_area.height.saturating_sub(2) as usize,
            Pane::Bottom => {
                let h = self.bottom_area.map_or(0, |a| a.height);
                let header: u16 = match self.bottom_tab {
                    BottomTab::Snapshots => 3, // border + tab bar + table header
                    _ => 2,
                };
                h.saturating_sub(header) as usize
            }
        }
    }

    /// Move selection by `count` items (positive = down, negative = up).
    /// Does not wrap focus to adjacent panes.
    fn move_by(&mut self, count: isize) {
        match self.focused_pane {
            Pane::Sidebar => {
                if count > 0 {
                    for _ in 0..count {
                        self.tree_state.key_down();
                    }
                } else {
                    for _ in 0..count.unsigned_abs() {
                        self.tree_state.key_up();
                    }
                }
                self.on_tree_selection_changed();
            }
            Pane::Detail => {
                if count > 0 {
                    self.detail_scroll = self.detail_scroll.saturating_add(count as usize);
                } else {
                    self.detail_scroll = self.detail_scroll.saturating_sub(count.unsigned_abs());
                }
            }
            Pane::Bottom => {
                let max = self.bottom_list_len().saturating_sub(1);
                let cur = self.bottom_selected();
                let new = if count > 0 {
                    (cur + count as usize).min(max)
                } else {
                    cur.saturating_sub(count.unsigned_abs())
                };
                self.set_bottom_selected(new);
                self.on_bottom_selection_changed();
            }
        }
    }

    /// H — jump to top of visible screen area.
    fn select_screen_top(&mut self) {
        match self.focused_pane {
            Pane::Sidebar => {
                let offset = self.tree_state.get_offset();
                #[allow(deprecated)]
                self.tree_state.select_visible_index(offset);
                self.on_tree_selection_changed();
            }
            Pane::Detail => {} // H is a no-op for scrollable content
            Pane::Bottom => {
                let offset = self.bottom_offset();
                self.set_bottom_selected(offset);
                self.on_bottom_selection_changed();
            }
        }
    }

    /// M — jump to middle of visible screen area.
    fn select_screen_middle(&mut self) {
        let half = self.pane_visible_height() / 2;
        match self.focused_pane {
            Pane::Sidebar => {
                let target = self.tree_state.get_offset() + half;
                #[allow(deprecated)]
                self.tree_state.select_visible_index(target);
                self.on_tree_selection_changed();
            }
            Pane::Detail => {
                // Scroll so current position is roughly middle
            }
            Pane::Bottom => {
                let target = (self.bottom_offset() + half).min(self.bottom_list_len().saturating_sub(1));
                self.set_bottom_selected(target);
                self.on_bottom_selection_changed();
            }
        }
    }

    /// L — jump to bottom of visible screen area.
    fn select_screen_bottom(&mut self) {
        let height = self.pane_visible_height();
        match self.focused_pane {
            Pane::Sidebar => {
                let target = self.tree_state.get_offset() + height.saturating_sub(1);
                #[allow(deprecated)]
                self.tree_state.select_visible_index(target);
                self.on_tree_selection_changed();
            }
            Pane::Detail => {
                self.detail_scroll = self.detail_scroll.saturating_add(height);
            }
            Pane::Bottom => {
                let target = (self.bottom_offset() + height.saturating_sub(1))
                    .min(self.bottom_list_len().saturating_sub(1));
                self.set_bottom_selected(target);
                self.on_bottom_selection_changed();
            }
        }
    }

    /// n/N — jump to next/previous match of last search query.
    fn search_jump(&mut self, forward: bool) {
        let Some((target, query)) = self.last_search.clone() else {
            return;
        };
        // Only works if we're in the right pane for the saved search
        let pane_matches = match target {
            crate::search::SearchTarget::Tree => self.focused_pane == Pane::Sidebar,
            _ => self.focused_pane == Pane::Bottom,
        };
        if !pane_matches {
            return;
        }

        let candidates: Vec<String> = self.search_candidates().to_vec();
        let candidate_refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();

        let mut search = crate::search::SearchState::new(
            self.focused_pane,
            self.bottom_tab,
        );
        search.query = query;
        search.update_matches(&candidate_refs);

        if search.matches.is_empty() {
            return;
        }

        // Find the current selection's position in matches to jump relative to it
        let current_idx = match self.focused_pane {
            Pane::Sidebar => {
                let selected = self.tree_state.selected();
                let path = selected.last();
                path.and_then(|p| candidates.iter().position(|c| c == p))
            }
            Pane::Bottom => Some(self.bottom_selected()),
            Pane::Detail => None,
        };

        // Find which match entry corresponds to current selection
        let match_pos = current_idx.and_then(|ci| {
            search.matches.iter().position(|&m| m == ci)
        });

        let next_match_pos = match match_pos {
            Some(pos) => {
                if forward {
                    (pos + 1) % search.matches.len()
                } else if pos == 0 {
                    search.matches.len() - 1
                } else {
                    pos - 1
                }
            }
            None => 0, // No current match — start at first
        };

        let target_idx = search.matches[next_match_pos];

        match self.focused_pane {
            Pane::Sidebar => {
                if let Some(path) = candidates.get(target_idx) {
                    self.select_tree_node(path);
                    self.on_tree_selection_changed();
                }
            }
            Pane::Bottom => {
                self.set_bottom_selected(target_idx);
                self.on_bottom_selection_changed();
            }
            Pane::Detail => {}
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
