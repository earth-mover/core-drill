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

## Install

```bash
cargo install --git ssh://git@github.com/earth-mover/core-drill
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
claude mcp add --transport stdio core-drill -- core-drill --serve
```

Then from Claude Code, ask the agent to investigate repo path/URL to start exploring.

## Design

core-drill tries to be **fast**, not light. It aggressively fetches and caches metadata so navigation feels instant — branches, tags, ancestry, tree, and chunk stats are all kept in memory once loaded. On a slow S3 connection the first load may take a moment, but subsequent interactions are immediate.

## License

MIT
