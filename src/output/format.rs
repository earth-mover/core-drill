//! Markdown formatting helpers shared by the CLI output, MCP tools, and the TUI.
//!
//! All functions are pure (no I/O) and return `String`. Callers decide whether
//! to print, truncate, or embed the result in a larger response.

use humansize::{BINARY, format_size};

use crate::fetch::{FlatNode, FlatNodeType};
use crate::store::types::{
    BranchInfo, ChunkStats, DiffSummary, RepoConfig, SnapshotEntry, TagInfo,
};

// ─── Primitive helpers ────────────────────────────────────────

/// Truncate a string to at most `max` bytes at a valid UTF-8 boundary.
pub(crate) fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Format a dimension list as a `×`-separated string (e.g. `"721 × 1440"`).
pub(crate) fn fmt_dims(dims: &[u64]) -> String {
    dims.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(" × ")
}

// ─── Node / tree formatting ───────────────────────────────────

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
                    let parent = crate::util::parent_path(&n.path);
                    parent == node.path
                })
                .count();
            format!("{indent}- **{}/** ({} children)\n", node.name, child_count)
        }
    }
}

/// Format chunk statistics as markdown (type breakdown, sizes, avg chunk size).
pub(crate) fn fmt_chunk_stats(stats: &ChunkStats) -> String {
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
        if data_size > 0 {
            let avg = data_size / stats.total_chunks as u64;
            out.push_str(&format!(
                "- **Data size:** {} (avg {} / chunk)\n",
                format_size(data_size, BINARY),
                format_size(avg, BINARY),
            ));
        } else {
            out.push_str(&format!(
                "- **Data size:** {}\n",
                format_size(data_size, BINARY)
            ));
        }
    }
    out
}

// ─── Shared section helper ────────────────────────────────────

/// Render a headed list section, capping at `limit` entries.
/// Returns empty string if `items` is empty.
pub(crate) fn fmt_section(heading: &str, items: &[String], limit: usize) -> String {
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

// ─── Higher-level formatters (MCP + CLI) ─────────────────────

/// Format a `DiffSummary` as markdown.
pub(crate) fn fmt_diff_detail(detail: &DiffSummary, snapshot_id: &str) -> String {
    if detail.is_initial_commit {
        return format!(
            "# Diff: `{}`\n\nInitial commit — all nodes are new.",
            truncate(snapshot_id, 12)
        );
    }
    const DIFF_SECTION_LIMIT: usize = 50;
    let mut out = format!("# Diff: `{}`\n\n", truncate(snapshot_id, 12));
    if let Some(ref pid) = detail.parent_id {
        out.push_str(&format!("Parent: `{}`\n\n", truncate(pid, 12)));
    }

    out.push_str(&fmt_section(
        "Added Arrays",
        &detail.added_arrays,
        DIFF_SECTION_LIMIT,
    ));
    out.push_str(&fmt_section(
        "Added Groups",
        &detail.added_groups,
        DIFF_SECTION_LIMIT,
    ));
    out.push_str(&fmt_section(
        "Deleted Arrays",
        &detail.deleted_arrays,
        DIFF_SECTION_LIMIT,
    ));
    out.push_str(&fmt_section(
        "Deleted Groups",
        &detail.deleted_groups,
        DIFF_SECTION_LIMIT,
    ));
    out.push_str(&fmt_section(
        "Modified Arrays",
        &detail.modified_arrays,
        DIFF_SECTION_LIMIT,
    ));
    out.push_str(&fmt_section(
        "Modified Groups",
        &detail.modified_groups,
        DIFF_SECTION_LIMIT,
    ));

    if !detail.chunk_changes.is_empty() {
        let chunk_items: Vec<String> = detail
            .chunk_changes
            .iter()
            .map(|(path, count)| format!("{path}: {count} chunks written"))
            .collect();
        out.push_str(&fmt_section(
            "Updated Chunks",
            &chunk_items,
            DIFF_SECTION_LIMIT,
        ));
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

/// Format a `RepoConfig` as markdown.
pub(crate) fn fmt_repo_config(cfg: &RepoConfig) -> String {
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

/// Format a repo overview as markdown (branches, tags, recent snapshots).
/// Used by both the MCP `info`/`open` tools and the CLI.
pub(crate) fn fmt_repo_overview(
    repo_url: &str,
    branches: &[BranchInfo],
    tags: &[TagInfo],
    ancestry: &[SnapshotEntry],
    r#ref: &str,
) -> String {
    let mut out = format!("# Repository: {}\n\n", repo_url);

    out.push_str(&format!("## Branches ({})\n\n", branches.len()));
    let show_branches = if branches.len() > 10 {
        5
    } else {
        branches.len()
    };
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
            truncate(&b.snapshot_id, 12),
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
        for t in tags {
            out.push_str(&format!(
                "- **{}** → `{}`\n",
                t.name,
                truncate(&t.snapshot_id, 12)
            ));
        }
    }

    if !ancestry.is_empty() {
        out.push_str(&format!(
            "\n## Recent Snapshots ({} total on `{}`)\n\n",
            ancestry.len(),
            r#ref
        ));
        out.push_str("| # | Snapshot | Time | Message |\n|---|----------|------|---------|");
        for (i, e) in ancestry.iter().take(5).enumerate() {
            let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC").to_string();
            out.push_str(&format!(
                "\n| {} | `{}` | {} | {} |",
                i + 1,
                truncate(&e.id, 12),
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
