//! Fetching for raw object inspection: object paths, node-path context, and
//! chunk-ref lookups across snapshots and manifests.

use std::collections::HashMap;

use color_eyre::eyre::{Result, bail, eyre};
use icechunk::Repository;
use icechunk::format::manifest::{ChunkPayload, ManifestExtents};
use icechunk::format::snapshot::{NodeData, Snapshot};
use icechunk::format::{ChunkIndices, NodeId, SnapshotId};
use serde::Serialize;
use tokio::io::AsyncReadExt;

use super::resolve_ref_to_snapshot_id;
use crate::raw::{self, DecodeOptions, FileKind, OpenedFile, RawObject, RawValue};
use crate::sanitize::sanitize;

/// Vector elements shown per table vector by `object`.
pub const DEFAULT_MAX_ITEMS: usize = 50;
/// Coordinates classified by `chunk-changes`.
pub const DEFAULT_CHANGE_LIMIT: usize = 200;

fn parse_snapshot_id(id: &str) -> Result<SnapshotId> {
    SnapshotId::try_from(id).map_err(|e| eyre!("bad snapshot id '{id}': {e}"))
}

/// Object keys are joined onto the repo prefix, so refuse anything that
/// could climb out of it on filesystem-backed storage.
fn check_key(path: &str) -> Result<()> {
    let ok = path.split('/').all(|seg| {
        !seg.is_empty()
            && seg != "."
            && seg != ".."
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    });
    if !ok {
        bail!("invalid object path '{path}'");
    }
    Ok(())
}

/// Turn a user-supplied target into candidate object paths, most likely first.
///
/// Accepts `repo`, a repo-relative path (`manifests/<id>`), a bare object ID
/// (tried as snapshot, then manifest, then chunk), or a branch, tag, or
/// snapshot-ID prefix (its snapshot). `snapshots/<ref>` and
/// `transactions/<ref>` resolve the ref too.
async fn candidate_paths(repo: &Repository, target: &str) -> Result<Vec<String>> {
    let t = target.trim_matches('/');
    if FileKind::from_path(t) == Some(FileKind::RepoInfo) && !t.contains('/') {
        return Ok(vec![t.to_string()]);
    }
    if let Some((dir, rest)) = t.split_once('/') {
        let kind = FileKind::from_path(t).ok_or_else(|| {
            eyre!("unknown directory '{dir}' (expected snapshots, transactions, manifests, chunks, or overwritten)")
        })?;
        return Ok(vec![match kind {
            FileKind::Snapshot | FileKind::TransactionLog if parse_snapshot_id(rest).is_err() => {
                format!("{dir}/{}", resolve_ref_to_snapshot_id(repo, rest).await?)
            }
            _ => t.to_string(),
        }]);
    }
    if parse_snapshot_id(t).is_ok() {
        return Ok([FileKind::Snapshot, FileKind::Manifest, FileKind::Chunk]
            .iter()
            .map(|k| format!("{}/{t}", k.dir()))
            .collect());
    }
    let id = resolve_ref_to_snapshot_id(repo, t).await?;
    Ok(vec![format!("{}/{id}", FileKind::Snapshot.dir())])
}

async fn fetch_bytes(repo: &Repository, path: &str) -> Result<Vec<u8>> {
    check_key(path)?;
    let am = repo.asset_manager();
    let (mut reader, _) = am
        .storage()
        .get_object(am.storage_settings(), path, None)
        .await
        .map_err(|e| eyre!("{path}: {e}"))?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

/// Fetch and decode a file. Decompression and verification are CPU-bound
/// (seconds for a large manifest), so they run off the async workers.
async fn fetch_opened(repo: &Repository, path: &str) -> Result<OpenedFile> {
    open_blocking(path, fetch_bytes(repo, path).await?).await
}

async fn open_blocking(path: &str, bytes: Vec<u8>) -> Result<OpenedFile> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || raw::open(&path, bytes)).await?
}

async fn open_target(repo: &Repository, target: &str) -> Result<OpenedFile> {
    let candidates = candidate_paths(repo, target).await?;
    let mut last_err = None;
    for path in &candidates {
        match fetch_bytes(repo, path).await {
            Ok(bytes) => return open_blocking(path, bytes).await,
            Err(e) => last_err = Some(e),
        }
    }
    match (candidates.len(), last_err) {
        (1, Some(e)) => Err(e),
        _ => bail!("'{target}' not found as {}", candidates.join(", ")),
    }
}

