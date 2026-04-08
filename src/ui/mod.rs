mod format;
mod help;
pub mod json_view;
pub mod shape_viz;
pub mod tree;

use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::{BottomTab, Pane};
use crate::store::LoadState;
use crate::store::types::TreeNodeType;
use crate::theme;

/// Main render — three-pane layout
pub fn render(app: &mut App, frame: &mut Frame) {
    if app.show_help {
        help::render(app, frame, frame.area());
        return;
    }

    // Top-level: status bar, main area, [bottom panel], hint bar
    let mut constraints = vec![
        Constraint::Length(1), // status bar
    ];

    if app.bottom_visible {
        constraints.push(Constraint::Min(10)); // main area (sidebar + detail)
        constraints.push(Constraint::Length(1));  // spacer before bottom panel
        constraints.push(Constraint::Length(10)); // bottom panel
    } else {
        constraints.push(Constraint::Min(10)); // main area takes all space
    }
    constraints.push(Constraint::Length(1)); // hint bar

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    let (status_area, main_area, bottom_area, hint_area) = if app.bottom_visible {
        // indices: 0=status, 1=main, 2=spacer, 3=bottom, 4=hint
        (vertical[0], vertical[1], Some(vertical[3]), vertical[4])
    } else {
        (vertical[0], vertical[1], None, vertical[2])
    };

    // Status bar
    render_status_bar(app, frame, status_area);

    // Main area: sidebar | detail
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // sidebar
            Constraint::Length(1),      // spacer
            Constraint::Percentage(70), // detail
        ])
        .split(main_area);

    // Store layout areas on App for mouse hit-testing
    app.sidebar_area = horizontal[0];
    app.detail_area = horizontal[2];
    app.bottom_area = bottom_area;

    render_sidebar(app, frame, horizontal[0]);
    render_detail(app, frame, horizontal[2]);

    // Bottom panel (if visible)
    if let Some(area) = bottom_area {
        render_bottom(app, frame, area);
    }

    // Hint bar
    render_hint_bar(app, frame, hint_area);
}

// ─── Status Bar ──────────────────────────────────────────────

fn render_status_bar(app: &App, frame: &mut Frame, area: Rect) {
    let status = match (&app.store.branches, &app.store.tags) {
        (LoadState::Loading, _) | (_, LoadState::Loading) => "connecting...",
        (LoadState::Error(_), _) | (_, LoadState::Error(_)) => "error",
        (LoadState::Loaded(_), LoadState::Loaded(_)) => "ready",
        _ => "",
    };

    let line = Line::from(vec![
        Span::styled(" ", app.theme.text),
        Span::styled(&app.repo_url, app.theme.branch),
        Span::styled("  ", app.theme.text_dim),
        Span::styled(status, if status == "ready" { app.theme.status_ok } else { app.theme.loading }),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ─── Sidebar (tree view) ─────────────────────────────────────

fn render_sidebar(app: &mut App, frame: &mut Frame, area: Rect) {
    let focused = app.focused_pane == Pane::Sidebar;
    let block = theme::panel("[1] Tree", focused, &app.theme);

    // Branch selector at top + tree below
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // branch selector
            Constraint::Min(1),   // tree
        ])
        .split(inner);

    // Branch selector
    let branch_line = Line::from(vec![
        Span::styled(" ", app.theme.text_dim),
        Span::styled(&app.current_branch, app.theme.branch),
        Span::styled(" ▾", app.theme.text_dim),
    ]);
    frame.render_widget(Paragraph::new(branch_line), chunks[0]);

    // Tree view — build TreeItems from cached store data
    let root_path = "/";
    let state = app
        .store
        .node_children
        .get(root_path)
        .unwrap_or(&LoadState::NotRequested);

    match state {
        LoadState::NotRequested | LoadState::Loading => {
            frame.render_widget(theme::loading_widget(&app.theme), chunks[1]);
        }
        LoadState::Error(msg) => {
            frame.render_widget(theme::error_widget(msg, &app.theme), chunks[1]);
        }
        LoadState::Loaded(nodes) => {
            let tree_items: Vec<tui_tree_widget::TreeItem<String>> = nodes
                .iter()
                .map(|node| build_tree_item(node, &app.store, 0))
                .collect();

            let tree = tui_tree_widget::Tree::new(&tree_items)
                .expect("unique identifiers")
                .highlight_style(if focused { app.theme.selected } else { app.theme.text_dim })
                .node_closed_symbol("▶ ")
                .node_open_symbol("▼ ")
                .node_no_children_symbol("─ ");

            frame.render_stateful_widget(tree, chunks[1], &mut app.tree_state);
        }
    }
}

/// Maximum recursion depth for tree building (safety limit)
const MAX_TREE_DEPTH: usize = 64;

/// Build a TreeItem from a store TreeNode, recursively including cached children.
/// `depth` tracks recursion depth to prevent stack overflow from circular references.
fn build_tree_item<'a>(
    node: &crate::store::TreeNode,
    store: &crate::store::DataStore,
    depth: usize,
) -> tui_tree_widget::TreeItem<'a, String> {
    let label = match &node.node_type {
        TreeNodeType::Group => node.name.clone(),
        TreeNodeType::Array(summary) => {
            let shape = summary
                .shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("×");
            format!("{} [{}]", node.name, shape)
        }
    };

    match &node.node_type {
        TreeNodeType::Group => {
            // Safety: stop recursing if we've gone too deep
            let children: Vec<tui_tree_widget::TreeItem<String>> =
                if depth >= MAX_TREE_DEPTH {
                    vec![]
                } else if let Some(LoadState::Loaded(child_nodes)) =
                    store.node_children.get(&node.path)
                {
                    child_nodes
                        .iter()
                        // Skip any child whose path matches this node (circular ref guard)
                        .filter(|child| child.path != node.path)
                        .map(|child| build_tree_item(child, store, depth + 1))
                        .collect()
                } else {
                    // No children loaded yet — show as expandable but empty
                    vec![]
                };
            tui_tree_widget::TreeItem::new(node.path.clone(), label, children)
                .expect("unique child identifiers")
        }
        TreeNodeType::Array(_) => {
            tui_tree_widget::TreeItem::new_leaf(node.path.clone(), label)
        }
    }
}

