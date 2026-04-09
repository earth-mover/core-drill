use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::Pane;
use crate::store::LoadState;
use crate::store::types::TreeNodeType;
use super::widgets::{clamped_scroll, labeled_lines, section_header, format_vcc_prefix, compute_grid_chunks, fmt_initialized, render_tabbed_panel};
use super::diff::render_snapshot_diff_detail;

/// Aggregated storage stats computed from the current tree + chunk stats cache.
struct StorageStats {
    total_arrays: usize,
    total_groups: usize,
    total_written: u64,
    known_native: usize,
    known_inline: usize,
    known_virtual: usize,
    native_bytes: u64,
    inline_bytes: u64,
    virtual_bytes: u64,
    stats_loaded: usize,
    virtual_prefixes: std::collections::HashMap<String, usize>,
}

impl StorageStats {
    fn from_store(store: &crate::store::DataStore) -> Self {
        let mut s = Self {
            total_arrays: 0,
            total_groups: 0,
            total_written: 0,
            known_native: 0,
            known_inline: 0,
            known_virtual: 0,
            native_bytes: 0,
            inline_bytes: 0,
            virtual_bytes: 0,
            stats_loaded: 0,
            virtual_prefixes: std::collections::HashMap::new(),
        };

        for state in store.node_children.values() {
            if let crate::store::LoadState::Loaded(nodes) = state {
                for node in nodes {
                    match &node.node_type {
                        TreeNodeType::Group => s.total_groups += 1,
                        TreeNodeType::Array(summary) => {
                            s.total_arrays += 1;
                            if let Some(tc) = summary.total_chunks {
                                s.total_written += tc;
                            }
                        }
                    }
                }
            }
        }

        for ((_, _), state) in &store.chunk_stats {
            if let crate::store::LoadState::Loaded(stats) = state {
                s.stats_loaded += 1;
                s.known_native += stats.native_count;
                s.known_inline += stats.inline_count;
                s.known_virtual += stats.virtual_count;
                s.native_bytes += stats.native_total_bytes;
                s.inline_bytes += stats.inline_total_bytes;
                s.virtual_bytes += stats.virtual_total_bytes;
                for (prefix, count) in &stats.virtual_prefixes {
                    *s.virtual_prefixes.entry(prefix.clone()).or_insert(0) += count;
                }
            }
        }

        s
    }

    fn total_bytes(&self) -> u64 {
        self.native_bytes + self.inline_bytes + self.virtual_bytes
    }

    fn stored_bytes(&self) -> u64 {
        self.native_bytes + self.inline_bytes
    }

    fn breakdown_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.known_native > 0 {
            parts.push(format!("{} native", self.known_native));
        }
        if self.known_inline > 0 {
            parts.push(format!("{} inline", self.known_inline));
        }
        if self.known_virtual > 0 {
            parts.push(format!("{} virtual", self.known_virtual));
        }
        parts
    }
}

/// Find a TreeNode by its path, searching all cached children in the store.
pub(super) fn find_node_by_path<'a>(
    store: &'a crate::store::DataStore,
    path: &str,
) -> Option<&'a crate::store::TreeNode> {
    for state in store.node_children.values() {
        if let crate::store::LoadState::Loaded(nodes) = state
            && let Some(node) = nodes.iter().find(|n| n.path == path)
        {
            return Some(node);
        }
    }
    None
}