/// Parent from the repo info file; `Snapshot::parent_id` is unset in V2.
async fn parent_of(repo: &Repository, id: &SnapshotId) -> Result<Option<SnapshotId>> {
    let (info, _) = repo.asset_manager().fetch_repo_info().await?;
    Ok(info.find_snapshot(id)?.parent_id)
}

/// Per-snapshot lookup tables, built in one pass over its nodes.
#[derive(Default)]
struct SnapshotIndex {
    paths: HashMap<[u8; 8], String>,
    manifests: HashMap<[u8; 8], Vec<(String, ManifestExtents)>>,
}

impl SnapshotIndex {
    fn build(snapshot: &Snapshot) -> Result<Self> {
        let mut index = SnapshotIndex::default();
        for node in snapshot.iter() {
            let node = node?;
            index
                .paths
                .insert(node.id.0, sanitize(&node.path.to_string()));
            if let NodeData::Array { manifests, .. } = node.node_data {
                index.manifests.insert(node.id.0, extents_of(manifests));
            }
        }
        Ok(index)
    }
}

fn extents_of(
    manifests: Vec<icechunk::format::manifest::ManifestRef>,
) -> Vec<(String, ManifestExtents)> {
    manifests
        .into_iter()
        .map(|m| (m.object_id.to_string(), m.extents))
        .collect()
}

/// Node paths from a snapshot and its parent, so nodes deleted by the commit
/// still get labels. The commit's own path wins where both have the node.
async fn commit_node_paths(repo: &Repository, id: &SnapshotId) -> Result<HashMap<[u8; 8], String>> {
    let am = repo.asset_manager();
    let mut paths = SnapshotIndex::build(&*am.fetch_snapshot(id).await?)?.paths;
    // V1 repos have no repo info file to find the parent in.
    if let Ok(Some(parent)) = parent_of(repo, id).await {
        for (node, path) in SnapshotIndex::build(&*am.fetch_snapshot(&parent).await?)?.paths {
            paths.entry(node).or_insert(path);
        }
    }
    Ok(paths)
}

/// Node paths for labeling node IDs: a transaction log uses its own snapshot
/// and that snapshot's parent; a manifest uses `context` or, failing that,
/// the `main` branch tip.
async fn node_context(
    repo: &Repository,
    file: &OpenedFile,
    context: Option<&str>,
) -> Result<HashMap<[u8; 8], String>> {
    match file.kind {
        FileKind::TransactionLog => {
            let id = file.path.rsplit('/').next().unwrap_or_default();
            // Labels are best-effort; the log decodes without them.
            match parse_snapshot_id(id) {
                Ok(id) => Ok(commit_node_paths(repo, &id).await.unwrap_or_default()),
                Err(_) => Ok(HashMap::new()),
            }
        }
        FileKind::Manifest => {
            let snapshot = match context {
                Some(r) => Some(resolve_ref_to_snapshot_id(repo, r).await?),
                None => resolve_ref_to_snapshot_id(repo, "main").await.ok(),
            };
            match snapshot {
                Some(id) => {
                    let snap = repo
                        .asset_manager()
                        .fetch_snapshot(&parse_snapshot_id(&id)?)
                        .await?;
                    Ok(SnapshotIndex::build(&snap)?.paths)
                }
                None => Ok(HashMap::new()),
            }
        }
        _ => Ok(HashMap::new()),
    }
}

/// Fetch and decode one metadata file. `selector` narrows the output to a
/// subtree (`arrays/0/refs/5`); `context` names the snapshot used to label
/// node IDs in a manifest.
pub(crate) async fn fetch_raw_object(
    repo: &Repository,
    target: &str,
    selector: Option<String>,
    max_items: usize,
    context: Option<&str>,
) -> Result<RawObject> {
    let file = open_target(repo, target).await?;
    let node_paths = node_context(repo, &file, context).await?;
    file.walk(DecodeOptions {
        selector,
        max_items,
        node_paths,
    })
}

// ─── Chunk ref lookup ──────────────────────────────────────