// ─── Detail pane ─────────────────────────────────────────────

/// Find a TreeNode by its path, searching all cached children in the store.
fn find_node_by_path<'a>(
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

/// Compute a clamped scroll offset: cap detail_scroll so the last content line
/// is still visible. `content_height` is the number of lines in `text`
/// (an approximation that ignores wrapping, so it's a conservative cap).
fn clamped_scroll(detail_scroll: usize, content_height: usize, area: Rect) -> u16 {
    // area still has the border — inner height is area.height - 2 (top + bottom border)
    let visible_height = (area.height as usize).saturating_sub(2);
    let max_scroll = content_height.saturating_sub(visible_height);
    detail_scroll.min(max_scroll) as u16
}

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    let focused = app.focused_pane == Pane::Detail;
    let block = theme::panel("[2] Detail", focused, &app.theme);

    // Show snapshot diff when bottom pane is focused (or detail pane with bottom context)
    // but NOT when sidebar is focused — sidebar navigation should show tree node details
    if app.focused_pane != Pane::Sidebar && app.bottom_tab == BottomTab::Snapshots
        && let Some(sid) = app.selected_snapshot_id()
            && (app.store.diffs.contains_key(&sid) || app.last_diff_requested.as_deref() == Some(&sid)) {
                let text = render_snapshot_diff_detail(app, &sid);
                let scroll = clamped_scroll(app.detail_scroll, text.len(), area);
                frame.render_widget(
                    Paragraph::new(text)
                        .block(block)
                        .wrap(Wrap { trim: false })
                        .scroll((scroll, 0)),
                    area,
                );
                return;
            }

    // Check what's selected in the tree
    let selected = app.tree_state.selected();
    let selected_path = selected.last();

    // For array nodes: split into header + shape (top), canvas viz (middle), storage + rest (bottom)
    if let Some(path) = selected_path
        && let Some(node) = find_node_by_path(&app.store, path)
            && let TreeNodeType::Array(summary) = &node.node_type {
                // inner_width: subtract 2 for the panel border
                let inner_width = area.width.saturating_sub(2);
                // Check if we have a canvas visualization (ndim > 0)
                if summary.shape.is_empty() {
                    // Scalar — no canvas, just text (header + all sections together)
                    let pre_text = render_array_detail_header(app, node, summary, inner_width);
                    let post_text = render_array_detail_storage(app, node.path.as_str(), summary, inner_width);
                    let mut text = pre_text;
                    text.extend(post_text);
                    let scroll = clamped_scroll(app.detail_scroll, text.len(), area);
                    frame.render_widget(
                        Paragraph::new(text)
                            .block(block)
                            .wrap(Wrap { trim: false })
                            .scroll((scroll, 0)),
                        area,
                    );
                } else {
                    // For arrays: single scrollable paragraph (no canvas)
                    let mut text = render_array_detail_header(app, node, summary, inner_width);
                    text.extend(render_array_detail_storage(app, node.path.as_str(), summary, inner_width));
                    let scroll = clamped_scroll(app.detail_scroll, text.len(), area);
                    frame.render_widget(
                        Paragraph::new(text)
                            .block(block)
                            .wrap(Wrap { trim: false })
                            .scroll((scroll, 0)),
                        area,
                    );
                }
                return;
            }

    // Non-array nodes: groups, repo overview
    let text = if let Some(path) = selected_path {
        if let Some(node) = find_node_by_path(&app.store, path) {
            match &node.node_type {
                TreeNodeType::Array(_) => unreachable!(),
                TreeNodeType::Group => {
                    render_group_detail(app, node)
                }
            }
        } else {
            render_repo_overview(app)
        }
    } else {
        render_repo_overview(app)
    };

    let scroll = clamped_scroll(app.detail_scroll, text.len(), area);
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

