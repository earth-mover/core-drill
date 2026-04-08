# Architecture

## Dual Output Modes

- **TUI mode** (default): full-screen ratatui app with three-pane layout, keyboard + mouse navigation
- **Structured mode** (`--output json|table`): prints data to stdout, no interactive UI — designed for piping and agent consumption

Both modes share the same data-fetching and parsing layers; only the rendering differs.

## Module Structure

```
src/
  main.rs              — entry point, mode dispatch
  cli.rs               — clap CLI definition with rich --help text
  app.rs               — coordinator: owns DataStore, Theme, pane focus, layout areas
  tui.rs               — terminal setup/teardown (mouse capture), tokio::select event loop
  repo.rs              — thin wrapper for opening repos (local, S3, GCS, Azure, HTTP)
  theme.rs             — Earthmover brand colors, shared styles, widget helpers
  multiplexer.rs       — zellij/tmux detection and focus passthrough
  store/
    mod.rs             — DataStore, LoadState, DataRequest/Response, background worker
    types.rs           — owned domain types (BranchInfo, TagInfo, TreeNode, DiffSummary, etc.)
  component/
    mod.rs             — Pane/BottomTab enums, Action enum, Component trait
  ui/
    mod.rs             — three-pane layout: sidebar, detail, bottom panel, status/hint bars
    format.rs          — zarr metadata parser for human-friendly display
    help.rs            — full-screen keybinding help overlay
    tree.rs            — flat-tree renderer with connectors (legacy; sidebar now uses tui_tree_widget)
```

## Data Flow

```
CLI args -> repo::open (detect backend) -> Repository
  -> DataStore::new(repo) spawns background worker
  -> App::load_initial_data() submits Branches + Tags + AllNodes + Ancestry
  -> Worker fetches via icechunk API, sends DataResponse via channel
  -> App::drain_responses() updates LoadState cache
  -> Auto-expand tree once AllNodes arrives (drill through single-child groups)
  -> UI renders from LoadState (NotRequested | Loading | Loaded | Error)
  -> User navigates -> new DataRequests submitted (e.g. SnapshotDiff on selection)
```

### AllNodes Pattern

Instead of lazy-loading children per group, `AllNodes` fetches the entire node tree
for a branch in a single `list_nodes("/")` call. The worker organizes nodes by parent
path into a `HashMap<String, Vec<TreeNode>>`, populating the entire `node_children`
cache at once. Expanding a group in the sidebar never triggers another fetch.

## Three-Pane Layout

```
+--Status Bar----------------------------------------------+
| repo: /path/to/repo                         ready        |
+--[1] Sidebar--+-[2] Detail---------------------------+   |
| branch: main  | Array / Group / Repo overview /      |   |
|   node tree   | Snapshot diff (context-dependent)    |   |
|   expand/     |                                      |   |
|   collapse    |                                      |   |
+---------------+--------------------------------------+   |
| [3] Bottom: [Snapshots] [Branches] [Tags]            |   |
|   snapshot log / branch list / tag list               |   |
+-------------------------------------------------------+  |
| hint bar: q:quit  ?:help  t:toggle log  ...              |
+----------------------------------------------------------+
```

- **Sidebar**: branch selector + hierarchical tree (tui_tree_widget)
- **Detail**: context-dependent — array metadata, group children, repo overview, or snapshot diff
- **Bottom**: toggleable panel with Snapshots/Branches/Tags tabs

### Pane Focus Model

Navigation uses a `Pane` enum (`Sidebar`, `Detail`, `Bottom`) instead of a `View` enum.
The `App.focused_pane` field determines which pane receives keyboard input. Focus moves
between panes via `Tab`/`Shift+Tab` (pane cycling), `1`/`2`/`3` (direct jump), or
`Ctrl+hjkl` (directional, with multiplexer passthrough at edges).

## Component Architecture

```
+-------------+     DataRequest     +--------------+    icechunk     +------------+
|  App        | -----------------> |   DataStore   | --------------> |  Worker    |
|  (pane      | <----------------- |   (cache)     | <-------------- |  (Arc<Repo>)|
|   focus,    |     LoadState      +--------------+   DataResponse   +------------+
|   layout)   |
+-------------+
```

- **App** is the coordinator: owns pane focus, layout areas, tree state, selection indices.
  Routes key/mouse events, processes Actions, drains responses, auto-requests diffs.
- **DataStore** owns cached data as `LoadState<T>`. Lives on main thread.
  Caches: branches, tags, ancestry (per branch), node_children (per parent path), diffs (per snapshot).
- **Worker** owns `Arc<Repository>`, runs in background tokio task.
  Each request spawns its own sub-task so they don't block each other.
- **Component trait** exists for future standalone components but is not yet used;
  the current UI renders directly from App + DataStore state.

## Multiplexer Integration

`multiplexer.rs` detects zellij or tmux at startup via environment variables (`ZELLIJ`, `TMUX`).
When the user presses `Ctrl+hjkl` at an edge pane (e.g., `Ctrl+h` while already in Sidebar),
the keystroke is passed through to the multiplexer (`zellij action move-focus` or
`tmux select-pane`) so it can switch to its own adjacent pane. Fire-and-forget, never blocks UI.

## Mouse Support

`tui.rs` enables `EnableMouseCapture` on init and `DisableMouseCapture` on restore.
`App::handle_mouse` performs hit-testing against stored layout `Rect`s (`sidebar_area`,
`detail_area`, `bottom_area`) to focus the clicked pane and select the clicked row.

## Async Model

- tokio runtime, UI on main thread
- `tokio::select!` with crossterm `EventStream` + 16ms timeout (~60fps)
- All I/O spawned as tokio tasks in the worker (one sub-task per request)
- `mpsc::unbounded_channel` for request/response
- `drain_responses()` called each frame before rendering

## Design Principles

- **Thin layer on icechunk** — presentation only, never override icechunk defaults
- **Metadata-first** — compute from cheap repo info before fetching snapshots/manifests
- **LoadState pattern** — explicit four-state enum, components always know what to render
- **Buy over build** — icechunk does parsing, caching, storage; we just display
- **Single-fetch tree** — AllNodes loads the whole tree upfront; no lazy per-group fetches

## Performance Shortcuts from the Format

- All lists are sorted -> binary search for lookups
- Ancestry encoded as parent_offset index -> O(1) parent lookup
- Snapshot metadata in repo file -> no need to fetch snapshot files for timestamps/messages
- ManifestRef extents -> know which manifest covers which chunks
- Node paths sorted component-wise -> efficient tree reconstruction
- AllNodes returns the entire tree in one call -> no waterfall of requests
