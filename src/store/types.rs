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
    /// Crockford Base32 NodeId string, used for diff resolution without a snapshot fetch.
    pub node_id: String,
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

/// Unresolved diff: NodeId strings straight from the transaction log.
/// Paths are resolved on the main thread using the node_children cache.
#[derive(Debug, Clone)]
pub struct RawDiff {
    pub snapshot_id: String,
    pub parent_id: Option<String>,
    pub added_array_ids: Vec<String>,
    pub added_group_ids: Vec<String>,
    pub deleted_array_ids: Vec<String>,
    pub deleted_group_ids: Vec<String>,
    pub modified_array_ids: Vec<String>,
    pub modified_group_ids: Vec<String>,
    /// (node_id, chunk_count)
    pub chunk_change_ids: Vec<(String, usize)>,
}

/// Summary info for an array node
#[derive(Debug, Clone, Serialize)]
pub struct ArraySummary {
    pub shape: Vec<u64>,
    pub dimension_names: Option<Vec<String>>,
    pub manifest_count: usize,
    pub zarr_metadata: String,
}

/// Chunk type breakdown for an array
#[derive(Debug, Clone, Serialize)]
pub struct ChunkStats {
    pub total_chunks: usize,
    pub inline_count: usize,
    pub native_count: usize,
    pub virtual_count: usize,
    /// Common URL prefixes for virtual chunks, with counts
    pub virtual_prefixes: Vec<(String, usize)>,
    /// Total size of virtual chunks in bytes (sum of length fields)
    pub virtual_total_bytes: u64,
    /// False when only total_chunks is known (fast path, no manifest fetches).
    /// True when inline/native/virtual breakdown is fully populated.
    pub stats_complete: bool,
}
