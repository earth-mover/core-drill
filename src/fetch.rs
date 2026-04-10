//! Canonical fetch layer for all data-fetching operations.
//!
//! Every module that needs repository data (output.rs, mcp.rs, store/mod.rs)
//! delegates to functions here. This eliminates duplication and ensures
//! consistent error handling via `color_eyre::Result`.

use icechunk::Repository;

use crate::sanitize::sanitize;
use crate::store::types::*;
use crate::ui::format::ZarrMetadata;

// ─── Data structures for flat tree output ────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FlatNodeType {
    Group,
    Array,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct FlatNode {
    pub path: String,
    pub name: String,
    pub node_type: FlatNodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_shape: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_chunks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid_chunks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codecs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_value: Option<String>,
}

impl FlatNode {
    /// Shorthand for constructing a group node (all array fields are None).
    pub fn group(path: String, name: String) -> Self {
        Self {
            path,
            name,
            node_type: FlatNodeType::Group,
            shape: None,
            dtype: None,
            chunk_shape: None,
            dimensions: None,
            total_chunks: None,
            grid_chunks: None,
            codecs: None,
            fill_value: None,
        }
    }

    pub fn is_group(&self) -> bool {
        self.node_type == FlatNodeType::Group
    }
    pub fn is_array(&self) -> bool {
        self.node_type == FlatNodeType::Array
    }
}

impl std::fmt::Display for FlatNodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlatNodeType::Group => write!(f, "group"),
            FlatNodeType::Array => write!(f, "array"),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct RepoInfo {
    pub url: String,
    pub branch_count: usize,
    pub tag_count: usize,
    pub snapshot_count: usize,
    pub branches: Vec<BranchInfo>,
    pub tags: Vec<TagInfo>,
}

// ─── Data fetching (direct icechunk API, no DataStore) ───────

pub(crate) async fn fetch_repo_info(
    repo: &Repository,
    repo_url: &str,
) -> color_eyre::Result<RepoInfo> {
    let (branches, tags) = tokio::join!(fetch_branches(repo), fetch_tags(repo));
    let branches = branches?;
    let tags = tags?;
    // Get snapshot count from the main/first branch ancestry
    let main = branches
        .iter()
        .find(|b| b.name == "main")
        .or(branches.first());
    let snapshot_count = if let Some(branch) = main {
        fetch_ancestry(repo, &branch.name)
            .await
            .map(|a| a.len())
            .unwrap_or(0)
    } else {
        0
    };
    Ok(RepoInfo {
        url: repo_url.to_string(),
        branch_count: branches.len(),
        tag_count: tags.len(),
        snapshot_count,
        branches,
        tags,
    })
}

pub(crate) async fn fetch_branches(repo: &Repository) -> color_eyre::Result<Vec<BranchInfo>> {
    let (repo_info, _) = repo.asset_manager().fetch_repo_info().await?;
    let mut result: Vec<BranchInfo> = repo_info
        .branches()?
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
    // Sort: "main" first, then alphabetical
    result.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
        ("main", _) => std::cmp::Ordering::Less,
        (_, "main") => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(result)
}

pub(crate) async fn fetch_tags(repo: &Repository) -> color_eyre::Result<Vec<TagInfo>> {
    let (repo_info, _) = repo.asset_manager().fetch_repo_info().await?;
    let mut result: Vec<TagInfo> = repo_info
        .tags()?
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
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// Resolve a ref string to a VersionInfo. Tries: branch, tag, then snapshot ID.
pub(crate) async fn resolve_ref(
    repo: &Repository,
    r: &str,
) -> color_eyre::Result<icechunk::repository::VersionInfo> {
    use icechunk::format::SnapshotId;
    use icechunk::repository::VersionInfo;

    // Try branch first
    if repo.lookup_branch(r).await.is_ok() {
        return Ok(VersionInfo::BranchTipRef(r.to_string()));
    }
    // Try tag
    if repo.lookup_tag(r).await.is_ok() {
        return Ok(VersionInfo::TagRef(r.to_string()));
    }
    // Try exact snapshot ID (full 20-char Crockford Base32)
    if let Ok(snap_id) = r.try_into() {
        return Ok(VersionInfo::SnapshotId(snap_id));
    }

    // Try prefix match — like git short hashes.
    // Uses the repo info file (single cached fetch) which has all snapshot IDs.
    let r_upper = r.to_uppercase();
    if r_upper.len() >= 4 && r_upper.chars().all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)) {
        if let Ok((repo_info, _)) = repo.asset_manager().fetch_repo_info().await {
            let mut matches: Vec<SnapshotId> = Vec::new();
            if let Ok(snapshots) = repo_info.all_snapshots() {
                for snap_result in snapshots {
                    if let Ok(info) = snap_result {
                        let full_id: String = (&info.id).into();
                        if full_id.starts_with(&r_upper) {
                            matches.push(info.id);
                            if matches.len() > 1 {
                                color_eyre::eyre::bail!(
                                    "ambiguous snapshot prefix '{r}' — matches {} snapshots",
                                    matches.len()
                                );
                            }
                        }
                    }
                }
            }
            if matches.len() == 1 {
                return Ok(VersionInfo::SnapshotId(matches.remove(0)));
            }
        }
    }

    color_eyre::eyre::bail!("ref not found: '{r}' (not a branch, tag, or snapshot ID)")
}

