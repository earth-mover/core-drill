pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use icechunk::Repository;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::sanitize::sanitize;

pub use types::*;

/// Broad error categories for user-friendly display
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum ErrorKind {
    /// 401/403, expired tokens, access denied
    Auth,
    /// Connection timeout, DNS failure, network unreachable
    Network,
    /// 404, missing repo, missing branch
    NotFound,
    /// Anything else
    Other,
}

/// Classify an error string into a user-facing category.
/// Pattern-matches common substrings from icechunk/object_store/reqwest errors.
#[allow(dead_code)]
pub fn classify_error(msg: &str) -> ErrorKind {
    let lower = msg.to_lowercase();
    if lower.contains("403")
        || lower.contains("401")
        || lower.contains("access denied")
        || lower.contains("forbidden")
        || lower.contains("expired")
        || lower.contains("not authorized")
        || lower.contains("authentication")
        || lower.contains("invalid token")
    {
        ErrorKind::Auth
    } else if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection refused")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("unreachable")
        || lower.contains("connect error")
        || lower.contains("no such host")
    {
        ErrorKind::Network
    } else if lower.contains("404")
        || lower.contains("not found")
        || lower.contains("no such")
        || lower.contains("does not exist")
    {
        ErrorKind::NotFound
    } else {
        ErrorKind::Other
    }
}

/// Loading state for cached data
#[derive(Debug, Clone)]
pub enum LoadState<T> {
    NotRequested,
    Loading,
    Loaded(T),
    Error(String),
}

impl<T> LoadState<T> {
    #[allow(dead_code)]
    pub fn is_loaded(&self) -> bool {
        matches!(self, LoadState::Loaded(_))
    }

    pub fn as_loaded(&self) -> Option<&T> {
        match self {
            LoadState::Loaded(v) => Some(v),
            _ => None,
        }
    }

    pub fn error_kind(&self) -> Option<ErrorKind> {
        match self {
            LoadState::Error(msg) => Some(classify_error(msg)),
            _ => None,
        }
    }
}

/// What components can request from the background worker
#[derive(Debug)]
pub enum DataRequest {
    Branches,
    Tags,
    Ancestry {
        branch: String,
    },
    /// Fetch ALL nodes in the tree for a branch (or specific snapshot) in a single request.
    /// Results are organized by parent path so every group's children are cached at once.
    /// If `snapshot_id` is Some, fetches the tree at that specific snapshot instead of the branch tip.
    AllNodes {
        branch: String,
        snapshot_id: Option<String>,
    },
    /// Fetch diff between a snapshot and its parent.
    /// `parent_id` is provided by the caller from the cached ancestry data so the
    /// worker never needs to fetch the child snapshot — only the transaction log.
    SnapshotDiff {
        snapshot_id: String,
        parent_id: Option<String>,
    },
    /// Fetch chunk type statistics for an array node at a specific snapshot
    ChunkStats {
        snapshot_id: String,
        path: String,
    },
    /// Fetch repository configuration, status, and feature flags.
    RepoConfig,
    #[allow(dead_code)]
    OpsLog,
}

/// What the background worker sends back
#[derive(Debug)]
pub enum DataResponse {
    Branches(Result<Vec<BranchInfo>, String>),
    Tags(Result<Vec<TagInfo>, String>),
    Ancestry {
        branch: String,
        result: Result<Vec<SnapshotEntry>, String>,
    },
    /// All nodes organized by parent path. One response populates the entire node_children cache.
    AllNodes(Result<HashMap<String, Vec<TreeNode>>, String>),
    /// Unresolved diff: NodeId strings from the transaction log.
    /// Paths are resolved on the main thread using the node_children cache.
    SnapshotDiff {
        snapshot_id: String,
        result: Result<RawDiff, String>,
    },
    ChunkStats {
        snapshot_id: String,
        path: String,
        result: Result<ChunkStats, String>,
    },
    RepoConfig(Result<RepoConfig, String>),
    OpsLog(Result<Vec<OpsLogEntry>, String>),
}

