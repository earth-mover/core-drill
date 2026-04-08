# Architecture

## Dual Output Modes

- **TUI mode** (default): full-screen ratatui app with panels, keyboard navigation
- **Structured mode** (`--output json|table`): prints data to stdout, no interactive UI — designed for piping and agent consumption

Both modes share the same data-fetching and parsing layers; only the rendering differs.

## Module Structure

```
src/
  main.rs        — entry point, mode dispatch
  cli.rs         — clap CLI definition with rich --help text
  app.rs         — application state machine, navigation, event handling
  tui.rs         — terminal setup/teardown, render loop
  storage.rs     — async object_store abstraction (local/S3/GCS/Azure)
  format.rs      — icechunk V2 binary parsing (header, decompression, flatbuffers)
  ui/
    mod.rs       — view dispatch
    overview.rs  — repo overview panel
    help.rs      — keybinding help overlay
```

## Data Flow

```
CLI args → storage backend init → fetch repo info (single cheap read)
  → parse branches, tags, snapshots, ancestry, ops log, status
  → lazy-load snapshots on demand (one fetch per snapshot)
  → lazy-load manifests on demand (one fetch per manifest)
  → never fetch chunks unless explicitly requested
```

## Async Model

- tokio runtime on main thread
- UI rendering on main thread via ratatui
- All I/O spawned as tokio tasks
- `tokio::sync::mpsc` channels for background → UI communication
- Loading states shown in UI while fetches are in-flight

## Metadata Fetch Hierarchy (cheap → expensive)

1. **Repo info file** (single fetch): branches, tags, all snapshot IDs + timestamps + messages, full ancestry graph (parent_offset), ops log, repo status, config, feature flags
2. **Snapshot file** (one fetch per snapshot): complete node tree (sorted by path), array zarr metadata, shape, dimension names, manifest refs with extents
3. **Manifest file** (one fetch per manifest): chunk coordinate → location mappings, inline chunks, virtual refs
4. **Chunk files**: actual data — avoid unless user explicitly requests

## Performance Shortcuts from the Format

- All lists are sorted → binary search for lookups
- Ancestry encoded as parent_offset index → O(1) parent lookup, no sequential reads
- Snapshot metadata in repo file → no need to fetch snapshot files just for timestamps/messages
- ManifestRef extents → know which manifest has which chunks without reading all manifests
- Node paths sorted component-wise → efficient tree reconstruction
