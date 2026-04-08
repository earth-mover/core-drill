# Architecture

## Dual Output Modes

- **TUI mode** (default): full-screen ratatui app with panels, keyboard navigation
- **Structured mode** (`--output json|table`): prints data to stdout, no interactive UI — designed for piping and agent consumption

Both modes share the same data-fetching and parsing layers; only the rendering differs.

## Module Structure

```
src/
  main.rs              — entry point, mode dispatch
  cli.rs               — clap CLI definition with rich --help text
  app.rs               — coordinator: owns DataStore, Theme, NavigationContext
  tui.rs               — terminal setup/teardown, tokio::select event loop
  repo.rs              — thin wrapper for opening repos (local, S3, GCS, Azure, HTTP)
  theme.rs             — Earthmover brand colors, shared styles, widget helpers
  store/
    mod.rs             — DataStore, LoadState, DataRequest/Response, background worker
    types.rs           — owned domain types (BranchInfo, TagInfo, TreeNode, etc.)
  component/
    mod.rs             — Component trait, Action enum, NavigationTarget, View
    (branch_list.rs)   — planned: BranchList component
    (tag_list.rs)      — planned: TagList component
    (snapshot_log.rs)  — planned: linear commit history
    (diff_view.rs)     — planned: snapshot diff viewer
    (node_tree.rs)     — planned: hierarchical node browser
    (array_detail.rs)  — planned: array metadata detail
    (ops_log.rs)       — planned: mutation history
  ui/
    mod.rs             — top-level layout dispatch (status bar, tabs, content, hints)
    overview.rs        — repo overview panel
    help.rs            — keybinding help overlay
```

## Data Flow

```
CLI args → repo::open (detect backend) → Repository
  → DataStore::new(repo) spawns background worker
  → App::load_initial_data() submits Branches + Tags requests
  → Worker fetches via icechunk API, sends DataResponse via channel
  → App::drain_responses() updates LoadState cache
  → UI renders from LoadState (NotRequested | Loading | Loaded | Error)
  → User navigates → new DataRequests submitted → lazy loading
```

## Component Architecture

```
┌─────────────┐     DataRequest     ┌──────────────┐    icechunk     ┌────────────┐
│  Components  │ ─────────────────> │   DataStore   │ ──────────────> │  Worker    │
│  (UI state)  │ <───────────────── │   (cache)     │ <────────────── │  (Arc<Repo>)│
└─────────────┘     LoadState       └──────────────┘   DataResponse   └────────────┘
       │                                  │
       │         ┌─────────┐              │
       └────────>│   App   │<─────────────┘
                 │(coordinator)
                 └─────────┘
```

- **Components** own UI state (selection, scroll). Never touch icechunk.
- **DataStore** owns cached data as `LoadState<T>`. Lives on main thread.
- **Worker** owns `Arc<Repository>`, runs in background tokio task.
- **App** is the coordinator: routes events, processes Actions, drains responses.

## Async Model

- tokio runtime, UI on main thread
- `tokio::select!` with crossterm `EventStream` + 16ms timeout (~60fps)
- All I/O spawned as tokio tasks in the worker
- `mpsc::unbounded_channel` for request/response
- `drain_responses()` called each frame before rendering

## Navigation

Actions carry transition-specific data (`NavigationTarget::Log { branch }`).
App also maintains `NavigationContext` (current_branch, current_snapshot, current_path)
as convenience state derived from the latest navigation.

## Design Principles

- **Thin layer on icechunk** — presentation only, never override icechunk defaults
- **Metadata-first** — compute from cheap repo info before fetching snapshots/manifests
- **LoadState pattern** — explicit four-state enum, components always know what to render
- **Buy over build** — icechunk does parsing, caching, storage; we just display

## Performance Shortcuts from the Format

- All lists are sorted → binary search for lookups
- Ancestry encoded as parent_offset index → O(1) parent lookup
- Snapshot metadata in repo file → no need to fetch snapshot files for timestamps/messages
- ManifestRef extents → know which manifest covers which chunks
- Node paths sorted component-wise → efficient tree reconstruction
