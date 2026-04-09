# Next Session Plan

## What was done last session

- Error classification (auth/network/not-found) with retry (R key), improved status bar
- Ops log as dedicated detail pane tab (Node / Repo / Ops Log), plus CLI and MCP
- Search candidate caching (invalidated on data change)
- VCC display: `__al_source` resolves to `org/repo → bucket (platform)`
- Aggregated virtual sources in repo overview with de-duplicated counts
- Background chunk stats scan: drip-fed at 4 concurrent, non-blocking startup

## Priority 1: Agent Testing & Real-World Polish

The MCP server and CLI output are functional but untested with real agents.

- **MCP server**: Configure in Claude Code settings, have an agent investigate a real arraylake repo. Capture friction points.
- **CLI markdown**: Have an agent pipe `--output md` commands. Is the output token-efficient and actionable?
- **Large repo perf**: Test fuzzy search and background scan on repos with 100+ arrays. Does the drip-feed feel right? Should the concurrency be tunable?
- **Arraylake repos**: Test with real org/repo on dev API. Verify VCC display, ops log, credential flow.

## Priority 2: Clipboard & Export

Users inspecting repos need to copy things out.

- **Copy to clipboard**: `y` to yank snapshot ID, array path, or current detail content
- **Export detail**: Write current detail pane content to a file (markdown or JSON)
- Use `arboard` or `cli-clipboard` crate for cross-platform clipboard

## Priority 3: Performance Refinements

- **Cache storage summary aggregation**: Currently recomputed every frame in `render_repo_overview`. Cache and invalidate when chunk_stats changes.
- **`find_node_by_path` index**: Currently O(N×M) linear scan per frame. Build a HashMap<path, &TreeNode> on data change.
- **Branch switching debounce**: Rapid j/k in branches tab fires many AllNodes requests. Debounce with a short delay.

## Priority 4: Credential & Connection UX

- **Mid-session 403**: Detect expired credentials on background fetch errors. Show "credentials expired — press R to retry" prominently (not just in error widget).
- **Esc to cancel**: Allow cancelling a long initial connection with Esc (requires CancellationToken in the worker).
- **Partial load**: If ancestry loads but tree fails, show what we have instead of full error.

## Priority 5: Deeper Inspection Views

- **Per-manifest stats**: Break down chunk stats by manifest (useful for repos with multiple manifests per array).
- **Virtual refs table**: Dedicated view showing all virtual refs for an array: source URL, offset, length, grouped by source file.
- **Transaction log detail**: Expand diff view to show actual chunk-level changes, not just counts.

## Architecture Notes

Read these memory files for context:
- `feedback_data_model.md` — centralized state setters, NEVER bypass
- `feedback_reactive_navigation.md` — all navigation reactively updates dependents
- `feedback_reusable_components.md` — shared UI helpers, don't duplicate
- `reference_arraylake_integration.md` — how al:org/repo works
- `feedback_reactive_displays.md` — cache keys must include snapshot+branch context
- `feedback_detail_on_select.md` — detail pane updates on j/k selection, not Enter
