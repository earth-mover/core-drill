# Next Session Plan

## Priority 1: Agent Testing & Polish

The MCP server and CLI output modes need real-world testing with agents. Key areas:

- **MCP server with Claude Code**: Configure as an MCP server in Claude Code settings, have an agent use it to investigate a real arraylake repo. Capture friction.
- **CLI markdown output**: Have an agent pipe `--output md` commands and verify the output is token-efficient and actionable.
- **Search UX**: Test fuzzy search on large repos — does it perform well? Does the tree filtering feel right?
- **Arraylake repos**: Test with real org/repo on dev API. Virtual chunk display needs verification.

## Priority 2: Virtual Chunk Deep Inspection

Virtual chunks are a key use case. Current state:
- We show virtual chunk counts, sizes, and source prefixes per array
- VCC prefixes show `__al_source: path/` but the actual base URL is unknown for arraylake-managed containers
- Repo overview shows "Virtual Sources" section from config (empty for arraylake repos since VCC is session-level)

**To do:**
- Consider calling arraylake API to resolve VCC container → actual S3 prefix mapping
- Add a dedicated "Virtual Refs" view that shows a table of all virtual refs for an array: source URL, offset, length
- Show whether virtual sources are accessible (HEAD check?)

## Priority 3: Ops Log

The `OpsLog` subcommand/tab is stubbed. Implement:
- `repo.ops_log()` returns mutation history
- Display in a new bottom tab or as a CLI subcommand
- Show: operation type, timestamp, user, affected paths

## Priority 4: Graceful Error Handling

- **Network timeout**: Show "connecting..." in status bar, allow Esc to cancel
- **Mid-session 403**: Detect expired credentials, show "credentials expired — press R to retry"
- **Corrupt/missing repo**: Clear error message instead of backtrace
- **Partial load**: If ancestry loads but tree fails, show what we have

## Priority 5: Performance for Large Repos

- Cache search candidates (currently rebuilt every keypress)
- Cache Storage Summary aggregation (currently computed every frame)
- Consider `find_node_by_path` index (currently O(N×M) linear scan per frame)
- Branch switching debounce for rapid j/k navigation

## Priority 6: Quality of Life

- Copy snapshot ID / array path to clipboard
- Export detail pane content to file
- Branch/tag timestamp enrichment from ancestry cache
- More array attributes in CLI/MCP output (zarr attributes, not just metadata)

## Architecture Notes

Read these memory files for context:
- `feedback_data_model.md` — centralized state setters, NEVER bypass
- `feedback_reactive_navigation.md` — all navigation reactively updates dependents
- `feedback_reusable_components.md` — shared UI helpers, don't duplicate
- `reference_arraylake_integration.md` — how al:org/repo works
- `feedback_reactive_displays.md` — cache keys must include snapshot+branch context