/// Central data cache. Lives on the main thread.
/// Components read from here; background worker writes via channel.
pub struct DataStore {
    // Cached data
    pub branches: LoadState<Vec<BranchInfo>>,
    pub tags: LoadState<Vec<TagInfo>>,
    pub ancestry: HashMap<String, LoadState<Vec<SnapshotEntry>>>,
    pub node_children: HashMap<String, LoadState<Vec<TreeNode>>>,
    pub diffs: HashMap<String, LoadState<DiffSummary>>,
    pub chunk_stats: HashMap<(String, String), LoadState<ChunkStats>>,
    pub repo_config: LoadState<RepoConfig>,
    pub ops_log: LoadState<Vec<OpsLogEntry>>,

    // Channel for sending requests to background worker
    request_tx: mpsc::UnboundedSender<DataRequest>,
    // Channel for receiving responses from background worker
    response_rx: mpsc::UnboundedReceiver<DataResponse>,
}

impl DataStore {
    /// Create a new DataStore and spawn the background worker task.
    /// Returns the DataStore ready for use.
    pub fn new(repo: Repository) -> Self {
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (response_tx, response_rx) = mpsc::unbounded_channel();

        spawn_worker(Arc::new(repo), request_rx, response_tx);

        Self {
            branches: LoadState::NotRequested,
            tags: LoadState::NotRequested,
            ancestry: HashMap::new(),
            node_children: HashMap::new(),
            diffs: HashMap::new(),
            chunk_stats: HashMap::new(),
            repo_config: LoadState::NotRequested,
            ops_log: LoadState::NotRequested,
            request_tx,
            response_rx,
        }
    }

    /// Submit a data request to the background worker.
    /// Marks the corresponding cache entry as Loading only if no data is already
    /// loaded — this avoids flash-to-loading when refreshing existing data.
    pub fn submit(&mut self, request: DataRequest) {
        // Only transition to Loading if not already Loaded (avoids UI flash)
        macro_rules! set_loading {
            ($field:expr) => {
                if !$field.is_loaded() {
                    $field = LoadState::Loading;
                }
            };
        }
        macro_rules! set_loading_map {
            ($map:expr, $key:expr) => {
                if !$map.get(&$key).is_some_and(|s| s.is_loaded()) {
                    $map.insert($key, LoadState::Loading);
                }
            };
        }
        match &request {
            DataRequest::Branches => set_loading!(self.branches),
            DataRequest::Tags => set_loading!(self.tags),
            DataRequest::Ancestry { branch } => {
                set_loading_map!(self.ancestry, branch.clone());
            }
            DataRequest::AllNodes { .. } => {
                set_loading_map!(self.node_children, "/".to_string());
            }
            DataRequest::SnapshotDiff { snapshot_id, .. } => {
                set_loading_map!(self.diffs, snapshot_id.clone());
            }
            DataRequest::ChunkStats { snapshot_id, path } => {
                let key = (snapshot_id.clone(), path.clone());
                set_loading_map!(self.chunk_stats, key);
            }
            DataRequest::RepoConfig => set_loading!(self.repo_config),
            DataRequest::OpsLog => set_loading!(self.ops_log),
        }

        if let Err(e) = self.request_tx.send(request) {
            error!("Failed to send data request: {}", e);
        }
    }

    /// Drain pending responses from the background worker, up to `MAX_PER_FRAME`.
    /// Capping per-frame updates keeps the UI smooth even when many chunk-stats
    /// responses arrive simultaneously from high-concurrency scans.
    /// Returns true if any responses were processed (caller should notify components).
    pub fn drain_responses(&mut self) -> bool {
        const MAX_PER_FRAME: usize = 4;
        let mut had_responses = false;
        let mut count = 0;
        while count < MAX_PER_FRAME {
            match self.response_rx.try_recv() {
                Ok(response) => {
                    had_responses = true;
                    self.apply_response(response);
                    count += 1;
                }
                Err(_) => break,
            }
        }
        had_responses
    }

    /// Check if response channel has a message (for tokio::select wakeup)
    #[allow(dead_code)]
    pub async fn recv_response(&mut self) -> Option<DataResponse> {
        self.response_rx.recv().await
    }

    /// Resolve a NodeId string (Crockford Base32) to a path string using the
    /// node_children cache. Falls back to a `<node:ID>` placeholder if not found.
    fn node_id_to_path(&self, node_id: &str) -> String {
        for state in self.node_children.values() {
            if let LoadState::Loaded(nodes) = state
                && let Some(node) = nodes.iter().find(|n| n.node_id == node_id)
            {
                return node.path.clone();
            }
        }
        format!("<node:{}>", node_id)
    }