/// Produce one or more `Line`s for a label/value pair.
///
/// If the label + value fit within `max_width` columns, a single line is returned.
/// Otherwise the value is split at word boundaries (spaces) and continuation lines
/// are indented to align with the start of the value column (i.e. `label.len()` spaces).
fn labeled_lines<'a>(
    label: &'a str,
    value: String,
    label_style: Style,
    value_style: Style,
    max_width: u16,
) -> Vec<Line<'a>> {
    let label_len = label.len();
    let available = (max_width as usize).saturating_sub(label_len);

    // Fast path: everything fits on one line.
    if value.len() <= available || available == 0 {
        return vec![Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(value, value_style),
        ])];
    }

    // Split the value into chunks that fit within `available` columns.
    // We split on spaces so we don't break inside tokens.
    let indent = " ".repeat(label_len);
    let mut result: Vec<Line<'a>> = Vec::new();
    let mut current_line = String::new();
    let mut first = true;

    for word in value.split_inclusive(' ') {
        if current_line.len() + word.len() <= available || current_line.is_empty() {
            current_line.push_str(word);
        } else {
            // Flush the current line.
            if first {
                result.push(Line::from(vec![
                    Span::styled(label, label_style),
                    Span::styled(current_line.trim_end().to_string(), value_style),
                ]));
                first = false;
            } else {
                let ind = indent.clone();
                result.push(Line::from(vec![
                    Span::styled(ind, label_style),
                    Span::styled(current_line.trim_end().to_string(), value_style),
                ]));
            }
            current_line = word.to_string();
        }
    }

    // Flush any remaining text.
    if !current_line.is_empty() {
        let trimmed = current_line.trim_end().to_string();
        if first {
            result.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(trimmed, value_style),
            ]));
        } else {
            let ind = indent.clone();
            result.push(Line::from(vec![
                Span::styled(ind, label_style),
                Span::styled(trimmed, value_style),
            ]));
        }
    }

    result
}

/// Render the header + Shape & Layout section for an array node (shown above the canvas viz).
fn render_array_detail_header<'a>(app: &'a App, node: &crate::store::TreeNode, summary: &crate::store::types::ArraySummary, max_width: u16) -> Vec<Line<'a>> {
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

    let separator = "\u{2500}";

    let mut lines = vec![Line::from("")];
    lines.extend(labeled_lines("  Array: ", node.name.clone(), app.theme.text_dim, app.theme.text_bold, max_width));
    lines.extend(labeled_lines("  Path:  ", node.path.clone(), app.theme.text_dim, app.theme.text, max_width));

    // ─── Shape & Layout ──────────────────
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {0}{0}{0} Shape & Layout {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", separator),
        app.theme.text_dim,
    )));

    lines.extend(labeled_lines("  Shape:         ", shape_str.clone(), app.theme.text_dim, app.theme.branch, max_width));

    // Parse metadata early so we can use chunk_shape for the layout section
    let meta = if !summary.zarr_metadata.is_empty() {
        format::ZarrMetadata::parse(&summary.zarr_metadata)
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
        lines.extend(labeled_lines("  Chunk shape:   ", chunk_str, app.theme.text_dim, app.theme.text, max_width));
        lines.extend(labeled_lines("  Data type:     ", meta.data_type.clone(), app.theme.text_dim, app.theme.text, max_width));

        // Show v2 dtype if different from data_type
        if let Some(ref v2dt) = meta.v2_dtype
            && v2dt != &meta.data_type {
                lines.extend(labeled_lines("  Dtype (v2):    ", v2dt.clone(), app.theme.text_dim, app.theme.text, max_width));
            }
    }

    lines.extend(labeled_lines("  Dimensions:    ", dim_names, app.theme.text_dim, app.theme.text, max_width));

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
            lines.extend(labeled_lines("  Chunks/dim:    ", chunks_per_dim.join(" \u{00d7} "), app.theme.text_dim, app.theme.text, max_width));
        }

        // Memory layout order (v2)
        if let Some(ref order) = meta.order {
            lines.extend(labeled_lines("  Order:         ", order.clone(), app.theme.text_dim, app.theme.text, max_width));
        }
    }

    // Chunk grid summary line (textual) — the graphical canvas follows immediately below
    if let Some(summary_line) = crate::ui::shape_viz::chunk_summary_line(summary, &app.theme) {
        lines.push(summary_line);
    }

    lines
}

