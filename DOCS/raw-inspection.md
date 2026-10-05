# Raw object inspection

`core-drill` normally shows a repo at the semantic level: branches, the node tree, diffs. Three commands go one level down and show Icechunk's metadata files as the flatbuffers they are:

| Command | Answers |
|---------|---------|
| `object <target>` | What does this file store, field by field? |
| `chunk-ref <array> <coords>` | Which manifest entry holds this chunk, and what does it record? |
| `chunk-changes <commit>` | Which chunks did this commit add, overwrite, or delete? |

All three work with `--output json` and as MCP tools (`object`, `chunk_ref`, `chunk_changes`). For the same views in a browser, see the [web viewer](../web/README.md).

This page covers the CLI. For the file layout itself, see [icechunk-v2-format.md](icechunk-v2-format.md).

The examples use a small repo with one commit for each kind of chunk change. To build it locally (it lands in `test-data/`, which is gitignored), run:

```bash
uv run scripts/create_chunk_lifecycle_repo.py
```

Snapshot IDs are random, so yours will differ from the ones shown here. The same history is published on Arraylake as `al:iansorg/icechunk-format-examples`; it was written by `scripts/create_chunk_lifecycle_repo.py --arraylake iansorg/icechunk-format-examples`:

```bash
core-drill al:iansorg/icechunk-format-examples log                    # commit IDs and messages
core-drill al:iansorg/icechunk-format-examples chunk-changes <commit>
```

## `object`: decode a metadata file

```bash
core-drill ./repo object repo                      # the repo info file
core-drill ./repo object main                      # snapshot at the tip of main
core-drill ./repo object transactions/main         # that commit's transaction log
core-drill ./repo object manifests/<id>            # a manifest
core-drill ./repo object <id>                      # bare ID: snapshot, then manifest, then chunk
```

The output starts with the file header (spec version, file type, compression, the writing library), stored and decoded sizes, and whether the file passes the same flatbuffer verification icechunk applies when it opens it. After that comes every field present in the file, in schema order:

```text
id: → snapshots/CAGX6GH0EPG33D68Z51G, transactions/CAGX6GH0EPG33D68Z51G
nodes: 3 items
  [1] NodeSnapshot
    id: YD6EA2BZZYTH8
    path: "/native"
    user_data: {"attributes":{},"chunk_grid":… <641 bytes>
    node_data: ArrayNodeData
      manifests: 1 item
        [0] ManifestRef
          object_id: → manifests/GFFPFGXK5R0JXEZ9WNXG
          extents: 2 items
            [0]: {from: 0, to: 2}
```

The decoder adds the following to the raw fields:

- **IDs that name other files** are printed as `→ dir/ID`. Pass `dir/ID` back to `object` to follow the link.
- **Node IDs** are labeled with their paths. A transaction log uses its own snapshot and its parent's. A manifest uses `--snapshot <ref>` (default: the tip of `main`).
- **Timestamps** get an ISO form next to the stored integer.
- **Opaque byte fields** are decoded where the format defines them: zarr metadata JSON in `user_data`, FlexBuffers in metadata and config, and dictionary-compressed virtual locations (`compressed_location`).
- **Fields absent from the file** are omitted. Flatbuffers doesn't store scalar fields that equal their default.

Long vectors show 50 items (`-n` changes this; `-n 0` shows all). Use `--at` to select a subtree or a window of a vector:

```bash
core-drill ./repo object manifests/<id> --at arrays/0/refs/100..150
core-drill ./repo object main --at nodes/1/node_data
```

Decoding is driven by `schema/*.bfbs`, compiled from the Icechunk `.fbs` files, so fields newer than the linked icechunk library still show up. Every read is bounds-checked, so a corrupt file produces an error instead of a crash. To refresh the schema after a format change, run `scripts/gen-schema.sh path/to/icechunk/icechunk-format/flatbuffers`.

## Which chunks changed between two versions?

`chunk-changes` classifies every chunk coordinate a commit touched:

```bash
core-drill ./repo chunk-changes MNB5ZD7ZTTZT01TTXV6G
```

```text
## /native — 1 coordinate listed

- (1, 1) **deleted**
  - before: native chunk `P7TN6M0SRMNJMNXGXJ80`, 4318 bytes at offset 0 — in `manifests/NC0XXACBQRMJMMHKFRF0` at `arrays/0/refs/3`
```

Each coordinate gets one of these kinds:

| Kind | Before (parent) | After (this commit) |
|------|-----------------|---------------------|
| added | no ref | ref |
| overwritten | ref | different ref |
| rewritten | ref | identical ref |
| deleted | ref | no ref |
| absent | no ref | no ref (e.g. written and deleted within the commit) |

Use `--path /array` to restrict to one array, and `-n` to cap how many coordinates are classified (default 200; `-n 0` for all). Each commit is compared with its parent. For two versions further apart, run it on each commit in between (`core-drill ./repo log` lists them), or use `chunk-ref` on the two versions for a specific chunk.

## How does a manifest record a chunk deletion?

A manifest has no entry for a deleted chunk. Two files show the change:

1. The commit's **transaction log** lists the coordinate in `updated_chunks`. That list holds every coordinate whose ref was "added, overwritten, or deleted" ([transaction_log.fbs](../schema/transaction_log.fbs)), without saying which:

   ```bash
   core-drill ./repo object transactions/MNB5ZD7ZTTZT01TTXV6G --at updated_chunks
   ```
   ```text
   [0] ArrayUpdatedChunks
     node_id: YD6EA2BZZYTH8 (/native)
     chunks: 1 item
       [0]: {coords: [1, 1]}
   ```

2. The commit's **manifest** for that array has no `ChunkRef` with that index. Refs are sorted by index, so the gap is visible:

   ```bash
   core-drill ./repo object manifests/<new manifest id> --at arrays/0/refs
   ```

   The new manifest holds three refs, `[0, 0]`, `[0, 1]`, and `[1, 0]`; `[1, 1]` is gone.

`chunk-changes` combines the two: it reads `updated_chunks`, then looks each coordinate up in the parent's and the commit's manifests. A deleted chunk reads as the array's fill value.

## What checksum does a virtual ref store?

`chunk-ref` finds the manifest entry for one chunk and prints it:

```bash
core-drill ./repo chunk-ref /virtual 8
```

```text
- **Manifest:** `manifests/ZJ199AC7MWDAWV2EPSJG` at `arrays/0/refs/7`
- **Ref:** virtual `s3://example-bucket/model-output/run-00/file-00008.nc`, 4 bytes at offset 32, ETag "etag-00008"

index: [8]
offset: 32
length: 4
checksum_etag: "\"etag-00008\""
compressed_location: "s3://example-bucket/model-output/run-00/file-00008.nc" <27 bytes>
```

A virtual ref stores at most one checksum for its target object:

- `checksum_etag`: the object's ETag string.
- `checksum_last_modified`: seconds since the Unix epoch. 0 means no checksum.

When a manifest holds many virtual refs (1000 or more by default), icechunk compresses the URLs with a zstd dictionary stored in the manifest's `location_dictionary`. They appear as `compressed_location`, which `core-drill` shows decompressed.

`chunk-ref` works for native and inline refs too, and on any version (`-r <branch|tag|snapshot>`). When no ref exists, it says so: the chunk was never written, or it was deleted.