    fn apply_response(&mut self, response: DataResponse) {
        match response {
            DataResponse::Branches(result) => {
                self.branches = match result {
                    Ok(branches) => LoadState::Loaded(branches),
                    Err(e) => LoadState::Error(e),
                };
            }
            DataResponse::Tags(result) => {
                self.tags = match result {
                    Ok(tags) => LoadState::Loaded(tags),
                    Err(e) => LoadState::Error(e),
                };
            }
            DataResponse::Ancestry { branch, result } => {
                let state = match result {
                    Ok(entries) => LoadState::Loaded(entries),
                    Err(e) => LoadState::Error(e),
                };
                self.ancestry.insert(branch, state);
            }
            DataResponse::AllNodes(result) => {
                self.node_children.clear(); // Replace, don't merge
                match result {
                    Ok(children_by_parent) => {
                        for (parent_path, nodes) in children_by_parent {
                            self.node_children
                                .insert(parent_path, LoadState::Loaded(nodes));
                        }
                    }
                    Err(e) => {
                        self.node_children
                            .insert("/".to_string(), LoadState::Error(e));
                    }
                }
            }
            DataResponse::SnapshotDiff {
                snapshot_id,
                result,
            } => {
                let state = match result {
                    Ok(raw) => {
                        // Resolve NodeId strings to paths using the cached node_children.
                        // This is pure in-memory work — zero network calls.
                        let summary = DiffSummary {
                            snapshot_id: raw.snapshot_id,
                            parent_id: raw.parent_id,
                            added_arrays: raw
                                .added_array_ids
                                .iter()
                                .map(|id| self.node_id_to_path(id))
                                .collect(),
                            added_groups: raw
                                .added_group_ids
                                .iter()
                                .map(|id| self.node_id_to_path(id))
                                .collect(),
                            deleted_arrays: raw
                                .deleted_array_ids
                                .iter()
                                .map(|id| self.node_id_to_path(id))
                                .collect(),
                            deleted_groups: raw
                                .deleted_group_ids
                                .iter()
                                .map(|id| self.node_id_to_path(id))
                                .collect(),
                            modified_arrays: raw
                                .modified_array_ids
                                .iter()
                                .map(|id| self.node_id_to_path(id))
                                .collect(),
                            modified_groups: raw
                                .modified_group_ids
                                .iter()
                                .map(|id| self.node_id_to_path(id))
                                .collect(),
                            chunk_changes: raw
                                .chunk_change_ids
                                .iter()
                                .map(|(id, count)| (self.node_id_to_path(id), *count))
                                .collect(),
                            moved_nodes: raw
                                .moved_node_ids
                                .iter()
                                .map(|(_, from, to)| (sanitize(from), sanitize(to)))
                                .collect(),
                            is_initial_commit: raw.is_initial_commit,
                        };
                        LoadState::Loaded(summary)
                    }
                    Err(e) => LoadState::Error(e),
                };
                self.diffs.insert(snapshot_id, state);
            }
            DataResponse::ChunkStats {
                snapshot_id,
                path,
                result,
            } => {
                let state = match result {
                    Ok(stats) => LoadState::Loaded(stats),
                    Err(e) => LoadState::Error(e),
                };
                self.chunk_stats.insert((snapshot_id, path), state);
            }
            DataResponse::RepoConfig(result) => {
                self.repo_config = match result {
                    Ok(config) => LoadState::Loaded(config),
                    Err(e) => LoadState::Error(e),
                };
            }
            DataResponse::OpsLog(result) => {
                self.ops_log = match result {
                    Ok(entries) => LoadState::Loaded(entries),
                    Err(e) => LoadState::Error(e),
                };
            }
        }
    }
}

