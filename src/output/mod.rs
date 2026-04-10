//! Structured (non-interactive) output for agents and scripts.
//!
//! Currently supports Markdown output (`--output md`), designed to be
//! token-efficient for LLM agents while remaining human-readable.
//!
//! Design principle: show enough overview that an agent can drill deeper
//! with more specific subcommands.

pub(crate) mod format;

// Re-export all formatting helpers so existing `crate::output::*` call-sites
// continue to work without any changes.
pub(crate) use format::*;

use std::sync::Arc;

use icechunk::Repository;

use crate::cli::{Command, OutputFormat};
use crate::fetch::*;

// ─── Public entry points ──────────────────────────────────────

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

// ─── REPL command parser ──────────────────────────────────────

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
        Some(Command::Alias { .. }) => unreachable!("alias handled before repo open"),
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
                        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
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
                        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
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
                let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC").to_string();
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
        Some(Command::Alias { .. }) => unreachable!("alias handled before repo open"),
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
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
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
            let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC").to_string();
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
            print!("{}", fmt_node_detail(node));
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
                    print!("{}", fmt_node_detail(node));
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

fn print_md_tree_nodes(tree: &[FlatNode]) {
    let groups = tree.iter().filter(|n| n.is_group()).count();
    let arrays = tree.iter().filter(|n| n.is_array()).count();
    println!("{} groups, {} arrays\n", groups, arrays);
    for node in tree {
        print!("{}", fmt_tree_line(node, tree));
    }
}
