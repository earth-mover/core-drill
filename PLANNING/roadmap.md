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
- [DONE] Branch switching (select branch in bottom panel to reload tree)
- [DONE] Search / filter within tree and lists (fuzzy search with nucleo)
- [DONE] Chunk type info in array detail (inline/native/virtual breakdown)
- [DONE] Error classification (auth/network/not-found) with retry (R key)
- [DONE] Background chunk stats scanning (drip-fed, non-blocking)
- [DONE] Search candidate caching
- Credential handling (env vars, profiles, mid-session 403 detection)
- Connection status in UI (cancel with Esc, timeout handling)
- Caching layer for repeated reads

## Phase 3 — Deep Inspection

- [DONE] Ops log viewer: mutation history (dedicated detail tab + CLI + MCP)
- [DONE] Virtual source aggregation in repo overview (resolved VCC → bucket/org)
- Manifest viewer: per-manifest chunk stats, storage distribution
- Virtual ref analysis: dedicated view of all virtual refs for an array
- Transaction log viewer: what changed per commit (beyond current diff view)

## Phase 4 — Advanced

- Export detail pane content to file / clipboard
- Copy snapshot ID / array path to clipboard
- Branch/tag timestamp enrichment from ancestry cache
- Bookmarks / saved queries
- TUI screenshots for documentation