/// Spawn the background worker that processes data requests using the Repository.
fn spawn_worker(
    repo: Arc<Repository>,
    mut request_rx: mpsc::UnboundedReceiver<DataRequest>,
    response_tx: mpsc::UnboundedSender<DataResponse>,
) {
    tokio::spawn(async move {
        info!("Data store worker started");
        while let Some(request) = request_rx.recv().await {
            let repo = Arc::clone(&repo);
            let tx = response_tx.clone();
            // Each request gets its own task so they don't block each other
            tokio::spawn(async move {
                let response = process_request(&repo, request).await;
                let _ = tx.send(response);
            });
        }
        info!("Data store worker shutting down");
    });
}

/// Process a single data request against the repository
async fn process_request(repo: &Repository, request: DataRequest) -> DataResponse {
    match request {
        DataRequest::Branches => {
            let result = fetch_branches(repo).await;
            DataResponse::Branches(result)
        }
        DataRequest::Tags => {
            let result = fetch_tags(repo).await;
            DataResponse::Tags(result)
        }
        DataRequest::Ancestry { branch } => {
            let result = fetch_ancestry(repo, &branch).await;
            DataResponse::Ancestry { branch, result }
        }
        DataRequest::AllNodes {
            branch,
            snapshot_id,
        } => {
            let result = fetch_all_nodes(repo, &branch, snapshot_id.as_deref()).await;
            DataResponse::AllNodes(result)
        }
        DataRequest::SnapshotDiff {
            snapshot_id,
            parent_id,
        } => {
            let result = fetch_diff(repo, &snapshot_id, parent_id.as_deref()).await;
            DataResponse::SnapshotDiff {
                snapshot_id,
                result,
            }
        }
        DataRequest::ChunkStats { snapshot_id, path } => {
            let result = fetch_chunk_stats(repo, &snapshot_id, &path).await;
            DataResponse::ChunkStats {
                snapshot_id,
                path,
                result,
            }
        }
        DataRequest::RepoConfig => {
            let result = fetch_repo_config(repo).await;
            DataResponse::RepoConfig(result)
        }
        DataRequest::OpsLog => {
            let result = fetch_ops_log(repo).await;
            DataResponse::OpsLog(result)
        }
    }
}

async fn fetch_branches(repo: &Repository) -> Result<Vec<BranchInfo>, String> {
    let (repo_info, _) = repo.asset_manager().fetch_repo_info().await.map_err(|e| e.to_string())?;
    let mut result: Vec<BranchInfo> = repo_info
        .branches()
        .map_err(|e| e.to_string())?
        .map(|(name, snap_id)| {
            let (tip_timestamp, tip_message) = repo_info
                .find_snapshot(&snap_id)
                .map(|info| (Some(info.flushed_at), Some(sanitize(&info.message))))
                .unwrap_or((None, None));
            BranchInfo {
                name: sanitize(name),
                snapshot_id: snap_id.to_string(),
                tip_timestamp,
                tip_message,
            }
        })
        .collect();
    // Put "main" first, then alphabetical
    result.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
        ("main", _) => std::cmp::Ordering::Less,
        (_, "main") => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(result)
}

async fn fetch_tags(repo: &Repository) -> Result<Vec<TagInfo>, String> {
    let (repo_info, _) = repo.asset_manager().fetch_repo_info().await.map_err(|e| e.to_string())?;
    let result: Vec<TagInfo> = repo_info
        .tags()
        .map_err(|e| e.to_string())?
        .map(|(name, snap_id)| {
            let (tip_timestamp, tip_message) = repo_info
                .find_snapshot(&snap_id)
                .map(|info| (Some(info.flushed_at), Some(sanitize(&info.message))))
                .unwrap_or((None, None));
            TagInfo {
                name: sanitize(name),
                snapshot_id: snap_id.to_string(),
                tip_timestamp,
                tip_message,
            }
        })
        .collect();
    Ok(result)
}

async fn fetch_ancestry(repo: &Repository, branch: &str) -> Result<Vec<SnapshotEntry>, String> {
    let (repo_info, _) = repo.asset_manager().fetch_repo_info().await.map_err(|e| e.to_string())?;

    // Resolve branch to snapshot ID from repo info (in-memory, no network)
    let snapshot_id = repo_info.resolve_branch(branch).map_err(|e| e.to_string())?;

    // Walk ancestry in-memory. The current repo info file contains ALL snapshots
    // (each new file copies the full snapshot array + inserts the new one), so
    // ancestry() gives complete history with no chain-following needed.
    // The repo_before_updates linked list is for the ops log, not snapshots.
    let ancestry = repo_info.ancestry(&snapshot_id).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for result in ancestry {
        let info = result.map_err(|e| e.to_string())?;
        entries.push(SnapshotEntry {
            id: info.id.to_string(),
            parent_id: info.parent_id.map(|id| id.to_string()),
            timestamp: info.flushed_at,
            message: sanitize(&info.message),
        });
    }
    Ok(entries)
}