/// Render the Storage + Attributes + Raw Metadata sections for an array node (shown after the canvas viz).
fn render_array_detail_storage<'a>(app: &'a App, path: &str, summary: &crate::store::types::ArraySummary, max_width: u16) -> Vec<Line<'a>> {
    let separator = "\u{2500}";

    let meta = if !summary.zarr_metadata.is_empty() {
        format::ZarrMetadata::parse(&summary.zarr_metadata)
    } else {
        None
    };

    let mut lines = Vec::new();

    // ─── Storage ─────────────────────────
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {0}{0}{0} Storage {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", separator),
        app.theme.text_dim,
    )));

    if let Some(ref meta) = meta {
        let codec_display = meta.codec_chain_display();
        if !codec_display.is_empty() {
            lines.extend(labeled_lines("  Codecs:        ", codec_display, app.theme.text_dim, app.theme.text, max_width));
        }

        // v2 compressor (shown separately if codecs were also present)
        if let Some(ref comp) = meta.compressor
            && !meta.codecs.is_empty() {
                // Already shown via codec_chain_display, but if both exist show compressor
                // separately for clarity
                lines.extend(labeled_lines("  Compressor:    ", comp.clone(), app.theme.text_dim, app.theme.text, max_width));
            }

        // v2 filters
        if !meta.filters.is_empty() {
            lines.extend(labeled_lines("  Filters:       ", meta.filters.join(", "), app.theme.text_dim, app.theme.text, max_width));
        }

        lines.extend(labeled_lines("  Fill value:    ", meta.fill_value.clone(), app.theme.text_dim, app.theme.text, max_width));
        lines.extend(labeled_lines("  Zarr format:   ", meta.zarr_format.to_string(), app.theme.text_dim, app.theme.text, max_width));

        if meta.dimension_separator != "/" {
            lines.extend(labeled_lines("  Dim separator: ", meta.dimension_separator.clone(), app.theme.text_dim, app.theme.text, max_width));
        }

        // Storage transformers
        if !meta.storage_transformers.is_empty() {
            lines.extend(labeled_lines("  Transformers:  ", meta.storage_transformers.join(", "), app.theme.text_dim, app.theme.text, max_width));
        }
    }

    lines.extend(labeled_lines("  Manifests:     ", summary.manifest_count.to_string(), app.theme.text_dim, app.theme.text, max_width));

    // ─── Chunk Types ─────────────────────
    let chunk_section_header = format!(
        "  {0}{0}{0} Chunk Types {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}",
        separator
    );
    match app.store.chunk_stats.get(path) {
        None | Some(LoadState::NotRequested) => {
            // No full stats yet — show snapshot-derived total if available
            if let Some(total) = summary.total_chunks {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(chunk_section_header, app.theme.text_dim)));
                if total == 0 {
                    lines.push(Line::from(Span::styled("  (no chunks written)", app.theme.text_dim)));
                } else {
                    lines.extend(labeled_lines(
                        "  Total:         ",
                        format!("{total}"),
                        app.theme.text_dim,
                        app.theme.text,
                        max_width,
                    ));
                }
            }
            // If total_chunks is None (pre-V2 snapshot), skip section silently
        }
        Some(LoadState::Loading) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(chunk_section_header, app.theme.text_dim)));
            if let Some(total) = summary.total_chunks {
                lines.extend(labeled_lines(
                    "  Total:         ",
                    format!("{total} (loading type breakdown...)", ),
                    app.theme.text_dim,
                    app.theme.text,
                    max_width,
                ));
            } else {
                lines.push(Line::from(Span::styled("  Loading...", app.theme.loading)));
            }
        }
        Some(LoadState::Error(e)) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(chunk_section_header, app.theme.text_dim)));
            lines.push(Line::from(Span::styled(format!("  Error: {e}"), app.theme.error)));
        }
        Some(LoadState::Loaded(stats)) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(chunk_section_header, app.theme.text_dim)));

            let total = stats.total_chunks.max(1);

            // Build the total line — include breakdown summary if all types present
            let has_breakdown = stats.native_count > 0 || stats.inline_count > 0 || stats.virtual_count > 0;
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
                lines.extend(labeled_lines("  Total:         ", stats.total_chunks.to_string(), app.theme.text_dim, app.theme.text, max_width));
            }

            if stats.native_count > 0 {
                let pct = stats.native_count * 100 / total;
                let size_str = humansize::format_size(stats.native_total_bytes, humansize::BINARY);
                lines.extend(labeled_lines("  Native:        ", format!("{} ({pct}%)   {size_str}", stats.native_count), app.theme.text_dim, app.theme.text, max_width));
            }
            if stats.inline_count > 0 {
                let pct = stats.inline_count * 100 / total;
                let size_str = humansize::format_size(stats.inline_total_bytes, humansize::BINARY);
                lines.extend(labeled_lines("  Inline:        ", format!("{} ({pct}%)   {size_str}", stats.inline_count), app.theme.text_dim, app.theme.text, max_width));
            }
            if stats.virtual_count > 0 {
                let pct = stats.virtual_count * 100 / total;
                let size_str = humansize::format_size(stats.virtual_total_bytes, humansize::BINARY);
                lines.extend(labeled_lines("  Virtual:       ", format!("{} ({pct}%)   {size_str}", stats.virtual_count), app.theme.text_dim, app.theme.text, max_width));
                if !stats.virtual_prefixes.is_empty() {
                    lines.push(Line::from(Span::styled("  Sources:", app.theme.text_dim)));
                    for (prefix, count) in &stats.virtual_prefixes {
                        lines.push(Line::from(vec![
                            Span::styled(format!("    {prefix}/"), app.theme.text),
                            Span::styled(format!("  ({count} chunks)"), app.theme.text_dim),
                        ]));
                    }
                }
            }
            // If all zeros (empty array), show explicit zero
            if stats.total_chunks == 0 {
                lines.push(Line::from(Span::styled("  (no chunks written)", app.theme.text_dim)));
            }
        }
    }

    // ─── Attributes ──────────────────────
    if let Some(ref meta) = meta
        && !meta.attributes.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {0}{0}{0} Attributes {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", separator),
                app.theme.text_dim,
            )));
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
                let json_lines = json_view::render_json(&attr_json, &app.theme, 10, 50);
                lines.extend(json_lines);
            }
        }

    // ─── Raw Metadata ────────────────────
    if let Some(ref meta) = meta
        && !meta.extra_fields.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {0}{0}{0} Raw Metadata {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", separator),
                app.theme.text_dim,
            )));
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
                let json_lines = json_view::render_json(&extra_json, &app.theme, 10, 50);
                lines.extend(json_lines);
            }
        }

    // Fallback: if metadata was present but couldn't be parsed, show with json_view
    if !summary.zarr_metadata.is_empty() && meta.is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {0}{0}{0} Raw Metadata {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", separator),
            app.theme.text_dim,
        )));
        let json_lines = json_view::render_json(&summary.zarr_metadata, &app.theme, 10, 50);
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

    if let Some(crate::store::LoadState::Loaded(children)) =
        app.store.node_children.get(&node.path)
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

    vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Repository: ", app.theme.text_dim),
            Span::styled(app.repo_url.clone(), app.theme.branch),
        ]),
        Line::from(vec![
            Span::styled("  Branch:     ", app.theme.text_dim),
            Span::styled(app.current_branch.clone(), app.theme.branch),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Branches:   ", app.theme.text_dim),
            Span::styled(branch_count.to_string(), app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Tags:       ", app.theme.text_dim),
            Span::styled(tag_count.to_string(), app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Snapshots:  ", app.theme.text_dim),
            Span::styled(snapshot_count.to_string(), app.theme.text),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Select a node in the tree or a snapshot in the log.",
            app.theme.text_dim,
        )),
    ]
}

