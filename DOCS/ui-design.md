# UI Design

## Modes

### Interactive TUI (default)
Full-screen ratatui application with panel-based layout and keyboard navigation.

### Structured Output (`--output json|table`)
Non-interactive. Prints requested data to stdout. Designed for:
- Piping to jq, grep, etc.
- Agent/LLM consumption
- Scripting and automation

## TUI Layout

```
┌─ Status Bar ──────────────────────────────────────────────┐
│ repo: /path/to/repo  branch: main  status: Online         │
├─ Navigation ──────────────────────────────────────────────┤
│ [Overview] [Branches] [Tags] [Log] [Tree] [Ops]          │
├───────────────────────────────────────────────────────────┤
│                                                           │
│                    Main Content Area                      │
│                                                           │
├─ Detail/Preview ──────────────────────────────────────────┤
│ Selected item details                                     │
├─ Help ────────────────────────────────────────────────────┤
│ q:quit  ?:help  /:search  tab:next-panel  esc:back       │
└───────────────────────────────────────────────────────────┘
```

## Views

| View | Content | Data Source |
|------|---------|-------------|
| Overview | Repo summary: status, branches, tags, snapshot count, config | Repo info |
| Branches | Branch list with target snapshot | Repo info |
| Tags | Tag list with target snapshot | Repo info |
| Log | Snapshot history with ancestry graph | Repo info |
| Tree | Node hierarchy at a snapshot | Snapshot file |
| Array Detail | Shape, zarr metadata, dimensions, manifest refs | Snapshot file |
| Ops Log | Mutation history | Repo info |

## Keybindings

- `q` / `Ctrl-c` — quit
- `?` — toggle help overlay
- `Tab` / `Shift-Tab` — cycle panels
- `j/k` or `↑/↓` — navigate lists
- `Enter` — drill into selected item
- `Esc` / `Backspace` — go back
- `/` — search/filter
- `1-6` — jump to view by number

## Loading States

All async fetches show a spinner in the status bar. Content areas show "Loading..." placeholder until data arrives. Previously loaded data remains visible while refreshing.