/// Fetch ALL nodes from root in a single request and organize them by parent path.
/// This populates the entire tree cache at once — expanding a group never needs another fetch.
/// If `snapshot_id` is Some, fetches the tree at that specific snapshot instead of the branch tip.
async fn fetch_all_nodes(
    repo: &Repository,
    branch: &str,
    snapshot_id: Option<&str>,
) -> Result<HashMap<String, Vec<TreeNode>>, String> {
    use icechunk::format::snapshot::NodeData;
    use icechunk::repository::VersionInfo;

    let version = if let Some(sid) = snapshot_id {
        use icechunk::format::SnapshotId;
        let snap_id: SnapshotId = sid.try_into().map_err(|e: &str| e.to_string())?;
        VersionInfo::SnapshotId(snap_id)
    } else {
        VersionInfo::BranchTipRef(branch.to_string())
    };
    let session = repo
        .readonly_session(&version)
        .await
        .map_err(|e| e.to_string())?;

    // Fetch the snapshot once so we can cross-reference manifest metadata for chunk counts.
    // This is the same snapshot already fetched internally by `readonly_session` — the asset
    // manager caches it, so this is effectively a free in-memory lookup.
    let snapshot = repo
        .asset_manager()
        .fetch_snapshot(session.snapshot_id())
        .await
        .map_err(|e| e.to_string())?;

    // Fetch ALL nodes from root — the snapshot has them sorted by path already
    let nodes_iter = session
        .list_nodes(&icechunk::format::Path::root())
        .await
        .map_err(|e| e.to_string())?;

    let mut children_by_parent: HashMap<String, Vec<TreeNode>> = HashMap::new();
    // Ensure root always has an entry even if empty
    children_by_parent.insert("/".to_string(), Vec::new());

    for node_result in nodes_iter {
        let node = node_result.map_err(|e| e.to_string())?;
        let path_str = node.path.to_string();

        // Skip the root node itself
        if path_str == "/" {
            continue;
        }

        // Derive parent path: everything up to the last '/'
        let parent_path = match path_str.rfind('/') {
            Some(0) => "/".to_string(),
            Some(idx) => path_str[..idx].to_string(),
            None => "/".to_string(),
        };

        let name = path_str.rsplit('/').next().unwrap_or("").to_string();

        let node_type = match &node.node_data {
            NodeData::Group => TreeNodeType::Group,
            NodeData::Array {
                shape,
                dimension_names,
                manifests,
            } => {
                let dims: Vec<u64> = shape.iter().map(|d| d.array_length()).collect();
                let dim_names = dimension_names.as_ref().map(|names| {
                    names
                        .iter()
                        .filter_map(|n| {
                            let opt: Option<String> = n.clone().into();
                            opt.map(|s| sanitize(&s))
                        })
                        .collect()
                });
                let zarr_metadata = String::from_utf8_lossy(&node.user_data).to_string();

                // Sum chunk refs across all manifests using snapshot metadata — zero extra fetches.
                let total_chunks: Option<u64> = {
                    let mut sum: u64 = 0;
                    let mut all_found = true;
                    for mref in manifests.iter() {
                        if let Ok(Some(info)) = snapshot.manifest_info(&mref.object_id) {
                            sum += info.num_chunk_refs as u64;
                        } else {
                            all_found = false;
                            break;
                        }
                    }
                    if all_found { Some(sum) } else { None }
                };

                TreeNodeType::Array(ArraySummary {
                    shape: dims,
                    dimension_names: dim_names,
                    manifest_count: manifests.len(),
                    zarr_metadata: sanitize(&zarr_metadata),
                    total_chunks,
                })
            }
        };

        children_by_parent
            .entry(parent_path)
            .or_default()
            .push(TreeNode {
                path: sanitize(&path_str),
                name: sanitize(&name),
                node_type,
                node_id: node.id.to_string(),
            });
    }

    Ok(children_by_parent)
}