fn render_snapshot_diff_detail<'a>(app: &'a App, snapshot_id: &str) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    // --- Snapshot header (from ancestry, always available instantly) ---
    let entry = app
        .store
        .ancestry
        .get(&app.current_branch)
        .and_then(|s| s.as_loaded())
        .and_then(|entries| entries.iter().find(|e| e.id == snapshot_id));

    if let Some(entry) = entry {
        let short_id = if entry.id.len() > 12 {
            &entry.id[..12]
        } else {
            &entry.id
        };
        let parent_short = entry
            .parent_id
            .as_ref()
            .map(|p| if p.len() > 12 { &p[..12] } else { p.as_str() })
            .unwrap_or("none");

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Snapshot:  ", app.theme.text_dim),
            Span::styled(short_id.to_string(), app.theme.snapshot_id),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Parent:    ", app.theme.text_dim),
            Span::styled(parent_short.to_string(), app.theme.snapshot_id),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Time:      ", app.theme.text_dim),
            Span::styled(
                entry.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                app.theme.timestamp,
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Message:   ", app.theme.text_dim),
            Span::styled(entry.message.clone(), app.theme.text),
        ]));
    }

    // --- Separator ---
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  \u{2500}\u{2500}\u{2500} Changes \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
        app.theme.text_dim,
    )));

    // --- Diff section (may still be loading) ---
    let state = app.store.diffs.get(snapshot_id);

    match state {
        None | Some(LoadState::NotRequested) => {
            lines.push(Line::from(Span::styled(
                "  Waiting for diff request...",
                app.theme.text_dim,
            )));
        }
        Some(LoadState::Loading) => {
            lines.push(Line::from(Span::styled(
                "  Computing diff...",
                app.theme.loading,
            )));
        }
        Some(LoadState::Error(msg)) => {
            lines.push(Line::from(Span::styled(
                format!("  {msg}"),
                app.theme.error,
            )));
        }
        Some(LoadState::Loaded(diff)) => {
            if diff.is_initial_commit {
                // Initial commit: no parent to diff against — show the repository contents instead.
                lines.push(Line::from(Span::styled(
                    "  Repository initialized",
                    app.theme.text_bold,
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  This is the initial commit. All arrays and groups",
                    app.theme.text_dim,
                )));
                lines.push(Line::from(Span::styled(
                    "  were created fresh \u{2014} no parent to diff against.",
                    app.theme.text_dim,
                )));

                // Show all cached nodes as "+" added entries.
                let mut all_paths: Vec<String> = Vec::new();
                for state in app.store.node_children.values() {
                    if let LoadState::Loaded(nodes) = state {
                        for node in nodes {
                            if matches!(node.node_type, crate::store::TreeNodeType::Group) {
                                all_paths.push(format!("{} (group)", node.path));
                            } else {
                                all_paths.push(node.path.clone());
                            }
                        }
                    }
                }

                if !all_paths.is_empty() {
                    all_paths.sort();
                    let count = all_paths.len();
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "  \u{2500}\u{2500}\u{2500} Contents \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
                        app.theme.text_dim,
                    )));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Contents ({count}):"),
                        &all_paths,
                        "+",
                        app.theme.added,
                    );
                }
            } else {
                let added_count = diff.added_arrays.len() + diff.added_groups.len();
                let deleted_count = diff.deleted_arrays.len() + diff.deleted_groups.len();
                let modified_count = diff.modified_arrays.len() + diff.modified_groups.len();

                let total_chunks_changed: usize =
                    diff.chunk_changes.iter().map(|(_, n)| n).sum();
                let mut summary_spans = vec![
                    Span::styled("  ", app.theme.text_dim),
                    Span::styled(format!("{added_count} added"), app.theme.added),
                    Span::styled(", ", app.theme.text_dim),
                    Span::styled(format!("{deleted_count} removed"), app.theme.removed),
                    Span::styled(", ", app.theme.text_dim),
                    Span::styled(format!("{modified_count} modified"), app.theme.modified),
                ];
                if total_chunks_changed > 0 {
                    summary_spans.push(Span::styled("  |  ", app.theme.text_dim));
                    summary_spans.push(Span::styled(
                        format!("{total_chunks_changed} chunks changed"),
                        app.theme.text_dim,
                    ));
                }
                lines.push(Line::from(summary_spans));

                // Added section (groups + arrays, grouped by parent)
                if !diff.added_groups.is_empty() || !diff.added_arrays.is_empty() {
                    let mut all_added: Vec<String> = diff
                        .added_groups
                        .iter()
                        .map(|p| format!("{p} (group)"))
                        .collect();
                    all_added.extend(diff.added_arrays.iter().cloned());
                    lines.push(Line::from(""));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Added ({added_count}):"),
                        &all_added,
                        "+",
                        app.theme.added,
                    );
                }

                // Removed section
                if !diff.deleted_groups.is_empty() || !diff.deleted_arrays.is_empty() {
                    let mut all_deleted: Vec<String> = diff
                        .deleted_groups
                        .iter()
                        .map(|p| format!("{p} (group)"))
                        .collect();
                    all_deleted.extend(diff.deleted_arrays.iter().cloned());
                    lines.push(Line::from(""));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Removed ({deleted_count}):"),
                        &all_deleted,
                        "-",
                        app.theme.removed,
                    );
                }

                // Modified section
                if !diff.modified_groups.is_empty() || !diff.modified_arrays.is_empty() {
                    let mut all_modified: Vec<String> = diff
                        .modified_groups
                        .iter()
                        .map(|p| format!("{p} (group)"))
                        .collect();
                    all_modified.extend(diff.modified_arrays.iter().cloned());
                    lines.push(Line::from(""));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Modified ({modified_count}):"),
                        &all_modified,
                        "~",
                        app.theme.modified,
                    );
                }

                // Chunk changes
                if !diff.chunk_changes.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "  Chunk Changes:",
                        app.theme.text_bold,
                    )));
                    let max_show = 20;
                    let total = diff.chunk_changes.len();
                    for (path, count) in diff.chunk_changes.iter().take(max_show) {
                        let (annotation, extra_source_lines) =
                            match app.store.chunk_stats.get(path.as_str()) {
                                Some(LoadState::Loaded(stats)) if stats.stats_complete => {
                                    let v = stats.virtual_count;
                                    let s = stats.native_count;
                                    let i = stats.inline_count;
                                    if v > 0 && s == 0 && i == 0 {
                                        if stats.virtual_prefixes.len() == 1 {
                                            // Exactly one source — show inline
                                            let p = &stats.virtual_prefixes[0].0;
                                            (format!("  (virtual \u{2192} {p})"), vec![])
                                        } else {
                                            // Multiple sources — list them indented below
                                            let source_lines: Vec<Line> = stats
                                                .virtual_prefixes
                                                .iter()
                                                .map(|(prefix, cnt)| {
                                                    Line::from(vec![
                                                        Span::styled(
                                                            format!("    {prefix}/"),
                                                            app.theme.text,
                                                        ),
                                                        Span::styled(
                                                            format!("  ({cnt} chunks)"),
                                                            app.theme.text_dim,
                                                        ),
                                                    ])
                                                })
                                                .collect();
                                            ("  (all virtual)".to_string(), source_lines)
                                        }
                                    } else if s > 0 && v == 0 && i == 0 {
                                        ("  (all stored)".to_string(), vec![])
                                    } else if i > 0 && v == 0 && s == 0 {
                                        ("  (all inline)".to_string(), vec![])
                                    } else {
                                        (format!("  (virtual: {v}, stored: {s}, inline: {i})"), vec![])
                                    }
                                }
                                _ => (String::new(), vec![]),
                            };
                        let mut row = vec![
                            Span::styled(format!("    {path}  "), app.theme.text),
                            Span::styled(format!("{count} chunks"), app.theme.text_dim),
                        ];
                        if !annotation.is_empty() {
                            row.push(Span::styled(annotation, app.theme.text_dim));
                        }
                        lines.push(Line::from(row));
                        lines.extend(extra_source_lines);
                    }
                    if total > max_show {
                        lines.push(Line::from(Span::styled(
                            format!("    ... and {} more", total - max_show),
                            app.theme.text_dim,
                        )));
                    }
                }
            }
        }
    }

    lines
}

