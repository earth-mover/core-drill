//! MCP (Model Context Protocol) server for core-drill.
//!
//! Exposes repository inspection as MCP tools that agents can call.
//! The server can start with or without a pre-opened repo. Use the
//! `open` tool to connect to any repo on demand.
//!
//! Start with: `core-drill --serve`
//! Or pre-open: `core-drill <repo> --serve`

use std::collections::HashMap;
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

use crate::fetch;
use crate::output;
use crate::repo;
use crate::store::types::{BranchInfo, SnapshotEntry, TagInfo};

/// Cached data shared across MCP tool calls for the current repo session.
#[derive(Debug, Default)]
#[allow(dead_code)]
struct McpCache {
    branches: Option<Vec<BranchInfo>>,
    tags: Option<Vec<TagInfo>>,
    /// Ancestry (snapshot log) per ref name
    ancestry: HashMap<String, Vec<SnapshotEntry>>,
}

/// MCP server wrapping an open icechunk repository.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CoreDrillServer {
    tool_router: ToolRouter<Self>,
    repo: Arc<RwLock<Option<Arc<Repository>>>>,
    repo_url: Arc<RwLock<String>>,
    cache: Arc<RwLock<McpCache>>,
}

impl CoreDrillServer {
    pub fn new(repo: Option<Repository>, repo_url: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            repo: Arc::new(RwLock::new(repo.map(Arc::new))),
            repo_url: Arc::new(RwLock::new(repo_url)),
            cache: Arc::new(RwLock::new(McpCache::default())),
        }
    }
}

// ─── Tool parameter structs ─────────────────────────────────

#[allow(dead_code)] // fields read via serde deserialization
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EmptyParams {}

