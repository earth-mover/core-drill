# Next Session Plan

## What was done last session

- Error classification (auth/network/not-found) with retry (R key), improved status bar
- Ops log as dedicated detail pane tab (Node / Repo / Ops Log), plus CLI and MCP
- Search candidate caching (invalidated on data change)
- VCC display: `__al_source` resolves to `org/repo → bucket (platform)`
- Aggregated virtual sources in repo overview with de-duplicated counts
- Background chunk stats scan: drip-fed at 4 concurrent, non-blocking startup

## Priority 1: MCP Server Improvements

The MCP server is the primary interface for AI agents. It needs feature parity and real-world testing.

### Missing MCP tools (build these first)
- **`ops-log`** — Expose `fetch_ops_log` as an MCP tool (matches CLI `ops-log` subcommand). Params: `limit`.
- **`diff`** — Show what changed between two snapshots (added/deleted/modified nodes, chunk changes). This is the most useful tool for agents investigating repo history. Params: `snapshot_id`, or `from`/`to` refs.
- **`array-detail`** — Detailed array inspection: shape, dtype, codecs, chunk stats (inline/native/virtual breakdown), virtual source URLs. Currently `tree --path` gives metadata but not chunk stats. Params: `path`, `ref`.
- **`config`** — Repository config, feature flags, virtual chunk containers, status. Currently only in `info` output but buried.

### MCP polish
- Tool descriptions should guide agents on WHEN to use each tool, not just what it does.
- `info` tool currently dumps everything — consider making it lighter (overview only) and pointing agents to drill-down tools.
- Test with Claude Code: configure as MCP server, have an agent investigate `al:earthmover/...` repo. Capture what's missing or confusing.

### CLI output
- Ensure `--output md` and `--output json` cover all the same data as MCP tools.
- Test token efficiency — are the markdown tables too wide? Should we truncate snapshot IDs shorter?

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