/// Group a list of paths by their parent directory.
/// Returns `(parent_path, vec_of_leaf_names)` sorted by parent.
fn group_by_parent(paths: &[String]) -> Vec<(String, Vec<String>)> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for path in paths {
        // Find the last '/' to split parent from leaf
        match path.rfind('/') {
            Some(0) => {
                // Root-level item: parent is "/", leaf is the rest
                groups
                    .entry("/".to_string())
                    .or_default()
                    .push(path[1..].to_string());
            }
            Some(idx) => {
                let parent = format!("{}/", &path[..idx]);
                let leaf = path[idx + 1..].to_string();
                groups.entry(parent).or_default().push(leaf);
            }
            None => {
                // No slash at all — treat entire string as leaf under "/"
                groups
                    .entry("/".to_string())
                    .or_default()
                    .push(path.clone());
            }
        }
    }

    groups.into_iter().collect()
}

/// Render a section of paths grouped by parent directory with truncation.
/// `prefix` is the symbol to show before each leaf ("+", "-", "~").
fn render_grouped_paths<'a>(
    lines: &mut Vec<Line<'a>>,
    header: &str,
    paths: &[String],
    prefix: &str,
    style: Style,
) {
    const MAX_ITEMS: usize = 20;
    const SHOW_ITEMS: usize = 15;

    lines.push(Line::from(Span::styled(header.to_string(), style)));

    let grouped = group_by_parent(paths);
    let total_items: usize = paths.len();
    let mut shown = 0;

    for (parent, leaves) in &grouped {
        if shown >= SHOW_ITEMS && total_items > MAX_ITEMS {
            break;
        }

        // If there's only one group and one leaf, show flat
        if grouped.len() == 1 && leaves.len() == 1 {
            lines.push(Line::from(Span::styled(
                format!("    {prefix} {parent}{}", leaves[0]),
                style,
            )));
            shown += 1;
            continue;
        }

        lines.push(Line::from(Span::styled(
            format!("    {parent}"),
            style,
        )));

        for leaf in leaves {
            if shown >= SHOW_ITEMS && total_items > MAX_ITEMS {
                break;
            }
            lines.push(Line::from(Span::styled(
                format!("      {prefix} {leaf}"),
                style,
            )));
            shown += 1;
        }
    }

    if total_items > MAX_ITEMS {
        let remaining = total_items - SHOW_ITEMS;
        lines.push(Line::from(Span::styled(
            format!("    ... and {remaining} more"),
            style,
        )));
    }
}

