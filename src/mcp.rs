//! MCP (Model Context Protocol) server for core-drill.
//!
//! Exposes repository inspection as MCP tools that agents can call.
//! The repo stays open for the lifetime of the server, avoiding
//! repeated S3/GCS fetches on each tool call.
//!
//! Start with: `core-drill <repo> --serve`
//! Configure in Claude Code settings as an MCP server.

use std::sync::Arc;

use icechunk::Repository;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    schemars, tool, tool_handler, tool_router,
};

use crate::output;

/// MCP server wrapping an open icechunk repository.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CoreDrillServer {
    tool_router: ToolRouter<Self>,
    repo: Arc<Repository>,
    repo_url: String,
}

impl CoreDrillServer {
    pub fn new(repo: Repository, repo_url: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            repo: Arc::new(repo),
            repo_url,
        }
    }
}

// ─── Tool parameter structs ─────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EmptyParams {}

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

#[tool_router]
impl CoreDrillServer {
    #[tool(
        description = "Get repository overview: branches, tags, recent snapshots, and full node tree. Call this first to understand what's in the repo."
    )]
    async fn info(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        let repo = &self.repo;
        let branches = match output::fetch_branches(repo).await {
            Ok(b) => b,
            Err(e) => return format!("Error: {e}"),
        };
        let tags = match output::fetch_tags(repo).await {
            Ok(t) => t,
            Err(e) => return format!("Error: {e}"),
        };

        let mut out = format!("# Repository: {}\n\n", self.repo_url);

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
            if let Ok(ancestry) = output::fetch_ancestry(repo, &branch.name).await {
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

            if let Ok(tree) = output::fetch_tree_flat(repo, &branch.name, None).await {
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

        out.push_str("\n---\n*Tools: `info`, `branches`, `tags`, `log`, `tree`*");
        out
    }

    #[tool(description = "List all branches with their tip snapshot IDs")]
    async fn branches(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        match output::fetch_branches(&self.repo).await {
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
        }
    }

    #[tool(description = "List all tags")]
    async fn tags(&self, Parameters(_params): Parameters<EmptyParams>) -> String {
        match output::fetch_tags(&self.repo).await {
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
        }
    }

    #[tool(
        description = "Show snapshot history (commit log). Use ref to specify a branch, tag, or snapshot ID."
    )]
    async fn log(&self, Parameters(params): Parameters<LogParams>) -> String {
        match output::fetch_ancestry(&self.repo, &params.r#ref).await {
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
        }
    }

    #[tool(
        description = "Browse the node tree. Use path to inspect a specific array's detailed metadata (shape, dtype, chunks, codecs, fill value, initialization %)."
    )]
    async fn tree(&self, Parameters(params): Parameters<TreeParams>) -> String {
        match output::fetch_tree_flat(&self.repo, &params.r#ref, params.path.as_deref()).await {
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
        }
    }
}

// Formatting reuses output::fmt_tree_line and output::fmt_node_detail

// ─── ServerHandler trait ────────────────────────────────────

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CoreDrillServer {}

/// Start the MCP server on stdio transport.
pub async fn serve(repo: Repository, repo_url: String) -> color_eyre::Result<()> {
    use rmcp::ServiceExt;

    let server = CoreDrillServer::new(repo, repo_url);
    let transport = rmcp::transport::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