pub(super) fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    use crate::component::DetailMode;

    let focused = app.focused_pane == Pane::Detail;
    let active_tab = match app.detail_mode {
        DetailMode::Node => 0,
        DetailMode::Repo => 1,
        DetailMode::OpsLog => 2,
        DetailMode::Branch => 3,
        DetailMode::Snapshot => 4,
    };
    let (content_area, _tab_bar) = match render_tabbed_panel(
        "[2] Detail",
        &["Node", "Repo", "Ops Log", "Branch", "Snap"],
        active_tab,
        focused,
        &app.theme,
        frame,
        area,
    ) {
        Some(areas) => areas,
        None => return,
    };

    // Helper: render text into the content area with scrolling
    let render_text = |text: Vec<Line>, frame: &mut Frame| {
        let scroll = clamped_scroll(app.detail_scroll, text.len(), content_area);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            content_area,
        );
    };

    // Repo mode
    if app.detail_mode == DetailMode::Repo {
        render_text(render_repo_overview(app), frame);
        return;
    }

    // Ops Log mode
    if app.detail_mode == DetailMode::OpsLog {
        render_text(render_ops_log(app), frame);
        return;
    }

    // Branch mode
    if app.detail_mode == DetailMode::Branch {
        if let Some(branches) = app.store.branches.as_loaded()
            && let Some(branch) = branches.get(app.bottom_selected())
        {
            let branch_name = branch.name.clone();
            let is_current = branch_name == app.current_branch;
            render_text(render_branch_detail(app, &branch_name, is_current), frame);
        } else {
            render_text(vec![
                Line::from(""),
                Line::from(Span::styled("  Select a branch in the bottom panel.", app.theme.text_dim)),
            ], frame);
        }
        return;
    }

    // Snapshot mode
    if app.detail_mode == DetailMode::Snapshot {
        if let Some(sid) = app.selected_snapshot_id()
            && (app.store.diffs.contains_key(&sid) || app.last_diff_requested.as_deref() == Some(&sid))
        {
            let inner_width = content_area.width;
            render_text(render_snapshot_diff_detail(app, &sid, inner_width), frame);
        } else {
            render_text(vec![
                Line::from(""),
                Line::from(Span::styled("  Select a snapshot in the bottom panel.", app.theme.text_dim)),
            ], frame);
        }
        return;
    }

    // Node mode below — the remaining default

    // Check what's selected in the tree
    let selected = app.tree_state.selected();
    let selected_path = selected.last();

    // For array nodes: split into header + shape (top), canvas viz (middle), storage + rest (bottom)
    if let Some(path) = selected_path
        && let Some(node) = find_node_by_path(&app.store, path)
        && let TreeNodeType::Array(summary) = &node.node_type
    {
        let inner_width = content_area.width;
        let snapshot_id = app
            .selected_snapshot_id()
            .or_else(|| app.get_branch_tip_snapshot_id());
        let mut text = render_array_detail_header(app, node, summary, inner_width);
        text.extend(render_array_detail_storage(
            app,
            node.path.as_str(),
            snapshot_id.as_deref(),
            summary,
            inner_width,
        ));
        let scroll = clamped_scroll(app.detail_scroll, text.len(), content_area);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            content_area,
        );
        return;
    }

    // Non-array nodes: groups, repo overview
    let text = if let Some(path) = selected_path {
        if let Some(node) = find_node_by_path(&app.store, path) {
            match &node.node_type {
                TreeNodeType::Array(_) => unreachable!(),
                TreeNodeType::Group => render_group_detail(app, node),
            }
        } else {
            render_repo_overview(app)
        }
    } else {
        render_repo_overview(app)
    };

    let scroll = clamped_scroll(app.detail_scroll, text.len(), content_area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content_area,
    );
}

