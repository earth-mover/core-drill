//! MCP (Model Context Protocol) server for core-drill.
//!
//! Exposes repository inspection as MCP tools that agents can call.
//! The server can start with or without a pre-opened repo. Use the
//! `open` tool to connect to any repo on demand.
//!
//! Start with: `core-drill --serve`
//! Or pre-open: `core-drill <repo> --serve`

use std::sync::Arc;

use humansize::{format_size, BINARY};
use icechunk::Repository;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    schemars, tool, tool_handler, tool_router,
};
use tokio::sync::RwLock;

use tracing::info;

use crate::output;
use crate::repo;

/// MCP server wrapping an open icechunk repository.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CoreDrillServer {
    tool_router: ToolRouter<Self>,
    repo: Arc<RwLock<Option<Arc<Repository>>>>,
    repo_url: Arc<RwLock<String>>,
}

impl CoreDrillServer {
    pub fn new(repo: Option<Repository>, repo_url: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            repo: Arc::new(RwLock::new(repo.map(Arc::new))),
            repo_url: Arc::new(RwLock::new(repo_url)),
        }
    }
}

// ─── Tool parameter structs ─────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EmptyParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct OpenParams {
    /// Path, URL, or Arraylake reference (e.g., "./my-repo", "s3://bucket/prefix", "al:org/repo")
    repo: String,
    /// Cloud storage region (optional, for S3)
    region: Option<String>,
    /// Storage endpoint URL (optional, for S3-compatible services)
    endpoint_url: Option<String>,
    /// Arraylake API endpoint (optional, defaults to https://dev.api.earthmover.io)
    arraylake_api: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LogParams {
    /// Branch name, tag name, or snapshot ID (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
    /// Maximum number of snapshots to return
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TreeParams {
    /// Branch name, tag name, or snapshot ID (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
    /// Filter to a specific path (e.g. "/stations/latitude") for detailed metadata
    path: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct InfoParams {
    /// Branch, tag, or snapshot ID to show overview for (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct OpsLogParams {
    /// Maximum number of entries to return
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DiffParams {
    /// Snapshot ID to show changes for
    snapshot_id: String,
    /// Parent snapshot ID (optional — if omitted, auto-detected from ancestry)
    parent_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// Fuzzy search query
    query: String,
    /// Branch, tag, or snapshot ID (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
    /// Maximum results to return (default: 20)
    limit: Option<usize>,
}

fn default_ref() -> String {
    "main".to_string()
}

// ─── Tool implementations ───────────────────────────────────

/// Helper macro-style function: acquire repo read lock, return error string if no repo open.
macro_rules! require_repo {
    ($self:expr) => {{
        let guard = $self.repo.read().await;
        match &*guard {
            Some(repo) => Arc::clone(repo),
            None => return "Error: No repository open. Use the `open` tool first.".to_string(),
        }
    }};
}

use crate::store::types::ChunkStats;

fn fmt_chunk_stats(stats: &ChunkStats) -> String {
    let mut out = "\n## Chunk Statistics\n\n".to_string();
    out.push_str(&format!("- **Total:** {} chunks\n", stats.total_chunks));
    if stats.stats_complete && stats.total_chunks > 0 {
        let total = stats.total_chunks as f64;
        let native_pct = (stats.native_count as f64 / total * 100.0) as u64;
        let inline_pct = (stats.inline_count as f64 / total * 100.0) as u64;
        let virtual_pct = (stats.virtual_count as f64 / total * 100.0) as u64;

        out.push_str(&format!(
            "- **Native:** {} ({}%)  {}\n",
            stats.native_count,
            native_pct,
            format_size(stats.native_total_bytes, BINARY)
        ));
        out.push_str(&format!(
            "- **Inline:** {} ({}%)  {}\n",
            stats.inline_count,
            inline_pct,
            format_size(stats.inline_total_bytes, BINARY)
        ));
        out.push_str(&format!(
            "- **Virtual:** {} ({}%)  {}  ({} sources)\n",
            stats.virtual_count,
            virtual_pct,
            format_size(stats.virtual_total_bytes, BINARY),
            stats.virtual_source_count
        ));
        let data_size =
            stats.native_total_bytes + stats.inline_total_bytes + stats.virtual_total_bytes;
        out.push_str(&format!(
            "- **Data size:** {}\n",
            format_size(data_size, BINARY)
        ));
    }
    out
}

#[tool_router]
impl CoreDrillServer {
    #[tool(
        description = "Open an Icechunk repository for inspection. Must be called before other tools. Accepts local paths, S3/GCS URLs, S3-compatible (R2, MinIO, Tigris via endpoint_url), or Arraylake refs (al:org/repo)."
    )]
    async fn open(&self, Parameters(params): Parameters<OpenParams>) -> String {
        let _start = std::time::Instant::now();
        info!("MCP open: repo={}", params.repo);
        let arraylake_api = params
            .arraylake_api
            .unwrap_or_else(|| "https://dev.api.earthmover.io".to_string());
        let overrides = repo::StorageOverrides {
            region: params.region,
            endpoint_url: params.endpoint_url,
        };

        let result = match crate::open_repo(&params.repo, &arraylake_api, &overrides).await {
            Ok((repository, identity)) => {
                let label = identity.display_short();
                let msg = format!("Opened repository: {label}");

                *self.repo.write().await = Some(Arc::new(repository));
                *self.repo_url.write().await = label;

                msg
            }
            Err(e) => format!("Error opening repository: {e}"),
        };
        info!("MCP open completed in {:?}", _start.elapsed());
        result
    }

    #[tool(
        description = "Repository overview: branches, tags, recent snapshots, and node tree. Good starting point after opening a repo."
    )]
    async fn info(&self, Parameters(params): Parameters<InfoParams>) -> String {
        let _start = std::time::Instant::now();
        info!("MCP info ref={}", params.r#ref);
        let repo = require_repo!(self);
        let repo_url = self.repo_url.read().await;

        // Fetch branches, tags, ancestry, and tree concurrently
        let (branches_res, tags_res, ancestry_res, tree_res) = tokio::join!(
            output::fetch_branches(&repo),
            output::fetch_tags(&repo),
            output::fetch_ancestry(&repo, &params.r#ref),
            output::fetch_tree_flat(&repo, &params.r#ref, None),
        );

        let branches = match branches_res {
            Ok(b) => b,
            Err(e) => return format!("Error fetching branches: {e}"),
        };
        let tags = tags_res.unwrap_or_default();

        let mut out = format!("# Repository: {}\n\n", *repo_url);

        out.push_str(&format!("## Branches ({})\n\n", branches.len()));
        for b in &branches {
            let ts = b
                .tip_timestamp
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_default();
            let msg = b.tip_message.as_deref().unwrap_or("");
            let msg_part = if msg.is_empty() {
                String::new()
            } else {
                format!(" — {msg}")
            };
            out.push_str(&format!(
                "- **{}** → `{}`  {}{}\n",
                b.name,
                output::truncate(&b.snapshot_id, 12),
                ts,
                msg_part
            ));
        }

        if !tags.is_empty() {
            out.push_str(&format!("\n## Tags ({})\n\n", tags.len()));
            for t in &tags {
                out.push_str(&format!(
                    "- **{}** → `{}`\n",
                    t.name,
                    output::truncate(&t.snapshot_id, 12)
                ));
            }
        }

        if let Ok(ancestry) = ancestry_res {
            out.push_str(&format!(
                "\n## Snapshots ({} commits on {})\n\n",
                ancestry.len(),
                params.r#ref
            ));
            out.push_str(
                "| # | Snapshot | Time | Message |\n|---|----------|------|---------|",
            );
            for (i, e) in ancestry.iter().take(5).enumerate() {
                let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC").to_string();
                out.push_str(&format!(
                    "\n| {} | `{}` | {} | {} |",
                    i + 1,
                    output::truncate(&e.id, 12),
                    ts,
                    e.message
                ));
            }
            if ancestry.len() > 5 {
                out.push_str(&format!(
                    "\n\n*({} more — use `log` tool)*",
                    ancestry.len() - 5
                ));
            }
        }

        if let Ok(tree) = tree_res {
            let main_name = &params.r#ref;
            let groups = tree.iter().filter(|n| n.is_group()).count();
            let arrays = tree.iter().filter(|n| n.is_array()).count();
            out.push_str(&format!(
                "\n\n## Tree (at {})\n\n{} groups, {} arrays\n\n",
                main_name, groups, arrays
            ));
            for node in &tree {
                out.push_str(&output::fmt_tree_line(node, &tree));
            }
        }

        out.push_str("\n---\n*Tools: `tree` (with `path` for array detail + chunk stats), `log` (history), `branches`/`tags` (refs), `diff` (snapshot changes), `ops_log` (mutation history), `config` (repo settings), `search` (fuzzy find)*");
        info!("MCP info completed in {:?}", _start.elapsed());
        out
    }

    #[tool(description = "List all branches with their tip snapshot IDs.")]
    async fn branches(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_branches(&repo).await {
            Ok(branches) => {
                let mut out = format!("# Branches ({})\n\n", branches.len());
                out.push_str("| Branch | Snapshot |\n|--------|----------|\n");
                for b in &branches {
                    out.push_str(&format!(
                        "| {} | `{}` |\n",
                        b.name,
                        output::truncate(&b.snapshot_id, 12)
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP branches completed in {:?}", _start.elapsed());
        result
    }

    #[tool(description = "List all tags with their snapshot IDs.")]
    async fn tags(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_tags(&repo).await {
            Ok(tags) if tags.is_empty() => "(no tags)".to_string(),
            Ok(tags) => {
                let mut out = format!("# Tags ({})\n\n", tags.len());
                for t in &tags {
                    out.push_str(&format!(
                        "- **{}** → `{}`\n",
                        t.name,
                        output::truncate(&t.snapshot_id, 12)
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP tags completed in {:?}", _start.elapsed());
        result
    }

    #[tool(
        description = "Show snapshot history (commit log) for a branch, tag, or snapshot ID."
    )]
    async fn log(&self, Parameters(params): Parameters<LogParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_ancestry(&repo, &params.r#ref).await {
            Ok(entries) => {
                let total = entries.len();
                let display: Vec<_> = if let Some(n) = params.limit {
                    entries.into_iter().take(n).collect()
                } else {
                    entries
                };
                let mut out = format!(
                    "# Snapshot Log ({}, {} total commits)\n\n",
                    params.r#ref, total
                );
                out.push_str(
                    "| # | Snapshot | Time | Message |\n|---|----------|------|---------|",
                );
                for (i, e) in display.iter().enumerate() {
                    let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC").to_string();
                    out.push_str(&format!(
                        "\n| {} | `{}` | {} | {} |",
                        i + 1,
                        output::truncate(&e.id, 12),
                        ts,
                        e.message
                    ));
                }
                if let Some(n) = params.limit {
                    if n < total {
                        out.push_str(&format!("\n\n*Showing {} of {} commits*", n, total));
                    }
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP log completed in {:?}", _start.elapsed());
        result
    }

    #[tool(
        description = "Browse the node tree. Without `path`: lists all groups and arrays. With `path`: shows detailed array metadata (shape, dtype, chunks, codecs, fill value, chunk statistics)."
    )]
    async fn tree(&self, Parameters(params): Parameters<TreeParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_tree_flat(&repo, &params.r#ref, params.path.as_deref()).await {
            Ok(tree) => {
                if let Some(ref filter_path) = params.path {
                    if let Some(node) = tree.iter().find(|n| n.path == *filter_path) {
                        let mut out = output::fmt_node_detail(node);
                        // Append chunk stats for arrays
                        if node.is_array() {
                            if let Ok(snap_id) = output::resolve_ref_to_snapshot_id(&repo, &params.r#ref).await {
                                if let Ok(stats) = output::fetch_chunk_stats(&repo, &snap_id, &node.path).await {
                                    out.push_str(&fmt_chunk_stats(&stats));
                                }
                            }
                        }
                        out
                    } else if tree.is_empty() {
                        format!("No nodes found at path: {filter_path}")
                    } else {
                        let mut out = format!("# Tree: {} ({} nodes)\n\n", filter_path, tree.len());
                        for node in &tree {
                            out.push_str(&output::fmt_node_detail(node));
                            out.push('\n');
                        }
                        out
                    }
                } else {
                    let groups = tree.iter().filter(|n| n.is_group()).count();
                    let arrays = tree.iter().filter(|n| n.is_array()).count();
                    let mut out = format!(
                        "# Tree (at {})\n\n{} groups, {} arrays\n\n",
                        params.r#ref, groups, arrays
                    );
                    for node in &tree {
                        out.push_str(&output::fmt_tree_line(node, &tree));
                    }
                    out
                }
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP tree completed in {:?}", _start.elapsed());
        result
    }

    #[tool(description = "Show repository operations log (mutation history): commits, branch/tag operations, config changes, GC runs.")]
    async fn ops_log(&self, Parameters(params): Parameters<OpsLogParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_ops_log(&repo, params.limit).await {
            Ok(entries) => {
                if entries.is_empty() {
                    "(no operations recorded)".to_string()
                } else {
                    let mut out = format!("# Operations Log ({} entries)\n\n", entries.len());
                    out.push_str("| Time | Operation |\n|------|-----------|");
                    for entry in &entries {
                        let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC");
                        out.push_str(&format!("\n| {} | {} |", ts, entry.description));
                    }
                    out
                }
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP ops_log completed in {:?}", _start.elapsed());
        result
    }

    #[tool(description = "Show what changed in a snapshot: added/deleted/modified arrays and groups, chunk changes. Use snapshot IDs from the `log` tool.")]
    async fn diff(&self, Parameters(params): Parameters<DiffParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_diff(&repo, &params.snapshot_id, params.parent_id.as_deref()).await {
            Ok(detail) => {
                if detail.is_initial_commit {
                    return format!(
                        "# Diff: `{}`\n\nInitial commit — all nodes are new.",
                        output::truncate(&params.snapshot_id, 12)
                    );
                }
                let mut out = format!("# Diff: `{}`\n\n", output::truncate(&params.snapshot_id, 12));
                if let Some(ref pid) = detail.parent_id {
                    out.push_str(&format!("Parent: `{}`\n\n", output::truncate(pid, 12)));
                }

                if !detail.added_arrays.is_empty() {
                    out.push_str(&format!("## Added Arrays ({})\n\n", detail.added_arrays.len()));
                    for p in &detail.added_arrays {
                        out.push_str(&format!("- {p}\n"));
                    }
                    out.push('\n');
                }
                if !detail.added_groups.is_empty() {
                    out.push_str(&format!("## Added Groups ({})\n\n", detail.added_groups.len()));
                    for p in &detail.added_groups {
                        out.push_str(&format!("- {p}\n"));
                    }
                    out.push('\n');
                }
                if !detail.deleted_arrays.is_empty() {
                    out.push_str(&format!("## Deleted Arrays ({})\n\n", detail.deleted_arrays.len()));
                    for p in &detail.deleted_arrays {
                        out.push_str(&format!("- {p}\n"));
                    }
                    out.push('\n');
                }
                if !detail.deleted_groups.is_empty() {
                    out.push_str(&format!("## Deleted Groups ({})\n\n", detail.deleted_groups.len()));
                    for p in &detail.deleted_groups {
                        out.push_str(&format!("- {p}\n"));
                    }
                    out.push('\n');
                }
                if !detail.modified_arrays.is_empty() {
                    out.push_str(&format!("## Modified Arrays ({})\n\n", detail.modified_arrays.len()));
                    for p in &detail.modified_arrays {
                        out.push_str(&format!("- {p}\n"));
                    }
                    out.push('\n');
                }
                if !detail.modified_groups.is_empty() {
                    out.push_str(&format!("## Modified Groups ({})\n\n", detail.modified_groups.len()));
                    for p in &detail.modified_groups {
                        out.push_str(&format!("- {p}\n"));
                    }
                    out.push('\n');
                }
                if !detail.chunk_changes.is_empty() {
                    out.push_str(&format!("## Chunk Changes ({})\n\n", detail.chunk_changes.len()));
                    for (path, count) in &detail.chunk_changes {
                        out.push_str(&format!("- {path}: {count} chunks\n"));
                    }
                    out.push('\n');
                }
                if !detail.moved_nodes.is_empty() {
                    out.push_str(&format!("## Moved ({})\n\n", detail.moved_nodes.len()));
                    for (from, to) in &detail.moved_nodes {
                        out.push_str(&format!("- {from} \u{2192} {to}\n"));
                    }
                    out.push('\n');
                }
                if detail.added_arrays.is_empty()
                    && detail.added_groups.is_empty()
                    && detail.deleted_arrays.is_empty()
                    && detail.deleted_groups.is_empty()
                    && detail.modified_arrays.is_empty()
                    && detail.modified_groups.is_empty()
                    && detail.chunk_changes.is_empty()
                    && detail.moved_nodes.is_empty()
                {
                    out.push_str("No structural changes detected.\n");
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP diff completed in {:?}", _start.elapsed());
        result
    }

    #[tool(description = "Show repository configuration: spec version, status, feature flags, virtual chunk containers, inline threshold.")]
    async fn config(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_repo_config(&repo).await {
            Ok(cfg) => {
                let mut out = "# Repository Configuration\n\n".to_string();
                out.push_str(&format!("- **Spec version:** {}\n", cfg.spec_version));
                out.push_str(&format!("- **Availability:** {}\n", cfg.availability));
                if let Some(threshold) = cfg.inline_chunk_threshold {
                    out.push_str(&format!(
                        "- **Inline chunk threshold:** {}\n",
                        format_size(threshold as u64, BINARY)
                    ));
                }

                if !cfg.feature_flags.is_empty() {
                    out.push_str("\n## Feature Flags\n\n");
                    for flag in &cfg.feature_flags {
                        let state = if flag.enabled { "enabled" } else { "disabled" };
                        let source = if flag.explicit { "explicit" } else { "default" };
                        out.push_str(&format!("- **{}**: {} ({})\n", flag.name, state, source));
                    }
                }

                if !cfg.virtual_chunk_containers.is_empty() {
                    out.push_str("\n## Virtual Chunk Containers\n\n");
                    for (name, prefix) in &cfg.virtual_chunk_containers {
                        out.push_str(&format!("- **{name}** → `{prefix}`\n"));
                    }
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP config completed in {:?}", _start.elapsed());
        result
    }

    #[tool(description = "Fuzzy search for nodes by path. Returns matching array and group paths ranked by relevance.")]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_tree_flat(&repo, &params.r#ref, None).await {
            Ok(tree) => {
                let paths: Vec<&str> = tree.iter().map(|n| n.path.as_str()).collect();
                let limit = params.limit.unwrap_or(20);

                let pattern = nucleo::pattern::Pattern::new(
                    &params.query,
                    nucleo::pattern::CaseMatching::Smart,
                    nucleo::pattern::Normalization::Smart,
                    nucleo::pattern::AtomKind::Fuzzy,
                );
                let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);

                let mut scored: Vec<(usize, u32)> = paths
                    .iter()
                    .enumerate()
                    .filter_map(|(i, path)| {
                        let mut buf = Vec::new();
                        let haystack = nucleo::Utf32Str::new(path, &mut buf);
                        pattern
                            .score(haystack, &mut matcher)
                            .map(|score| (i, score))
                    })
                    .collect();

                scored.sort_by(|a, b| b.1.cmp(&a.1));
                scored.truncate(limit);

                if scored.is_empty() {
                    format!("No matches for \"{}\"", params.query)
                } else {
                    let mut out = format!(
                        "# Search: \"{}\" ({} matches)\n\n",
                        params.query,
                        scored.len()
                    );
                    for (i, score) in &scored {
                        let node = &tree[*i];
                        let kind = if node.is_array() { "array" } else { "group" };
                        out.push_str(&format!(
                            "- `{}` ({}, score: {})\n",
                            node.path, kind, score
                        ));
                    }
                    out
                }
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP search completed in {:?}", _start.elapsed());
        result
    }
}

// Formatting reuses output::fmt_tree_line and output::fmt_node_detail

// ─── ServerHandler trait ────────────────────────────────────

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CoreDrillServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "core-drill",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "Icechunk repository inspector. The repo connection stays open for the session — \
             subsequent calls are fast (no re-auth or re-fetch). \
             Start with `open`, then use `info`, `tree`, `log`, `branches`, `tags`, `diff`, `ops_log`, `config`, `search` to explore.",
        )
    }
}

/// Start the MCP server on stdio transport.
pub async fn serve(repo: Option<Repository>, repo_url: String) -> color_eyre::Result<()> {
    use rmcp::ServiceExt;

    let server = CoreDrillServer::new(repo, repo_url);
    let transport = rmcp::transport::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
