# UI Design

## Modes

### Interactive TUI (default)
Full-screen ratatui application with three-pane layout, keyboard and mouse navigation.

### Structured Output (`--output json|table`)
Non-interactive. Prints requested data to stdout. Designed for:
- Piping to jq, grep, etc.
- Agent/LLM consumption
- Scripting and automation

## TUI Layout

```
+--Status Bar----------------------------------------------+
| repo: /path/to/repo                          ready       |
+--[1] Tree-----+--[2] Detail-----------------------------+|
| main v        | (context-dependent)                     ||
| v root        |                                         ||
|   v group_a   | Array: shape, zarr metadata, dims       ||
|     - arr_1   | Group: path, child count, child list    ||
|     - arr_2   | Repo overview: branch/tag/snap counts   ||
|   > group_b   | Snapshot diff: added/removed/modified   ||
|               |                                         ||
+---------------+-----------------------------------------+|
| [3] Bottom: [*Snapshots] [Branches] [Tags]              ||
|  Snapshot      Time              Message                 ||
|  a1b2c3d4e5f6  2025-03-15 14:30  initial commit         ||
|> f7e8d9c0b1a2  2025-03-16 09:15  add temperature data   ||
+----------------------------------------------------------+
| q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:nav   |
+----------------------------------------------------------+
```

### Sidebar (Pane 1)

- **Branch selector** at top: shows current branch name with dropdown indicator
- **Node tree** below: hierarchical view of groups and arrays
  - Groups show expand/collapse indicator (v open, > closed)
  - Arrays show inline shape summary: `arr_name [100x200x3]`
  - Uses `tui_tree_widget` for stateful expand/collapse
  - Auto-expands on load: drills through single-child groups, opens all children when all are groups

### Detail (Pane 2)

Context-dependent content based on what is selected:

| Selection | Detail Content |
|-----------|---------------|
| Array node in sidebar | Array metadata: name, path, shape, dimensions, manifest count, zarr metadata (data type, chunk shape, codecs, fill value) |
| Group node in sidebar | Group info: path, child count, child listing with type icons |
| Nothing selected | Repo overview: repository URL, current branch, branch/tag/snapshot counts |
| Snapshot in bottom panel (when focused) | Snapshot diff: parent->child comparison showing added/removed/modified arrays and groups, chunk change counts |

#### Array Detail — Section Breakdown

The array detail is rendered in two functions (`src/ui/mod.rs`):

- `render_array_detail_header` — upper sections (Shape & Layout)
- `render_array_detail_storage` — lower sections (Storage, Chunk Types, Attributes, Raw Metadata)

**Shape & Layout section** (from `ArraySummary` + parsed `ZarrMetadata`):
- Array name and path
- Shape (e.g. `100 × 200 × 3`)
- Chunk shape (from zarr metadata)
- Data type
- Dimension names
- Chunks per dimension: `ceil(shape[i] / chunk_shape[i])` per axis
- Storage order (v2 C/F)
- Chunk grid summary line (textual, from `shape_viz::chunk_summary_line`)

**Storage section** (from `ZarrMetadata`):
- Codec chain / compressor
- Fill value, zarr format, dimension separator
- Storage transformers
- Manifest count

**Chunk Types section** (from `ArraySummary::total_chunks` + async `ChunkStats`):

| State | What's shown |
|-------|-------------|
| `NotRequested` / `None` | Total from `summary.total_chunks` (cheap, from snapshot manifest metadata) + Initialized fraction |
| `Loading` | Total with "(loading type breakdown...)" + Initialized fraction |
| `Loaded(stats)` | Full breakdown: native / inline / virtual counts, sizes, virtual source URLs + Initialized fraction |
| `Error` | Error message |

**Initialized fraction** — added to all states where `total_chunks` is known:
- Formula: `written_chunks / grid_chunks` where `grid_chunks = ∏ ceil(shape[i] / chunk_shape[i])`
- Displayed as: `X of Y  (Z%)`
- Computed by `compute_grid_chunks(summary, meta)` in `src/ui/mod.rs`
- Requires both `summary.shape` and `meta.chunk_shape` to be non-empty and same length
- Sparse arrays (e.g. ERA5 on a sparse grid) will show low percentages

**Attributes section**: key/value pairs from zarr metadata rendered with `json_view`.

**Raw Metadata section**: extra unrecognized zarr metadata fields, rendered with `json_view`.

### Bottom Panel (Pane 3)

Toggleable (press `t`) panel with three tabs:

- **Snapshots**: table with snapshot ID (truncated), timestamp, message. Selecting a row auto-requests its diff.
- **Branches**: list of branch names
- **Tags**: list of tag names

Tab switching: `Tab`/`Shift+Tab` cycles tabs when bottom panel is focused.

## Navigation

### Pane Focus

| Key | Action |
|-----|--------|
| `Tab` | Cycle panes forward: Sidebar -> Detail -> Bottom -> Sidebar |
| `Shift+Tab` | Cycle panes backward |
| `1` / `2` / `3` | Jump directly to Sidebar / Detail / Bottom |
| `Ctrl+h` / `Ctrl+Left` | Move focus left (or pass to multiplexer at edge) |
| `Ctrl+l` / `Ctrl+Right` | Move focus right (or pass to multiplexer at edge) |
| `Ctrl+j` / `Ctrl+Down` | Move focus down (or pass to multiplexer at edge) |
| `Ctrl+k` / `Ctrl+Up` | Move focus up (or pass to multiplexer at edge) |

### Within Panes

| Key | Action |
|-----|--------|
| `j` / `Down` | Move selection down / scroll down |
| `k` / `Up` | Move selection up / scroll up |
| `Enter` | Expand/collapse group (sidebar), select item (bottom) |

### Global

| Key | Action |
|-----|--------|
| `q` | Quit |
| `?` | Toggle help overlay |
| `t` | Toggle bottom panel visibility |

### Mouse

- Click to focus a pane and select the clicked row
- Hit-testing against stored layout rects (sidebar, detail, bottom)

### Multiplexer Passthrough

When running inside zellij or tmux, `Ctrl+hjkl` at a pane edge passes focus
to the multiplexer instead of wrapping within core-drill. Detected automatically
via `$ZELLIJ` / `$TMUX` environment variables.

## Loading States

All async fetches show status in the status bar ("connecting...", "ready", "error").
Content areas show "Loading..." placeholder until data arrives. The LoadState enum
(`NotRequested | Loading | Loaded | Error`) ensures every component knows exactly
what to render at all times.
