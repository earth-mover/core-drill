pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use icechunk::Repository;
use tokio::sync::mpsc;
use tracing::{error, info};

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
            name,
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
            name,
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
            message: info.message.clone(),
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
                            opt
                        })
                        .collect()
                });
                let zarr_metadata = String::from_utf8_lossy(&node.user_data).to_string();

                TreeNodeType::Array(ArraySummary {
                    shape: dims,
                    dimension_names: dim_names,
                    manifest_count: manifests.len(),
                    zarr_metadata,
                })
            }
        };

        children_by_parent
            .entry(parent_path)
            .or_default()
            .push(TreeNode {
                path: path_str,
                name,
                node_type,
            });
    }

    Ok(children_by_parent)
}

/// Fetch the diff between a snapshot and its parent.
/// If the snapshot has no parent (initial snapshot), returns an error message.
async fn fetch_diff(
    repo: &Repository,
    branch: &str,
    snapshot_id: &str,
) -> Result<DiffSummary, String> {
    use icechunk::repository::VersionInfo;

    // Look up ancestry to find the parent of this snapshot
    let ancestry = fetch_ancestry(repo, branch).await?;

    let entry = ancestry
        .iter()
        .find(|e| e.id == snapshot_id)
        .ok_or_else(|| format!("Snapshot {} not found in ancestry", snapshot_id))?;

    let parent_id = entry
        .parent_id
        .as_ref()
        .ok_or_else(|| "Initial snapshot has no parent to diff against".to_string())?;

    let from_snap_id: icechunk::format::SnapshotId =
        parent_id.as_str().try_into().map_err(|e: &str| e.to_string())?;
    let to_snap_id: icechunk::format::SnapshotId =
        snapshot_id.try_into().map_err(|e: &str| e.to_string())?;

    let from_version = VersionInfo::SnapshotId(from_snap_id);
    let to_version = VersionInfo::SnapshotId(to_snap_id);

    let diff = repo
        .diff(&from_version, &to_version)
        .await
        .map_err(|e| e.to_string())?;

    Ok(DiffSummary {
        snapshot_id: snapshot_id.to_string(),
        parent_id: Some(parent_id.clone()),
        added_arrays: diff.new_arrays.iter().map(|p| p.to_string()).collect(),
        added_groups: diff.new_groups.iter().map(|p| p.to_string()).collect(),
        deleted_arrays: diff.deleted_arrays.iter().map(|p| p.to_string()).collect(),
        deleted_groups: diff.deleted_groups.iter().map(|p| p.to_string()).collect(),
        modified_arrays: diff.updated_arrays.iter().map(|p| p.to_string()).collect(),
        modified_groups: diff.updated_groups.iter().map(|p| p.to_string()).collect(),
        chunk_changes: diff
            .updated_chunks
            .iter()
            .map(|(p, chunks)| (p.to_string(), chunks.len()))
            .collect(),
    })
}
