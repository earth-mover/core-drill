//! Structured (non-interactive) output for agents and scripts.
//!
//! Currently supports Markdown output (`--output md`), designed to be
//! token-efficient for LLM agents while remaining human-readable.
//!
//! Design principle: show enough overview that an agent can drill deeper
//! with more specific subcommands.

use std::sync::Arc;

use icechunk::Repository;

use crate::cli::{Command, OutputFormat};
use crate::sanitize::sanitize;
use crate::store::types::*;
use crate::ui::format::ZarrMetadata;

/// Run the non-interactive output path. Prints to stdout and returns.
pub async fn run(
    repo: Repository,
    format: OutputFormat,
    command: Option<Command>,
    repo_url: &str,
) -> color_eyre::Result<()> {
    let repo = Arc::new(repo);

    match format {
        OutputFormat::Json => run_json(repo, command, repo_url).await,
        OutputFormat::Md => run_md(repo, command, repo_url).await,
        OutputFormat::Table => {
            eprintln!("Table output not yet implemented, using markdown");
            run_md(repo, command, repo_url).await
        }
    }
}

/// Run a persistent REPL session. Reads commands from stdin, one per line.
/// Responses are separated by `---END---` markers on their own line.
/// The repo stays open across commands, avoiding repeated S3 fetches.
pub async fn run_repl(
    repo: Repository,
    format: OutputFormat,
    repo_url: &str,
) -> color_eyre::Result<()> {
    use std::io::BufRead;

    let repo = Arc::new(repo);
    let stdin = std::io::stdin();

    // Print ready marker so callers know we're accepting commands
    println!("READY");
    println!("---END---");

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim().to_string();

        if line.is_empty() || line == "quit" || line == "exit" {
            break;
        }

        let command = parse_repl_command(&line);
        match command {
            Ok(cmd) => {
                let result = match format {
                    OutputFormat::Json => run_json(repo.clone(), cmd, repo_url).await,
                    _ => run_md(repo.clone(), cmd, repo_url).await,
                };
                if let Err(e) = result {
                    println!("Error: {e}");
                }
            }
            Err(msg) => {
                println!("Error: {msg}");
            }
        }

        // Response separator — caller reads until this line
        println!("---END---");
    }

    Ok(())
}

