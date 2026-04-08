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

## Cost Model (cheap → expensive)

1. **Repo info** (single fetch): branches, tags, all snapshot metadata, ancestry graph, ops log, status
2. **Snapshot file** (one per snapshot): node tree, array metadata, manifest refs
3. **Manifest file** (one per manifest): chunk locations
4. **Chunk files**: actual data — avoid

## Caching

AssetManager handles caching internally for snapshots, manifests, transaction logs, and chunks. No need to cache these ourselves.

**We should cache:** SnapshotInfo ancestry chains, node tree listings, diff results, computed branch→snapshot mappings.

## Gotchas & Issues for Upstream Docs

### S3Options doesn't implement Default
`S3Options` requires all fields to be specified manually. Would be nice to have `Default` impl or a builder pattern. Filed mentally for upstream contribution.

### diff() ancestry requirement
`Repository::diff(from, to)` requires `from` to be an ancestor of `to`. Returns `BadSnapshotChainForDiff` otherwise. Not immediately obvious from the type signature — needs documenting.

### VersionInfo is the universal ref resolver
`VersionInfo::BranchTipRef(name)`, `VersionInfo::TagRef(name)`, `VersionInfo::SnapshotId(id)`, `VersionInfo::AsOf { branch, at }` — use this everywhere instead of raw IDs.

### list_nodes returns Iterator not Stream
`Session::list_nodes()` returns `impl Iterator`, not `impl Stream`. This is synchronous iteration over the in-memory snapshot data. No need for async handling.

### Repository is Clone (Arc internally)
Safe to clone and move into spawned tasks.