/// Resolve a ref to a snapshot ID string.
pub(crate) async fn resolve_ref_to_snapshot_id(
    repo: &Repository,
    r: &str,
) -> color_eyre::Result<String> {
    let version = resolve_ref(repo, r).await?;
    match version {
        icechunk::repository::VersionInfo::SnapshotId(id) => Ok((&id).into()),
        _ => {
            let session = repo.readonly_session(&version).await?;
            Ok(session.snapshot_id().to_string())
        }
    }
}

pub(crate) async fn fetch_ancestry(
    repo: &Repository,
    r: &str,
) -> color_eyre::Result<Vec<SnapshotEntry>> {
    let (repo_info, _) = repo.asset_manager().fetch_repo_info().await?;

    // Resolve ref to snapshot ID using repo_info (no extra network calls)
    let snapshot_id = if let Ok(id) = repo_info.resolve_branch(r) {
        id
    } else if let Ok(id) = repo_info.resolve_tag(r) {
        id
    } else if let Ok(id) = r.try_into() {
        id
    } else {
        color_eyre::eyre::bail!("ref not found: '{r}' (not a branch, tag, or snapshot ID)");
    };

    // Walk ancestry in-memory. The current repo info file contains ALL snapshots
    // (each new file copies the full snapshot array + inserts the new one), so
    // ancestry() gives complete history with no chain-following needed.
    // The repo_before_updates linked list is for the ops log, not snapshots.
    let ancestry = repo_info.ancestry(&snapshot_id)?;
    let mut entries = Vec::new();
    for result in ancestry {
        let info = result?;
        entries.push(SnapshotEntry {
            id: info.id.to_string(),
            parent_id: info.parent_id.map(|id| id.to_string()),
            timestamp: info.flushed_at,
            message: sanitize(&info.message),
        });
    }
    Ok(entries)
}

pub(crate) async fn fetch_tree_flat(
    repo: &Repository,
    r: &str,
    path_filter: Option<&str>,
) -> color_eyre::Result<Vec<FlatNode>> {
    use icechunk::format::snapshot::NodeData;

    let version = resolve_ref(repo, r).await?;
    let session = repo.readonly_session(&version).await?;

    let snapshot = repo
        .asset_manager()
        .fetch_snapshot(session.snapshot_id())
        .await?;

    let nodes_iter = session.list_nodes(&icechunk::format::Path::root()).await?;

    let mut flat_nodes = Vec::new();

    for node_result in nodes_iter {
        let node = node_result?;
        let path_str = node.path.to_string();
        if path_str == "/" {
            continue;
        }

        let name = crate::util::leaf_name(&path_str).to_string();

        match &node.node_data {
            NodeData::Group => {
                flat_nodes.push(FlatNode::group(sanitize(&path_str), sanitize(&name)));
            }
            NodeData::Array {
                shape,
                dimension_names,
                manifests,
            } => {
                let dims: Vec<u64> = shape.iter().map(|d| d.array_length()).collect();
                let dim_names: Option<Vec<String>> = dimension_names.as_ref().map(|names| {
                    names
                        .iter()
                        .filter_map(|n| {
                            let opt: Option<String> = n.clone().into();
                            opt.map(|s| sanitize(&s))
                        })
                        .collect()
                });

                let zarr_metadata = String::from_utf8_lossy(&node.user_data).to_string();
                let meta = if !zarr_metadata.is_empty() {
                    ZarrMetadata::parse(&zarr_metadata)
                } else {
                    None
                };

                let chunk_shape_vec = meta.as_ref().map(|m| m.chunk_shape.clone());
                let dtype = meta.as_ref().map(|m| m.data_type.clone());
                let codecs = meta
                    .as_ref()
                    .map(|m| m.codec_chain_display())
                    .filter(|s| !s.is_empty());
                let fill_value = meta.as_ref().map(|m| m.fill_value.clone());

                // Total chunks from snapshot manifest metadata
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

                // Grid size: product of ceil(shape[i] / chunk_shape[i])
                let grid_chunks = meta.as_ref().and_then(|m| {
                    if dims.is_empty()
                        || m.chunk_shape.is_empty()
                        || dims.len() != m.chunk_shape.len()
                    {
                        return None;
                    }
                    dims.iter()
                        .zip(m.chunk_shape.iter())
                        .try_fold(1u64, |acc, (s, c)| {
                            if *c == 0 {
                                return None;
                            }
                            acc.checked_mul(s.div_ceil(*c))
                        })
                });

                flat_nodes.push(FlatNode {
                    path: sanitize(&path_str),
                    name: sanitize(&name),
                    node_type: FlatNodeType::Array,
                    shape: Some(dims.clone()),
                    dtype,
                    chunk_shape: chunk_shape_vec,
                    dimensions: dim_names,
                    total_chunks,
                    grid_chunks,
                    codecs,
                    fill_value,
                });
            }
        }
    }

    // Apply path filter if specified: exact match, children (/path/...), or prefix (/path*)
    if let Some(filter) = path_filter {
        flat_nodes.retain(|n| n.path == filter || n.path.starts_with(filter));
    }

    Ok(flat_nodes)
}

