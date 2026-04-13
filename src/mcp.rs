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

use crate::sanitize::sanitize;

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
struct McpCache {
    branches: Option<Vec<BranchInfo>>,
    tags: Option<Vec<TagInfo>>,
    /// Ancestry (snapshot log) per ref name
    ancestry: HashMap<String, Vec<SnapshotEntry>>,
}

/// MCP server wrapping an open icechunk repository.
#[derive(Debug, Clone)]
pub struct CoreDrillServer {
    tool_router: ToolRouter<Self>,
    repo: Arc<RwLock<Option<Arc<Repository>>>>,
    repo_url: Arc<RwLock<String>>,
    /// Preserved identity for code generation (retains region/endpoint/anonymous)
    repo_identity: Arc<RwLock<Option<crate::app::RepoIdentity>>>,
    cache: Arc<RwLock<McpCache>>,
}

impl CoreDrillServer {
    pub fn new(repo: Option<Repository>, repo_url: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            repo: Arc::new(RwLock::new(repo.map(Arc::new))),
            repo_url: Arc::new(RwLock::new(repo_url)),
            repo_identity: Arc::new(RwLock::new(None)),
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
    /// Use anonymous (unsigned) requests — useful for public repos
    #[serde(default)]
    anonymous: bool,
}

#[allow(dead_code)] // fields read via serde deserialization
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LogParams {
    /// Branch name, tag name, or snapshot ID (default: "main")
    #[serde(default = "default_ref")]
    r#ref: String,
    /// Maximum number of snapshots to return
    limit: Option<usize>,
    /// Skip this many snapshots before returning results (for pagination). Applied after search filter.
    offset: Option<usize>,
    /// Filter to snapshots whose message contains this string (case-insensitive substring match)
    search: Option<String>,
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
    /// Fetch full chunk statistics (type breakdown, sizes). Slower — iterates all chunks. Default: false.
    #[serde(default)]
    chunk_stats: bool,
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

fn default_lang() -> String {
    "python".to_string()
}

#[allow(dead_code)] // fields read via serde deserialization
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ScriptParams {
    /// Language: "python" (default) or "rust"
    #[serde(default = "default_lang")]
    lang: String,
    /// Branch name (default: "main")
    #[serde(default = "default_ref")]
    branch: String,
    /// Snapshot ID (overrides branch if set)
    snapshot: Option<String>,
    /// Zarr group path to navigate to (e.g. "/data/temperature")
    path: Option<String>,
}

fn default_ref() -> String {
    "main".to_string()
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

use crate::output::fmt_chunk_stats;

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

        let mut branches = match branches_res {
            Ok(b) => b,
            Err(e) => return format!("Error fetching branches: {e}"),
        };
        // "main" first, then most recently updated
        branches.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
            ("main", _) => std::cmp::Ordering::Less,
            (_, "main") => std::cmp::Ordering::Greater,
            _ => b.tip_timestamp.cmp(&a.tip_timestamp),
        });

        let tags = tags_res.unwrap_or_default();
        let ancestry = ancestry_res.unwrap_or_default();

