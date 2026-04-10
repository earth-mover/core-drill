use ratatui::prelude::*;

use crate::app::App;
use crate::store::LoadState;
use crate::store::types::ArraySummary;
use crate::fetch::ZarrMetadata;
use crate::ui::widgets::{
    compute_grid_chunks, fmt_initialized, format_vcc_prefix, labeled_lines, section_header,
};

/// Render the header + Shape & Layout section for an array node (shown above the canvas viz).
pub(super) fn render_array_detail_header<'a>(
    app: &'a App,
    node: &crate::store::TreeNode,
    summary: &'a ArraySummary,
    max_width: u16,
) -> (Vec<Line<'a>>, Option<&'a ZarrMetadata>) {
    let shape_str = crate::output::fmt_dims(&summary.shape);

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

    // Use cached parsed metadata (parsed once at construction, not per frame)
    let meta = summary.parsed_metadata.as_ref();

    if let Some(meta) = meta {
        let chunk_str = crate::output::fmt_dims(&meta.chunk_shape);
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
    if let Some(meta) = meta {
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

    (lines, meta)
}

/// Render the Storage + Attributes + Raw Metadata sections for an array node (shown after the canvas viz).
pub(super) fn render_array_detail_storage<'a>(
    app: &'a App,
    path: &str,
    snapshot_id: Option<&str>,
    summary: &ArraySummary,
    meta: Option<&ZarrMetadata>,
    max_width: u16,
) -> Vec<Line<'a>> {
    // Pre-compute grid size (requires both shape and chunk_shape from metadata)
    let grid_chunks: Option<u64> = meta.and_then(|m| compute_grid_chunks(summary, m));

    let mut lines = Vec::new();

    // ─── Storage ─────────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Storage"));

    if let Some(meta) = meta {
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
            let total_data =
                stats.native_total_bytes + stats.inline_total_bytes + stats.virtual_total_bytes;
            if total_data > 0 {
                let avg_bytes = total_data / stats.total_chunks as u64;
                lines.extend(labeled_lines(
                    "  Data size:     ",
                    format!(
                        "{}  (avg {} / chunk)",
                        humansize::format_size(total_data, humansize::BINARY),
                        humansize::format_size(avg_bytes, humansize::BINARY),
                    ),
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
    if let Some(meta) = meta
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
            let json_lines = crate::ui::json_view::render_json(&attr_json, &app.theme, 10, 50);
            lines.extend(json_lines);
        }
    }

    // ─── Raw Metadata ────────────────────
    if let Some(meta) = meta
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
            let json_lines = crate::ui::json_view::render_json(&extra_json, &app.theme, 10, 50);
            lines.extend(json_lines);
        }
    }

    // Fallback: if metadata was present but couldn't be parsed, show with json_view
    if !summary.zarr_metadata.is_empty() && meta.is_none() {
        lines.push(Line::from(""));
        lines.push(section_header("Raw Metadata"));
        let json_lines =
            crate::ui::json_view::render_json(&summary.zarr_metadata, &app.theme, 10, 50);
        lines.extend(json_lines);
    }

    lines
}