// ─── Bottom panel (snapshots / branches / tags) ──────────────

fn render_bottom(app: &App, frame: &mut Frame, area: Rect) {
    let focused = app.focused_pane == Pane::Bottom;

    // Outer block wraps everything (tabs + content)
    let block = Block::default()
        .title(" [3] Version Control ")
        .borders(Borders::ALL)
        .border_type(app.theme.border_type)
        .border_style(if focused { app.theme.border_focused } else { app.theme.border });

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 {
        return;
    }

    // Split inner into tab bar + content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar (single line, no border)
            Constraint::Min(1),   // content
        ])
        .split(inner);

    // Tab bar
    let tab_labels: Vec<Line> = [
        ("Snapshots", BottomTab::Snapshots),
        ("Branches", BottomTab::Branches),
        ("Tags", BottomTab::Tags),
    ]
    .iter()
    .map(|(name, tab)| {
        let marker = if app.bottom_tab == *tab { "●" } else { "○" };
        Line::from(format!("{marker} {name}"))
    })
    .collect();

    let tabs = Tabs::new(tab_labels)
        .select(match app.bottom_tab {
            BottomTab::Snapshots => 0,
            BottomTab::Branches => 1,
            BottomTab::Tags => 2,
        })
        .style(app.theme.text_dim)
        .highlight_style(if focused { app.theme.selected } else { app.theme.text });
    frame.render_widget(tabs, chunks[0]);

    // Content based on active tab
    match app.bottom_tab {
        BottomTab::Snapshots => render_snapshot_list(app, frame, chunks[1], focused),
        BottomTab::Branches => render_branch_list(app, frame, chunks[1], focused),
        BottomTab::Tags => render_tag_list(app, frame, chunks[1], focused),
    }
}

