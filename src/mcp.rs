//! MCP (Model Context Protocol) server for core-drill.
//!
//! Exposes repository inspection as MCP tools that agents can call.
//! The server can start with or without a pre-opened repo. Use the
//! `open` tool to connect to any repo on demand.
//!
//! Start with: `core-drill --serve`
//! Or pre-open: `core-drill <repo> --serve`

use std::sync::Arc;

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
    async fn info(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let _start = std::time::Instant::now();
        info!("MCP info");
        let repo = require_repo!(self);
        let repo_url = self.repo_url.read().await;

        let branches = match output::fetch_branches(&repo).await {
            Ok(b) => b,
            Err(e) => return format!("Error: {e}"),
        };
        let tags = match output::fetch_tags(&repo).await {
            Ok(t) => t,
            Err(e) => return format!("Error: {e}"),
        };

        let mut out = format!("# Repository: {}\n\n", *repo_url);

        out.push_str(&format!("## Branches ({})\n\n", branches.len()));
        for b in &branches {
            let ts = b
                .tip_timestamp
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
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

        let main_branch = branches
            .iter()
            .find(|b| b.name == "main")
            .or(branches.first());
        if let Some(branch) = main_branch {
            if let Ok(ancestry) = output::fetch_ancestry(&repo, &branch.name).await {
                out.push_str(&format!("\n## Snapshots ({})\n\n", ancestry.len()));
                out.push_str(
                    "| # | Snapshot | Time | Message |\n|---|----------|------|---------|",
                );
                for (i, e) in ancestry.iter().take(5).enumerate() {
                    let ts = e.timestamp.format("%Y-%m-%d %H:%M").to_string();
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

            if let Ok(tree) = output::fetch_tree_flat(&repo, &branch.name, None).await {
                let groups = tree.iter().filter(|n| n.is_group()).count();
                let arrays = tree.iter().filter(|n| n.is_array()).count();
                out.push_str(&format!(
                    "\n\n## Tree (at {})\n\n{} groups, {} arrays\n\n",
                    branch.name, groups, arrays
                ));
                for node in &tree {
                    out.push_str(&output::fmt_tree_line(node, &tree));
                }
            }
        }

        out.push_str("\n---\n*Drill deeper: `tree` with `path` for array detail, `log` for history, `branches`/`tags` for refs*");
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
                let entries: Vec<_> = if let Some(n) = params.limit {
                    entries.into_iter().take(n).collect()
                } else {
                    entries
                };
                let mut out = format!("# Snapshot Log ({})\n\n", params.r#ref);
                out.push_str(
                    "| # | Snapshot | Time | Message |\n|---|----------|------|---------|",
                );
                for (i, e) in entries.iter().enumerate() {
                    let ts = e.timestamp.format("%Y-%m-%d %H:%M").to_string();
                    out.push_str(&format!(
                        "\n| {} | `{}` | {} | {} |",
                        i + 1,
                        output::truncate(&e.id, 12),
                        ts,
                        e.message
                    ));
                }
                out.push_str(&format!("\n\n{} snapshot(s)", entries.len()));
                out
            }
            Err(e) => format!("Error: {e}"),
        };
        info!("MCP log completed in {:?}", _start.elapsed());
        result
    }

    #[tool(
        description = "Browse the node tree. Without `path`: lists all groups and arrays. With `path`: shows detailed array metadata (shape, dtype, chunks, codecs, fill value)."
    )]
    async fn tree(&self, Parameters(params): Parameters<TreeParams>) -> String {
        let _start = std::time::Instant::now();
        let repo = require_repo!(self);
        let result = match output::fetch_tree_flat(&repo, &params.r#ref, params.path.as_deref()).await {
            Ok(tree) => {
                if let Some(ref filter_path) = params.path {
                    if let Some(node) = tree.iter().find(|n| n.path == *filter_path) {
                        output::fmt_node_detail(node)
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
             Start with `open`, then use `info`, `tree`, `log`, `branches`, `tags` to explore.",
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
