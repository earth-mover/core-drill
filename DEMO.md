# core-drill Demo

## Sample Repos

```
s3://icechunk-public-data/v1/era5_weatherbench2   # ERA5 weather data, 6 arrays, 1.2 TiB
```

## CLI Quick Tour

```bash
# Repo overview — auto-detects anonymous access
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 info

# Browse the full node tree
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 tree

# Array detail with chunk stats (slower — iterates all chunks)
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 tree --path /1x721x1440/2m_temperature

# Snapshot history
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 log

# All branches
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 branches

# Operations log
core-drill --output md --anon s3://icechunk-public-data/v1/era5_weatherbench2 ops-log

# JSON output (pipe to jq)
core-drill --output json --anon s3://icechunk-public-data/v1/era5_weatherbench2 info | jq .
```

## Interactive TUI

```bash
core-drill --anon s3://icechunk-public-data/v1/era5_weatherbench2
```

Key bindings: `j/k` navigate, `Tab` switch panes, `/` fuzzy search,
`zo/zc` expand/collapse, `zR/zM` expand/collapse all, `1-5` detail tabs, `q` quit.

## MCP Server for AI Agents

```bash
# Register (one-time)
claude mcp add --scope user --transport stdio core-drill -- core-drill --serve

# Then ask Claude Code to explore any repo:
```

### Agent Prompt

```
Use the core-drill MCP tools to inspect the Icechunk repository at
s3://icechunk-public-data/v1/era5_weatherbench2 (set anonymous=true).

1. Open the repo and summarize what's in it
2. Show the full node tree
3. Pick the most interesting array and get its detailed metadata including chunk stats
4. Show the snapshot history and diff the two most recent snapshots
5. Summarize your findings: what data is stored, how it's chunked, how it's evolved
```

## Storage Backends

```bash
# S3 — auto credential fallback (env creds → anonymous → helpful error)
core-drill s3://bucket/prefix
core-drill --anon s3://bucket/prefix          # skip credential probing
core-drill --region us-west-2 s3://bucket/prefix

# GCS
core-drill gs://bucket/prefix

# Azure
core-drill az://account/container/prefix

# HTTP
core-drill https://host/path

# Local
core-drill ./my-local-repo

# Arraylake
core-drill al:myorg/myrepo
```
