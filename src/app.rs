use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;

use crate::component::{Action, BottomTab, DetailMode, Pane};
use crate::search::SearchState;
use crate::store::{DataRequest, DataStore, LoadState, TreeNodeType};
use crate::theme::Theme;

/// Structured repo identity — replaces ad-hoc pipe-delimited strings.
#[derive(Debug, Clone)]
pub enum RepoIdentity {
    Local {
        path: String,
    },
    S3 {
        url: String,
    },
    Arraylake {
        org: String,
        repo: String,
        bucket: String,
        platform: String,
        region: String,
    },
}

impl RepoIdentity {
    /// Short display for status bar
    pub fn display_short(&self) -> String {
        match self {
            Self::Local { path } => path.clone(),
            Self::S3 { url } => url.clone(),
            Self::Arraylake {
                org, repo, bucket, ..
            } => format!("{org}/{repo}  ({bucket})"),
        }
    }
}

/// Application coordinator.
/// Owns the data store, theme, pane focus, and layout state.
pub struct App {
    pub store: DataStore,
    pub theme: Theme,
    pub should_quit: bool,

    // ─── Version control state (source of truth) ─────────────
    /// The active branch. Changing this reloads ancestry + tree.
    pub current_branch: String,
    /// The active snapshot ID. Derived from branch tip initially,
    /// updated when navigating snapshots/tags. Drives tree + diffs.
    pub current_snapshot: Option<String>,
    pub repo_info: RepoIdentity,

    // ─── View state (UI only) ────────────────────────────────
    pub focused_pane: Pane,
    pub bottom_visible: bool,
    pub bottom_tab: BottomTab,
    pub detail_mode: DetailMode,
    pub show_help: bool,

    // ─── Selection state ─────────────────────────────────────
    pub tree_state: tui_tree_widget::TreeState<String>,
    pub detail_scroll: usize,
    /// Per-tab selection index (not shared across tabs)
    pub tab_selection: [usize; 3], // [Snapshots, Branches, Tags]
    /// Per-tab scroll offset
    pub tab_offset: [usize; 3],

    // ─── Search ──────────────────────────────────────────────
    /// Active fuzzy search. None = not searching.
    pub search: Option<SearchState>,

    // ─── Internal bookkeeping ────────────────────────────────
    tree_auto_expanded: bool,
    /// Dedup guard: last snapshot we requested a diff for
    pub last_diff_requested: Option<String>,
    /// Dedup guard: last snapshot we requested a tree for
    pub last_tree_snapshot_requested: Option<String>,
    /// Cached tree search candidates — invalidated when node_children changes.
    tree_candidate_cache: Option<Vec<String>>,

    // Layout areas (updated each render for mouse hit-testing)
    pub sidebar_area: Rect,
    pub detail_area: Rect,
    pub bottom_area: Option<Rect>,
}

impl App {
    pub fn new(store: DataStore, repo_info: RepoIdentity) -> Self {
        Self {
            store,
            theme: Theme::default(),
            should_quit: false,
            focused_pane: Pane::Sidebar,
            bottom_visible: true,
            bottom_tab: BottomTab::Snapshots,
            detail_mode: DetailMode::Node,
            show_help: false,
            current_branch: "main".to_string(),
            repo_info,
            tree_state: tui_tree_widget::TreeState::<String>::default(),
            current_snapshot: None,
            detail_scroll: 0,
            tab_selection: [0; 3],
            tab_offset: [0; 3],
            search: None,
            tree_auto_expanded: false,
            last_diff_requested: None,
            last_tree_snapshot_requested: None,
            tree_candidate_cache: None,
            sidebar_area: Rect::default(),
            detail_area: Rect::default(),
            bottom_area: None,
        }
    }

    // ─── Tab selection accessors ────────────────────────────
    fn tab_index(tab: BottomTab) -> usize {
        match tab {
            BottomTab::Snapshots => 0,
            BottomTab::Branches => 1,
            BottomTab::Tags => 2,
        }
    }
    pub fn bottom_selected(&self) -> usize {
        self.tab_selection[Self::tab_index(self.bottom_tab)]
    }
    pub fn bottom_offset(&self) -> usize {
        self.tab_offset[Self::tab_index(self.bottom_tab)]
    }
    fn set_bottom_selected(&mut self, val: usize) {
        self.tab_selection[Self::tab_index(self.bottom_tab)] = val;
    }
    fn set_bottom_offset(&mut self, val: usize) {
        self.tab_offset[Self::tab_index(self.bottom_tab)] = val;
    }