/// Where a chunk ref lives and what it stores.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkLocation {
    pub manifest: String,
    /// Selector for this ref inside the manifest, for `object --at`.
    pub at: String,
    pub chunk_ref: RawValue,
    /// Full-fidelity ref for comparisons; `chunk_ref` holds display previews.
    #[serde(skip)]
    payload: ChunkPayload,
}

/// Caches manifests across many lookups.
struct ChunkLocator<'r> {
    repo: &'r Repository,
    manifests: HashMap<String, OpenedFile>,
}

impl<'r> ChunkLocator<'r> {
    fn new(repo: &'r Repository) -> Self {
        Self {
            repo,
            manifests: HashMap::new(),
        }
    }

    /// Find the ref for `coords` among an array's manifests (from its
    /// snapshot's `ManifestRef`s).
    async fn locate(
        &mut self,
        manifests: &[(String, ManifestExtents)],
        node: &[u8; 8],
        coords: &[u32],
    ) -> Result<Option<ChunkLocation>> {
        for (id, extents) in manifests {
            if !extents.contains(coords) {
                continue;
            }
            if !self.manifests.contains_key(id) {
                let path = format!("{}/{id}", FileKind::Manifest.dir());
                let file = fetch_opened(self.repo, &path).await?;
                if let Some(err) = &file.verify_error {
                    bail!("{path} failed verification: {err}");
                }
                self.manifests.insert(id.clone(), file);
            }
            let file = &self.manifests[id];
            let (Some((a, r)), Some(manifest)) =
                (file.find_chunk_ref(node, coords)?, file.manifest())
            else {
                continue;
            };
            let at = format!("arrays/{a}/refs/{r}");
            let chunk_ref = file
                .walk(DecodeOptions {
                    selector: Some(at.clone()),
                    ..Default::default()
                })?
                .value;
            let payload =
                manifest.get_chunk_payload(&NodeId::new(*node), &ChunkIndices(coords.to_vec()))?;
            return Ok(Some(ChunkLocation {
                manifest: id.clone(),
                at,
                chunk_ref,
                payload,
            }));
        }
        Ok(None)
    }
}

/// A chunk ref found by array path and coordinates.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkRefLookup {
    pub snapshot: String,
    pub path: String,
    pub node_id: String,
    pub coords: Vec<u32>,
    /// `None` when no manifest holds a ref for these coordinates (the chunk
    /// reads as fill value).
    pub location: Option<ChunkLocation>,
}

pub(crate) async fn fetch_chunk_ref(
    repo: &Repository,
    r: &str,
    array_path: &str,
    coords: &[u32],
) -> Result<ChunkRefLookup> {
    let snapshot = resolve_ref_to_snapshot_id(repo, r).await?;
    let snap = repo
        .asset_manager()
        .fetch_snapshot(&parse_snapshot_id(&snapshot)?)
        .await?;
    let path = icechunk::format::Path::try_from(array_path)
        .map_err(|e| eyre!("bad path '{array_path}': {e}"))?;
    let node = snap
        .get_node(&path)
        .map_err(|_| eyre!("no node at '{array_path}' in snapshot {snapshot}"))?;
    let NodeData::Array {
        manifests, shape, ..
    } = node.node_data.clone()
    else {
        bail!("'{array_path}' is a group, not an array");
    };
    if coords.len() != shape.len() {
        bail!(
            "'{array_path}' has {} dimensions but {} coordinates were given",
            shape.len(),
            coords.len()
        );
    }
    let location = ChunkLocator::new(repo)
        .locate(&extents_of(manifests), &node.id.0, coords)
        .await?;
    Ok(ChunkRefLookup {
        snapshot,
        path: sanitize(&node.path.to_string()),
        node_id: node.id.to_string(),
        coords: coords.to_vec(),
        location,
    })
}

