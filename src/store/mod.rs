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
    /// Marks the corresponding cache entry as Loading.
    pub fn submit(&mut self, request: DataRequest) {
        // Mark as loading
        match &request {
            DataRequest::Branches => self.branches = LoadState::Loading,
            DataRequest::Tags => self.tags = LoadState::Loading,
            DataRequest::Ancestry { branch } => {
                self.ancestry.insert(branch.clone(), LoadState::Loading);
            }
            DataRequest::AllNodes { .. } => {
                // Mark the root as loading; the response will populate all paths.
                self.node_children
                    .insert("/".to_string(), LoadState::Loading);
            }
            DataRequest::SnapshotDiff { snapshot_id, .. } => {
                self.diffs.insert(snapshot_id.clone(), LoadState::Loading);
            }
            DataRequest::ChunkStats { snapshot_id, path } => {
                self.chunk_stats
                    .insert((snapshot_id.clone(), path.clone()), LoadState::Loading);
            }
            DataRequest::RepoConfig => self.repo_config = LoadState::Loading,
            DataRequest::OpsLog => self.ops_log = LoadState::Loading,
        }

        if let Err(e) = self.request_tx.send(request) {
            error!("Failed to send data request: {}", e);
        }
    }

    /// Drain all pending responses from the background worker.
    /// Returns true if any responses were processed (caller should notify components).
    pub fn drain_responses(&mut self) -> bool {
        let mut had_responses = false;
        while let Ok(response) = self.response_rx.try_recv() {
            had_responses = true;
            self.apply_response(response);
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
    let branches = repo.list_branches().await.map_err(|e| e.to_string())?;

    let mut result = Vec::with_capacity(branches.len());
    for name in branches {
        let snapshot_id = repo
            .lookup_branch(&name)
            .await
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        result.push(BranchInfo {
            name: sanitize(&name),
            snapshot_id,
            tip_timestamp: None,
            tip_message: None,
        });
    }
    // Put "main" first so it's always visible at the top
    result.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
        ("main", _) => std::cmp::Ordering::Less,
        (_, "main") => std::cmp::Ordering::Greater,
        (a, b) => a.cmp(b),
    });
    Ok(result)
}

async fn fetch_tags(repo: &Repository) -> Result<Vec<TagInfo>, String> {
    let tags = repo.list_tags().await.map_err(|e| e.to_string())?;

    let mut result = Vec::with_capacity(tags.len());
    for name in tags {
        let snapshot_id = repo
            .lookup_tag(&name)
            .await
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        result.push(TagInfo {
            name: sanitize(&name),
            snapshot_id,
            tip_timestamp: None,
            tip_message: None,
        });
    }
    Ok(result)
}

async fn fetch_ancestry(repo: &Repository, branch: &str) -> Result<Vec<SnapshotEntry>, String> {
    use futures::StreamExt;
    use icechunk::repository::VersionInfo;

    let version = VersionInfo::BranchTipRef(branch.to_string());
    let stream = repo.ancestry(&version).await.map_err(|e| e.to_string())?;

    futures::pin_mut!(stream);
    let mut entries = Vec::new();
    while let Some(result) = stream.next().await {
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
async fn fetch_chunk_stats(
    repo: &Repository,
    snapshot_id: &str,
    path: &str,
) -> Result<ChunkStats, String> {
    use futures::StreamExt;
    use icechunk::format::SnapshotId;
    use icechunk::repository::VersionInfo;

    let snap_id: SnapshotId = snapshot_id.try_into().map_err(|e: &str| e.to_string())?;
    let version = VersionInfo::SnapshotId(snap_id);
    let session = repo
        .readonly_session(&version)
        .await
        .map_err(|e| e.to_string())?;
    let node_path = icechunk::format::Path::try_from(path)
        .map_err(|e: icechunk::format::PathError| e.to_string())?;

    let mut total = 0usize;
    let mut inline = 0usize;
    let mut inline_total_bytes = 0u64;
    let mut native = 0usize;
    let mut native_total_bytes = 0u64;
    let mut virtual_count = 0usize;
    let mut virtual_total_bytes = 0u64;
    let mut url_counts: HashMap<String, usize> = HashMap::new();

    let stream = session.array_chunk_iterator(&node_path).await;
    futures::pin_mut!(stream);

    while let Some(result) = stream.next().await {
        let chunk_info = result.map_err(|e| e.to_string())?;
        total += 1;
        match &chunk_info.payload {
            icechunk::format::manifest::ChunkPayload::Inline(bytes) => {
                inline += 1;
                inline_total_bytes += bytes.len() as u64;
            }
            icechunk::format::manifest::ChunkPayload::Ref(chunk_ref) => {
                native += 1;
                native_total_bytes += chunk_ref.length;
            }
            icechunk::format::manifest::ChunkPayload::Virtual(vref) => {
                virtual_count += 1;
                virtual_total_bytes += vref.length;
                // Extract URL prefix (everything up to and including the last '/')
                let url = vref.location.url();
                let prefix = url.rsplit_once('/').map(|x| x.0).unwrap_or(url).to_string();
                *url_counts.entry(prefix).or_insert(0) += 1;
            }
            _ => {}
        }
    }

    let virtual_source_count = url_counts.len();
    let mut virtual_prefixes: Vec<(String, usize)> = url_counts.into_iter().collect();
    virtual_prefixes.sort_by(|a, b| b.1.cmp(&a.1));
    virtual_prefixes.truncate(10);

    Ok(ChunkStats {
        total_chunks: total,
        inline_count: inline,
        inline_total_bytes,
        native_count: native,
        native_total_bytes,
        virtual_count,
        virtual_prefixes,
        virtual_source_count,
        virtual_total_bytes,
        stats_complete: true,
    })
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
        is_initial_commit: false,
    })
}

