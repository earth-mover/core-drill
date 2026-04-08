# Icechunk Rust API — Lessons & Cookbook

Reference: https://docs.rs/icechunk/2.0.0-alpha.7/icechunk/

## Opening a Repository

```rust
use icechunk::Repository;
let storage = icechunk::new_local_filesystem_storage(&path).await?;
let repo = Repository::open(None, storage, HashMap::new()).await?;
```

For S3 anonymous access:
```rust
use icechunk::storage::{S3Credentials, S3Options, new_s3_storage};

let config = S3Options {
    region: Some("us-east-1".to_string()),
    anonymous: true,
    // must specify all fields — S3Options does NOT impl Default
    endpoint_url: None,
    allow_http: false,
    force_path_style: false,
    network_stream_timeout_seconds: None,
    requester_pays: false,
};
let storage = new_s3_storage(config, bucket, prefix, Some(S3Credentials::Anonymous))?;
```

## Cost Model (cheap -> expensive)

1. **Repo info** (single fetch): branches, tags, all snapshot metadata, ancestry graph, ops log, status
2. **Snapshot file** (one per snapshot): node tree, array metadata, manifest refs
3. **Manifest file** (one per manifest): chunk locations
4. **Chunk files**: actual data — avoid

## AllNodes Fetch Pattern

The most efficient way to populate a tree view is a single `list_nodes` call from root:

```rust
let session = repo.readonly_session(&version).await?;
let nodes_iter = session.list_nodes(&icechunk::format::Path::root()).await?;
```

This returns **all descendants** (not just direct children) as an iterator. Nodes are
sorted by path, so you can organize them by parent path in a single pass:

```rust
let mut children_by_parent: HashMap<String, Vec<TreeNode>> = HashMap::new();
for node in nodes_iter {
    let parent_path = derive_parent(&node.path);
    children_by_parent.entry(parent_path).or_default().push(to_tree_node(node));
}
```

This populates the entire tree cache in one request. Expanding a group in the UI
never needs another fetch — children are already cached.

## Caching

AssetManager handles caching internally for snapshots, manifests, transaction logs, and chunks. No need to cache these ourselves.

**We should cache:** SnapshotInfo ancestry chains, node tree listings, diff results, computed branch->snapshot mappings.

## Diff API

### Basic usage

```rust
use icechunk::repository::VersionInfo;
use icechunk::format::SnapshotId;

let from_id: SnapshotId = parent_id_str.try_into()?;
let to_id: SnapshotId = snapshot_id_str.try_into()?;

let diff = repo
    .diff(&VersionInfo::SnapshotId(from_id), &VersionInfo::SnapshotId(to_id))
    .await?;
```

The result contains `new_arrays`, `new_groups`, `deleted_arrays`, `deleted_groups`,
`updated_arrays`, `updated_groups`, and `updated_chunks`.

### Gotcha: ancestry requirement

`Repository::diff(from, to)` requires `from` to be an **ancestor** of `to`.
Returns `BadSnapshotChainForDiff` otherwise. This means you must look up the
parent snapshot ID from the ancestry chain first — you cannot diff arbitrary
unrelated snapshots.

### VersionInfo::SnapshotId for diff lookups

To diff a specific snapshot against its parent, use `VersionInfo::SnapshotId(id)` for
both the `from` and `to` parameters. Do not use `BranchTipRef` or `TagRef` here — those
resolve to the current tip, not a historical snapshot.

## Gotchas & Issues for Upstream Docs

### S3Options doesn't implement Default
`S3Options` requires all fields to be specified manually. Would be nice to have `Default` impl or a builder pattern. Filed mentally for upstream contribution.

### VersionInfo is the universal ref resolver
`VersionInfo::BranchTipRef(name)`, `VersionInfo::TagRef(name)`, `VersionInfo::SnapshotId(id)`, `VersionInfo::AsOf { branch, at }` — use this everywhere instead of raw IDs.

### list_nodes returns Iterator not Stream
`Session::list_nodes()` returns `impl Iterator`, not `impl Stream`. This is synchronous iteration over the in-memory snapshot data. No need for async handling. Note: `list_nodes` at root returns **all descendants**, not just direct children.

### Repository is Clone (Arc internally)
Safe to clone and move into spawned tasks.
