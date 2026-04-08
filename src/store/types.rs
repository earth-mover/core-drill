use chrono::{DateTime, Utc};
use serde::Serialize;

/// Branch with resolved tip info
#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub snapshot_id: String,
    pub tip_timestamp: Option<DateTime<Utc>>,
    pub tip_message: Option<String>,
}

/// Tag with resolved tip info
#[derive(Debug, Clone, Serialize)]
pub struct TagInfo {
    pub name: String,
    pub snapshot_id: String,
    pub tip_timestamp: Option<DateTime<Utc>>,
    pub tip_message: Option<String>,
}

/// Snapshot in an ancestry chain
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub message: String,
}

/// Node in the tree (group or array)
#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    pub path: String,
    pub name: String,
    pub node_type: TreeNodeType,
}

#[derive(Debug, Clone, Serialize)]
pub enum TreeNodeType {
    Group,
    Array(ArraySummary),
}

/// Summary of differences between two snapshots
#[derive(Debug, Clone, Serialize)]
pub struct DiffSummary {
    pub snapshot_id: String,
    pub parent_id: Option<String>,
    pub added_arrays: Vec<String>,
    pub added_groups: Vec<String>,
    pub deleted_arrays: Vec<String>,
    pub deleted_groups: Vec<String>,
    pub modified_arrays: Vec<String>,
    pub modified_groups: Vec<String>,
    pub chunk_changes: Vec<(String, usize)>,
}

/// Summary info for an array node
#[derive(Debug, Clone, Serialize)]
pub struct ArraySummary {
    pub shape: Vec<u64>,
    pub dimension_names: Option<Vec<String>>,
    pub manifest_count: usize,
    pub zarr_metadata: String,
}
