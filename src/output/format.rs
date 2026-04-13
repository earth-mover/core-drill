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

    // Rich metadata from parsed zarr metadata
    if let Some(ref meta) = node.zarr_metadata {
        out.push_str(&format!("- **Zarr format:** {}\n", meta.zarr_format));

        if let Some(ref v2dt) = meta.v2_dtype
            && v2dt != &meta.data_type
        {
            out.push_str(&format!("- **Dtype (v2):** `{v2dt}`\n"));
        }
        if let Some(ref order) = meta.order {
            out.push_str(&format!("- **Order:** {order}\n"));
        }
        if meta.dimension_separator != "/" {
            out.push_str(&format!(
                "- **Dimension separator:** `{}`\n",
                meta.dimension_separator
            ));
        }
        if let Some(ref comp) = meta.compressor
            && !meta.codecs.is_empty()
        {
            out.push_str(&format!("- **Compressor:** {comp}\n"));
        }
        if !meta.filters.is_empty() {
            out.push_str(&format!("- **Filters:** {}\n", meta.filters.join(", ")));
        }
        if !meta.storage_transformers.is_empty() {
            out.push_str(&format!(
                "- **Storage transformers:** {}\n",
                meta.storage_transformers.join(", ")
            ));
        }
    }

    if let Some(count) = node.manifest_count {
        out.push_str(&format!("- **Manifests:** {count}\n"));
    }

    // Attributes section
    if let Some(ref meta) = node.zarr_metadata {
        if !meta.attributes.is_empty() {
            out.push_str("\n#### Attributes\n\n");
            for (k, v) in &meta.attributes {
                out.push_str(&format!("- **{k}:** {v}\n"));
            }
        }

        if !meta.extra_fields.is_empty() {
            out.push_str("\n#### Extra Metadata\n\n");
            for (k, v) in &meta.extra_fields {
                out.push_str(&format!("- **{k}:** {v}\n"));
            }
        }
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

// ─── MCP-specific formatters ──────────────────────────────────

/// Format search results for non-fuzzy modes.
pub(crate) fn fmt_search_results(
    query: &str,
    mode: &str,
    matched: &[&FlatNode],
    limit: usize,
    all_nodes: &[FlatNode],
) -> String {
    let total = matched.len();
    if total == 0 {
        return format!("No matches for \"{}\" ({} mode)", query, mode);
    }
    let mut out = format!("# Search: \"{}\" ({} matches, {})\n\n", query, total, mode);
    for node in matched.iter().take(limit) {
        out.push_str(&fmt_tree_line(node, all_nodes));
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
pub(crate) fn fmt_collapsed_tree(
    nodes: &[&FlatNode],
    all_nodes: &[FlatNode],
    max_lines: usize,
) -> String {
    if nodes.is_empty() {
        return String::new();
    }

    // Group nodes by (parent_path, type, name_prefix).
    // name_prefix = everything up to the last `-` or `_` separator, or the full name if no separator.
    struct CollapsedGroup<'a> {
        prefix: String, // shared prefix (e.g. "burst-133012-")
        nodes: Vec<&'a FlatNode>,
        depth: usize,
    }

    let mut groups: Vec<CollapsedGroup> = Vec::new();

    // Sort by path so similarly-named siblings are consecutive for collapsing
    let mut sorted: Vec<&&FlatNode> = nodes.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));

    for node in sorted.into_iter().copied() {
        let parent = crate::util::parent_path(&node.path);
        let depth = node.path.matches('/').count().saturating_sub(1);

        // Find a prefix: strip trailing digits/chars after last `-` or `_`
        let prefix = {
            let name = &node.name;
            // Find the last separator position
            let sep_pos = name.rfind(['-', '_']);
            match sep_pos {
                Some(pos) if pos > 0 && pos < name.len() - 1 => {
                    format!("{}/{}", parent, &name[..=pos])
                }
                _ => node.path.clone(),
            }
        };

        // Try to append to the last group if same prefix, type, and depth
        if let Some(last) = groups.last_mut()
            && last.prefix == prefix
            && last.depth == depth
            && last.nodes[0].node_type == node.node_type
        {
            last.nodes.push(node);
            continue;
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
                out.push_str(&fmt_tree_line(node, all_nodes));
                lines += 1;
            }
        } else {
            // Collapse into a summary line
            let indent = "  ".repeat(group.depth);
            let count = group.nodes.len();
            let kind = if group.nodes[0].is_array() {
                "arrays"
            } else {
                "groups"
            };

            // Show representative metadata from first node
            let sample = group.nodes[0];
            let meta = if sample.is_array() {
                let dtype = sample.dtype.as_deref().unwrap_or("?");
                let shape = sample
                    .shape
                    .as_ref()
                    .map(|s| fmt_dims(s))
                    .unwrap_or_else(|| "?".to_string());
                format!(" `{dtype}` `[{shape}]`")
            } else {
                String::new()
            };

            out.push_str(&format!(
                "{indent}- **{}\\*** ({} {}){}\n",
                crate::util::leaf_name(&group.prefix),
                count,
                kind,
                meta
            ));
            lines += 1;
        }
    }

    let total_items: usize = groups.iter().map(|g| g.nodes.len()).sum();
    let shown_items: usize = {
        let mut count = 0;
        let mut l = 0;
        for group in &groups {
            if l >= max_lines {
                break;
            }
            if group.nodes.len() < 3 {
                for _ in &group.nodes {
                    if l >= max_lines {
                        break;
                    }
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

// ─── Repo overview ─────────────────────────────────────────────

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
