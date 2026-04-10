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
core-drill s3://icechunk-public-data/v1/era5_weatherbench2
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

## Aliases

Save frequently-used repos as short names:

```bash
# Add aliases
core-drill alias add era5 s3://icechunk-public-data/v1/era5_weatherbench2 --anonymous
core-drill alias add myrepo al:myorg/myrepo
core-drill alias add dev-repo al:myorg/myrepo --arraylake-api dev

# Use them anywhere
core-drill era5
core-drill myrepo --output json info
core-drill dev-repo

# List / remove
core-drill alias list
core-drill alias rm era5
```

Storage flags (`--region`, `--anonymous`, `--endpoint-url`, `--arraylake-api`) are saved with the alias. CLI flags override alias values when both are present.

The `--arraylake-api` flag accepts full URLs or shorthands: `dev` (dev.api.earthmover.io) and `prod` (api.earthmover.io).

Aliases are stored in `~/.config/core-drill/config.toml` (Linux) or `~/Library/Application Support/core-drill/config.toml` (macOS).

## Updating

```bash
core-drill self-update
```

## Tab completion

```bash
core-drill install-completions
```

Auto-detects your shell and adds completion setup to `~/.zshrc`, `~/.bashrc`, or `~/.config/fish/config.fish`. Completions include subcommands, flags, and alias names. Restart your shell or `source` the config to activate.

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