fn render_snapshot_list(app: &App, frame: &mut Frame, area: Rect, focused: bool) {
    let state = app
        .store
        .ancestry
        .get(&app.current_branch)
        .unwrap_or(&LoadState::NotRequested);

    match state {
        LoadState::NotRequested | LoadState::Loading => {
            frame.render_widget(theme::loading_widget(&app.theme), area);
        }
        LoadState::Error(msg) => {
            frame.render_widget(theme::error_widget(msg, &app.theme), area);
        }
        LoadState::Loaded(entries) => {
            let visible_start = app.bottom_table_offset;
            let rows: Vec<Row> = entries
                .iter()
                .enumerate()
                .skip(visible_start)
                .map(|(i, entry)| {
                    let is_selected = i == app.bottom_selected;
                    let is_active = Some(i) == app.active_snapshot_index;
                    let short_id = if entry.id.len() > 12 {
                        &entry.id[..12]
                    } else {
                        &entry.id
                    };
                    let row = Row::new(vec![
                        Cell::from(Span::raw(short_id)),
                        Cell::from(Span::raw(
                            entry.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                        )),
                        Cell::from(Span::raw(&entry.message)),
                    ]);
                    if is_selected && focused {
                        row.style(app.theme.selected)
                    } else if is_selected {
                        row.style(app.theme.selected_inactive)
                    } else if is_active {
                        row.style(app.theme.active)
                    } else {
                        row.style(app.theme.text)
                    }
                })
                .collect();

            let widths = [
                Constraint::Length(14),
                Constraint::Length(18),
                Constraint::Min(20),
            ];
            let table = Table::new(rows, widths)
                .header(
                    Row::new(vec!["Snapshot", "Time", "Message (Enter=diff)"])
                        .style(app.theme.text_bold),
                );
            frame.render_widget(table, area);
        }
    }
}

fn render_branch_list(app: &App, frame: &mut Frame, area: Rect, focused: bool) {
    match &app.store.branches {
        LoadState::NotRequested | LoadState::Loading => {
            frame.render_widget(theme::loading_widget(&app.theme), area);
        }
        LoadState::Error(msg) => {
            frame.render_widget(theme::error_widget(msg, &app.theme), area);
        }
        LoadState::Loaded(branches) => {
            let items: Vec<ListItem> = branches
                .iter()
                .enumerate()
                .map(|(i, branch)| {
                    let is_selected = i == app.bottom_selected;
                    let style = if is_selected && focused {
                        app.theme.selected
                    } else if is_selected {
                        app.theme.selected_inactive
                    } else {
                        app.theme.branch
                    };
                    ListItem::new(Span::styled(&branch.name, style))
                })
                .collect();
            frame.render_widget(List::new(items), area);
        }
    }
}

fn render_tag_list(app: &App, frame: &mut Frame, area: Rect, focused: bool) {
    match &app.store.tags {
        LoadState::NotRequested | LoadState::Loading => {
            frame.render_widget(theme::loading_widget(&app.theme), area);
        }
        LoadState::Error(msg) => {
            frame.render_widget(theme::error_widget(msg, &app.theme), area);
        }
        LoadState::Loaded(tags) => {
            if tags.is_empty() {
                frame.render_widget(
                    Paragraph::new("  No tags in this repository").style(app.theme.text_dim),
                    area,
                );
                return;
            }
            let items: Vec<ListItem> = tags
                .iter()
                .enumerate()
                .map(|(i, tag)| {
                    let is_selected = i == app.bottom_selected;
                    let style = if is_selected && focused {
                        app.theme.selected
                    } else if is_selected {
                        app.theme.selected_inactive
                    } else {
                        app.theme.tag
                    };
                    ListItem::new(Span::styled(&tag.name, style))
                })
                .collect();
            frame.render_widget(List::new(items), area);
        }
    }
}

// ─── Hint bar ────────────────────────────────────────────────

fn render_hint_bar(app: &App, frame: &mut Frame, area: Rect) {
    let hints = match app.focused_pane {
        Pane::Sidebar => " q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:navigate  d/u:detail-scroll  Enter:expand ",
        Pane::Detail => " q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:scroll ",
        Pane::Bottom => " q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:navigate  d/u:detail-scroll  Tab:next tab  Shift+Tab:prev tab  Enter:select ",
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hints, app.theme.text_dim)),
        area,
    );
}
