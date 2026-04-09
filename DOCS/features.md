# core-drill Feature Reference

Complete inventory of all features, keybindings, and modes.

## Modes

### Interactive TUI (default)
```bash
core-drill ./my-repo
core-drill s3://bucket/prefix
core-drill al:org/repo
```
Three-pane layout: Sidebar (tree) | Detail (tabs) | Bottom (tabs).

### Structured Output
```bash
core-drill <repo> --output json [subcommand]
core-drill <repo> --output md [subcommand]
```
Non-interactive. Prints to stdout for scripting and agent consumption.

### REPL Mode
```bash
core-drill <repo> --repl [--output md]
```
Persistent session. Reads subcommands from stdin, one per line. Responses separated by `---END---`. Keeps the repo connection open between queries.

### MCP Server
```bash
core-drill <repo> --serve
```
Exposes repo inspection as MCP tools on stdio for AI agents.

---

## Repository Sources

| Syntax | Backend |
|--------|---------|
| `./path` or `/abs/path` | Local filesystem |
| `s3://bucket/prefix` | AWS S3 |
| `al:org/repo` | Arraylake (credentials from `~/.arraylake/token.json`) |
| `--region us-east-1` | Override S3 region |
| `--endpoint-url URL` | S3-compatible (MinIO, R2, Tigris) |
| `--arraylake-api URL` | Override Arraylake API endpoint |

---

## TUI Layout

### Panes
- **[1] Sidebar** — Node tree (groups and arrays). Expand/collapse with Enter.
- **[2] Detail** — Context-dependent. Three tabs: **Node**, **Repo**, **Ops Log**.
- **[3] Bottom** — Three tabs: **Snapshots**, **Branches**, **Tags**.

### Detail Pane Tabs

| Tab | Shows |
|-----|-------|
| **Node** | Array metadata (shape, dimensions, codecs, chunk grid, fill value), chunk stats (inline/native/virtual breakdown, source URLs), group info. Automatically updates when selecting nodes in the sidebar or snapshots in the bottom pane. |
| **Repo** | Repository overview: identity, branch, contents (branch/tag/snapshot counts), storage summary (arrays, groups, chunks, size breakdown), virtual sources (aggregated across all arrays, resolved VCC names), configuration (spec version, status, inline threshold), feature flags, virtual chunk containers. |
| **Ops Log** | Full mutation history from `repo.ops_log()`. Shows timestamp and operation for every repo mutation: commits, branch/tag creates/deletes, config changes, GC runs, migrations, etc. |

### Bottom Pane Tabs

| Tab | Shows |
|-----|-------|
| **Snapshots** | Ancestry chain for the current branch. Selecting a snapshot loads its tree and diff. |
| **Branches** | All branches sorted ("main" first). Selecting a branch switches the active branch. |
| **Tags** | All tags. Selecting a tag loads its snapshot. |

---

## Keybindings

### Global (work in any pane)
| Key | Action |
|-----|--------|
| `q` | Quit |
| `?` | Toggle help overlay |
| `t` | Toggle bottom pane visibility |
| `R` | Retry all failed data fetches |
| `1` | Focus sidebar |
| `2` | Focus detail pane |
| `3` | Focus bottom pane (opens it if hidden) |
| `d` | Scroll detail down (half-page) |
| `u` | Scroll detail up (half-page) |
| `/` | Start fuzzy search (sidebar or bottom pane) |
| `Ctrl+h/j/k/l` | Move between panes (passes through to zellij/tmux at edges) |

### Navigation (pane-local)
| Key | Sidebar | Detail | Bottom |
|-----|---------|--------|--------|
| `j` / `↓` | Select next node | Scroll down | Select next item |
| `k` / `↑` | Select prev node | Scroll up | Select prev item |
| `Enter` | Expand/collapse group | — | Select item (branch/snapshot/tag) |
| `h` / `←` | — | Switch tab left (or focus sidebar) | Switch tab left |
| `l` / `→` | Focus detail | Switch tab right | Switch tab right |
| `Tab` | Next pane | Next detail tab | Next bottom tab |
| `Shift+Tab` | Prev pane | Prev detail tab | Prev bottom tab |

### Search Mode (activated by `/`)
| Key | Action |
|-----|--------|
| Type | Fuzzy filter the current list |
| `↑` / `↓` | Navigate matches |
| `Enter` | Select match and exit search |
| `Esc` | Cancel search |
| `Backspace` | Delete character (empty = cancel) |

### Mouse
- Click a pane to focus it
- Click a row to select it

---

## Data Features

### Auto-expand Tree
On initial load, single-child groups are automatically expanded to show the first meaningful branching point.

### Background Chunk Stats Scan
After the tree loads, chunk stats (inline/native/virtual breakdown) are progressively fetched for all arrays. The Repo tab's Storage Summary and Virtual Sources sections update in real time. Requests are drip-fed (max 4 concurrent) to avoid blocking the UI.

### Snapshot Diffs
When a snapshot is selected in the bottom pane, the detail pane shows what changed: added/deleted/modified arrays and groups, chunk changes. Uses transaction logs (1 S3 fetch per diff).

### Virtual Chunk Source Resolution
For Arraylake repos, `__al_source` VCC containers are resolved to the actual org/repo and bucket name (e.g., `myorg/myrepo → my-bucket (S3): data/path/`).

### Error Classification
Network errors are classified as auth (401/403), network (timeout, DNS), not-found (404), or generic. Status bar and error widgets show actionable hints (e.g., "credentials may be expired — R to retry").

---

## CLI Subcommands

All subcommands work with `--output json` and `--output md`.

| Command | Description |
|---------|-------------|
| `info` | Repository overview (status, branches, tags, snapshot count) |
| `branches` | List all branches with snapshot IDs |
| `tags` | List all tags with snapshot IDs |
| `log [-r ref] [-n limit]` | Snapshot history with ancestry |
| `tree [-r ref] [-p path]` | Node tree at a given ref, optional path filter |
| `ops-log [-n limit]` | Operations log (mutation history) |

### Examples
```bash
# Overview in markdown
core-drill s3://bucket/repo --output md

# JSON tree filtered to a path
core-drill al:org/repo --output json tree -p /data/temperature

# Last 10 operations
core-drill ./my-repo --output md ops-log -n 10

# REPL session for an agent
core-drill al:org/repo --repl --output md
```

---

## MCP Tools

When running with `--serve`, these tools are exposed:

| Tool | Params | Description |
|------|--------|-------------|
| `info` | — | Repository overview (branches, tags, recent snapshots, full tree) |
| `branches` | — | List all branches with snapshot IDs |
| `tags` | — | List all tags with snapshot IDs |
| `log` | `ref`, `limit` | Snapshot history for a branch/tag/snapshot |
| `tree` | `ref`, `path` | Node tree; use `path` to get detailed array metadata |

### Planned MCP tools (not yet implemented)
- `ops-log` — Mutation history
- `diff` — Snapshot diff (added/deleted/modified nodes)
- `array-detail` — Full array inspection with chunk stats
- `config` — Repository config and feature flags

---

## Multiplexer Integration

Ctrl+hjkl navigation at pane edges delegates to the terminal multiplexer:
- **Zellij**: Detected automatically, uses `zellij action move-focus`
- **Tmux**: Detected automatically, uses `tmux select-pane`

When not at an edge, Ctrl+hjkl moves between core-drill's own panes.
