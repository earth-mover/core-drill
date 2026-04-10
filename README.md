# 🔩 core-drill

```
    ╔════════════╗
    ║  ┌──────┐  ║
    ║  │ CORE │  ║
    ║  │ DRILL│  ║
    ║  └──┬┬──┘  ║
    ╚═════╪╪═════╝
    ──────╫╫──────
    ░░░░░░╫╫░░░░░░  ◄ snow (repo name)
    ▒▒▒▒▒▒╫╫▒▒▒▒▒▒  ◄ firn (snapshots)
    ▓▓▓▓▓▓╫╫▓▓▓▓▓▓  ◄ ice  (group hierarchy)
    ██████╫╫██████  ◄ deep ice  (virtual chunk sources)
    ██████╨╨██████
```

Terminal UI + MCP for inspecting [Icechunk](https://icechunk.io) V2 repositories.

Drill deep into your Icechunk repos to discover their past.

## Try it now

No auth needed — explore a public ERA5 weather dataset on S3:

```bash
# Interactive TUI
core-drill --anon s3://icechunk-public-data/v1/era5_weatherbench2

# CLI overview
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 info

# Browse the node tree
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 tree

# Snapshot history
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 log

# Array detail with chunk stats
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 tree --path /1x721x1440/2m_temperature --chunk-stats
```

## Install

### Shell (Linux / macOS)

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/earth-mover/core-drill/releases/latest/download/core-drill-installer.sh | sh
```

### Nix

```bash
nix run github:earth-mover/core-drill -- --help
# or install permanently
nix profile install github:earth-mover/core-drill
```

### From source

```bash
cargo install --git https://github.com/earth-mover/core-drill
```

## Usage

```bash
# Local repo
core-drill ./my-repo

# S3
core-drill s3://bucket/prefix --region us-east-1

# S3-compatible (R2, MinIO, Tigris)
core-drill s3://bucket/prefix --endpoint-url https://...

# GCS
core-drill gs://bucket/prefix

# Arraylake
core-drill al:org/repo

# CLI output (markdown or JSON, no TUI)
core-drill s3://bucket/prefix --output md
core-drill s3://bucket/prefix --output json

# MCP server for AI agents
core-drill --serve
```

## MCP setup

Add core-drill as an MCP server so Claude Code (or any MCP client) can inspect Icechunk repos:

```bash
claude mcp add --scope user --transport stdio core-drill -- core-drill --serve
```

Then from Claude Code, ask the agent to investigate repo path/URL to start exploring.

## Design

core-drill tries to be **fast**, not light. It aggressively fetches and caches metadata so navigation feels instant — branches, tags, ancestry, tree, and chunk stats are all kept in memory once loaded. On a slow S3 connection the first load may take a moment, but subsequent interactions are immediate.

## License

MIT
