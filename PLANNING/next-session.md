# Next Session Plan

## What was done last session

### Error handling & UX
- Error classification (auth/network/not-found) with retry (R key), improved status bar
- Animated loading screen during repo open (ASCII ice core drill)
- Detail pane defaults to Repo tab on startup

### TUI features
- Ops log as dedicated detail pane tab (Node / Repo / Ops Log)
- Background chunk stats scan: drip-fed at 4 concurrent, non-blocking
- Aggregated virtual sources in repo overview with de-duplicated counts
- Data size display (total including virtual sources, stored vs referenced breakdown)
- VCC display: `__al_source` resolves to `org/repo → bucket (platform)`
- Search candidate caching (invalidated on data change)

### MCP server (major)
- **Repo-agnostic**: `--serve` no longer requires a repo arg. Agents call `open` to connect.
- **10 tools**: open, info, branches, tags, log, tree, ops_log, diff, config, search
- **Shared fetch functions**: output.rs is canonical, store/mod.rs delegates to it
- **Parallel fetches**: info tool uses tokio::join! for branches/tags/ancestry/tree
- **Server instructions**: guides agents through open → info → drill-down workflow
- **Timing logs**: all tools log elapsed time via tracing::info (RUST_LOG=info)
- **UTC timestamps**: all displayed times include UTC suffix

### Architecture
- Consolidated duplicate fetch logic: store/mod.rs delegates to output.rs
- DiffDetail type for resolved-path diffs (MCP/CLI use)
- resolve_ref_to_snapshot_id helper for branch→snapshot resolution

## Priority 1: Test MCP with agent and iterate

The MCP now has feature parity. Test it:
1. Rebuild: `cargo build`
2. MCP is already configured: `claude mcp add --transport stdio core-drill -- ./target/debug/core-drill --serve`
3. Have an agent explore an Arraylake repo using ALL 10 tools
4. Also test with a larger repo (e.g., ERA5 WeatherBench2) for performance
5. Fix any issues found

## Priority 2: Remaining agent feedback

From the first agent test session:
- **Zarr attributes**: Show user attributes in tree detail when present
- **JSON output mode**: Optional format param on all tools for structured extraction
- **Snapshot detail tool**: Inspect a single snapshot by ID for full metadata

## Priority 3: CLI feature parity

The CLI (`--output md/json`) is missing several features that MCP now has:
- `diff` subcommand
- `config` subcommand  
- `search` subcommand
- Chunk stats in tree output

## Priority 4: Performance

- Cache storage summary aggregation in TUI (recomputed every frame)
- find_node_by_path index (currently O(N×M) per frame)
- Branch switching debounce

## Priority 5: Clipboard & Export

- `y` to yank snapshot ID, array path, or detail content
- Export detail pane to file

## MCP Installation

```bash
# Build
cargo build

# Add to Claude Code (already configured if done previously)
claude mcp add --transport stdio core-drill -- ./target/debug/core-drill --serve
```

## Architecture Notes

- `output.rs` is the canonical source for all fetch functions (fetch_branches, fetch_tags, fetch_ancestry, fetch_tree_flat, fetch_ops_log, fetch_chunk_stats, fetch_diff, fetch_repo_config, resolve_ref_to_snapshot_id)
- `store/mod.rs` delegates to output.rs for chunk_stats, repo_config, ops_log
- `mcp.rs` calls output.rs directly
- All MCP tools use `require_repo!` macro and timing logs