/// Render the header + Shape & Layout section for an array node (shown above the canvas viz).
fn render_array_detail_header<'a>(
    app: &'a App,
    node: &crate::store::TreeNode,
    summary: &crate::store::types::ArraySummary,
    max_width: u16,
) -> Vec<Line<'a>> {
    let shape_str = summary
        .shape
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(" \u{00d7} ");

    let dim_names = summary
        .dimension_names
        .as_ref()
        .map(|dims| dims.join(", "))
        .unwrap_or_else(|| "\u{2014}".to_string());

    let mut lines = vec![Line::from("")];
    lines.extend(labeled_lines(
        "  Array: ",
        node.name.clone(),
        app.theme.text_dim,
        app.theme.text_bold,
        max_width,
    ));
    lines.extend(labeled_lines(
        "  Path:  ",
        node.path.clone(),
        app.theme.text_dim,
        app.theme.text,
        max_width,
    ));

    // ─── Shape & Layout ──────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Shape & Layout"));

    lines.extend(labeled_lines(
        "  Shape:         ",
        shape_str.clone(),
        app.theme.text_dim,
        app.theme.text,
        max_width,
    ));

    // Parse metadata early so we can use chunk_shape for the layout section
    let meta = if !summary.zarr_metadata.is_empty() {
        super::format::ZarrMetadata::parse(&summary.zarr_metadata)
    } else {
        None
    };

    if let Some(ref meta) = meta {
        let chunk_str = meta
            .chunk_shape
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(" \u{00d7} ");
        lines.extend(labeled_lines(
            "  Chunk shape:   ",
            chunk_str,
            app.theme.text_dim,
            app.theme.text,
            max_width,
        ));
        lines.extend(labeled_lines(
            "  Data type:     ",
            meta.data_type.clone(),
            app.theme.text_dim,
            app.theme.text,
            max_width,
        ));

        // Show v2 dtype if different from data_type
        if let Some(ref v2dt) = meta.v2_dtype
            && v2dt != &meta.data_type
        {
            lines.extend(labeled_lines(
                "  Dtype (v2):    ",
                v2dt.clone(),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }
    }

    lines.extend(labeled_lines(
        "  Dimensions:    ",
        dim_names,
        app.theme.text_dim,
        app.theme.text,
        max_width,
    ));

    // Chunks per dimension: shape[i] / chunk_shape[i]
    if let Some(ref meta) = meta {
        if !summary.shape.is_empty()
            && !meta.chunk_shape.is_empty()
            && summary.shape.len() == meta.chunk_shape.len()
        {
            let chunks_per_dim: Vec<String> = summary
                .shape
                .iter()
                .zip(meta.chunk_shape.iter())
                .map(|(&s, &c)| {
                    if c > 0 {
                        s.div_ceil(c).to_string()
                    } else {
                        "?".to_string()
                    }
                })
                .collect();
            lines.extend(labeled_lines(
                "  Chunks/dim:    ",
                chunks_per_dim.join(" \u{00d7} "),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }

        // Memory layout order (v2)
        if let Some(ref order) = meta.order {
            lines.extend(labeled_lines(
                "  Order:         ",
                order.clone(),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }
    }

    // Chunk grid summary line (textual) — the graphical canvas follows immediately below
    if let Some(summary_line) = crate::ui::shape_viz::chunk_summary_line(summary, &app.theme) {
        lines.push(summary_line);
    }

    lines
}

/// Render the Storage + Attributes + Raw Metadata sections for an array node (shown after the canvas viz).
fn render_array_detail_storage<'a>(
    app: &'a App,
    path: &str,
    snapshot_id: Option<&str>,
    summary: &crate::store::types::ArraySummary,
    max_width: u16,
) -> Vec<Line<'a>> {
    let meta = if !summary.zarr_metadata.is_empty() {
        super::format::ZarrMetadata::parse(&summary.zarr_metadata)
    } else {
        None
    };

    // Pre-compute grid size (requires both shape and chunk_shape from metadata)
    let grid_chunks: Option<u64> = meta.as_ref().and_then(|m| compute_grid_chunks(summary, m));

    let mut lines = Vec::new();

    // ─── Storage ─────────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Storage"));

    if let Some(ref meta) = meta {
        let codec_display = meta.codec_chain_display();
        if !codec_display.is_empty() {
            lines.extend(labeled_lines(
                "  Codecs:        ",
                codec_display,
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }

        // v2 compressor (shown separately if codecs were also present)
        if let Some(ref comp) = meta.compressor
            && !meta.codecs.is_empty()
        {
            // Already shown via codec_chain_display, but if both exist show compressor
            // separately for clarity
            lines.extend(labeled_lines(
                "  Compressor:    ",
                comp.clone(),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }

        // v2 filters
        if !meta.filters.is_empty() {
            lines.extend(labeled_lines(
                "  Filters:       ",
                meta.filters.join(", "),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }

        lines.extend(labeled_lines(
            "  Fill value:    ",
            meta.fill_value.clone(),
            app.theme.text_dim,
            app.theme.text,
            max_width,
        ));
        lines.extend(labeled_lines(
            "  Zarr format:   ",
            meta.zarr_format.to_string(),
            app.theme.text_dim,
            app.theme.text,
            max_width,
        ));

        if meta.dimension_separator != "/" {
            lines.extend(labeled_lines(
                "  Dim separator: ",
                meta.dimension_separator.clone(),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }

        // Storage transformers
        if !meta.storage_transformers.is_empty() {
            lines.extend(labeled_lines(
                "  Transformers:  ",
                meta.storage_transformers.join(", "),
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
        }
    }

    lines.extend(labeled_lines(
        "  Manifests:     ",
        summary.manifest_count.to_string(),
        app.theme.text_dim,
        app.theme.text,
        max_width,
    ));

    // ─── Chunk Types ─────────────────────
    let chunk_stats_key = snapshot_id.map(|sid| (sid.to_string(), path.to_string()));
    match chunk_stats_key
        .as_ref()
        .and_then(|k| app.store.chunk_stats.get(k))
    {
        None | Some(LoadState::NotRequested) => {
            // No full stats yet — show snapshot-derived total if available
            if let Some(total) = summary.total_chunks {
                lines.push(Line::from(""));
                lines.push(section_header("Chunk Types"));
                if total == 0 {
                    lines.push(Line::from(Span::styled(
                        "  (no chunks written)",
                        app.theme.text_dim,
                    )));
                } else {
                    lines.extend(labeled_lines(
                        "  Total:         ",
                        format!("{total}"),
                        app.theme.text_dim,
                        app.theme.text,
                        max_width,
                    ));
                    if let Some(grid) = grid_chunks {
                        lines.extend(labeled_lines(
                            "  Initialized:   ",
                            fmt_initialized(total, grid),
                            app.theme.text_dim,
                            app.theme.text,
                            max_width,
                        ));
                    }
                }
            }
            // If total_chunks is None (pre-V2 snapshot), skip section silently
        }
        Some(LoadState::Loading) => {
            lines.push(Line::from(""));
            lines.push(section_header("Chunk Types"));
            if let Some(total) = summary.total_chunks {
                lines.extend(labeled_lines(
                    "  Total:         ",
                    format!("{total} (loading type breakdown...)"),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
                if let Some(grid) = grid_chunks {
                    lines.extend(labeled_lines(
                        "  Initialized:   ",
                        fmt_initialized(total, grid),
                        app.theme.text_dim,
                        app.theme.text,
                        max_width,
                    ));
                }
            } else {
                lines.push(Line::from(Span::styled("  Loading...", app.theme.loading)));
            }
        }
        Some(LoadState::Error(e)) => {
            lines.push(Line::from(""));
            lines.push(section_header("Chunk Types"));
            lines.push(Line::from(Span::styled(
                format!("  Error: {e}"),
                app.theme.error,
            )));
        }
        Some(LoadState::Loaded(stats)) => {
            lines.push(Line::from(""));
            lines.push(section_header("Chunk Types"));

            let total = stats.total_chunks.max(1);

            // Build the total line — include breakdown summary if all types present
            let has_breakdown =
                stats.native_count > 0 || stats.inline_count > 0 || stats.virtual_count > 0;
            if has_breakdown {
                let mut parts = Vec::new();
                if stats.native_count > 0 {
                    parts.push(format!("{} native", stats.native_count));
                }
                if stats.inline_count > 0 {
                    parts.push(format!("{} inline", stats.inline_count));
                }
                if stats.virtual_count > 0 {
                    parts.push(format!("{} virtual", stats.virtual_count));
                }
                lines.extend(labeled_lines(
                    "  Total:         ",
                    format!("{}: {}", stats.total_chunks, parts.join(", ")),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            } else {
                lines.extend(labeled_lines(
                    "  Total:         ",
                    stats.total_chunks.to_string(),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            }

            // Total data size across all chunk types
            let total_data = stats.native_total_bytes + stats.inline_total_bytes + stats.virtual_total_bytes;
            if total_data > 0 {
                lines.extend(labeled_lines(
                    "  Data size:     ",
                    humansize::format_size(total_data, humansize::BINARY),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            }

            if stats.native_count > 0 {
                let pct = stats.native_count * 100 / total;
                let size_str = humansize::format_size(stats.native_total_bytes, humansize::BINARY);
                lines.extend(labeled_lines(
                    "  Native:        ",
                    format!("{} ({pct}%)   {size_str}", stats.native_count),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            }
            if stats.inline_count > 0 {
                let pct = stats.inline_count * 100 / total;
                let size_str = humansize::format_size(stats.inline_total_bytes, humansize::BINARY);
                lines.extend(labeled_lines(
                    "  Inline:        ",
                    format!("{} ({pct}%)   {size_str}", stats.inline_count),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            }
            if stats.virtual_count > 0 {
                let pct = stats.virtual_count * 100 / total;
                let size_str = humansize::format_size(stats.virtual_total_bytes, humansize::BINARY);
                lines.extend(labeled_lines(
                    "  Virtual:       ",
                    format!(
                        "{} ({pct}%)   {size_str}   {} source{}",
                        stats.virtual_count,
                        stats.virtual_source_count,
                        if stats.virtual_source_count == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
                if !stats.virtual_prefixes.is_empty() {
                    lines.push(Line::from(Span::styled("    Sources:", app.theme.text_dim)));
                    for (prefix, count) in &stats.virtual_prefixes {
                        let display = format_vcc_prefix(prefix, &app.repo_info);
                        lines.push(Line::from(vec![
                            Span::styled(display, app.theme.text),
                            Span::styled(format!("  ({count} chunks)"), app.theme.text_dim),
                        ]));
                    }
                    if stats.virtual_source_count > stats.virtual_prefixes.len() {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "      ... and {} more source{}",
                                stats.virtual_source_count - stats.virtual_prefixes.len(),
                                if stats.virtual_source_count - stats.virtual_prefixes.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ),
                            app.theme.text_dim,
                        )));
                    }
                }
            }
            // Initialized fraction (written / total grid positions)
            if stats.total_chunks > 0
                && let Some(grid) = grid_chunks
            {
                lines.extend(labeled_lines(
                    "  Initialized:   ",
                    fmt_initialized(stats.total_chunks as u64, grid),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            }

            // If all zeros (empty array), show explicit zero
            if stats.total_chunks == 0 {
                lines.push(Line::from(Span::styled(
                    "  (no chunks written)",
                    app.theme.text_dim,
                )));
            }
        }
    }

    // ─── Attributes ──────────────────────
    if let Some(ref meta) = meta
        && !meta.attributes.is_empty()
    {
        lines.push(Line::from(""));
        lines.push(section_header("Attributes"));
        // Build a JSON object from the attributes and render with json_view
        let attr_obj: serde_json::Value = serde_json::Value::Object(
            meta.attributes
                .iter()
                .map(|(k, v)| {
                    // Try to parse the value back to JSON; if it fails, keep as string
                    let json_val = serde_json::from_str(v)
                        .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
                    (k.clone(), json_val)
                })
                .collect(),
        );
        if let Ok(attr_json) = serde_json::to_string(&attr_obj) {
            let json_lines = super::json_view::render_json(&attr_json, &app.theme, 10, 50);
            lines.extend(json_lines);
        }
    }

    // ─── Raw Metadata ────────────────────
    if let Some(ref meta) = meta
        && !meta.extra_fields.is_empty()
    {
        lines.push(Line::from(""));
        lines.push(section_header("Raw Metadata"));
        // Build a JSON object from extra fields and render with json_view
        let extra_obj: serde_json::Value = serde_json::Value::Object(
            meta.extra_fields
                .iter()
                .map(|(k, v)| {
                    let json_val = serde_json::from_str(v)
                        .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
                    (k.clone(), json_val)
                })
                .collect(),
        );
        if let Ok(extra_json) = serde_json::to_string(&extra_obj) {
            let json_lines = super::json_view::render_json(&extra_json, &app.theme, 10, 50);
            lines.extend(json_lines);
        }
    }

    // Fallback: if metadata was present but couldn't be parsed, show with json_view
    if !summary.zarr_metadata.is_empty() && meta.is_none() {
        lines.push(Line::from(""));
        lines.push(section_header("Raw Metadata"));
        let json_lines = super::json_view::render_json(&summary.zarr_metadata, &app.theme, 10, 50);
        lines.extend(json_lines);
    }

    lines
}

fn render_group_detail<'a>(app: &'a App, node: &crate::store::TreeNode) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Group", app.theme.text_bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Path:      ", app.theme.text_dim),
            Span::styled(node.path.clone(), app.theme.text),
        ]),
    ];

    if let Some(crate::store::LoadState::Loaded(children)) = app.store.node_children.get(&node.path)
    {
        lines.push(Line::from(vec![
            Span::styled("  Children:  ", app.theme.text_dim),
            Span::styled(children.len().to_string(), app.theme.text),
        ]));
        lines.push(Line::from(""));

        for child in children {
            let icon = match &child.node_type {
                TreeNodeType::Group => "📁 ",
                TreeNodeType::Array(_) => "📊 ",
            };
            lines.push(Line::from(Span::styled(
                format!("    {icon}{}", child.name),
                app.theme.text,
            )));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Children:  ", app.theme.text_dim),
            Span::styled("not loaded (press Enter to expand)", app.theme.text_dim),
        ]));
    }

    lines
}

fn render_repo_overview<'a>(app: &'a App) -> Vec<Line<'a>> {
    let branch_count = app.store.branches.as_loaded().map(|b| b.len()).unwrap_or(0);
    let tag_count = app.store.tags.as_loaded().map(|t| t.len()).unwrap_or(0);
    let snapshot_count = app
        .store
        .ancestry
        .get(&app.current_branch)
        .and_then(|s| s.as_loaded())
        .map(|a| a.len())
        .unwrap_or(0);

    let mut lines = Vec::new();

    // ─── Repository ─────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Repository"));

    match &app.repo_info {
        crate::app::RepoIdentity::Arraylake {
            org,
            repo,
            bucket,
            platform,
            region,
        } => {
            lines.push(Line::from(vec![
                Span::styled("  Organization:  ", app.theme.text_dim),
                Span::styled(org.clone(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Repo name:     ", app.theme.text_dim),
                Span::styled(repo.clone(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Bucket:        ", app.theme.text_dim),
                Span::styled(bucket.clone(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Platform:      ", app.theme.text_dim),
                Span::styled(platform.clone(), app.theme.text),
            ]));
            if region != "?" {
                lines.push(Line::from(vec![
                    Span::styled("  Region:        ", app.theme.text_dim),
                    Span::styled(region.clone(), app.theme.text),
                ]));
            }
        }
        crate::app::RepoIdentity::Local { path } => {
            lines.push(Line::from(vec![
                Span::styled("  Location:      ", app.theme.text_dim),
                Span::styled(path.clone(), app.theme.text),
            ]));
        }
        crate::app::RepoIdentity::S3 { url } => {
            lines.push(Line::from(vec![
                Span::styled("  Location:      ", app.theme.text_dim),
                Span::styled(url.clone(), app.theme.text),
            ]));
        }
    }

    lines.push(Line::from(vec![
        Span::styled("  Branch:        ", app.theme.text_dim),
        Span::styled(app.current_branch.clone(), app.theme.branch),
    ]));

    // ─── Contents ───────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Contents"));
    lines.push(Line::from(vec![
        Span::styled("  Branches:    ", app.theme.text_dim),
        Span::styled(branch_count.to_string(), app.theme.text),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Tags:        ", app.theme.text_dim),
        Span::styled(tag_count.to_string(), app.theme.text),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Snapshots:   ", app.theme.text_dim),
        Span::styled(snapshot_count.to_string(), app.theme.text),
    ]));

    // ─── Storage Summary ─────────────────
    {
        let ss = StorageStats::from_store(&app.store);

        if ss.total_arrays > 0 || ss.total_groups > 0 {
            lines.push(Line::from(""));
            lines.push(section_header("Storage Summary"));
            lines.push(Line::from(vec![
                Span::styled("  Arrays:      ", app.theme.text_dim),
                Span::styled(ss.total_arrays.to_string(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Groups:      ", app.theme.text_dim),
                Span::styled(ss.total_groups.to_string(), app.theme.text),
            ]));
            if ss.total_written > 0 {
                lines.push(Line::from(vec![
                    Span::styled("  Chunks:      ", app.theme.text_dim),
                    Span::styled(ss.total_written.to_string(), app.theme.text),
                ]));
            }

            if ss.stats_loaded > 0 {
                let total_bytes = ss.total_bytes();
                let stored_bytes = ss.stored_bytes();
                let parts = ss.breakdown_parts();

                let suffix = if ss.stats_loaded < ss.total_arrays {
                    format!("  ({}/{} arrays scanned)", ss.stats_loaded, ss.total_arrays)
                } else {
                    String::new()
                };

                lines.push(Line::from(vec![
                    Span::styled("  Breakdown:   ", app.theme.text_dim),
                    Span::styled(format!("{}{}", parts.join(", "), suffix), app.theme.text),
                ]));
                let size_label = if ss.stats_loaded < ss.total_arrays {
                    format!(
                        "{}+  (scanning…)",
                        humansize::format_size(total_bytes, humansize::BINARY)
                    )
                } else {
                    humansize::format_size(total_bytes, humansize::BINARY)
                };
                lines.push(Line::from(vec![
                    Span::styled("  Data size:   ", app.theme.text_dim),
                    Span::styled(size_label, app.theme.text),
                ]));
                if ss.virtual_bytes > 0 && stored_bytes > 0 {
                    lines.push(Line::from(vec![
                        Span::styled("    Stored:    ", app.theme.text_dim),
                        Span::styled(
                            humansize::format_size(stored_bytes, humansize::BINARY),
                            app.theme.text,
                        ),
                        Span::styled("  (in this repo)", app.theme.text_dim),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("    Virtual:   ", app.theme.text_dim),
                        Span::styled(
                            humansize::format_size(ss.virtual_bytes, humansize::BINARY),
                            app.theme.text,
                        ),
                        Span::styled("  (external sources)", app.theme.text_dim),
                    ]));
                }
            }

            // ─── Virtual Sources ─────────────
            if !ss.virtual_prefixes.is_empty() {
                let mut sorted_prefixes: Vec<(String, usize)> =
                    ss.virtual_prefixes.into_iter().collect();
                sorted_prefixes.sort_by(|a, b| b.1.cmp(&a.1));

                let total_vchunks: usize = sorted_prefixes.iter().map(|(_, c)| c).sum();

                lines.push(Line::from(""));
                lines.push(section_header("Virtual Sources"));
                lines.push(Line::from(vec![
                    Span::styled("  Total:       ", app.theme.text_dim),
                    Span::styled(
                        format!(
                            "{total_vchunks} chunks, {}",
                            humansize::format_size(ss.virtual_bytes, humansize::BINARY)
                        ),
                        app.theme.text,
                    ),
                ]));
                for (prefix, count) in &sorted_prefixes {
                    let display = format_vcc_prefix(prefix, &app.repo_info);
                    let display = display.trim_start();
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {display}"), app.theme.text),
                        Span::styled(format!("  ({count} chunks)"), app.theme.text_dim),
                    ]));
                }
                if ss.stats_loaded < ss.total_arrays {
                    lines.push(Line::from(Span::styled(
                        format!("  ({}/{} arrays scanned)", ss.stats_loaded, ss.total_arrays),
                        app.theme.text_dim,
                    )));
                }
            }
        }
    }

    // ─── Configuration ──────────────────
    if let crate::store::LoadState::Loaded(config) = &app.store.repo_config {
        lines.push(Line::from(""));
        lines.push(section_header("Configuration"));
        lines.push(Line::from(vec![
            Span::styled("  Spec version:  ", app.theme.text_dim),
            Span::styled(config.spec_version.clone(), app.theme.text),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Status:        ", app.theme.text_dim),
            Span::styled(config.availability.clone(), app.theme.text),
        ]));
        if let Some(threshold) = config.inline_chunk_threshold {
            lines.push(Line::from(vec![
                Span::styled("  Inline ≤       ", app.theme.text_dim),
                Span::styled(format!("{threshold} bytes"), app.theme.text),
            ]));
        }

        // ─── Feature Flags ──────────────
        if !config.feature_flags.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_header("Feature Flags"));
            for flag in &config.feature_flags {
                let status = if flag.enabled { "on" } else { "off" };
                let explicit = if flag.explicit { "" } else { " (default)" };
                let style = if flag.enabled {
                    app.theme.status_ok
                } else {
                    app.theme.text_dim
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}: ", flag.name), app.theme.text_dim),
                    Span::styled(format!("{status}{explicit}"), style),
                ]));
            }
        }

        // ─── Virtual Chunk Containers ───
        if !config.virtual_chunk_containers.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_header("Virtual Sources"));
            for (name, prefix) in &config.virtual_chunk_containers {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {name}: "), app.theme.text_dim),
                    Span::styled(prefix.clone(), app.theme.text),
                ]));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Navigate the tree or select a snapshot to see details.",
        app.theme.text_dim,
    )));

    lines
}

fn render_ops_log<'a>(app: &'a App) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));

    match &app.store.ops_log {
        crate::store::LoadState::Loaded(entries) if !entries.is_empty() => {
            lines.push(Line::from(Span::styled(
                format!("  {} operations", entries.len()),
                app.theme.text_dim,
            )));
            lines.push(Line::from(""));

            for entry in entries {
                let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
                lines.push(Line::from(vec![
                    Span::styled(format!("  {ts}  "), app.theme.text_dim),
                    Span::styled(entry.description.clone(), app.theme.text),
                ]));
            }
        }
        crate::store::LoadState::Loaded(_) => {
            lines.push(Line::from(Span::styled(
                "  No operations recorded.",
                app.theme.text_dim,
            )));
        }
        crate::store::LoadState::Loading => {
            lines.push(Line::from(Span::styled(
                "  Loading...",
                app.theme.loading,
            )));
        }
        crate::store::LoadState::Error(e) => {
            let kind = crate::store::classify_error(e);
            let hint = match kind {
                crate::store::ErrorKind::Auth => "  (credentials may be expired — press R to retry)",
                crate::store::ErrorKind::Network => "  (network issue — press R to retry)",
                crate::store::ErrorKind::NotFound => "  (not found — press R to retry)",
                crate::store::ErrorKind::Other => "  (press R to retry)",
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  Error: {e}"), app.theme.error),
                Span::styled(hint, app.theme.text_dim),
            ]));
        }
        crate::store::LoadState::NotRequested => {
            lines.push(Line::from(Span::styled(
                "  Not loaded yet.",
                app.theme.text_dim,
            )));
        }
    }

    lines
}

fn render_branch_detail<'a>(app: &'a App, branch_name: &str, is_current: bool) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    // ─── Branch Header ─────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Branch"));

    lines.push(Line::from(vec![
        Span::styled("  Name:        ", app.theme.text_dim),
        Span::styled(branch_name.to_string(), app.theme.branch),
        if is_current {
            Span::styled("  (active)", app.theme.status_ok)
        } else {
            Span::styled("  (press Enter to switch)", app.theme.text_dim)
        },
    ]));

    // Find the BranchInfo for snapshot ID
    if let Some(branch) = app.store.branches.as_loaded()
        .and_then(|bs| bs.iter().find(|b| b.name == branch_name))
    {
        lines.push(Line::from(vec![
            Span::styled("  Tip:         ", app.theme.text_dim),
            Span::styled(
                crate::output::truncate(&branch.snapshot_id, 12).to_string(),
                app.theme.text,
            ),
        ]));
    }

    // ─── Recent Commits ────────────────
    if let Some(crate::store::LoadState::Loaded(ancestry)) = app.store.ancestry.get(branch_name) {
        lines.push(Line::from(""));
        lines.push(section_header(&format!("Recent Commits ({})", ancestry.len())));

        for entry in ancestry.iter().take(10) {
            let ts = entry.timestamp.format("%Y-%m-%d %H:%M");
            let msg = if entry.message.is_empty() {
                "(no message)".to_string()
            } else {
                entry.message.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {ts}  "), app.theme.text_dim),
                Span::styled(
                    crate::output::truncate(&entry.id, 8).to_string(),
                    app.theme.text_dim,
                ),
                Span::styled(format!("  {msg}"), app.theme.text),
            ]));
        }
        if ancestry.len() > 10 {
            lines.push(Line::from(Span::styled(
                format!("  … {} more", ancestry.len() - 10),
                app.theme.text_dim,
            )));
        }
    } else if is_current {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Loading commit history…",
            app.theme.loading,
        )));
    }

    // ─── Storage Stats (only for active branch — data already loaded) ───
    if is_current {
        let ss = StorageStats::from_store(&app.store);

        if ss.total_arrays > 0 || ss.total_groups > 0 {
            lines.push(Line::from(""));
            lines.push(section_header("Storage"));
            lines.push(Line::from(vec![
                Span::styled("  Arrays:      ", app.theme.text_dim),
                Span::styled(ss.total_arrays.to_string(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Groups:      ", app.theme.text_dim),
                Span::styled(ss.total_groups.to_string(), app.theme.text),
            ]));
            if ss.total_written > 0 {
                lines.push(Line::from(vec![
                    Span::styled("  Chunks:      ", app.theme.text_dim),
                    Span::styled(ss.total_written.to_string(), app.theme.text),
                ]));
            }

            if ss.stats_loaded > 0 {
                let parts = ss.breakdown_parts();

                let suffix = if ss.stats_loaded < ss.total_arrays {
                    format!("  ({}/{} arrays scanned)", ss.stats_loaded, ss.total_arrays)
                } else {
                    String::new()
                };

                lines.push(Line::from(vec![
                    Span::styled("  Breakdown:   ", app.theme.text_dim),
                    Span::styled(format!("{}{}", parts.join(", "), suffix), app.theme.text),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Data size:   ", app.theme.text_dim),
                    Span::styled(
                        humansize::format_size(ss.total_bytes(), humansize::BINARY),
                        app.theme.text,
                    ),
                ]));
            }
        }
    }

    lines
}
