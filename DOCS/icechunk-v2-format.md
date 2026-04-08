# Icechunk V2 Binary Format Reference

## File Header (39 bytes)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 12 | Magic | `ICE🧊CHUNK` (UTF-8) |
| 12 | 24 | Impl ID | Left-aligned, space-padded |
| 36 | 1 | Spec version | `2` |
| 37 | 1 | File type | See below |
| 38 | 1 | Compression | `0`=none, `1`=zstd |

After header: payload (zstd-compressed flatbuffers if compression=1).

## File Types

| Value | Type | Path |
|-------|------|------|
| 1 | Snapshot | `$ROOT/snapshots/{id}` |
| 2 | Manifest | `$ROOT/manifests/{id}` |
| 3 | Attributes | (deprecated in V2) |
| 4 | TransactionLog | `$ROOT/transactions/{id}` |
| 5 | Chunk | `$ROOT/chunks/{id}` |
| 6 | RepoInfo | `$ROOT/repo` |

## Object IDs

- 12 bytes → Crockford Base32 → 20 characters
- 8 bytes → Crockford Base32 → 13 characters

## File Layout

```
$ROOT/
  repo                          ← single mutable file, entry point
  snapshots/{id}                ← immutable
  manifests/{id}                ← immutable
  transactions/{id}             ← immutable, optional
  chunks/{id}                   ← immutable, binary data
  overwritten/repo.{ts}.{id}   ← backup repo files (ops log overflow)
```

## Key Structures

### Repo Info (`$ROOT/repo`)
- Branches (sorted by name) → snapshot index
- Tags (sorted by name) → snapshot index
- All snapshots: id, parent_offset, timestamp, message, metadata
  - parent_offset: -1 = initial snapshot, otherwise index into snapshots list
- Ops log (latest_updates): timestamped mutation records
- Status: Online/ReadOnly/Offline + reason
- Config: FlexBuffers-encoded
- Feature flags

### Snapshot (`$ROOT/snapshots/{id}`)
- Nodes sorted by component-wise lexicographic path order
- Each node: id (8 bytes), path, zarr metadata (user_data), Group or Array
- Array data: shape (array_length, num_chunks per dim), dimension_names, manifest refs
- ManifestRef: manifest id + extent ranges per dimension

### Manifest (`$ROOT/manifests/{id}`)
- ArrayManifest entries sorted by node ID
- ChunkRef entries sorted by coordinates
- Chunk types: inline (embedded), native ($ROOT/chunks/), virtual (external URL)
- Optional zstd dictionary for virtual locations

### Transaction Log (`$ROOT/transactions/{id}`)
- New/deleted groups and arrays
- Updated arrays/groups (metadata changes)
- Updated chunks (node id + coordinates touched)
- Move operations (from, to, node_id, node_type)

## Performance-Critical Details

- All sorted lists enable binary search
- Ancestry is O(1) via parent_offset indexing
- Snapshot metadata available in repo file without fetching snapshot files
- ManifestRef extents tell you which manifest covers which chunk range
- Nodes sorted component-wise: `/a < /a/b < /ab < /b`
