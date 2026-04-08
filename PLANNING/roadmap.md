# Roadmap

## Phase 1 — MVP: Local Repo Inspection

- [DONE] Parse repo info file (branches, tags, snapshots, ancestry, status)
- [DONE] Display repo overview, branch/tag lists, snapshot history with ancestry
- [DONE] Browse node tree within a snapshot (AllNodes single-fetch, tui_tree_widget)
- [DONE] Array detail view (shape, zarr metadata, dimensions, manifest refs)
- [DONE] Group detail view (path, child count, child listing)
- [DONE] Dual-mode: TUI + JSON output
- [DONE] Excellent --help text
- [DONE] Three-pane layout (sidebar/detail/bottom) with pane focus model
- [DONE] Mouse support (click to focus pane, click to select row)
- [DONE] Multiplexer passthrough (zellij/tmux) for Ctrl+hjkl at pane edges
- [DONE] Snapshot diff view (added/deleted/modified arrays+groups, chunk changes)
- [DONE] Auto-expand tree on load (drill through single-child groups)
- [DONE] Toggleable bottom panel with Snapshots/Branches/Tags tabs

## Phase 2 — Remote Repos & Polish

- [DONE] S3/GCS/Azure via object_store
- Credential handling (env vars, profiles)
- Connection status in UI
- Caching layer for repeated reads
- Branch switching (select branch in bottom panel to reload tree)
- Search / filter within tree and lists
- Chunk type info in array detail (data type, codec chain summary)

## Phase 3 — Deep Inspection

- Manifest viewer: chunk stats, storage distribution
- Virtual ref analysis: external file locations, coverage
- Transaction log viewer: what changed per commit
- Ops log viewer: mutation history

## Phase 4 — Advanced

- Search across nodes/metadata
- Export views as JSON/CSV
- Bookmarks / saved queries
- TUI screenshots for documentation