pub(crate) async fn fetch_ops_log(
    repo: &Repository,
    limit: Option<usize>,
) -> color_eyre::Result<Vec<OpsLogEntry>> {
    use futures::StreamExt;
    use icechunk::format::repo_info::UpdateType;

    let (stream, _repo_info, _version) = repo.ops_log().await?;
    futures::pin_mut!(stream);

    let max = limit.unwrap_or(usize::MAX);
    let mut entries = Vec::new();
    while let Some(result) = stream.next().await {
        let (timestamp, update_type, backup_path) = result?;

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
                crate::output::truncate(&new_snap_id.to_string(), 12)
            ),
            UpdateType::CommitAmendedUpdate { branch, .. } => {
                format!("Commit amended on {}", sanitize(branch))
            }
            UpdateType::NewDetachedSnapshotUpdate { new_snap_id } => {
                format!("Detached snapshot: {}", crate::output::truncate(&new_snap_id.to_string(), 12))
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

        if entries.len() >= max {
            break;
        }
    }
    Ok(entries)
}

/// Fetch chunk type statistics for an array node by iterating all its chunks.
pub(crate) async fn fetch_chunk_stats(
    repo: &Repository,
    snapshot_id: &str,
    path: &str,
) -> color_eyre::Result<ChunkStats> {
    use futures::StreamExt;
    use icechunk::format::SnapshotId;
    use icechunk::repository::VersionInfo;
    use std::collections::HashMap;

    let snap_id: SnapshotId = snapshot_id.try_into().map_err(|e: &str| color_eyre::eyre::eyre!(e))?;
    let version = VersionInfo::SnapshotId(snap_id);
    let session = repo.readonly_session(&version).await?;
    let node_path = icechunk::format::Path::try_from(path)?;

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
        let chunk_info = result?;
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

/// Extracted node ID vectors from a transaction log, used by both the TUI (RawDiff)
/// and the CLI/MCP (DiffSummary) fetch paths.
pub(crate) struct TxLogIds {
    pub added_array_ids: Vec<String>,
    pub added_group_ids: Vec<String>,
    pub deleted_array_ids: Vec<String>,
    pub deleted_group_ids: Vec<String>,
    pub modified_array_ids: Vec<String>,
    pub modified_group_ids: Vec<String>,
    pub chunk_change_ids: Vec<(String, usize)>,
}

/// Extract node IDs from a transaction log into categorized vectors.
/// Moves are NOT included here because they have different shapes in each call site
/// (store/mod.rs needs a 3-tuple with node_id; fetch.rs only needs from/to paths).
pub(crate) fn extract_tx_log_ids(
    tx_log: &icechunk::format::transaction_log::TransactionLog,
) -> TxLogIds {
    TxLogIds {
        added_array_ids: tx_log.new_arrays().map(|id| id.to_string()).collect(),
        added_group_ids: tx_log.new_groups().map(|id| id.to_string()).collect(),
        deleted_array_ids: tx_log.deleted_arrays().map(|id| id.to_string()).collect(),
        deleted_group_ids: tx_log.deleted_groups().map(|id| id.to_string()).collect(),
        modified_array_ids: tx_log.updated_arrays().map(|id| id.to_string()).collect(),
        modified_group_ids: tx_log.updated_groups().map(|id| id.to_string()).collect(),
        chunk_change_ids: tx_log
            .updated_chunks()
            .map(|(node_id, chunks_iter)| (node_id.to_string(), chunks_iter.count()))
            .collect(),
    }
}

/// Fetch the diff for a snapshot with resolved paths (for CLI/MCP output).
/// If `parent_id` is `None`, auto-resolves the parent from ancestry.
/// Returns an initial-commit marker only when the snapshot truly has no parent.
pub(crate) async fn fetch_diff(
    repo: &Repository,
    snapshot_id: &str,
    parent_id: Option<&str>,
) -> color_eyre::Result<DiffSummary> {
    use futures::StreamExt;
    use std::collections::HashMap;

    // Resolve snapshot ref (supports prefix matching like git short hashes)
    let version = resolve_ref(repo, snapshot_id).await?;
    // Reuse session if we need one to extract the snapshot ID (avoids redundant
    // readonly_session later when we list nodes).
    let (snap_id, existing_session) = match &version {
        icechunk::repository::VersionInfo::SnapshotId(id) => (id.clone(), None),
        _ => {
            let session = repo.readonly_session(&version).await?;
            let id = session.snapshot_id().clone();
            (id, Some(session))
        }
    };
    let full_snapshot_id: String = (&snap_id).into();

    // If no parent_id provided, resolve it from ancestry
    let parent_id = match parent_id {
        Some(pid) => Some(pid.to_string()),
        None => {
            let stream = repo.ancestry(&version).await?;
            futures::pin_mut!(stream);
            stream
                .next()
                .await
                .and_then(|r| r.ok())
                .and_then(|info| info.parent_id.map(|id| id.to_string()))
        }
    };

    if parent_id.is_none() {
        return Ok(DiffSummary {
            snapshot_id: full_snapshot_id,
            parent_id: None,
            added_arrays: vec![],
            added_groups: vec![],
            deleted_arrays: vec![],
            deleted_groups: vec![],
            modified_arrays: vec![],
            modified_groups: vec![],
            chunk_changes: vec![],
            moved_nodes: vec![],
            is_initial_commit: true,
        });
    }

    // Fetch tx_log and session in parallel — they are independent S3 fetches.
    let (tx_log_result, session) = tokio::join!(
        repo.asset_manager().fetch_transaction_log(&snap_id),
        async {
            match existing_session {
                Some(s) => Ok(s),
                None => {
                    let snap_version =
                        icechunk::repository::VersionInfo::SnapshotId(snap_id.clone());
                    repo.readonly_session(&snap_version).await
                }
            }
        }
    );
    let tx_log = tx_log_result?;
    let session = session?;

    let raw_ids = extract_tx_log_ids(&tx_log);

    // moves() yields Move { node_id, from, to } — paths are already resolved in the tx log.
    let moved: Vec<(String, String)> = tx_log
        .moves()
        .filter_map(|r| r.ok())
        .map(|mv| (sanitize(&mv.from.to_string()), sanitize(&mv.to.to_string())))
        .collect();

    // Build NodeId→Path map by listing all nodes at this snapshot
    let nodes_iter = session.list_nodes(&icechunk::format::Path::root()).await?;

    let mut id_to_path: HashMap<String, String> = HashMap::new();
    for node_result in nodes_iter {
        let node = node_result?;
        id_to_path.insert(node.id.to_string(), sanitize(&node.path.to_string()));
    }

    let resolve = |id: &str| -> String {
        id_to_path
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("<node:{}>", id))
    };

    Ok(DiffSummary {
        snapshot_id: full_snapshot_id,
        parent_id,
        added_arrays: raw_ids.added_array_ids.iter().map(|id| resolve(id)).collect(),
        added_groups: raw_ids.added_group_ids.iter().map(|id| resolve(id)).collect(),
        deleted_arrays: raw_ids.deleted_array_ids.iter().map(|id| resolve(id)).collect(),
        deleted_groups: raw_ids.deleted_group_ids.iter().map(|id| resolve(id)).collect(),
        modified_arrays: raw_ids.modified_array_ids.iter().map(|id| resolve(id)).collect(),
        modified_groups: raw_ids.modified_group_ids.iter().map(|id| resolve(id)).collect(),
        chunk_changes: raw_ids.chunk_change_ids
            .iter()
            .map(|(id, count)| (resolve(id), *count))
            .collect(),
        moved_nodes: moved,
        is_initial_commit: false,
    })
}

/// Fetch repository configuration, status, and feature flags.
pub(crate) async fn fetch_repo_config(
    repo: &Repository,
) -> color_eyre::Result<RepoConfig> {
    let config = repo.config();
    let spec_version = repo.spec_version().to_string();
    let inline_threshold = config.inline_chunk_threshold_bytes;

    let availability = match repo.get_status().await {
        Ok(status) => format!("{:?}", status.availability),
        Err(_) => "unknown".to_string(),
    };

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