        output::fmt_repo_overview(&repo_url, &branches, &tags, &ancestry, r#ref)
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
        let start = std::time::Instant::now();
        info!("MCP open: repo={}", params.repo);
        let overrides = repo::StorageOverrides {
            region: params.region,
            endpoint_url: params.endpoint_url,
            anonymous: params.anonymous,
        };

        let result =
            match crate::open_repo(&params.repo, params.arraylake_api.as_deref(), &overrides).await
            {
                Ok((repository, identity)) => {
                    let label = identity.display_short();
                    let repo = Arc::new(repository);

                    *self.repo.write().await = Some(Arc::clone(&repo));
                    *self.repo_url.write().await = label;
                    *self.repo_identity.write().await = Some(identity);
                    *self.cache.write().await = McpCache::default();

                    // Return info overview immediately — saves the agent an extra round trip
                    self.format_info(&repo, "main").await
                }
                Err(e) => format!("Error opening repository: {e}"),
            };
        info!("MCP open completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Repository overview: branches, tags, and recent snapshots. Good starting point after opening a repo. Use `tree` to browse arrays, `search` to find a specific array."
    )]
    async fn info(&self, Parameters(params): Parameters<InfoParams>) -> String {
        let start = std::time::Instant::now();
        info!("MCP info ref={}", params.r#ref);
        let repo = require_repo!(self);
        let result = self.format_info(&repo, &params.r#ref).await;
        info!("MCP info completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "List all branches with their tip snapshot IDs. Optionally filter by snapshot_id to find which branches point at a given snapshot."
    )]
    async fn branches(&self, Parameters(params): Parameters<BranchesParams>) -> String {
        let start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match self.cached_branches(&repo).await {
            Ok(branches) => {
                let filtered: Vec<_> = if let Some(ref snap) = params.snapshot_id {
                    branches
                        .iter()
                        .filter(|b| b.snapshot_id.starts_with(snap))
                        .collect()
                } else {
                    branches.iter().collect()
                };

                if filtered.is_empty() {
                    if let Some(ref snap_id) = params.snapshot_id {
                        format!("No branches point at snapshot `{snap_id}`")
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
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP branches completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(description = "List all tags with their snapshot IDs.")]
    async fn tags(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let start = std::time::Instant::now();
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
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP tags completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Show snapshot history (commit log) for a branch, tag, or snapshot ID. Use `offset` and `limit` to paginate (e.g. offset=100, limit=20 for commits 101-120). Use `search` to filter by commit message (case-insensitive substring match)."
    )]
    async fn log(&self, Parameters(params): Parameters<LogParams>) -> String {
        let start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match self.cached_ancestry(&repo, &params.r#ref).await {
            Ok(entries) => {
                let total = entries.len();

                // Filter by search term if provided
                let filtered: Vec<_> = if let Some(ref q) = params.search {
                    let q_lower = q.to_lowercase();
                    entries
                        .into_iter()
                        .filter(|e| e.message.to_lowercase().contains(&q_lower))
                        .collect()
                } else {
                    entries
                };
                let matched = filtered.len();

                if matched == 0 && let Some(ref query) = params.search {
                    format!(
                        "No snapshots matching \"{query}\" in {} ({total} total commits)",
                        params.r#ref,
                    )
                } else {
                    // Paginate
                    let offset = params.offset.unwrap_or(0);
                    let limit = params.limit.unwrap_or(20);
                    let display: Vec<_> =
                        filtered.into_iter().skip(offset).take(limit).collect();
                    let showing_end = offset + display.len();

                    // Header
                    let mut out = if params.search.is_some() {
                        format!(
                            "# Snapshot Log ({}, {} matches of {} total, showing {}-{})\n\n",
                            params.r#ref,
                            matched,
                            total,
                            offset + 1,
                            showing_end
                        )
                    } else {
                        format!(
                            "# Snapshot Log ({}, {} total, showing {}-{})\n\n",
                            params.r#ref,
                            total,
                            offset + 1,
                            showing_end
                        )
                    };

                    out.push_str(
                        "| # | Snapshot | Time | Message |\n|---|----------|------|---------|",
                    );
                    for (i, e) in display.iter().enumerate() {
                        let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC").to_string();
                        out.push_str(&format!(
                            "\n| {} | `{}` | {} | {} |",
                            offset + i + 1,
                            output::truncate(&e.id, 12),
                            ts,
                            e.message
                        ));
                    }

                    if display.is_empty() && matched > 0 {
                        out.push_str(&format!(
                            "\n\n*Offset {} exceeds {} results — use a smaller offset*",
                            offset, matched
                        ));
                    } else if showing_end < matched {
                        out.push_str(&format!(
                            "\n\n*{} more — use `offset={}` to continue*",
                            matched - showing_end,
                            showing_end
                        ));
                    }
                    out
                }
            }
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP log completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Browse the node tree. Without `path`: lists groups and arrays (use `depth` to limit nesting). With `path` on an array: detailed metadata (shape, dtype, codecs, chunk count). With `path` on a group: lists children. `path` also works as a prefix filter (e.g. `path=/burst` matches `/burst-001`, `/burst-002`). Use `depth=1` for direct children only. Add `chunk_stats=true` for full chunk type/size breakdown (slower — iterates all chunks)."
    )]
    async fn tree(&self, Parameters(params): Parameters<TreeParams>) -> String {
        let start = std::time::Instant::now();
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
                            // Array: show detailed metadata from tree (cheap)
                            // Chunk type/size breakdown only on request (iterates all chunks)
                            let mut out = output::fmt_node_detail(node);
                            if params.chunk_stats
                                && let Ok(stats) =
                                    fetch::fetch_chunk_stats(&repo, &snap_id, &node.path).await
                            {
                                out.push_str(&fmt_chunk_stats(&stats));
                            }
                            out
                        } else {
                            // Group: show children (not just "Type: group")
                            let base_depth = filter_path.matches('/').count();
                            let children: Vec<_> = tree
                                .iter()
                                .filter(|n| {
                                    n.path != *filter_path
                                        && n.path.starts_with(&format!("{filter_path}/"))
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
                            let mut out = format!(
                                "# {} ({} groups, {} arrays)\n\n",
                                filter_path, groups_count, arrays_count
                            );

                            const CHILD_LINE_LIMIT: usize = 30;
                            out.push_str(&output::fmt_collapsed_tree(&children, &tree, CHILD_LINE_LIMIT));
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
                        out.push_str(&output::fmt_collapsed_tree(&filtered, &tree, PREFIX_LINE_LIMIT));
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
                    out.push_str(&output::fmt_collapsed_tree(&filtered, &tree, TREE_LINE_LIMIT));
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
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP tree completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Show repository operations log (mutation history): commits, branch/tag operations, config changes, GC runs."
    )]
    async fn ops_log(&self, Parameters(params): Parameters<OpsLogParams>) -> String {
        let start = std::time::Instant::now();
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
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP ops_log completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Show what changed in a snapshot: added/deleted/modified arrays and groups, chunk changes. Use snapshot IDs from the `log` tool."
    )]
    async fn diff(&self, Parameters(params): Parameters<DiffParams>) -> String {
        let start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match fetch::fetch_diff(
            &repo,
            &params.snapshot_id,
            params.parent_id.as_deref(),
        )
        .await
        {
            Ok(detail) => output::fmt_diff_detail(&detail, &params.snapshot_id),
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP diff completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Show repository configuration: spec version, status, feature flags, virtual chunk containers, inline threshold."
    )]
    async fn config(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match fetch::fetch_repo_config(&repo).await {
            Ok(cfg) => output::fmt_repo_config(&cfg),
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP config completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Search for nodes by path. Modes: \"fuzzy\" (default, ranked by relevance), \"prefix\" (paths starting with query, e.g. /data), \"exact\" (substring match), \"glob\" (wildcards, e.g. /data/*/temperature). For listing direct children of a group, use `tree` with `path` and `depth=1`."
    )]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match fetch::fetch_tree_flat(&repo, &params.r#ref, None).await {
            Ok(tree) => {
                let limit = params.limit.unwrap_or(20);

                match params.mode.as_str() {
                    "prefix" => {
                        let matched: Vec<_> = tree
                            .iter()
                            .filter(|n| n.path.starts_with(&params.query))
                            .collect();
                        output::fmt_search_results(&params.query, "prefix", &matched, limit, &tree)
                    }
                    "exact" => {
                        let matched: Vec<_> = tree
                            .iter()
                            .filter(|n| n.path.contains(&params.query))
                            .collect();
                        output::fmt_search_results(&params.query, "exact", &matched, limit, &tree)
                    }
                    "glob" => {
                        let matched: Vec<_> = tree
                            .iter()
                            .filter(|n| glob_matches(&params.query, &n.path))
                            .collect();
                        output::fmt_search_results(&params.query, "glob", &matched, limit, &tree)
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
            Err(e) => format!("Error: {}", sanitize(&e.to_string())),
        };
        info!("MCP search completed in {:?}", start.elapsed());
        cap_output(result)
    }

    #[tool(
        description = "Generate a ready-to-run Python or Rust script for connecting to the currently open repo. No extra network calls needed. Returns code with PEP 723 metadata (Python) that can be saved and run directly."
    )]
    async fn script(&self, Parameters(params): Parameters<ScriptParams>) -> String {
        let identity_guard = self.repo_identity.read().await;
        let identity = match identity_guard.as_ref() {
            Some(id) => id,
            None => return "No repo open. Use the `open` tool first.".to_string(),
        };

        let ctx = crate::codegen::CodeContext {
            branch: params.branch,
            snapshot: params.snapshot,
            path: params.path,
        };

        let format = match params.lang.as_str() {
            "rust" => crate::codegen::ScriptFormat::Rust,
            _ => crate::codegen::ScriptFormat::Python,
        };
        let extra_deps = crate::config::load().map(|c| c.script_deps).unwrap_or_default();
        crate::codegen::generate_script(identity, &ctx, &format, &extra_deps)
    }
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
        if pi < pattern.len()
            && pattern[pi] == b'*'
            && pi + 1 < pattern.len()
            && pattern[pi + 1] == b'*'
        {
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
        assert!(glob_matches(
            "/data/*/temperature",
            "/data/era5/temperature"
        ));
        assert!(glob_matches(
            "/data/*/temperature",
            "/data/merra2/temperature"
        ));
        assert!(!glob_matches(
            "/data/*/temperature",
            "/data/era5/sub/temperature"
        ));
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
    use crate::fetch::{FlatNode, FlatNodeType};
    use crate::output::fmt_collapsed_tree;

    fn make_array(path: &str) -> FlatNode {
        let name = crate::util::leaf_name(path).to_string();
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
            manifest_count: None,
            zarr_metadata: None,
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
        assert!(
            line_count <= 3,
            "Expected <=3 lines, got {line_count}:\n{result}"
        );
        assert!(
            result.contains("100 arrays"),
            "Should mention 100 arrays:\n{result}"
        );
        assert!(
            result.contains("burst-"),
            "Should mention burst- prefix:\n{result}"
        );
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
        assert!(
            result.contains("temperature"),
            "Should list temperature:\n{result}"
        );
        assert!(
            result.contains("humidity"),
            "Should list humidity:\n{result}"
        );
        assert!(
            result.contains("pressure"),
            "Should list pressure:\n{result}"
        );
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
        assert!(
            result.contains("50 arrays"),
            "Should collapse burst:\n{result}"
        );
        assert!(
            result.contains("temperature"),
            "Should list temperature:\n{result}"
        );
        assert!(
            result.contains("humidity"),
            "Should list humidity:\n{result}"
        );
    }

    #[test]
    fn respects_line_limit() {
        // 10 different prefixes, 5 each = 50 nodes
        let mut nodes = Vec::new();
        for prefix in &[
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
        ] {
            for i in 0..5 {
                nodes.push(make_array(&format!("/{prefix}-{i:03}")));
            }
        }
        let refs: Vec<&FlatNode> = nodes.iter().collect();
        let result = fmt_collapsed_tree(&refs, &nodes, 5);

        let line_count = result
            .lines()
            .filter(|l| l.starts_with("- ") || l.starts_with("  -"))
            .count();
        assert!(
            line_count <= 5,
            "Should cap at 5 lines, got {line_count}:\n{result}"
        );
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
             Use `log`/`diff` for history, `search` for fuzzy find, `ops_log`/`config` for repo metadata. \
             `log` supports `offset`/`limit` for pagination and `search` for filtering by commit message.",
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