// ─── Chunk changes in one commit ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// No ref in the parent, a ref in this snapshot.
    Added,
    /// Refs in both, with different contents.
    Overwritten,
    /// Refs in both, identical (rewritten with the same bytes/target).
    Rewritten,
    /// A ref in the parent, none in this snapshot.
    Deleted,
    /// Listed in the transaction log but absent from both manifests,
    /// e.g. written and then deleted within the commit.
    Absent,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkChange {
    pub coords: Vec<u32>,
    pub kind: ChangeKind,
    pub before: Option<ChunkLocation>,
    pub after: Option<ChunkLocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArrayChunkChanges {
    pub path: Option<String>,
    pub node_id: String,
    /// Coordinates the transaction log lists for this array.
    pub listed: usize,
    pub changes: Vec<ChunkChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkChanges {
    pub snapshot: String,
    pub parent: Option<String>,
    pub message: String,
    pub arrays: Vec<ArrayChunkChanges>,
    /// True when `limit` cut the classified coordinates short.
    pub truncated: bool,
}

/// Classify every chunk coordinate in a commit's transaction log by looking
/// it up in the parent's and the commit's manifests.
///
/// The transaction log lists changed coordinates without the kind of change,
/// and a deleted chunk has no ref in the new manifest.
pub(crate) async fn fetch_chunk_changes(
    repo: &Repository,
    r: &str,
    path_filter: Option<&str>,
    limit: usize,
) -> Result<ChunkChanges> {
    let snapshot = resolve_ref_to_snapshot_id(repo, r).await?;
    let id = parse_snapshot_id(&snapshot)?;
    let am = repo.asset_manager();
    let snap = am.fetch_snapshot(&id).await?;
    let parent = parent_of(repo, &id).await?;
    let tx = am.fetch_transaction_log(&id).await?;

    let after_index = SnapshotIndex::build(&snap)?;
    let before_index = match &parent {
        Some(p) => SnapshotIndex::build(&*am.fetch_snapshot(p).await?)?,
        None => SnapshotIndex::default(),
    };

    let mut locator = ChunkLocator::new(repo);
    let mut arrays = Vec::new();
    let mut remaining = if limit == 0 { usize::MAX } else { limit };
    let mut truncated = false;

    for (node, coords) in tx.updated_chunks() {
        let node = node.0;
        let path = after_index
            .paths
            .get(&node)
            .or_else(|| before_index.paths.get(&node))
            .cloned();
        if let Some(filter) = path_filter
            && path.as_deref() != Some(filter)
        {
            continue;
        }
        let before_manifests = before_index
            .manifests
            .get(&node)
            .map_or(&[][..], Vec::as_slice);
        let after_manifests = after_index
            .manifests
            .get(&node)
            .map_or(&[][..], Vec::as_slice);
        let mut listed = 0;
        let mut changes = Vec::new();
        for ChunkIndices(c) in coords {
            listed += 1;
            if remaining == 0 {
                truncated = true;
                continue;
            }
            remaining -= 1;
            let before = locator.locate(before_manifests, &node, &c).await?;
            let after = locator.locate(after_manifests, &node, &c).await?;
            let kind = match (&before, &after) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Deleted,
                (None, None) => ChangeKind::Absent,
                (Some(b), Some(a)) if b.payload == a.payload => ChangeKind::Rewritten,
                (Some(_), Some(_)) => ChangeKind::Overwritten,
            };
            changes.push(ChunkChange {
                coords: c,
                kind,
                before,
                after,
            });
        }
        arrays.push(ArrayChunkChanges {
            path,
            node_id: NodeId::new(node).to_string(),
            listed,
            changes,
        });
    }

    Ok(ChunkChanges {
        snapshot,
        parent: parent.map(|p| p.to_string()),
        message: sanitize(&snap.message()),
        arrays,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bytes::Bytes;
    use icechunk::format::manifest::{
        Checksum, ChunkPayload, SecondsSinceEpoch, VirtualChunkLocation, VirtualChunkRef,
    };
    use icechunk::format::snapshot::ArrayShape;
    use icechunk::format::{ChunkIndices, Path};
    use icechunk::{Repository, new_in_memory_storage};

    use super::*;

    /// Five commits on /a (4 chunks along one axis): write 0-2, overwrite 0,
    /// delete 1, add virtual ref 3, then rewrite 2 with identical bytes.
    async fn lifecycle_repo() -> (Repository, Vec<String>) {
        let storage = new_in_memory_storage().await.unwrap();
        let repo = Repository::create(None, storage, HashMap::new(), None, true)
            .await
            .unwrap();
        let path = Path::try_from("/a").unwrap();
        let mut commits = Vec::new();

        let mut s = repo.writable_session("main").await.unwrap();
        s.add_array(
            path.clone(),
            ArrayShape::new(vec![(4, 4)]).unwrap(),
            None,
            Bytes::from_static(b"{}"),
        )
        .await
        .unwrap();
        for i in 0..3 {
            let payload = ChunkPayload::Inline(Bytes::from(format!("v1-{i}")));
            s.set_chunk_ref(path.clone(), ChunkIndices(vec![i]), Some(payload))
                .await
                .unwrap();
        }
        commits.push(s.commit("write").execute().await.unwrap().to_string());

        let edits: Vec<(&str, u32, Option<ChunkPayload>)> = vec![
            (
                "overwrite",
                0,
                Some(ChunkPayload::Inline(Bytes::from_static(b"v2-0"))),
            ),
            ("delete", 1, None),
            (
                "virtual",
                3,
                Some(ChunkPayload::Virtual(VirtualChunkRef {
                    location: VirtualChunkLocation::from_url("s3://bucket/data.nc").unwrap(),
                    offset: 100,
                    length: 50,
                    checksum: Some(Checksum::LastModified(SecondsSinceEpoch(1_717_243_200))),
                })),
            ),
            (
                "rewrite",
                2,
                Some(ChunkPayload::Inline(Bytes::from_static(b"v1-2"))),
            ),
        ];
        for (message, coord, payload) in edits {
            let mut s = repo.writable_session("main").await.unwrap();
            s.set_chunk_ref(path.clone(), ChunkIndices(vec![coord]), payload)
                .await
                .unwrap();
            commits.push(s.commit(message).execute().await.unwrap().to_string());
        }
        (repo, commits)
    }

    fn kinds(changes: &ChunkChanges) -> Vec<(Vec<u32>, ChangeKind)> {
        changes
            .arrays
            .iter()
            .flat_map(|a| a.changes.iter().map(|c| (c.coords.clone(), c.kind)))
            .collect()
    }

    #[tokio::test]
    async fn chunk_changes_classify_each_commit() {
        let (repo, c) = lifecycle_repo().await;
        let expect = [
            vec![
                (vec![0], ChangeKind::Added),
                (vec![1], ChangeKind::Added),
                (vec![2], ChangeKind::Added),
            ],
            vec![(vec![0], ChangeKind::Overwritten)],
            vec![(vec![1], ChangeKind::Deleted)],
            vec![(vec![3], ChangeKind::Added)],
            vec![(vec![2], ChangeKind::Rewritten)],
        ];
        for (commit, expected) in c.iter().zip(expect) {
            let changes = fetch_chunk_changes(&repo, commit, None, 0).await.unwrap();
            assert_eq!(kinds(&changes), expected, "commit {}", changes.message);
            assert_eq!(changes.arrays[0].path.as_deref(), Some("/a"));
        }
    }

    /// Display previews hold 32 bytes, so classification must compare full refs.
    #[tokio::test]
    async fn overwrite_differing_past_preview_is_not_rewritten() {
        let storage = new_in_memory_storage().await.unwrap();
        let repo = Repository::create(None, storage, HashMap::new(), None, true)
            .await
            .unwrap();
        let path = Path::try_from("/b").unwrap();
        let mut commit = None;
        for tail in [b'x', b'y'] {
            let mut s = repo.writable_session("main").await.unwrap();
            if tail == b'x' {
                s.add_array(
                    path.clone(),
                    ArrayShape::new(vec![(1, 1)]).unwrap(),
                    None,
                    Bytes::from_static(b"{}"),
                )
                .await
                .unwrap();
            }
            let mut data = vec![0u8; 40];
            data[39] = tail;
            s.set_chunk_ref(
                path.clone(),
                ChunkIndices(vec![0]),
                Some(ChunkPayload::Inline(Bytes::from(data))),
            )
            .await
            .unwrap();
            commit = Some(s.commit("write").execute().await.unwrap().to_string());
        }
        let changes = fetch_chunk_changes(&repo, &commit.unwrap(), None, 0)
            .await
            .unwrap();
        assert_eq!(kinds(&changes), vec![(vec![0], ChangeKind::Overwritten)]);
    }

    #[tokio::test]
    async fn chunk_ref_reports_virtual_checksum_and_missing_refs() {
        let (repo, c) = lifecycle_repo().await;
        let found = fetch_chunk_ref(&repo, "main", "/a", &[3]).await.unwrap();
        let loc = found.location.expect("virtual ref present");
        assert!(matches!(
            loc.chunk_ref.field("checksum_last_modified"),
            Some(RawValue::Time { raw: 1_717_243_200, iso }) if iso == "2024-06-01T12:00:00Z"
        ));
        assert!(
            matches!(loc.chunk_ref.field("location"), Some(RawValue::Str(s)) if s == "s3://bucket/data.nc")
        );

        let deleted = fetch_chunk_ref(&repo, "main", "/a", &[1]).await.unwrap();
        assert!(deleted.location.is_none());
        let before_delete = fetch_chunk_ref(&repo, &c[1], "/a", &[1]).await.unwrap();
        assert!(before_delete.location.is_some());
    }

    #[tokio::test]
    async fn raw_object_targets_resolve() {
        let (repo, c) = lifecycle_repo().await;
        let tip = c.last().unwrap();

        let snap = fetch_raw_object(&repo, "main", None, 50, None)
            .await
            .unwrap();
        assert_eq!(snap.path, format!("snapshots/{tip}"));
        assert!(snap.verify_error.is_none());
        assert!(matches!(snap.value.field("message"), Some(RawValue::Str(m)) if m == "rewrite"));

        let tx = fetch_raw_object(
            &repo,
            "transactions/main",
            Some("updated_chunks/0/node_id".into()),
            50,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(tx.value, RawValue::NodeId { path: Some(ref p), .. } if p == "/a"));

        let repo_file = fetch_raw_object(&repo, "repo", None, 50, None)
            .await
            .unwrap();
        assert_eq!(repo_file.kind, FileKind::RepoInfo);

        let manifest_id = fetch_chunk_ref(&repo, "main", "/a", &[0])
            .await
            .unwrap()
            .location
            .unwrap()
            .manifest;
        let manifest = fetch_raw_object(
            &repo,
            &manifest_id,
            Some("arrays/0/node_id".into()),
            50,
            None,
        )
        .await
        .unwrap();
        assert_eq!(manifest.kind, FileKind::Manifest);
        assert!(matches!(manifest.value, RawValue::NodeId { path: Some(ref p), .. } if p == "/a"));
    }

    #[tokio::test]
    async fn raw_object_rejects_bad_targets() {
        let (repo, _) = lifecycle_repo().await;
        for target in [
            "manifests/../repo",
            "snapshots/./x",
            "elsewhere/abc",
            "nosuchbranch",
        ] {
            assert!(
                fetch_raw_object(&repo, target, None, 50, None)
                    .await
                    .is_err(),
                "{target}"
            );
        }
        let err = fetch_raw_object(&repo, "repo", Some("nope".into()), 50, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("has no field 'nope'"), "{err}");
    }

    /// Hostile input must produce errors, never panics: truncate and flip
    /// every byte of each decompressed metadata file, re-wrapped uncompressed.
    #[tokio::test]
    async fn corrupt_files_never_panic() {
        let (repo, c) = lifecycle_repo().await;
        let manifest = fetch_chunk_ref(&repo, "main", "/a", &[0])
            .await
            .unwrap()
            .location
            .unwrap()
            .manifest;
        let tip = c.last().unwrap();
        for path in [
            "repo".to_string(),
            format!("snapshots/{tip}"),
            format!("transactions/{tip}"),
            format!("manifests/{manifest}"),
        ] {
            let opened = raw::open(&path, fetch_bytes(&repo, &path).await.unwrap()).unwrap();
            let mut header = fetch_bytes(&repo, &path).await.unwrap()[..39].to_vec();
            header[38] = 0;
            let body = opened.body().to_vec();
            let walk = |b: Vec<u8>| {
                let bytes = [header.clone(), b].concat();
                if let Ok(file) = raw::open(&path, bytes) {
                    let _ = file.walk(DecodeOptions {
                        max_items: 0,
                        ..Default::default()
                    });
                }
            };
            for len in 0..body.len() {
                walk(body[..len].to_vec());
            }
            for i in 0..body.len() {
                let mut b = body.clone();
                b[i] ^= 0xFF;
                walk(b);
            }
        }
    }

    #[test]
    fn check_key_rejects_traversal() {
        assert!(check_key("manifests/ABC").is_ok());
        assert!(check_key("overwritten/repo.123.ABC").is_ok());
        for bad in ["../x", "a//b", "a/./b", "a/b c", "/abs", "a/%2e%2e"] {
            assert!(check_key(bad).is_err(), "{bad}");
        }
    }
}
