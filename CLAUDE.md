# core-drill

Terminal UI for inspecting Icechunk V2 repositories — local and remote.

## Tech Stack

Rust, Ratatui (crossterm), tokio, object_store, flatbuffers, clap, tui_tree_widget

## Key Principles

- **V2 only** — no V1 support, no backcompat concerns
- **Metadata-first** — minimize S3 fetches; compute from cheap repo info file whenever possible
- **Async non-blocking** — UI must never hang; all I/O on background tokio tasks
- **Dual-mode output** — interactive TUI for humans, `--output json` for agents/scripts
- **Buy over build** — prefer existing crates over custom implementations
- **Excellent help** — every command/flag gets descriptive help text; invest in docs

## Building

```bash
cargo run -- <repo-path>           # dev mode
cargo build --release              # optimized build
```

## Module Structure

```
src/
  main.rs              — entry point, mode dispatch
  cli.rs               — clap CLI definition
  app.rs               — coordinator: pane focus, layout areas, key/mouse handling, auto-expand
  tui.rs               — terminal init (mouse capture), tokio::select event loop
  repo.rs              — open repos (local, S3, GCS, Azure, HTTP)
  theme.rs             — Earthmover brand colors, panel/widget helpers
  multiplexer.rs       — zellij/tmux detection, Ctrl+hjkl passthrough at pane edges
  store/
    mod.rs             — DataStore (cache), DataRequest/Response, background worker
    types.rs           — BranchInfo, TagInfo, TreeNode, DiffSummary, ArraySummary
  component/
    mod.rs             — Pane, BottomTab, Action enums; Component trait (not yet used)
  ui/
    mod.rs             — three-pane layout: sidebar (tree), detail, bottom (tabs)
    format.rs          — ZarrMetadata parser (data type, chunk shape, codecs, fill value)
    help.rs            — full-screen help overlay
    tree.rs            — flat-tree renderer (legacy; sidebar uses tui_tree_widget now)
```

## Key Patterns

- **Three-pane layout**: Sidebar (tree) | Detail (context-dependent) | Bottom (togglable tabs)
- **Pane focus model**: `Pane::Sidebar | Detail | Bottom` — not a View enum
- **AllNodes single-fetch**: `list_nodes("/")` loads entire tree; no per-group lazy loading
- **LoadState<T>**: `NotRequested | Loading | Loaded(T) | Error(String)` — always know what to render
- **Multiplexer passthrough**: Ctrl+hjkl at pane edges delegates to zellij/tmux
- **Mouse support**: click to focus pane and select row, using stored layout Rects
- **Auto-expand tree**: on initial load, drill through single-child groups to first meaningful level
- **Snapshot diffs**: auto-requested when bottom pane focused on Snapshots tab, uses `VersionInfo::SnapshotId`

## Critical Rules

- Never block the UI thread — use `tokio::spawn` for all storage fetches
- All storage operations return raw bytes; parsing is a separate step
- Sorted lists in icechunk format enable binary search — use it
- The repo info file is the single cheap entry point — extract max value from it before fetching snapshots/manifests
- `Repository::diff(from, to)` requires `from` to be an ancestor of `to`

## Documentation

- `PLANNING/architecture.md` — module structure, data flow, async model, pane layout
- `PLANNING/roadmap.md` — phased delivery plan with completion status
- `DOCS/icechunk-rust-api.md` — API cookbook, AllNodes pattern, diff gotchas
- `DOCS/icechunk-v2-format.md` — binary format quick reference
- `DOCS/ui-design.md` — three-pane TUI layout, navigation, **array detail pane sections** (Shape & Layout, Storage, Chunk Types including initialized fraction, Attributes)
