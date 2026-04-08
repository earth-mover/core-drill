pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use icechunk::Repository;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::sanitize::sanitize;

pub use types::*;

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
}

/// What components can request from the background worker
#[derive(Debug)]
pub enum DataRequest {
    Branches,
    Tags,
    Ancestry { branch: String },
    /// Fetch ALL nodes in the tree for a branch in a single request.
    /// Results are organized by parent path so every group's children are cached at once.
    AllNodes { branch: String },
    /// Fetch diff between a snapshot and its parent
    SnapshotDiff { branch: String, snapshot_id: String },
    /// Fetch chunk type statistics for an array node
    ChunkStats { branch: String, path: String },
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
    SnapshotDiff {
        snapshot_id: String,
        result: Result<DiffSummary, String>,
    },
    ChunkStats {
        path: String,
        result: Result<ChunkStats, String>,
    },
    OpsLog(Result<Vec<String>, String>),
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
    pub chunk_stats: HashMap<String, LoadState<ChunkStats>>,
    pub ops_log: LoadState<Vec<String>>,

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
                self.node_children.insert("/".to_string(), LoadState::Loading);
            }
            DataRequest::SnapshotDiff { snapshot_id, .. } => {
                self.diffs.insert(snapshot_id.clone(), LoadState::Loading);
            }
            DataRequest::ChunkStats { path, .. } => {
                self.chunk_stats.insert(path.clone(), LoadState::Loading);
            }
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
                match result {
                    Ok(children_by_parent) => {
                        for (parent_path, nodes) in children_by_parent {
                            self.node_children.insert(parent_path, LoadState::Loaded(nodes));
                        }
                    }
                    Err(e) => {
                        self.node_children.insert("/".to_string(), LoadState::Error(e));
                    }
                }
            }
            DataResponse::SnapshotDiff { snapshot_id, result } => {
                let state = match result {
                    Ok(summary) => LoadState::Loaded(summary),
                    Err(e) => LoadState::Error(e),
                };
                self.diffs.insert(snapshot_id, state);
            }
            DataResponse::ChunkStats { path, result } => {
                let state = match result {
                    Ok(stats) => LoadState::Loaded(stats),
                    Err(e) => LoadState::Error(e),
                };
                self.chunk_stats.insert(path, state);
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
            DataResponse::Ancestry {
                branch,
                result,
            }
        }
        DataRequest::AllNodes { branch } => {
            let result = fetch_all_nodes(repo, &branch).await;
            DataResponse::AllNodes(result)
        }
        DataRequest::SnapshotDiff { branch, snapshot_id } => {
            let result = fetch_diff(repo, &branch, &snapshot_id).await;
            DataResponse::SnapshotDiff {
                snapshot_id,
                result,
            }
        }
        DataRequest::ChunkStats { branch, path } => {
            let result = fetch_chunk_stats(repo, &branch, &path).await;
            DataResponse::ChunkStats { path, result }
        }
        DataRequest::OpsLog => {
            // TODO: implement ops log fetching
            DataResponse::OpsLog(Ok(vec![]))
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
async fn fetch_all_nodes(
    repo: &Repository,
    branch: &str,
) -> Result<HashMap<String, Vec<TreeNode>>, String> {
    use icechunk::format::snapshot::NodeData;
    use icechunk::repository::VersionInfo;

    let version = VersionInfo::BranchTipRef(branch.to_string());
    let session = repo.readonly_session(&version).await.map_err(|e| e.to_string())?;

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
            NodeData::Array { shape, dimension_names, manifests } => {
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

                TreeNodeType::Array(ArraySummary {
                    shape: dims,
                    dimension_names: dim_names,
                    manifest_count: manifests.len(),
                    zarr_metadata: sanitize(&zarr_metadata),
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
            });
    }

    Ok(children_by_parent)
}

/// Fetch chunk type statistics for an array node by iterating all its chunks.
async fn fetch_chunk_stats(repo: &Repository, branch: &str, path: &str) -> Result<ChunkStats, String> {
    use futures::StreamExt;
    use icechunk::repository::VersionInfo;

    let version = VersionInfo::BranchTipRef(branch.to_string());
    let session = repo.readonly_session(&version).await.map_err(|e| e.to_string())?;
    let node_path = icechunk::format::Path::try_from(path).map_err(|e: icechunk::format::PathError| e.to_string())?;

    let mut total = 0usize;
    let mut inline = 0usize;
    let mut native = 0usize;
    let mut virtual_count = 0usize;
    let mut virtual_total_bytes = 0u64;
    let mut url_counts: HashMap<String, usize> = HashMap::new();

    let stream = session.array_chunk_iterator(&node_path).await;
    futures::pin_mut!(stream);

    while let Some(result) = stream.next().await {
        let chunk_info = result.map_err(|e| e.to_string())?;
        total += 1;
        match &chunk_info.payload {
            icechunk::format::manifest::ChunkPayload::Inline(_) => inline += 1,
            icechunk::format::manifest::ChunkPayload::Ref(_) => native += 1,
            icechunk::format::manifest::ChunkPayload::Virtual(vref) => {
                virtual_count += 1;
                virtual_total_bytes += vref.length;
                // Extract URL prefix (everything up to and including the last '/')
                let url = vref.location.url();
                let prefix = url.rsplitn(2, '/').nth(1).unwrap_or(url).to_string();
                *url_counts.entry(prefix).or_insert(0) += 1;
            }
            _ => {}
        }
    }

    let mut virtual_prefixes: Vec<(String, usize)> = url_counts.into_iter().collect();
    virtual_prefixes.sort_by(|a, b| b.1.cmp(&a.1));
    virtual_prefixes.truncate(5);

    Ok(ChunkStats {
        total_chunks: total,
        inline_count: inline,
        native_count: native,
        virtual_count,
        virtual_prefixes,
        virtual_total_bytes,
    })
}

/// Fetch the diff between a snapshot and its parent using the transaction log directly.
/// This requires only 2 S3 fetches: the child snapshot + the transaction log.
/// (Previously required N ancestry fetches + 2 full snapshot fetches via Repository::diff().)
async fn fetch_diff(
    repo: &Repository,
    _branch: &str,
    snapshot_id: &str,
) -> Result<DiffSummary, String> {
    use icechunk::format::SnapshotId;

    let snap_id: SnapshotId =
        snapshot_id.try_into().map_err(|e: &str| e.to_string())?;

    // Fetch 1: child snapshot — gives us parent_id AND node table for path resolution.
    let snapshot = repo
        .asset_manager()
        .fetch_snapshot(&snap_id)
        .await
        .map_err(|e| e.to_string())?;

    // parent_id() is deprecated since 2.0 for V2 repos (parent is encoded differently),
    // but it is still the correct way to get the parent for both V1 and V2 snapshots.
    #[expect(deprecated)]
    let parent_id = match snapshot.parent_id() {
        Some(pid) => pid,
        None => return Err("Initial snapshot — no parent to diff against".to_string()),
    };

    // Build NodeId -> Path map from the child snapshot's node table.
    // We use the string representation of NodeId (Crockford Base32, 13 chars) as the key.
    let mut node_paths: HashMap<String, String> = HashMap::new();
    for node_result in snapshot.iter() {
        let node = node_result.map_err(|e| e.to_string())?;
        node_paths.insert(node.id.to_string(), node.path.to_string());
    }

    // Fetch 2: transaction log for this snapshot.
    let tx_log = repo
        .asset_manager()
        .fetch_transaction_log(&snap_id)
        .await
        .map_err(|e| e.to_string())?;

    // Helper: resolve a NodeId to a path string, falling back to a placeholder.
    let resolve = |node_id: &icechunk::format::NodeId| -> String {
        let key = node_id.to_string();
        match node_paths.get(&key) {
            Some(path) => sanitize(path),
            None => format!("<node:{}>", key),
        }
    };

    let added_arrays: Vec<String> = tx_log.new_arrays().map(|id| resolve(&id)).collect();
    let added_groups: Vec<String> = tx_log.new_groups().map(|id| resolve(&id)).collect();
    let deleted_arrays: Vec<String> =
        tx_log.deleted_arrays().map(|id| resolve(&id)).collect();
    let deleted_groups: Vec<String> =
        tx_log.deleted_groups().map(|id| resolve(&id)).collect();
    let modified_arrays: Vec<String> =
        tx_log.updated_arrays().map(|id| resolve(&id)).collect();
    let modified_groups: Vec<String> =
        tx_log.updated_groups().map(|id| resolve(&id)).collect();

    // updated_chunks gives (NodeId, Iterator<ChunkIndices>); we need (path, count).
    let chunk_changes: Vec<(String, usize)> = tx_log
        .updated_chunks()
        .map(|(node_id, chunks_iter)| {
            let path = resolve(&node_id);
            let count = chunks_iter.count();
            (path, count)
        })
        .collect();

    Ok(DiffSummary {
        snapshot_id: snapshot_id.to_string(),
        parent_id: Some(parent_id.to_string()),
        added_arrays,
        added_groups,
        deleted_arrays,
        deleted_groups,
        modified_arrays,
        modified_groups,
        chunk_changes,
    })
}