/// Fetch chunk type statistics for an array node by iterating all its chunks.
/// Delegates to the shared implementation in output.rs.
async fn fetch_chunk_stats(
    repo: &Repository,
    snapshot_id: &str,
    path: &str,
) -> Result<ChunkStats, String> {
    crate::output::fetch_chunk_stats(repo, snapshot_id, path)
        .await
        .map_err(|e| e.to_string())
}

/// Fetch the diff between a snapshot and its parent using only the transaction log.
/// Costs exactly 1 S3 fetch (the transaction log). Path resolution is done on the
/// main thread using the already-cached node_children data — zero extra network calls.
async fn fetch_diff(
    repo: &Repository,
    snapshot_id: &str,
    parent_id: Option<&str>,
) -> Result<RawDiff, String> {
    use icechunk::format::SnapshotId;

    // Initial commit: no parent snapshot exists, so there is no transaction log to fetch.
    // Return an empty diff flagged as the initial commit so the UI can display it gracefully.
    if parent_id.is_none() {
        return Ok(RawDiff {
            snapshot_id: snapshot_id.to_string(),
            parent_id: None,
            added_array_ids: vec![],
            added_group_ids: vec![],
            deleted_array_ids: vec![],
            deleted_group_ids: vec![],
            modified_array_ids: vec![],
            modified_group_ids: vec![],
            chunk_change_ids: vec![],
            moved_node_ids: vec![],
            is_initial_commit: true,
        });
    }

    let snap_id: SnapshotId = snapshot_id.try_into().map_err(|e: &str| e.to_string())?;

    // The only S3 fetch: transaction log for this snapshot.
    let tx_log = repo
        .asset_manager()
        .fetch_transaction_log(&snap_id)
        .await
        .map_err(|e| e.to_string())?;

    let added_array_ids: Vec<String> = tx_log.new_arrays().map(|id| id.to_string()).collect();
    let added_group_ids: Vec<String> = tx_log.new_groups().map(|id| id.to_string()).collect();
    let deleted_array_ids: Vec<String> = tx_log.deleted_arrays().map(|id| id.to_string()).collect();
    let deleted_group_ids: Vec<String> = tx_log.deleted_groups().map(|id| id.to_string()).collect();
    let modified_array_ids: Vec<String> =
        tx_log.updated_arrays().map(|id| id.to_string()).collect();
    let modified_group_ids: Vec<String> =
        tx_log.updated_groups().map(|id| id.to_string()).collect();

    // updated_chunks gives (NodeId, Iterator<ChunkIndices>); capture (id_string, count).
    let chunk_change_ids: Vec<(String, usize)> = tx_log
        .updated_chunks()
        .map(|(node_id, chunks_iter)| (node_id.to_string(), chunks_iter.count()))
        .collect();

    // moves() yields Move { node_id, from, to } — paths are already resolved in the tx log.
    let moved_node_ids: Vec<(String, String, String)> = tx_log
        .moves()
        .filter_map(|r| r.ok())
        .map(|mv| (mv.node_id.to_string(), mv.from.to_string(), mv.to.to_string()))
        .collect();

    Ok(RawDiff {
        snapshot_id: snapshot_id.to_string(),
        parent_id: parent_id.map(|s| s.to_string()),
        added_array_ids,
        added_group_ids,
        deleted_array_ids,
        deleted_group_ids,
        modified_array_ids,
        modified_group_ids,
        chunk_change_ids,
        moved_node_ids,
        is_initial_commit: false,
    })
}

/// Fetch repository configuration, status, and feature flags.
/// Delegates to the shared implementation in output.rs.
async fn fetch_repo_config(repo: &Repository) -> Result<RepoConfig, String> {
    crate::output::fetch_repo_config(repo)
        .await
        .map_err(|e| e.to_string())
}

/// Fetch the repository operations log (mutation history).
/// Delegates to the shared implementation in output.rs.
async fn fetch_ops_log(repo: &Repository) -> Result<Vec<OpsLogEntry>, String> {
    crate::output::fetch_ops_log(repo, None)
        .await
        .map_err(|e| e.to_string())
}
