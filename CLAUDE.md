# core-drill

Terminal UI for inspecting Icechunk V2 repositories — local and remote.

## Tech Stack

Rust, Ratatui (crossterm), tokio, object_store, flatbuffers, clap

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

## Critical Rules

- Never block the UI thread — use `tokio::spawn` for all storage fetches
- All storage operations return raw bytes; parsing is a separate step
- Sorted lists in icechunk format enable binary search — use it
- The repo info file is the single cheap entry point — extract max value from it before fetching snapshots/manifests

## Documentation

- `PLANNING/architecture.md` — module structure, data flow, async model
- `PLANNING/roadmap.md` — phased delivery plan
- `DOCS/icechunk-v2-format.md` — binary format quick reference
- `DOCS/ui-design.md` — TUI layout and interaction design