#[allow(dead_code)] // fields read via serde deserialization
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct BranchesParams {
    /// Filter to branches pointing at this snapshot ID (prefix match supported)
    snapshot_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct OpenParams {
    /// Path, URL, or Arraylake reference (e.g., "./my-repo", "s3://bucket/prefix", "al:org/repo")
    repo: String,
    /// Cloud storage region (optional, for S3)
    region: Option<String>,
    /// Storage endpoint URL (optional, for S3-compatible services)
    endpoint_url: Option<String>,
    /// Arraylake API endpoint (optional, uses arraylake crate default if not set)
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

#[allow(dead_code)] // fields read via serde deserialization
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TreeParams {
    /// Branch name, tag name, or snapshot ID (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
    /// Filter to a specific path (e.g. "/stations/latitude") for detailed metadata
    path: Option<String>,
    /// Maximum depth of children to show (e.g. 1 for direct children only). If omitted, shows all descendants.
    depth: Option<usize>,
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

#[allow(dead_code)] // fields read via serde deserialization
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// Search query — interpreted based on `mode`
    query: String,
    /// Branch, tag, or snapshot ID (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
    /// Maximum results to return (default: 20)
    limit: Option<usize>,
    /// Search mode: "fuzzy" (default, ranked by relevance), "prefix" (paths starting with query), "exact" (paths containing query as exact substring), "glob" (wildcard patterns like /data/*/temperature)
    #[serde(default = "default_search_mode")]
    mode: String,
}

fn default_search_mode() -> String {
    "fuzzy".to_string()
}

fn default_ref() -> String {
    "main".to_string()
}

/// Render a headed list section, capping at `limit` entries.
/// Returns empty string if `items` is empty.
fn fmt_section(heading: &str, items: &[String], limit: usize) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut out = format!("## {} ({})\n\n", heading, items.len());
    for item in items.iter().take(limit) {
        out.push_str(&format!("- {item}\n"));
    }
    if items.len() > limit {
        out.push_str(&format!("- *… {} more*\n", items.len() - limit));
    }
    out.push('\n');
    out
}

/// Hard output cap to prevent flooding the agent context window (~2 000 tokens).
const MAX_OUTPUT_CHARS: usize = 8_000;

fn cap_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_CHARS {
        return s;
    }
    // Find a char-safe byte boundary at or before MAX_OUTPUT_CHARS
    let safe_end = s[..=MAX_OUTPUT_CHARS.min(s.len() - 1)]
        .char_indices()
        .rev()
        .find(|(i, _)| *i <= MAX_OUTPUT_CHARS)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let truncated = &s[..safe_end];
    // Trim to last newline so we don't cut mid-line
    let cut = truncated.rfind('\n').map(|i| i + 1).unwrap_or(safe_end);
    format!(
        "{}\n\n*[output truncated at {} chars — use more specific tools or add filters]*",
        &s[..cut],
        MAX_OUTPUT_CHARS
    )
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
    /// Format the info overview for a repo+ref. Shared by `open` and `info` tools.
    async fn format_info(&self, repo: &Repository, r#ref: &str) -> String {
        let repo_url = self.repo_url.read().await;

        let (branches_res, tags_res, ancestry_res) = tokio::join!(
            self.cached_branches(repo),
            self.cached_tags(repo),
            self.cached_ancestry(repo, r#ref),
        );

        let branches = match branches_res {
            Ok(mut b) => {
                // "main" first, then most recently updated (tip_timestamp now populated from repo info)
                b.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
                    ("main", _) => std::cmp::Ordering::Less,
                    (_, "main") => std::cmp::Ordering::Greater,
                    _ => b.tip_timestamp.cmp(&a.tip_timestamp),
                });
                b
            }
            Err(e) => return format!("Error fetching branches: {e}"),
        };
        let tags = tags_res.unwrap_or_default();

        let mut out = format!("# Repository: {}\n\n", *repo_url);

        out.push_str(&format!("## Branches ({})\n\n", branches.len()));
        let show_branches = if branches.len() > 10 { 5 } else { branches.len() };
        for b in branches.iter().take(show_branches) {
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
        if branches.len() > show_branches {
            out.push_str(&format!(
                "\n*({} more — use `branches` tool for full list)*\n",
                branches.len() - show_branches
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
                "\n## Recent Snapshots ({} total on `{}`)\n\n",
                ancestry.len(),
                r#ref
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

        out.push_str("\n\n---\n*Next steps: `tree` (browse arrays), `search` (find array by name), `log` (full history), `diff` (snapshot changes), `ops_log` (mutation history), `config` (repo settings)*");
        out
    }

    /// Get branches, using cache if available.
    async fn cached_branches(&self, repo: &Repository) -> color_eyre::Result<Vec<BranchInfo>> {
        {
            let cache = self.cache.read().await;
            if let Some(ref branches) = cache.branches {
                return Ok(branches.clone());
            }
        }
        let branches = fetch::fetch_branches(repo).await?;
        self.cache.write().await.branches = Some(branches.clone());
        Ok(branches)
    }

    /// Get tags, using cache if available.
    async fn cached_tags(&self, repo: &Repository) -> color_eyre::Result<Vec<TagInfo>> {
        {
            let cache = self.cache.read().await;
            if let Some(ref tags) = cache.tags {
                return Ok(tags.clone());
            }
        }
        let tags = fetch::fetch_tags(repo).await?;
        self.cache.write().await.tags = Some(tags.clone());
        Ok(tags)
    }

    /// Get ancestry for a ref, using cache if available.
    async fn cached_ancestry(
        &self,
        repo: &Repository,
        r: &str,
    ) -> color_eyre::Result<Vec<SnapshotEntry>> {
        {
            let cache = self.cache.read().await;
            if let Some(entries) = cache.ancestry.get(r) {
                return Ok(entries.clone());
            }
        }
        let entries = fetch::fetch_ancestry(repo, r).await?;
        self.cache
            .write()
            .await
            .ancestry
            .insert(r.to_string(), entries.clone());
        Ok(entries)
    }

    #[tool(
        description = "Open an Icechunk repository and return an overview (branches, tags, recent snapshots). Must be called before other tools. Accepts local paths, S3/GCS URLs, S3-compatible (R2, MinIO, Tigris via endpoint_url), or Arraylake refs (al:org/repo)."
    )]
    async fn open(&self, Parameters(params): Parameters<OpenParams>) -> String {
        let _start = std::time::Instant::now();
        info!("MCP open: repo={}", params.repo);
        let overrides = repo::StorageOverrides {
            region: params.region,
            endpoint_url: params.endpoint_url,
        };

        let result = match crate::open_repo(&params.repo, params.arraylake_api.as_deref(), &overrides).await {
            Ok((repository, identity)) => {
                let label = identity.display_short();
                let repo = Arc::new(repository);

                *self.repo.write().await = Some(Arc::clone(&repo));
                *self.repo_url.write().await = label;
                *self.cache.write().await = McpCache::default();

                // Return info overview immediately — saves the agent an extra round trip
                self.format_info(&repo, "main").await
            }
            Err(e) => format!("Error opening repository: {e}"),
        };
        info!("MCP open completed in {:?}", _start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Repository overview: branches, tags, and recent snapshots. Good starting point after opening a repo. Use `tree` to browse arrays, `search` to find a specific array."
    )]
    async fn info(&self, Parameters(params): Parameters<InfoParams>) -> String {
        let _start = std::time::Instant::now();
        info!("MCP info ref={}", params.r#ref);
        let repo = require_repo!(self);
        let result = self.format_info(&repo, &params.r#ref).await;
        info!("MCP info completed in {:?}", _start.elapsed());
        cap_output(result)
    }

    #[tool(description = "List all branches with their tip snapshot IDs. Optionally filter by snapshot_id to find which branches point at a given snapshot.")]
    async fn branches(&self, Parameters(params): Parameters<BranchesParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match self.cached_branches(&repo).await {
            Ok(branches) => {
                let filtered: Vec<_> = if let Some(ref snap) = params.snapshot_id {
                    branches.iter().filter(|b| b.snapshot_id.starts_with(snap)).collect()
                } else {
                    branches.iter().collect()
                };

                if filtered.is_empty() {
                    if params.snapshot_id.is_some() {
                        format!("No branches point at snapshot `{}`", params.snapshot_id.as_deref().unwrap())
                    } else {
                        "(no branches)".to_string()
                    }
                } else {
                    let mut out = format!("# Branches ({})\n\n", filtered.len());
                    out.push_str("| Branch | Snapshot |\n|--------|----------|\n");
                    for b in &filtered {
                        out.push_str(&format!(
                            "| {} | `{}` |\n",
                            b.name,
                            output::truncate(&b.snapshot_id, 12)
                        ));
                    }
                    out
                }
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP branches completed in {:?}", _start.elapsed());
        cap_output(result)
    }

    #[tool(description = "List all tags with their snapshot IDs.")]
    async fn tags(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match self.cached_tags(&repo).await {
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
        cap_output(result)
    }

    #[tool(
        description = "Show snapshot history (commit log) for a branch, tag, or snapshot ID."
    )]
    async fn log(&self, Parameters(params): Parameters<LogParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match self.cached_ancestry(&repo, &params.r#ref).await {
            Ok(entries) => {
                let total = entries.len();
                let limit = params.limit.unwrap_or(20);
                let display: Vec<_> = entries.into_iter().take(limit).collect();
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
                if limit < total {
                    out.push_str(&format!(
                        "\n\n*Showing {} of {} commits — pass `limit` to see more*",
                        limit, total
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP log completed in {:?}", _start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Browse the node tree. Without `path`: lists groups and arrays (use `depth` to limit nesting). With `path` on an array: detailed metadata. With `path` on a group: lists children. `path` also works as a prefix filter (e.g. `path=/burst` matches `/burst-001`, `/burst-002`). Use `depth=1` for direct children only."
    )]
    async fn tree(&self, Parameters(params): Parameters<TreeParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let snap_id = match fetch::resolve_ref_to_snapshot_id(&repo, &params.r#ref).await {
            Ok(id) => id,
            Err(e) => return format!("Error resolving ref: {e}"),
        };
        let result = match fetch::fetch_tree_flat(&repo, &snap_id, params.path.as_deref()).await {
            Ok(tree) => {
                if let Some(ref filter_path) = params.path {
                    if let Some(node) = tree.iter().find(|n| n.path == *filter_path) {
                        if node.is_array() {
                            // Array: show detailed metadata + chunk stats
                            let mut out = output::fmt_node_detail(node);
                            if let Ok(stats) = fetch::fetch_chunk_stats(&repo, &snap_id, &node.path).await {
                                out.push_str(&fmt_chunk_stats(&stats));
                            }
                            out
                        } else {
                            // Group: show children (not just "Type: group")
                            let base_depth = filter_path.matches('/').count();
                            let children: Vec<_> = tree.iter()
                                .filter(|n| {
                                    n.path != *filter_path && n.path.starts_with(&format!("{filter_path}/"))
                                })
                                .filter(|n| {
                                    if let Some(max_depth) = params.depth {
                                        let node_depth = n.path.matches('/').count() - base_depth;
                                        node_depth <= max_depth
                                    } else {
                                        true
                                    }
                                })
                                .collect();

                            let groups_count = children.iter().filter(|n| n.is_group()).count();
                            let arrays_count = children.iter().filter(|n| n.is_array()).count();
                            let mut out = format!("# {} ({} groups, {} arrays)\n\n", filter_path, groups_count, arrays_count);

                            const CHILD_LINE_LIMIT: usize = 30;
                            out.push_str(&fmt_collapsed_tree(&children, &tree, CHILD_LINE_LIMIT));
                            if children.is_empty() {
                                out.push_str("*(empty group)*\n");
                            }
                            out
                        }
                    } else if tree.is_empty() {
                        format!("No nodes found at path: {filter_path}")
                    } else {
                        // Prefix match — e.g. path=/burst matched /burst-001, /burst-002, ...
                        let base_depth = filter_path.matches('/').count();
                        let filtered: Vec<_> = if let Some(max_depth) = params.depth {
                            tree.iter()
                                .filter(|n| {
                                    let node_depth = n.path.matches('/').count() - base_depth;
                                    node_depth <= max_depth
                                })
                                .collect()
                        } else {
                            tree.iter().collect()
                        };

                        let groups_count = filtered.iter().filter(|n| n.is_group()).count();
                        let arrays_count = filtered.iter().filter(|n| n.is_array()).count();
                        let mut out = format!(
                            "# Prefix: {}* ({} groups, {} arrays)\n\n",
                            filter_path, groups_count, arrays_count
                        );
                        const PREFIX_LINE_LIMIT: usize = 30;
                        out.push_str(&fmt_collapsed_tree(&filtered, &tree, PREFIX_LINE_LIMIT));
                        out
                    }
                } else {
                    // No path filter — show full tree with optional depth limit
                    let filtered: Vec<_> = if let Some(max_depth) = params.depth {
                        tree.iter()
                            .filter(|n| {
                                let node_depth = n.path.matches('/').count().saturating_sub(1);
                                node_depth < max_depth
                            })
                            .collect()
                    } else {
                        tree.iter().collect()
                    };

                    let groups_count = filtered.iter().filter(|n| n.is_group()).count();
                    let arrays_count = filtered.iter().filter(|n| n.is_array()).count();
                    let mut out = format!(
                        "# Tree (at {})\n\n{} groups, {} arrays\n\n",
                        params.r#ref, groups_count, arrays_count
                    );
                    const TREE_LINE_LIMIT: usize = 30;
                    out.push_str(&fmt_collapsed_tree(&filtered, &tree, TREE_LINE_LIMIT));
                    let total_all = tree.len();
                    if filtered.len() < total_all {
                        out.push_str(&format!(
                            "\n*Showing depth-limited view ({} of {} total nodes — remove `depth` to see all)*\n",
                            filtered.len(), total_all
                        ));
                    }
                    out
                }
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP tree completed in {:?}", _start.elapsed());
        cap_output(result)
    }

    #[tool(description = "Show repository operations log (mutation history): commits, branch/tag operations, config changes, GC runs.")]
    async fn ops_log(&self, Parameters(params): Parameters<OpsLogParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let ops_limit = Some(params.limit.unwrap_or(50));
        let result = match fetch::fetch_ops_log(&repo, ops_limit).await {
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
        cap_output(result)
    }

    #[tool(description = "Show what changed in a snapshot: added/deleted/modified arrays and groups, chunk changes. Use snapshot IDs from the `log` tool.")]
    async fn diff(&self, Parameters(params): Parameters<DiffParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match fetch::fetch_diff(&repo, &params.snapshot_id, params.parent_id.as_deref()).await {
            Ok(detail) => {
                if detail.is_initial_commit {
                    return format!(
                        "# Diff: `{}`\n\nInitial commit — all nodes are new.",
                        output::truncate(&params.snapshot_id, 12)
                    );
                }
                const DIFF_SECTION_LIMIT: usize = 50;
                let mut out = format!("# Diff: `{}`\n\n", output::truncate(&params.snapshot_id, 12));
                if let Some(ref pid) = detail.parent_id {
                    out.push_str(&format!("Parent: `{}`\n\n", output::truncate(pid, 12)));
                }

                out.push_str(&fmt_section("Added Arrays", &detail.added_arrays, DIFF_SECTION_LIMIT));
                out.push_str(&fmt_section("Added Groups", &detail.added_groups, DIFF_SECTION_LIMIT));
                out.push_str(&fmt_section("Deleted Arrays", &detail.deleted_arrays, DIFF_SECTION_LIMIT));
                out.push_str(&fmt_section("Deleted Groups", &detail.deleted_groups, DIFF_SECTION_LIMIT));
                out.push_str(&fmt_section("Modified Arrays", &detail.modified_arrays, DIFF_SECTION_LIMIT));
                out.push_str(&fmt_section("Modified Groups", &detail.modified_groups, DIFF_SECTION_LIMIT));

                if !detail.chunk_changes.is_empty() {
                    let chunk_items: Vec<String> = detail
                        .chunk_changes
                        .iter()
                        .map(|(path, count)| format!("{path}: {count} chunks written"))
                        .collect();
                    out.push_str(&fmt_section("Updated Chunks", &chunk_items, DIFF_SECTION_LIMIT));
                }

                if !detail.moved_nodes.is_empty() {
                    let moved_items: Vec<String> = detail
                        .moved_nodes
                        .iter()
                        .map(|(from, to)| format!("{from} \u{2192} {to}"))
                        .collect();
                    out.push_str(&fmt_section("Moved", &moved_items, DIFF_SECTION_LIMIT));
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
        cap_output(result)
    }

    #[tool(description = "Show repository configuration: spec version, status, feature flags, virtual chunk containers, inline threshold.")]
    async fn config(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match fetch::fetch_repo_config(&repo).await {
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
        cap_output(result)
    }

    #[tool(description = "Search for nodes by path. Modes: \"fuzzy\" (default, ranked by relevance), \"prefix\" (paths starting with query, e.g. /data), \"exact\" (substring match), \"glob\" (wildcards, e.g. /data/*/temperature). For listing direct children of a group, use `tree` with `path` and `depth=1`.")]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match fetch::fetch_tree_flat(&repo, &params.r#ref, None).await {
            Ok(tree) => {
                let limit = params.limit.unwrap_or(20);

                match params.mode.as_str() {
                    "prefix" => {
                        let matched: Vec<_> = tree.iter()
                            .filter(|n| n.path.starts_with(&params.query))
                            .collect();
                        fmt_search_results(&params.query, "prefix", &matched, limit, &tree)
                    }
                    "exact" => {
                        let matched: Vec<_> = tree.iter()
                            .filter(|n| n.path.contains(&params.query))
                            .collect();
                        fmt_search_results(&params.query, "exact", &matched, limit, &tree)
                    }
                    "glob" => {
                        let matched: Vec<_> = tree.iter()
                            .filter(|n| glob_matches(&params.query, &n.path))
                            .collect();
                        fmt_search_results(&params.query, "glob", &matched, limit, &tree)
                    }
                    _ => {
                        // Fuzzy (default)
                        let paths: Vec<&str> = tree.iter().map(|n| n.path.as_str()).collect();
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
                                "# Search: \"{}\" ({} matches, fuzzy)\n\n",
                                params.query,
                                scored.len()
                            );
                            for (i, _score) in &scored {
                                let node = &tree[*i];
                                let kind = if node.is_array() { "array" } else { "group" };
                                out.push_str(&format!("- `{}` ({})\n", node.path, kind));
                            }
                            out
                        }
                    }
                }
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP search completed in {:?}", _start.elapsed());
        cap_output(result)
    }
}

/// Format search results for non-fuzzy modes.
fn fmt_search_results(
    query: &str,
    mode: &str,
    matched: &[&fetch::FlatNode],
    limit: usize,
    all_nodes: &[fetch::FlatNode],
) -> String {
    let total = matched.len();
    if total == 0 {
        return format!("No matches for \"{}\" ({} mode)", query, mode);
    }
    let mut out = format!(
        "# Search: \"{}\" ({} matches, {})\n\n",
        query, total, mode
    );
    for node in matched.iter().take(limit) {
        out.push_str(&output::fmt_tree_line(node, all_nodes));
    }
    if total > limit {
        out.push_str(&format!(
            "\n*({} more — increase `limit` to see more)*\n",
            total - limit
        ));
    }
    out
}

/// Render a tree listing that collapses runs of similarly-named siblings.
///
/// Instead of listing 1000 `burst-*` arrays individually, groups them:
///   - **burst-133012-*** (1000 arrays) `float32` `[721 × 1440]`
///
/// `max_lines` caps the total output lines (each collapsed group = 1 line).
fn fmt_collapsed_tree(nodes: &[&fetch::FlatNode], all_nodes: &[fetch::FlatNode], max_lines: usize) -> String {
    if nodes.is_empty() {
        return String::new();
    }

    // Group nodes by (parent_path, type, name_prefix).
    // name_prefix = everything up to the last `-` or `_` separator, or the full name if no separator.
    struct CollapsedGroup<'a> {
        prefix: String,       // shared prefix (e.g. "burst-133012-")
        nodes: Vec<&'a fetch::FlatNode>,
        depth: usize,
    }

    let mut groups: Vec<CollapsedGroup> = Vec::new();

    // Sort by path so similarly-named siblings are consecutive for collapsing
    let mut sorted: Vec<&&fetch::FlatNode> = nodes.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));

    for node in sorted.into_iter().map(|n| *n) {
        let parent = match node.path.rfind('/') {
            Some(0) => "/",
            Some(idx) => &node.path[..idx],
            None => "/",
        };
        let depth = node.path.matches('/').count().saturating_sub(1);

        // Find a prefix: strip trailing digits/chars after last `-` or `_`
        let prefix = {
            let name = &node.name;
            // Find the last separator position
            let sep_pos = name.rfind(|c: char| c == '-' || c == '_');
            match sep_pos {
                Some(pos) if pos > 0 && pos < name.len() - 1 => {
                    format!("{}/{}", parent, &name[..=pos])
                }
                _ => node.path.clone(),
            }
        };

        // Try to append to the last group if same prefix, type, and depth
        if let Some(last) = groups.last_mut() {
            if last.prefix == prefix
                && last.depth == depth
                && last.nodes[0].node_type == node.node_type
            {
                last.nodes.push(node);
                continue;
            }
        }
        groups.push(CollapsedGroup {
            prefix,
            nodes: vec![node],
            depth,
        });
    }

    let mut out = String::new();
    let mut lines = 0;

    for group in &groups {
        if lines >= max_lines {
            break;
        }

        if group.nodes.len() < 3 {
            // Not worth collapsing — show individually
            for node in &group.nodes {
                if lines >= max_lines {
                    break;
                }
                out.push_str(&output::fmt_tree_line(node, all_nodes));
                lines += 1;
            }
        } else {
            // Collapse into a summary line
            let indent = "  ".repeat(group.depth);
            let count = group.nodes.len();
            let kind = if group.nodes[0].is_array() { "arrays" } else { "groups" };

            // Show representative metadata from first node
            let sample = group.nodes[0];
            let meta = if sample.is_array() {
                let dtype = sample.dtype.as_deref().unwrap_or("?");
                let shape = sample.shape.as_ref()
                    .map(|s| output::fmt_dims(s))
                    .unwrap_or_else(|| "?".to_string());
                format!(" `{dtype}` `[{shape}]`")
            } else {
                String::new()
            };

            out.push_str(&format!(
                "{indent}- **{}\\*** ({} {}){}\n",
                group.prefix.rsplit('/').next().unwrap_or(&group.prefix),
                count, kind, meta
            ));
            lines += 1;
        }
    }

    let total_items: usize = groups.iter().map(|g| g.nodes.len()).sum();
    let shown_items: usize = {
        let mut count = 0;
        let mut l = 0;
        for group in &groups {
            if l >= max_lines { break; }
            if group.nodes.len() < 3 {
                for _ in &group.nodes {
                    if l >= max_lines { break; }
                    count += 1;
                    l += 1;
                }
            } else {
                count += group.nodes.len();
                l += 1;
            }
        }
        count
    };

    if shown_items < total_items {
        out.push_str(&format!(
            "\n*({} more nodes — use `path` to drill into a prefix, or `search` to find specific arrays)*\n",
            total_items - shown_items
        ));
    }

    out
}

/// Convert a simple glob pattern to a matching function.
/// Supports `*` (any segment chars except `/`) and `**` (any path including `/`).
fn glob_matches(pattern: &str, path: &str) -> bool {
    glob_matches_inner(pattern.as_bytes(), path.as_bytes())
}

fn glob_matches_inner(pattern: &[u8], path: &[u8]) -> bool {
    let mut pi = 0; // pattern index
    let mut si = 0; // string index
    let mut star_pi = usize::MAX; // last `*` position in pattern
    let mut star_si = 0; // string position when `*` was hit

    while si < path.len() {
        if pi < pattern.len() && pattern[pi] == b'*' && pi + 1 < pattern.len() && pattern[pi + 1] == b'*' {
            // `**` — match everything including `/`
            pi += 2;
            if pi < pattern.len() && pattern[pi] == b'/' {
                pi += 1; // skip trailing `/` after `**`
            }
            // Try matching the rest of the pattern at every position
            while si <= path.len() {
                if glob_matches_inner(&pattern[pi..], &path[si..]) {
                    return true;
                }
                if si < path.len() {
                    si += 1;
                } else {
                    break;
                }
            }
            return false;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            // `*` — match anything except `/`
            star_pi = pi;
            star_si = si;
            pi += 1;
        } else if pi < pattern.len() && (pattern[pi] == path[si] || pattern[pi] == b'?') {
            pi += 1;
            si += 1;
        } else if star_pi != usize::MAX {
            // Backtrack to last `*`
            pi = star_pi + 1;
            star_si += 1;
            if path[star_si - 1] == b'/' {
                return false; // `*` doesn't cross `/`
            }
            si = star_si;
        } else {
            return false;
        }
    }

    // Consume trailing stars
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod glob_tests {
    use super::glob_matches;

    #[test]
    fn exact_match() {
        assert!(glob_matches("/data/temperature", "/data/temperature"));
        assert!(!glob_matches("/data/temperature", "/data/humidity"));
    }

    #[test]
    fn single_star_within_segment() {
        assert!(glob_matches("/data/burst-*", "/data/burst-001"));
        assert!(glob_matches("/data/burst-*", "/data/burst-999"));
        assert!(!glob_matches("/data/burst-*", "/data/other-001"));
        // `*` should not cross `/`
        assert!(!glob_matches("/data/*", "/data/sub/child"));
    }

    #[test]
    fn double_star_crosses_slashes() {
        assert!(glob_matches("/**/temperature", "/data/temperature"));
        assert!(glob_matches("/**/temperature", "/data/sub/temperature"));
        assert!(glob_matches("/data/**", "/data/a/b/c"));
    }

    #[test]
    fn star_in_middle() {
        assert!(glob_matches("/data/*/temperature", "/data/era5/temperature"));
        assert!(glob_matches("/data/*/temperature", "/data/merra2/temperature"));
        assert!(!glob_matches("/data/*/temperature", "/data/era5/sub/temperature"));
    }

    #[test]
    fn question_mark() {
        assert!(glob_matches("/burst-00?", "/burst-001"));
        assert!(glob_matches("/burst-00?", "/burst-009"));
        assert!(!glob_matches("/burst-00?", "/burst-0012"));
    }
}

#[cfg(test)]
mod collapse_tests {
    use super::fmt_collapsed_tree;
    use crate::fetch::{FlatNode, FlatNodeType};

    fn make_array(path: &str) -> FlatNode {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        FlatNode {
            path: path.to_string(),
            name,
            node_type: FlatNodeType::Array,
            shape: Some(vec![721, 1440]),
            dtype: Some("float32".to_string()),
            chunk_shape: None,
            dimensions: None,
            total_chunks: None,
            grid_chunks: None,
            codecs: None,
            fill_value: None,
        }
    }

    #[test]
    fn collapses_similar_siblings() {
        let nodes: Vec<FlatNode> = (0..100)
            .map(|i| make_array(&format!("/burst-{:04}", i)))
            .collect();
        let refs: Vec<&FlatNode> = nodes.iter().collect();
        let result = fmt_collapsed_tree(&refs, &nodes, 30);

        // Should collapse into 1 line, not 100
        let line_count = result.lines().filter(|l| !l.is_empty()).count();
        assert!(line_count <= 3, "Expected <=3 lines, got {line_count}:\n{result}");
        assert!(result.contains("100 arrays"), "Should mention 100 arrays:\n{result}");
        assert!(result.contains("burst-"), "Should mention burst- prefix:\n{result}");
    }

    #[test]
    fn preserves_unique_names() {
        let nodes = vec![
            make_array("/temperature"),
            make_array("/humidity"),
            make_array("/pressure"),
        ];
        let refs: Vec<&FlatNode> = nodes.iter().collect();
        let result = fmt_collapsed_tree(&refs, &nodes, 30);

        // All unique, no collapsing
        assert!(result.contains("temperature"), "Should list temperature:\n{result}");
        assert!(result.contains("humidity"), "Should list humidity:\n{result}");
        assert!(result.contains("pressure"), "Should list pressure:\n{result}");
    }

    #[test]
    fn mixed_collapse_and_unique() {
        let mut nodes: Vec<FlatNode> = (0..50)
            .map(|i| make_array(&format!("/burst-{:04}", i)))
            .collect();
        nodes.push(make_array("/temperature"));
        nodes.push(make_array("/humidity"));

        let refs: Vec<&FlatNode> = nodes.iter().collect();
        let result = fmt_collapsed_tree(&refs, &nodes, 30);

        // burst-* collapsed, temperature and humidity shown individually
        assert!(result.contains("50 arrays"), "Should collapse burst:\n{result}");
        assert!(result.contains("temperature"), "Should list temperature:\n{result}");
        assert!(result.contains("humidity"), "Should list humidity:\n{result}");
    }

    #[test]
    fn respects_line_limit() {
        // 10 different prefixes, 5 each = 50 nodes
        let mut nodes = Vec::new();
        for prefix in &["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa"] {
            for i in 0..5 {
                nodes.push(make_array(&format!("/{prefix}-{i:03}")));
            }
        }
        let refs: Vec<&FlatNode> = nodes.iter().collect();
        let result = fmt_collapsed_tree(&refs, &nodes, 5);

        let line_count = result.lines().filter(|l| l.starts_with("- ") || l.starts_with("  -")).count();
        assert!(line_count <= 5, "Should cap at 5 lines, got {line_count}:\n{result}");
    }
}

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
             Start with `open`, then `info` for a full overview (branches, snapshots, tree). \
             Use `tree` with a `path` param to drill into a specific array. \
             Use `log`/`diff` for history, `search` for fuzzy find, `ops_log`/`config` for repo metadata.",
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