/// Parse a single REPL line into a Command.
/// Supports: info, branches, tags, log [-r REF] [-n LIMIT], tree [-r REF] [-p PATH]
fn parse_repl_command(line: &str) -> Result<Option<Command>, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(None);
    }

    match parts[0] {
        "info" => Ok(Some(Command::Info)),
        "branches" => Ok(Some(Command::Branches)),
        "tags" => Ok(Some(Command::Tags)),
        "help" => {
            Ok(None) // Will print overview
        }
        "log" => {
            let mut r#ref = "main".to_string();
            let mut limit = None;
            let mut i = 1;
            while i < parts.len() {
                match parts[i] {
                    "-r" | "--ref" => {
                        i += 1;
                        if i < parts.len() {
                            r#ref = parts[i].to_string();
                        }
                    }
                    "-n" | "--limit" => {
                        i += 1;
                        if i < parts.len() {
                            limit = parts[i].parse().ok();
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Ok(Some(Command::Log { r#ref, limit }))
        }
        "tree" => {
            let mut r#ref = "main".to_string();
            let mut path = None;
            let mut i = 1;
            while i < parts.len() {
                match parts[i] {
                    "-r" | "--ref" => {
                        i += 1;
                        if i < parts.len() {
                            r#ref = parts[i].to_string();
                        }
                    }
                    "-p" | "--path" => {
                        i += 1;
                        if i < parts.len() {
                            path = Some(parts[i].to_string());
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Ok(Some(Command::Tree { r#ref, path }))
        }
        other => Err(format!(
            "unknown command: '{other}'. Available: info, branches, tags, log, tree"
        )),
    }
}

// ─── JSON ────────────────────────────────────────────────────

async fn run_json(
    repo: Arc<Repository>,
    command: Option<Command>,
    repo_url: &str,
) -> color_eyre::Result<()> {
    match command {
        None | Some(Command::Info) => {
            let info = fetch_repo_info(&repo, repo_url).await?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Some(Command::Branches) => {
            let branches = fetch_branches(&repo).await?;
            println!("{}", serde_json::to_string_pretty(&branches)?);
        }
        Some(Command::Tags) => {
            let tags = fetch_tags(&repo).await?;
            println!("{}", serde_json::to_string_pretty(&tags)?);
        }
        Some(Command::Log { ref r#ref, limit }) => {
            let entries = fetch_ancestry(&repo, r#ref).await?;
            let entries = if let Some(n) = limit {
                entries.into_iter().take(n).collect()
            } else {
                entries
            };
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        Some(Command::Tree {
            ref r#ref,
            ref path,
        }) => {
            let tree = fetch_tree_flat(&repo, r#ref, path.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&tree)?);
        }
        Some(Command::OpsLog { limit }) => {
            let entries = fetch_ops_log(&repo, limit).await?;
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
    }
    Ok(())
}

// ─── Markdown ────────────────────────────────────────────────

async fn run_md(
    repo: Arc<Repository>,
    command: Option<Command>,
    repo_url: &str,
) -> color_eyre::Result<()> {
    match command {
        None | Some(Command::Info) => {
            print_md_overview(&repo, repo_url).await?;
        }
        Some(Command::Branches) => {
            let branches = fetch_branches(&repo).await?;
            println!("# Branches\n");
            if branches.is_empty() {
                println!("(none)");
            } else {
                println!("| Branch | Snapshot | Time | Message |");
                println!("|--------|----------|------|---------|");
                for b in &branches {
                    let ts = b
                        .tip_timestamp
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    let msg = b.tip_message.as_deref().unwrap_or("");
                    println!(
                        "| {} | `{}` | {} | {} |",
                        b.name,
                        truncate(&b.snapshot_id, 12),
                        ts,
                        msg
                    );
                }
            }
        }
        Some(Command::Tags) => {
            let tags = fetch_tags(&repo).await?;
            println!("# Tags\n");
            if tags.is_empty() {
                println!("(none)");
            } else {
                println!("| Tag | Snapshot | Time | Message |");
                println!("|-----|----------|------|---------|");
                for t in &tags {
                    let ts = t
                        .tip_timestamp
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    let msg = t.tip_message.as_deref().unwrap_or("");
                    println!(
                        "| {} | `{}` | {} | {} |",
                        t.name,
                        truncate(&t.snapshot_id, 12),
                        ts,
                        msg
                    );
                }
            }
        }
        Some(Command::Log { ref r#ref, limit }) => {
            let entries = fetch_ancestry(&repo, r#ref).await?;
            let entries: Vec<_> = if let Some(n) = limit {
                entries.into_iter().take(n).collect()
            } else {
                entries
            };
            println!("# Snapshot Log ({})\n", r#ref);
            println!("| # | Snapshot | Time | Message |");
            println!("|---|----------|------|---------|");
            for (i, e) in entries.iter().enumerate() {
                let ts = e.timestamp.format("%Y-%m-%d %H:%M").to_string();
                println!(
                    "| {} | `{}` | {} | {} |",
                    i + 1,
                    truncate(&e.id, 12),
                    ts,
                    e.message
                );
            }
            println!("\n{} snapshot(s) total", entries.len());
        }
        Some(Command::Tree {
            ref r#ref,
            ref path,
        }) => {
            print_md_tree(&repo, r#ref, path.as_deref()).await?;
        }
        Some(Command::OpsLog { limit }) => {
            let entries = fetch_ops_log(&repo, limit).await?;
            println!("# Operations Log\n");
            if entries.is_empty() {
                println!("(no operations recorded)");
            } else {
                println!("| Time | Operation |");
                println!("|------|-----------|");
                for entry in &entries {
                    let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC");
                    println!("| {} | {} |", ts, entry.description);
                }
            }
        }
    }
    Ok(())
}

/// Overview: repo info + branches + tree summary.
/// Designed to give an agent enough context to decide what to drill into.
async fn print_md_overview(repo: &Repository, repo_url: &str) -> color_eyre::Result<()> {
    let (branches, tags) = tokio::join!(fetch_branches(repo), fetch_tags(repo));
    let branches = branches?;
    let tags = tags?;

    println!("# Repository: {}\n", repo_url);

    // Branches
    println!("## Branches ({})\n", branches.len());
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
        println!(
            "- **{}** → `{}`  {}{}",
            b.name,
            truncate(&b.snapshot_id, 12),
            ts,
            msg_part
        );
    }

    // Tags
    if !tags.is_empty() {
        println!("\n## Tags ({})\n", tags.len());
        for t in &tags {
            println!("- **{}** → `{}`", t.name, truncate(&t.snapshot_id, 12));
        }
    }

    // Snapshot count from main branch ancestry
    let main_branch = branches
        .iter()
        .find(|b| b.name == "main")
        .or(branches.first());
    if let Some(branch) = main_branch {
        let ancestry = fetch_ancestry(repo, &branch.name).await?;
        println!("\n## Snapshots ({})\n", ancestry.len());
        // Show last 5 snapshots
        let recent: Vec<_> = ancestry.iter().take(5).collect();
        println!("| # | Snapshot | Time | Message |");
        println!("|---|----------|------|---------|");
        for (i, e) in recent.iter().enumerate() {
            let ts = e.timestamp.format("%Y-%m-%d %H:%M").to_string();
            println!(
                "| {} | `{}` | {} | {} |",
                i + 1,
                truncate(&e.id, 12),
                ts,
                e.message
            );
        }
        if ancestry.len() > 5 {
            println!(
                "\n*({} more — use `log` subcommand to see all)*",
                ancestry.len() - 5
            );
        }

        // Tree summary at branch tip
        println!("\n## Tree (at {})\n", branch.name);
        let tree = fetch_tree_flat(repo, &branch.name, None).await?;
        print_md_tree_nodes(&tree);
    }

    // Hint for agents
    println!("\n---");
    println!(
        "*Subcommands: `info`, `branches`, `tags`, `log [-r REF] [-n LIMIT]`, `tree [-r REF] [-p PATH]`*"
    );

    Ok(())
}

/// Print the tree in markdown. If a specific path is given, show detail for that node.
async fn print_md_tree(
    repo: &Repository,
    branch: &str,
    path_filter: Option<&str>,
) -> color_eyre::Result<()> {
    let tree = fetch_tree_flat(repo, branch, path_filter).await?;

    if let Some(filter_path) = path_filter {
        // Find the specific node and show detail
        if let Some(node) = tree.iter().find(|n| n.path == filter_path) {
            print_md_node_detail(node);
        } else {
            // Show nodes under this path prefix
            let matching: Vec<_> = tree
                .iter()
                .filter(|n| n.path.starts_with(filter_path))
                .collect();
            if matching.is_empty() {
                println!("No nodes found at path: {}", filter_path);
            } else {
                println!("# Tree: {} ({} nodes)\n", filter_path, matching.len());
                for node in matching {
                    print_md_node_detail(node);
                    println!();
                }
            }
        }
    } else {
        println!("# Tree (at {})\n", branch);
        print_md_tree_nodes(&tree);
    }

    Ok(())
}

pub(crate) fn fmt_dims(dims: &[u64]) -> String {
    dims.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(" × ")
}

fn print_md_tree_nodes(tree: &[FlatNode]) {
    let groups = tree.iter().filter(|n| n.is_group()).count();
    let arrays = tree.iter().filter(|n| n.is_array()).count();
    println!("{} groups, {} arrays\n", groups, arrays);
    for node in tree {
        print!("{}", fmt_tree_line(node, tree));
    }
}

fn print_md_node_detail(node: &FlatNode) {
    print!("{}", fmt_node_detail(node));
}

/// Format a single node's detailed metadata as markdown (shared by CLI + MCP).
pub(crate) fn fmt_node_detail(node: &FlatNode) -> String {
    let mut out = format!("### {}\n\n", node.path);
    out.push_str(&format!("- **Type:** {}\n", node.node_type));
    if let Some(ref shape) = node.shape {
        out.push_str(&format!("- **Shape:** `[{}]`\n", fmt_dims(shape)));
    }
    if let Some(ref dtype) = node.dtype {
        out.push_str(&format!("- **Data type:** `{dtype}`\n"));
    }
    if let Some(ref chunk_shape) = node.chunk_shape {
        out.push_str(&format!(
            "- **Chunk shape:** `[{}]`\n",
            fmt_dims(chunk_shape)
        ));
    }
    if let Some(ref dims) = node.dimensions {
        out.push_str(&format!("- **Dimensions:** {}\n", dims.join(", ")));
    }
    if let (Some(written), Some(grid)) = (node.total_chunks, node.grid_chunks) {
        if grid > 0 {
            let pct = written * 100 / grid;
            out.push_str(&format!(
                "- **Initialized:** {written} of {grid} ({pct}%)\n"
            ));
        }
    } else if let Some(written) = node.total_chunks {
        out.push_str(&format!("- **Total chunks:** {written}\n"));
    }
    if let Some(ref codecs) = node.codecs {
        out.push_str(&format!("- **Codecs:** {codecs}\n"));
    }
    if let Some(ref fill) = node.fill_value {
        out.push_str(&format!("- **Fill value:** `{fill}`\n"));
    }
    out
}

/// Format a single tree node as a one-line markdown entry (shared by CLI + MCP).
pub(crate) fn fmt_tree_line(node: &FlatNode, tree: &[FlatNode]) -> String {
    let depth = node.path.matches('/').count().saturating_sub(1);
    let indent = "  ".repeat(depth);
    match node.node_type {
        FlatNodeType::Array => {
            let shape = node
                .shape
                .as_ref()
                .map(|s| fmt_dims(s))
                .unwrap_or_else(|| "?".to_string());
            let dtype = node.dtype.as_deref().unwrap_or("?");
            let chunks_info =
                if let (Some(written), Some(grid)) = (node.total_chunks, node.grid_chunks) {
                    if grid > 0 {
                        let pct = written * 100 / grid;
                        format!("  ({written}/{grid} chunks, {pct}% initialized)")
                    } else {
                        format!("  ({written} chunks)")
                    }
                } else if let Some(written) = node.total_chunks {
                    format!("  ({written} chunks)")
                } else {
                    String::new()
                };
            format!(
                "{indent}- **{}** `{dtype}` `[{shape}]`{chunks_info}\n",
                node.name
            )
        }
        FlatNodeType::Group => {
            let child_count = tree
                .iter()
                .filter(|n| {
                    let parent = match n.path.rfind('/') {
                        Some(0) => "/",
                        Some(idx) => &n.path[..idx],
                        None => "/",
                    };
                    parent == node.path
                })
                .count();
            format!("{indent}- **{}/** ({} children)\n", node.name, child_count)
        }
    }
}

// ─── Data structures for flat tree output ────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FlatNodeType {
    Group,
    Array,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct FlatNode {
    pub path: String,
    pub name: String,
    pub node_type: FlatNodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_shape: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_chunks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid_chunks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codecs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_value: Option<String>,
}

impl FlatNode {
    /// Shorthand for constructing a group node (all array fields are None).
    pub fn group(path: String, name: String) -> Self {
        Self {
            path,
            name,
            node_type: FlatNodeType::Group,
            shape: None,
            dtype: None,
            chunk_shape: None,
            dimensions: None,
            total_chunks: None,
            grid_chunks: None,
            codecs: None,
            fill_value: None,
        }
    }

    pub fn is_group(&self) -> bool {
        self.node_type == FlatNodeType::Group
    }
    pub fn is_array(&self) -> bool {
        self.node_type == FlatNodeType::Array
    }
}

impl std::fmt::Display for FlatNodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlatNodeType::Group => write!(f, "group"),
            FlatNodeType::Array => write!(f, "array"),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct RepoInfo {
    url: String,
    branch_count: usize,
    tag_count: usize,
    snapshot_count: usize,
    branches: Vec<BranchInfo>,
    tags: Vec<TagInfo>,
}

// ─── Data fetching (direct icechunk API, no DataStore) ───────

pub(crate) async fn fetch_repo_info(
    repo: &Repository,
    repo_url: &str,
) -> color_eyre::Result<RepoInfo> {
    let (branches, tags) = tokio::join!(fetch_branches(repo), fetch_tags(repo));
    let branches = branches?;
    let tags = tags?;
    // Get snapshot count from the main/first branch ancestry
    let main = branches
        .iter()
        .find(|b| b.name == "main")
        .or(branches.first());
    let snapshot_count = if let Some(branch) = main {
        fetch_ancestry(repo, &branch.name)
            .await
            .map(|a| a.len())
            .unwrap_or(0)
    } else {
        0
    };
    Ok(RepoInfo {
        url: repo_url.to_string(),
        branch_count: branches.len(),
        tag_count: tags.len(),
        snapshot_count,
        branches,
        tags,
    })
}

pub(crate) async fn fetch_branches(repo: &Repository) -> color_eyre::Result<Vec<BranchInfo>> {
    let branch_names = repo.list_branches().await?;
    let mut result = Vec::with_capacity(branch_names.len());
    for name in branch_names {
        let snapshot_id = repo
            .lookup_branch(&name)
            .await
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        result.push(BranchInfo {
            name: sanitize(&name),
            snapshot_id,
            tip_timestamp: None,
            tip_message: None,
        });
    }
    Ok(result)
}

pub(crate) async fn fetch_tags(repo: &Repository) -> color_eyre::Result<Vec<TagInfo>> {
    let tag_names = repo.list_tags().await?;
    let mut result = Vec::with_capacity(tag_names.len());
    for name in tag_names {
        let snapshot_id = repo
            .lookup_tag(&name)
            .await
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        result.push(TagInfo {
            name: sanitize(&name),
            snapshot_id,
            tip_timestamp: None,
            tip_message: None,
        });
    }
    Ok(result)
}

/// Resolve a ref string to a VersionInfo. Tries: branch, tag, then snapshot ID.
async fn resolve_ref(
    repo: &Repository,
    r: &str,
) -> color_eyre::Result<icechunk::repository::VersionInfo> {
    use icechunk::repository::VersionInfo;

    // Try branch first
    if repo.lookup_branch(r).await.is_ok() {
        return Ok(VersionInfo::BranchTipRef(r.to_string()));
    }
    // Try tag
    if repo.lookup_tag(r).await.is_ok() {
        return Ok(VersionInfo::TagRef(r.to_string()));
    }
    // Try snapshot ID (Crockford Base32)
    if let Ok(snap_id) = r.try_into() {
        return Ok(VersionInfo::SnapshotId(snap_id));
    }

    color_eyre::eyre::bail!("ref not found: '{r}' (not a branch, tag, or snapshot ID)")
}

pub(crate) async fn fetch_ancestry(
    repo: &Repository,
    r: &str,
) -> color_eyre::Result<Vec<SnapshotEntry>> {
    use futures::StreamExt;

    let version = resolve_ref(repo, r).await?;
    let stream = repo.ancestry(&version).await?;
    futures::pin_mut!(stream);

    let mut entries = Vec::new();
    while let Some(result) = stream.next().await {
        let info = result?;
        entries.push(SnapshotEntry {
            id: info.id.to_string(),
            parent_id: info.parent_id.map(|id| id.to_string()),
            timestamp: info.flushed_at,
            message: sanitize(&info.message),
        });
    }
    Ok(entries)
}

pub(crate) async fn fetch_tree_flat(
    repo: &Repository,
    r: &str,
    path_filter: Option<&str>,
) -> color_eyre::Result<Vec<FlatNode>> {
    use icechunk::format::snapshot::NodeData;

    let version = resolve_ref(repo, r).await?;
    let session = repo.readonly_session(&version).await?;

    let snapshot = repo
        .asset_manager()
        .fetch_snapshot(session.snapshot_id())
        .await?;

    let nodes_iter = session.list_nodes(&icechunk::format::Path::root()).await?;

    let mut flat_nodes = Vec::new();

    for node_result in nodes_iter {
        let node = node_result?;
        let path_str = node.path.to_string();
        if path_str == "/" {
            continue;
        }

        let name = path_str.rsplit('/').next().unwrap_or("").to_string();

        match &node.node_data {
            NodeData::Group => {
                flat_nodes.push(FlatNode::group(sanitize(&path_str), sanitize(&name)));
            }
            NodeData::Array {
                shape,
                dimension_names,
                manifests,
            } => {
                let dims: Vec<u64> = shape.iter().map(|d| d.array_length()).collect();
                let dim_names: Option<Vec<String>> = dimension_names.as_ref().map(|names| {
                    names
                        .iter()
                        .filter_map(|n| {
                            let opt: Option<String> = n.clone().into();
                            opt.map(|s| sanitize(&s))
                        })
                        .collect()
                });

                let zarr_metadata = String::from_utf8_lossy(&node.user_data).to_string();
                let meta = if !zarr_metadata.is_empty() {
                    ZarrMetadata::parse(&zarr_metadata)
                } else {
                    None
                };

                let chunk_shape_vec = meta.as_ref().map(|m| m.chunk_shape.clone());
                let dtype = meta.as_ref().map(|m| m.data_type.clone());
                let codecs = meta
                    .as_ref()
                    .map(|m| m.codec_chain_display())
                    .filter(|s| !s.is_empty());
                let fill_value = meta.as_ref().map(|m| m.fill_value.clone());

                // Total chunks from snapshot manifest metadata
                let total_chunks: Option<u64> = {
                    let mut sum: u64 = 0;
                    let mut all_found = true;
                    for mref in manifests.iter() {
                        if let Ok(Some(info)) = snapshot.manifest_info(&mref.object_id) {
                            sum += info.num_chunk_refs as u64;
                        } else {
                            all_found = false;
                            break;
                        }
                    }
                    if all_found { Some(sum) } else { None }
                };

                // Grid size: product of ceil(shape[i] / chunk_shape[i])
                let grid_chunks = meta.as_ref().and_then(|m| {
                    if dims.is_empty()
                        || m.chunk_shape.is_empty()
                        || dims.len() != m.chunk_shape.len()
                    {
                        return None;
                    }
                    dims.iter()
                        .zip(m.chunk_shape.iter())
                        .try_fold(1u64, |acc, (s, c)| {
                            if *c == 0 {
                                return None;
                            }
                            acc.checked_mul(s.div_ceil(*c))
                        })
                });

                flat_nodes.push(FlatNode {
                    path: sanitize(&path_str),
                    name: sanitize(&name),
                    node_type: FlatNodeType::Array,
                    shape: Some(dims.clone()),
                    dtype,
                    chunk_shape: chunk_shape_vec,
                    dimensions: dim_names,
                    total_chunks,
                    grid_chunks,
                    codecs,
                    fill_value,
                });
            }
        }
    }

    // Apply path filter if specified
    if let Some(filter) = path_filter {
        flat_nodes.retain(|n| n.path == filter || n.path.starts_with(&format!("{filter}/")));
    }

    Ok(flat_nodes)
}

pub(crate) fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

pub(crate) async fn fetch_ops_log(
    repo: &Repository,
    limit: Option<usize>,
) -> color_eyre::Result<Vec<OpsLogEntry>> {
    use futures::StreamExt;
    use icechunk::format::repo_info::UpdateType;

    let (stream, _repo_info, _version) = repo.ops_log().await?;
    futures::pin_mut!(stream);

    let max = limit.unwrap_or(usize::MAX);
    let mut entries = Vec::new();
    while let Some(result) = stream.next().await {
        let (timestamp, update_type, backup_path) = result?;

        let description = match &update_type {
            UpdateType::RepoInitializedUpdate => "Repository initialized".to_string(),
            UpdateType::RepoMigratedUpdate {
                from_version,
                to_version,
            } => format!("Migrated from v{from_version} to v{to_version}"),
            UpdateType::RepoStatusChangedUpdate { status } => {
                format!("Status changed to {status:?}")
            }
            UpdateType::ConfigChangedUpdate => "Configuration changed".to_string(),
            UpdateType::MetadataChangedUpdate => "Metadata changed".to_string(),
            UpdateType::TagCreatedUpdate { name } => format!("Tag created: {}", sanitize(name)),
            UpdateType::TagDeletedUpdate { name, .. } => {
                format!("Tag deleted: {}", sanitize(name))
            }
            UpdateType::BranchCreatedUpdate { name } => {
                format!("Branch created: {}", sanitize(name))
            }
            UpdateType::BranchDeletedUpdate { name, .. } => {
                format!("Branch deleted: {}", sanitize(name))
            }
            UpdateType::BranchResetUpdate { name, .. } => {
                format!("Branch reset: {}", sanitize(name))
            }
            UpdateType::NewCommitUpdate {
                branch,
                new_snap_id,
            } => format!(
                "Commit on {}: {}",
                sanitize(branch),
                &new_snap_id.to_string()[..12]
            ),
            UpdateType::CommitAmendedUpdate { branch, .. } => {
                format!("Commit amended on {}", sanitize(branch))
            }
            UpdateType::NewDetachedSnapshotUpdate { new_snap_id } => {
                format!("Detached snapshot: {}", &new_snap_id.to_string()[..12])
            }
            UpdateType::GCRanUpdate => "Garbage collection ran".to_string(),
            UpdateType::ExpirationRanUpdate => "Snapshot expiration ran".to_string(),
            UpdateType::FeatureFlagChanged { id, new_value } => {
                format!("Feature flag '{id}' → {new_value:?}")
            }
        };

        entries.push(OpsLogEntry {
            timestamp,
            description,
            backup_path,
        });

        if entries.len() >= max {
            break;
        }
    }
    Ok(entries)
}
