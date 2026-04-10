mod keys;
mod tree;

use ratatui::prelude::Rect;

use crate::component::{BottomTab, DetailMode, Pane};
use crate::search::SearchState;
use crate::store::{DataRequest, DataStore, LoadState};
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
                org,
                repo,
                bucket,
                platform,
                ..
            } => format!("{org}/{repo}  ({bucket}, {platform})"),
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
    /// Vim-style pending `z` prefix for fold commands (zo, zc, zO, zC, zR, zM)
    pub pending_z: bool,
    /// Vim-style pending `g` prefix (gg = go to top)
    pub pending_g: bool,
    /// Last search query for n/N repeat (target + query string)
    pub last_search: Option<(crate::search::SearchTarget, String)>,

    // ─── Internal bookkeeping ────────────────────────────────
    pub(crate) tree_auto_expanded: bool,
    /// Dedup guard: last snapshot we requested a diff for
    pub last_diff_requested: Option<String>,
    /// Cached tree search candidates — invalidated when node_children changes.
    pub(crate) tree_candidate_cache: Option<Vec<String>>,
    /// Cached bottom-panel search candidates — invalidated when tab/data changes.
    pub(crate) bottom_candidate_cache: Option<Vec<String>>,
    /// Queue of array paths waiting to have chunk stats scanned.
    /// Drip-fed a few per frame to avoid blocking startup.
    pub(crate) chunk_scan_queue: Vec<String>,
    /// Snapshot ID for the current scan queue (invalidated on branch/snapshot change).
    pub(crate) chunk_scan_snapshot: Option<String>,
    /// Guard: branch existence sync already done for current branches data.
    pub(crate) branches_synced: bool,
    /// Guard: chunk scan has completed (nothing left to scan).
    pub(crate) chunk_scan_complete: bool,
    /// Count of in-flight chunk stats requests (Loading state), tracked incrementally.
    pub(crate) chunk_stats_in_flight: usize,

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
            focused_pane: Pane::Detail,
            bottom_visible: true,
            bottom_tab: BottomTab::Snapshots,
            detail_mode: DetailMode::Repo,
            show_help: false,
            current_branch: "main".to_string(),
            repo_info,
            tree_state: tui_tree_widget::TreeState::<String>::default(),
            current_snapshot: None,
            detail_scroll: 0,
            tab_selection: [0; 3],
            tab_offset: [0; 3],
            search: None,
            pending_z: false,
            pending_g: false,
            last_search: None,
            tree_auto_expanded: false,
            last_diff_requested: None,
            tree_candidate_cache: None,
            bottom_candidate_cache: None,
            chunk_scan_queue: Vec::new(),
            chunk_scan_snapshot: None,
            branches_synced: false,
            chunk_scan_complete: false,
            chunk_stats_in_flight: 0,
            sidebar_area: Rect::default(),
            detail_area: Rect::default(),
            bottom_area: None,
        }
    }

    // ─── Tab selection accessors ────────────────────────────
    pub(crate) fn tab_index(tab: BottomTab) -> usize {
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
    pub(crate) fn set_bottom_selected(&mut self, val: usize) {
        self.tab_selection[Self::tab_index(self.bottom_tab)] = val;
    }
    pub(crate) fn set_bottom_offset(&mut self, val: usize) {
        self.tab_offset[Self::tab_index(self.bottom_tab)] = val;
    }

    // ─── State setters that propagate to dependents ──────────

    /// Change branch. Reloads ancestry + tree. Resets snapshot to branch tip.
    pub(crate) fn set_branch(&mut self, branch: String) {
        if branch == self.current_branch {
            return;
        }
        self.current_branch = branch;
        self.current_snapshot = None; // will resolve to tip once ancestry loads
        self.tree_state = tui_tree_widget::TreeState::default();
        self.tree_auto_expanded = false;
        self.last_diff_requested = None;
        self.chunk_scan_complete = false;
        self.chunk_scan_snapshot = None;
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
    pub(crate) fn set_snapshot(&mut self, snapshot_id: Option<String>) {
        if self.current_snapshot == snapshot_id {
            return;
        }
        self.current_snapshot = snapshot_id;
        self.tree_auto_expanded = false;
        self.chunk_scan_complete = false;
        self.chunk_scan_snapshot = None;
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

    /// Called when tree selection changes — updates detail state.
    pub(crate) fn on_tree_selection_changed(&mut self) {
        self.detail_scroll = 0;
        self.detail_mode = DetailMode::Node;
        self.maybe_request_chunk_stats();
    }

    /// Called when any bottom-panel selection changes.
    pub(crate) fn on_bottom_selection_changed(&mut self) {
        self.detail_scroll = 0;
        // Auto-switch detail tab to match the bottom panel context
        match self.bottom_tab {
            BottomTab::Snapshots => self.detail_mode = DetailMode::Snapshot,
            BottomTab::Branches => self.detail_mode = DetailMode::Branch,
            BottomTab::Tags => {} // Tags don't have a dedicated detail view
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
        self.branches_synced = false;
        self.chunk_scan_complete = false;
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
    pub fn retry_failed(&mut self) {
        // Top-level data
        if matches!(self.store.branches, LoadState::Error(_)) {
            self.branches_synced = false;
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
            // Invalidate search candidate caches when data changes
            self.tree_candidate_cache = None;
            self.bottom_candidate_cache = None;
            // Decrement in-flight chunk stats counter
            self.chunk_stats_in_flight = self
                .chunk_stats_in_flight
                .saturating_sub(self.store.chunk_stats_received);
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
            // Queue up chunk stats scan for all arrays
            self.start_chunk_scan();
        }

        // Once branches load, sync the Branches tab selection to current_branch.
        // Also verify current_branch actually exists — fall back to first branch if not.
        // Pre-fetch ancestry for all branches (cheap — reads from repo info file).
        // Guarded: only run once per branches data load.
        if !self.branches_synced
            && let Some(LoadState::Loaded(branches)) = Some(&self.store.branches)
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
            // Collect branch names that need ancestry pre-fetching
            let branches_needing_ancestry: Vec<String> = branches
                .iter()
                .filter(|b| !self.store.ancestry.contains_key(&b.name))
                .map(|b| b.name.clone())
                .collect();
            // Pre-fetch ancestry for all branches so switching is instant.
            // The repo info file is fetched once; subsequent ancestry calls reuse it.
            for branch_name in branches_needing_ancestry {
                self.store.submit(DataRequest::Ancestry {
                    branch: branch_name,
                });
            }
            self.branches_synced = true;
        }

        // Auto-set the active snapshot to tip when ancestry first loads
        if self.current_snapshot.is_none()
            && let Some(LoadState::Loaded(entries)) = self.store.ancestry.get(&self.current_branch)
            && let Some(first) = entries.first()
        {
            self.current_snapshot = Some(first.id.clone());
            // Now that we have a snapshot ID, start scanning if tree is ready
            if self.tree_auto_expanded {
                self.start_chunk_scan();
            }
        }

        // Auto-request diff when bottom pane is focused on Snapshots tab
        self.maybe_request_snapshot_diff();

        // Drip-feed chunk stats requests from the background scan queue.
        // If the queue is empty but we have a snapshot and tree, restart the scan
        // (handles the race where start_chunk_scan ran before data was ready).
        // Guarded: once all arrays have been scanned, don't restart.
        if !self.chunk_scan_complete
            && self.chunk_scan_queue.is_empty()
            && self.chunk_scan_snapshot.is_none()
            && self.tree_auto_expanded
            && self.current_snapshot.is_some()
        {
            self.start_chunk_scan();
            if self.chunk_scan_queue.is_empty() {
                self.chunk_scan_complete = true;
            }
        }
        self.drain_chunk_scan_queue();
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
            .find_node(path)
            .map(|node| matches!(node.node_type, crate::store::TreeNodeType::Array(_)))
            .unwrap_or(false);

        if is_array {
            self.store.submit(DataRequest::ChunkStats {
                snapshot_id,
                path: path.clone(),
            });
        }
    }

    /// Populate the chunk scan queue with all array paths in the tree.
    /// Actual requests are dripped via `drain_chunk_scan_queue()` each frame.
    fn start_chunk_scan(&mut self) {
        let snapshot_id = self
            .selected_snapshot_id()
            .or_else(|| self.get_branch_tip_snapshot_id());
        let Some(snapshot_id) = snapshot_id else {
            return;
        };

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

        self.chunk_scan_snapshot = Some(snapshot_id);
        self.chunk_scan_queue = array_paths;
    }

    /// Submit a small batch of chunk stats requests from the scan queue.
    /// Called each frame tick so requests drip in without blocking.
    fn drain_chunk_scan_queue(&mut self) {
        let Some(ref snapshot_id) = self.chunk_scan_snapshot else {
            return;
        };
        let snapshot_id = snapshot_id.clone();

        // Keep at most ~16 concurrent requests
        let budget = 16usize.saturating_sub(self.chunk_stats_in_flight);
        if budget == 0 {
            return;
        }

        let mut submitted = 0;
        while submitted < budget {
            let Some(path) = self.chunk_scan_queue.pop() else {
                break;
            };
            let key = (snapshot_id.clone(), path.clone());
            if self.store.chunk_stats.contains_key(&key) {
                continue; // already cached or loading, skip
            }
            self.store.submit(DataRequest::ChunkStats {
                snapshot_id: snapshot_id.clone(),
                path,
            });
            self.chunk_stats_in_flight += 1;
            submitted += 1;
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
    /// Returns a borrowed slice backed by internal caches.
    pub(crate) fn search_candidates(&mut self) -> &[String] {
        match self.focused_pane {
            Pane::Sidebar => {
                if self.tree_candidate_cache.is_none() {
                    self.tree_candidate_cache = Some(crate::search::tree_candidates(&self.store));
                }
                self.tree_candidate_cache.as_ref().unwrap()
            }
            Pane::Bottom => {
                if self.bottom_candidate_cache.is_none() {
                    let candidates = match self.bottom_tab {
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
                    };
                    self.bottom_candidate_cache = Some(candidates);
                }
                self.bottom_candidate_cache.as_ref().unwrap()
            }
            Pane::Detail => &[], // No search in detail pane
        }
    }

    pub(crate) fn bottom_list_len(&self) -> usize {
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

    /// Set the detail mode and sync the bottom panel to match.
    /// Use this instead of setting `detail_mode` directly to keep panes in sync.
    pub(crate) fn set_detail_mode(&mut self, mode: DetailMode) {
        self.detail_mode = mode;
        self.detail_scroll = 0;
        // Sync bottom panel to match
        match mode {
            DetailMode::Branch => self.switch_bottom_tab(BottomTab::Branches),
            DetailMode::Snapshot => self.switch_bottom_tab(BottomTab::Snapshots),
            _ => {}
        }
    }

    pub(crate) fn switch_bottom_tab(&mut self, tab: BottomTab) {
        if self.bottom_tab != tab {
            self.bottom_tab = tab;
            self.bottom_candidate_cache = None;
            // Per-tab selection is preserved in tab_selection/tab_offset arrays,
            // so just switching the tab is enough — no need to reset to 0.
            self.on_bottom_selection_changed();
        }
    }

    pub(crate) fn clamp_bottom_table_offset(&mut self) {
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
}
