# Roadmap

## Phase 1 — MVP: Local Repo Inspection

- Parse repo info file (branches, tags, snapshots, ancestry, status)
- Display repo overview, branch/tag lists, snapshot history with ancestry
- Browse node tree within a snapshot
- Array detail view (shape, zarr metadata, dimensions)
- Dual-mode: TUI + JSON output
- Excellent --help text

## Phase 2 — Remote Repos

- S3/GCS/Azure via object_store
- Credential handling (env vars, profiles)
- Connection status in UI
- Caching layer for repeated reads

## Phase 3 — Deep Inspection

- Manifest viewer: chunk stats, storage distribution
- Virtual ref analysis: external file locations, coverage
- Transaction log viewer: what changed per commit
- Ops log viewer: mutation history

## Phase 4 — Advanced

- Search across nodes/metadata
- Diff between two snapshots (structural + metadata)
- Export views as JSON/CSV
- Bookmarks / saved queries
- TUI screenshots for documentation