/// Fetch repository configuration, status, and feature flags.
async fn fetch_repo_config(repo: &Repository) -> Result<RepoConfig, String> {
    let config = repo.config();
    let spec_version = repo.spec_version().to_string();
    let inline_threshold = config.inline_chunk_threshold_bytes;

    // Fetch status
    let availability = match repo.get_status().await {
        Ok(status) => format!("{:?}", status.availability),
        Err(_) => "unknown".to_string(),
    };

    // Fetch feature flags
    let flags = match repo.feature_flags().await {
        Ok(iter) => iter
            .map(|f| FeatureFlagInfo {
                name: sanitize(f.name()),
                enabled: f.enabled(),
                explicit: f.setting().is_some(),
            })
            .collect(),
        Err(_) => vec![],
    };

    // Virtual chunk containers
    let vcc = config
        .virtual_chunk_containers
        .as_ref()
        .map(|containers| {
            containers
                .iter()
                .map(|(name, container)| (sanitize(name), sanitize(container.url_prefix())))
                .collect()
        })
        .unwrap_or_default();

    Ok(RepoConfig {
        spec_version,
        inline_chunk_threshold: inline_threshold,
        availability,
        feature_flags: flags,
        virtual_chunk_containers: vcc,
    })
}

/// Fetch the repository operations log (mutation history).
async fn fetch_ops_log(repo: &Repository) -> Result<Vec<OpsLogEntry>, String> {
    use futures::StreamExt;
    use icechunk::format::repo_info::UpdateType;

    let (stream, _repo_info, _version) = repo.ops_log().await.map_err(|e| e.to_string())?;
    futures::pin_mut!(stream);

    let mut entries = Vec::new();
    while let Some(result) = stream.next().await {
        let (timestamp, update_type, backup_path) = result.map_err(|e| e.to_string())?;

        let description = match &update_type {
            UpdateType::RepoInitializedUpdate => "Repository initialized".to_string(),
            UpdateType::RepoMigratedUpdate {
                from_version,
                to_version,
            } => format!("Migrated from v{from_version} to v{to_version}"),
            UpdateType::RepoStatusChangedUpdate { status } => {
                format!("Status changed to {status:?}")
            }
            UpdateType::ConfigChangedUpdate => "Configuration changed".to_string(),
            UpdateType::MetadataChangedUpdate => "Metadata changed".to_string(),
            UpdateType::TagCreatedUpdate { name } => format!("Tag created: {}", sanitize(name)),
            UpdateType::TagDeletedUpdate { name, .. } => {
                format!("Tag deleted: {}", sanitize(name))
            }
            UpdateType::BranchCreatedUpdate { name } => {
                format!("Branch created: {}", sanitize(name))
            }
            UpdateType::BranchDeletedUpdate { name, .. } => {
                format!("Branch deleted: {}", sanitize(name))
            }
            UpdateType::BranchResetUpdate { name, .. } => {
                format!("Branch reset: {}", sanitize(name))
            }
            UpdateType::NewCommitUpdate {
                branch,
                new_snap_id,
            } => format!(
                "Commit on {}: {}",
                sanitize(branch),
                &new_snap_id.to_string()[..12]
            ),
            UpdateType::CommitAmendedUpdate { branch, .. } => {
                format!("Commit amended on {}", sanitize(branch))
            }
            UpdateType::NewDetachedSnapshotUpdate { new_snap_id } => {
                format!("Detached snapshot: {}", &new_snap_id.to_string()[..12])
            }
            UpdateType::GCRanUpdate => "Garbage collection ran".to_string(),
            UpdateType::ExpirationRanUpdate => "Snapshot expiration ran".to_string(),
            UpdateType::FeatureFlagChanged { id, new_value } => {
                format!("Feature flag '{id}' → {new_value:?}")
            }
        };

        entries.push(OpsLogEntry {
            timestamp,
            description,
            backup_path,
        });
    }
    Ok(entries)
}