    // ─── State setters that propagate to dependents ──────────

    /// Change branch. Reloads ancestry + tree. Resets snapshot to branch tip.
    fn set_branch(&mut self, branch: String) {
        if branch == self.current_branch {
            return;
        }
        self.current_branch = branch;
        self.current_snapshot = None; // will resolve to tip once ancestry loads
        self.tree_state = tui_tree_widget::TreeState::default();
        self.tree_auto_expanded = false;
        self.last_diff_requested = None;
        self.last_tree_snapshot_requested = None;
        // Fetch tree + ancestry for this branch (branches/tags/config don't change)
        self.store.submit(DataRequest::AllNodes {
            branch: self.current_branch.clone(),
            snapshot_id: None,
        });
        self.store.submit(DataRequest::Ancestry {
            branch: self.current_branch.clone(),
        });
    }

    /// Change snapshot. Reloads tree at that snapshot and requests diff.
    fn set_snapshot(&mut self, snapshot_id: Option<String>) {
        if self.current_snapshot == snapshot_id {
            return;
        }
        self.current_snapshot = snapshot_id;
        self.tree_auto_expanded = false;
        // Fetch tree at this snapshot (or branch tip if None)
        let snap = self.current_snapshot.clone();
        self.store.submit(DataRequest::AllNodes {
            branch: self.current_branch.clone(),
            snapshot_id: snap.clone(),
        });
        // Request diff if we have a snapshot
        if let Some(ref sid) = self.current_snapshot
            && self.last_diff_requested.as_ref() != Some(sid)
        {
            let parent_id = self
                .store
                .ancestry
                .get(&self.current_branch)
                .and_then(|s| s.as_loaded())
                .and_then(|entries| entries.iter().find(|e| e.id == *sid))
                .and_then(|e| e.parent_id.clone());
            self.last_diff_requested = Some(sid.clone());
            self.store.submit(DataRequest::SnapshotDiff {
                snapshot_id: sid.clone(),
                parent_id,
            });
        }
    }

    /// Build the full identifier path for tui_tree_widget selection.
    /// For "/stations/latitude" returns ["/stations", "/stations/latitude"].
    fn tree_identifier_path(path: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut built = String::new();
        for seg in segments {
            built = format!("{}/{}", built, seg);
            parts.push(built.clone());
        }
        parts
    }

    /// Select a tree node by path, opening all ancestor groups.
    fn select_tree_node(&mut self, path: &str) {
        let id_path = Self::tree_identifier_path(path);
        // Open all ancestor groups so the node is visible
        for i in 0..id_path.len().saturating_sub(1) {
            self.tree_state.open(id_path[..=i].to_vec());
        }
        self.tree_state.select(id_path);
    }

