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
    NodeChildren { branch: String, parent_path: String },
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
    NodeChildren {
        parent_path: String,
        result: Result<Vec<TreeNode>, String>,
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
            DataRequest::NodeChildren { parent_path, .. } => {
                self.node_children.insert(parent_path.clone(), LoadState::Loading);
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
            DataResponse::NodeChildren { parent_path, result } => {
                let state = match result {
                    Ok(nodes) => LoadState::Loaded(nodes),
                    Err(e) => LoadState::Error(e),
                };
                self.node_children.insert(parent_path, state);
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
        DataRequest::NodeChildren { branch, parent_path } => {
            let result = fetch_node_children(repo, &branch, &parent_path).await;
            DataResponse::NodeChildren {
                parent_path,
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

async fn fetch_node_children(
    repo: &Repository,
    branch: &str,
    parent_path: &str,
) -> Result<Vec<TreeNode>, String> {
    use icechunk::format::snapshot::NodeData;
    use icechunk::repository::VersionInfo;

    let version = VersionInfo::BranchTipRef(branch.to_string());
    let session = repo.readonly_session(&version).await.map_err(|e| e.to_string())?;
    let path = icechunk::format::Path::root();
    // If parent_path is not root, parse it
    let path = if parent_path == "/" || parent_path.is_empty() {
        path
    } else {
        icechunk::format::Path::new(parent_path).map_err(|e| e.to_string())?
    };
    let nodes_iter = session.list_nodes(&path).await.map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for node_result in nodes_iter {
        let node = node_result.map_err(|e| e.to_string())?;
        let path_str = node.path.to_string();
        let name = path_str.rsplit('/').next().unwrap_or("").to_string();

        let node_type = match &node.node_data {
            NodeData::Group => TreeNodeType::Group,
            NodeData::Array { shape, dimension_names, manifests } => {
                let dims: Vec<u64> = shape.iter().map(|d| d.array_length()).collect();
                let dim_names = dimension_names.as_ref().map(|names| {
                    names.iter().filter_map(|n| {
                        let opt: Option<String> = n.clone().into();
                        opt
                    }).collect()
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

        result.push(TreeNode {
            path: path_str,
            name,
            node_type,
        });
    }
    Ok(result)
}