    /// Sync the search cursor's selected item into the actual pane selection.
    fn sync_search_selection(&mut self, candidates: &[String]) {
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
                }
            }
            _ => {
                self.set_bottom_selected(idx);
            }
        }
    }

    /// Called when tree selection changes — updates detail state.
    fn on_tree_selection_changed(&mut self) {
        self.detail_scroll = 0;
        self.detail_mode = DetailMode::Node;
        self.maybe_request_chunk_stats();
    }

    /// Called when any bottom-panel selection changes.
    fn on_bottom_selection_changed(&mut self) {
        self.detail_scroll = 0;
        // Only switch to Node mode for Snapshots (where detail shows diffs).
        // Branches/Tags may be browsed while viewing the Repo tab.
        if self.bottom_tab == BottomTab::Snapshots {
            self.detail_mode = DetailMode::Node;
        }
        self.clamp_bottom_table_offset();
        match self.bottom_tab {
            BottomTab::Snapshots => {
                // Derive snapshot ID from selection index
                let snap_id = self
                    .store
                    .ancestry
                    .get(&self.current_branch)
                    .and_then(|s| s.as_loaded())
                    .and_then(|entries| entries.get(self.bottom_selected()))
                    .map(|e| e.id.clone());
                self.set_snapshot(snap_id);
            }
            BottomTab::Branches => {
                if let Some(branches) = self.store.branches.as_loaded()
                    && let Some(branch) = branches.get(self.bottom_selected())
                {
                    self.set_branch(branch.name.clone());
                }
            }
            BottomTab::Tags => {
                if let Some(tags) = self.store.tags.as_loaded()
                    && let Some(tag) = tags.get(self.bottom_selected())
                {
                    self.set_snapshot(Some(tag.snapshot_id.clone()));
                }
            }
        }
    }

    /// Kick off initial data loads
    pub fn load_initial_data(&mut self) {
        // Reset tree dedup guard when (re-)loading for a branch
        self.last_tree_snapshot_requested = None;
        self.store.submit(DataRequest::Branches);
        self.store.submit(DataRequest::Tags);
        self.store.submit(DataRequest::AllNodes {
            branch: self.current_branch.clone(),
            snapshot_id: None,
        });
        self.store.submit(DataRequest::Ancestry {
            branch: self.current_branch.clone(),
        });
        self.store.submit(DataRequest::RepoConfig);
        self.store.submit(DataRequest::OpsLog);
    }

    /// Re-submit requests for any data currently in an Error state.
    #[allow(dead_code)]
    pub fn retry_failed(&mut self) {
        // Top-level data
        if matches!(self.store.branches, LoadState::Error(_)) {
            self.store.submit(DataRequest::Branches);
        }
        if matches!(self.store.tags, LoadState::Error(_)) {
            self.store.submit(DataRequest::Tags);
        }
        if matches!(self.store.repo_config, LoadState::Error(_)) {
            self.store.submit(DataRequest::RepoConfig);
        }
        if matches!(self.store.ops_log, LoadState::Error(_)) {
            self.store.submit(DataRequest::OpsLog);
        }

        // Ancestry for current branch
        if let Some(LoadState::Error(_)) = self.store.ancestry.get(&self.current_branch) {
            self.store.submit(DataRequest::Ancestry {
                branch: self.current_branch.clone(),
            });
        }

        // Node tree (check root key)
        if let Some(LoadState::Error(_)) = self.store.node_children.get("/") {
            self.tree_auto_expanded = false;
            self.last_tree_snapshot_requested = None;
            self.store.submit(DataRequest::AllNodes {
                branch: self.current_branch.clone(),
                snapshot_id: self.current_snapshot.clone(),
            });
        }
    }

    /// Drain all pending responses from background worker
    pub fn drain_responses(&mut self) {
        let had_responses = self.store.drain_responses();
        if had_responses {
            // Invalidate search candidate cache when data changes
            self.tree_candidate_cache = None;
        }

        // After AllNodes data arrives, auto-expand groups so the user sees
        // meaningful content immediately instead of a collapsed root.
        if !self.tree_auto_expanded
            && let Some(LoadState::Loaded(_)) = self.store.node_children.get("/")
        {
            self.auto_expand_tree();
            self.tree_auto_expanded = true;
            // Kick off chunk stats for whatever array got auto-selected
            self.maybe_request_chunk_stats();
            // Start scanning all arrays in the background
            self.scan_all_chunk_stats();
        }

        // Once branches load, sync the Branches tab selection to current_branch.
        // Also verify current_branch actually exists — fall back to first branch if not.
        if let Some(LoadState::Loaded(branches)) = Some(&self.store.branches)
            && !branches.is_empty()
        {
            let branch_exists = branches.iter().any(|b| b.name == self.current_branch);
            if !branch_exists {
                // "main" doesn't exist — use the first branch
                self.current_branch = branches[0].name.clone();
            }
            // Sync Branches tab selection to current_branch
            if let Some(idx) = branches.iter().position(|b| b.name == self.current_branch) {
                self.tab_selection[1] = idx; // 1 = Branches tab
            }
        }

        // Auto-set the active snapshot to tip when ancestry first loads
        if self.current_snapshot.is_none()
            && let Some(LoadState::Loaded(entries)) = self.store.ancestry.get(&self.current_branch)
            && let Some(first) = entries.first()
        {
            self.current_snapshot = Some(first.id.clone());
            // Now that we have a snapshot ID, scan any arrays that were waiting
            if self.tree_auto_expanded {
                self.scan_all_chunk_stats();
            }
        }

        // Auto-request diff when bottom pane is focused on Snapshots tab
        self.maybe_request_snapshot_diff();
    }

    /// If the bottom pane is focused on the Snapshots tab and we have a selected
    /// snapshot that we haven't yet requested a diff for, submit the request.
    fn maybe_request_snapshot_diff(&mut self) {
        if self.bottom_tab != BottomTab::Snapshots {
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

        // Look up the parent_id from the cached ancestry — avoids fetching the snapshot.
        let parent_id = self
            .store
            .ancestry
            .get(&self.current_branch)
            .and_then(|s| s.as_loaded())
            .and_then(|entries| entries.iter().find(|e| e.id == sid))
            .and_then(|e| e.parent_id.clone());

        self.last_diff_requested = Some(sid.clone());
        self.store.submit(DataRequest::SnapshotDiff {
            snapshot_id: sid,
            parent_id,
        });
    }

    /// If an array node is currently selected in the sidebar and we haven't already
    /// fetched (or started fetching) its chunk stats, submit the request.
    fn maybe_request_chunk_stats(&mut self) {
        let selected = self.tree_state.selected();
        let Some(path) = selected.last() else { return };

        // Get the current snapshot context
        let snapshot_id = self
            .selected_snapshot_id()
            .or_else(|| self.get_branch_tip_snapshot_id());
        let Some(snapshot_id) = snapshot_id else {
            return;
        };

        let key = (snapshot_id.clone(), path.clone());

        // Only request if not already cached or loading
        if self.store.chunk_stats.contains_key(&key) {
            return;
        }

        // Check whether the selected node is an array
        let is_array = self
            .store
            .node_children
            .values()
            .find_map(|state| {
                if let LoadState::Loaded(nodes) = state {
                    nodes.iter().find(|n| n.path == *path)
                } else {
                    None
                }
            })
            .map(|node| matches!(node.node_type, crate::store::TreeNodeType::Array(_)))
            .unwrap_or(false);

        if is_array {
            self.store.submit(DataRequest::ChunkStats {
                snapshot_id,
                path: path.clone(),
            });
        }
    }

    /// Submit chunk stats requests for all arrays in the tree.
    /// Each request is a separate background task, so they run concurrently.
    fn scan_all_chunk_stats(&mut self) {
        let snapshot_id = self
            .selected_snapshot_id()
            .or_else(|| self.get_branch_tip_snapshot_id());
        let Some(snapshot_id) = snapshot_id else {
            return;
        };

        // Collect all array paths first to avoid borrow conflict with submit()
        let array_paths: Vec<String> = self
            .store
            .node_children
            .values()
            .flat_map(|state| match state {
                LoadState::Loaded(nodes) => nodes
                    .iter()
                    .filter(|n| matches!(n.node_type, crate::store::TreeNodeType::Array(_)))
                    .map(|n| n.path.clone())
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();

        for path in array_paths {
            let key = (snapshot_id.clone(), path.clone());
            if self.store.chunk_stats.contains_key(&key) {
                continue;
            }
            self.store.submit(DataRequest::ChunkStats {
                snapshot_id: snapshot_id.clone(),
                path,
            });
        }
    }

    /// Get the snapshot ID at the tip of the current branch, from the branches cache.
    pub fn get_branch_tip_snapshot_id(&self) -> Option<String> {
        self.store
            .branches
            .as_loaded()
            .and_then(|branches| branches.iter().find(|b| b.name == self.current_branch))
            .map(|b| b.snapshot_id.clone())
    }

    /// Get the snapshot ID for the currently selected snapshot.
    pub fn selected_snapshot_id(&self) -> Option<String> {
        self.current_snapshot.clone()
    }

    /// Get the searchable strings for the current pane context.
    fn search_candidates(&mut self) -> Vec<String> {
        match self.focused_pane {
            Pane::Sidebar => {
                if self.tree_candidate_cache.is_none() {
                    self.tree_candidate_cache =
                        Some(crate::search::tree_candidates(&self.store));
                }
                self.tree_candidate_cache.clone().unwrap()
            }
            Pane::Bottom => match self.bottom_tab {
                BottomTab::Snapshots => self
                    .store
                    .ancestry
                    .get(&self.current_branch)
                    .and_then(|s| s.as_loaded())
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|e| format!("{} {}", e.id, e.message))
                            .collect()
                    })
                    .unwrap_or_default(),
                BottomTab::Branches => self
                    .store
                    .branches
                    .as_loaded()
                    .map(|b| b.iter().map(|b| b.name.clone()).collect())
                    .unwrap_or_default(),
                BottomTab::Tags => self
                    .store
                    .tags
                    .as_loaded()
                    .map(|t| t.iter().map(|t| t.name.clone()).collect())
                    .unwrap_or_default(),
            },
            Pane::Detail => Vec::new(), // No search in detail pane
        }
    }

    /// Handle a key event
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Search mode intercepts all keys
        if self.search.is_some() {
            self.handle_search_key(key);
            return;
        }
        let action = self.map_key(key);
        self.process_action(action);
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let candidates = self.search_candidates();
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
                    }
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if let Some(ref mut search) = self.search {
                    search.next();
                    self.sync_search_selection(&candidates);
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(ref mut search) = self.search {
                    search.prev();
                    self.sync_search_selection(&candidates);
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut search) = self.search {
                    search.push_char(c, &candidate_refs);
                    self.sync_search_selection(&candidates);
                }
            }
            _ => {}
        }
    }

    fn map_key(&mut self, key: KeyEvent) -> Action {
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
                    self.detail_mode = match self.detail_mode {
                        DetailMode::Node => DetailMode::Repo,
                        DetailMode::Repo => DetailMode::OpsLog,
                        DetailMode::OpsLog => DetailMode::Node,
                    };
                    self.detail_scroll = 0;
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
                    self.detail_mode = match self.detail_mode {
                        DetailMode::Node => DetailMode::OpsLog,
                        DetailMode::Repo => DetailMode::Node,
                        DetailMode::OpsLog => DetailMode::Repo,
                    };
                    self.detail_scroll = 0;
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
                match self.detail_mode {
                    DetailMode::OpsLog => {
                        self.detail_mode = DetailMode::Repo;
                        self.detail_scroll = 0;
                        return Action::None;
                    }
                    DetailMode::Repo => {
                        self.detail_mode = DetailMode::Node;
                        self.detail_scroll = 0;
                        return Action::None;
                    }
                    DetailMode::Node => {
                        return Action::FocusPane(Pane::Sidebar);
                    }
                }
            }
            KeyCode::Char('l') | KeyCode::Right if self.focused_pane == Pane::Detail => {
                match self.detail_mode {
                    DetailMode::Node => {
                        self.detail_mode = DetailMode::Repo;
                        self.detail_scroll = 0;
                        return Action::None;
                    }
                    DetailMode::Repo => {
                        self.detail_mode = DetailMode::OpsLog;
                        self.detail_scroll = 0;
                        return Action::None;
                    }
                    DetailMode::OpsLog => {
                        return Action::None;
                    }
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

    fn bottom_list_len(&self) -> usize {
        match self.bottom_tab {
            BottomTab::Snapshots => self
                .store
                .ancestry
                .get(&self.current_branch)
                .and_then(|s| s.as_loaded())
                .map(|a| a.len())
                .unwrap_or(0),
            BottomTab::Branches => self
                .store
                .branches
                .as_loaded()
                .map(|b| b.len())
                .unwrap_or(0),
            BottomTab::Tags => self.store.tags.as_loaded().map(|t| t.len()).unwrap_or(0),
        }
    }

    fn switch_bottom_tab(&mut self, tab: BottomTab) {
        if self.bottom_tab != tab {
            self.bottom_tab = tab;
            // Per-tab selection is preserved in tab_selection/tab_offset arrays,
            // so just switching the tab is enough — no need to reset to 0.
            self.on_bottom_selection_changed();
        }
    }

    fn clamp_bottom_table_offset(&mut self) {
        let visible = self
            .bottom_area
            .map(|a| a.height as usize)
            .unwrap_or(10)
            .saturating_sub(4) // borders + header + tab bar
            .max(1);

        let sel = self.bottom_selected();
        let off = self.bottom_offset();
        if sel < off {
            self.set_bottom_offset(sel);
        } else if sel >= off + visible {
            self.set_bottom_offset(sel + 1 - visible);
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

    /// Auto-expand the tree when root children are all groups.
    /// Drills down through single-child groups so the user lands on
    /// the first meaningful level of the hierarchy.
    fn auto_expand_tree(&mut self) {
        let mut current_path = "/".to_string();
        let mut identifier_path: Vec<String> = Vec::new();

        while let Some(LoadState::Loaded(children)) = self.store.node_children.get(&current_path) {
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
        self.detail_mode = DetailMode::Node;
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
                            snapshot_id: None,
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
                self.on_bottom_selection_changed();
            }
        }
    }
}
