pub mod format;
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

    // Format a clean display name for the status bar
    let display_url = if let Some(al_parts) = app.repo_url.strip_prefix("al:") {
        let parts: Vec<&str> = al_parts.splitn(4, '|').collect();
        let org_repo = parts.first().unwrap_or(&"?");
        if let Some(bucket) = parts.get(1) {
            format!("{org_repo}  ({bucket})")
        } else {
            org_repo.to_string()
        }
    } else {
        app.repo_url.clone()
    };

    let line = Line::from(vec![
        Span::styled(" ", app.theme.text),
        Span::styled(display_url, app.theme.branch),
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
                .highlight_style(if focused {
                    app.theme.selected
                } else if app.focused_pane == Pane::Bottom {
                    // VC focused: no tree highlight, detail shows snapshot info
                    app.theme.text_dim
                } else {
                    // Detail or other: keep selection visible but dimmed
                    app.theme.selected_inactive
                })
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
    use crate::component::DetailMode;

    let focused = app.focused_pane == Pane::Detail;
    let active_tab = match app.detail_mode {
        DetailMode::Node => 0,
        DetailMode::Repo => 1,
    };
    let content_area = match render_tabbed_panel(
        "[2] Detail", &["Node", "Repo"], active_tab, focused, &app.theme, frame, area,
    ) {
        Some(area) => area,
        None => return,
    };

    // Repo mode: always show repo overview
    if app.detail_mode == DetailMode::Repo {
        let text = render_repo_overview(app);
        let scroll = clamped_scroll(app.detail_scroll, text.len(), area);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            content_area,
        );
        return;
    }

    // Node mode below — show snapshot detail when VC panel is on Snapshots tab,
    // UNLESS a tree node is actively being inspected (sidebar focused AND something selected)
    let tree_node_selected = app.focused_pane == Pane::Sidebar
        && !app.tree_state.selected().is_empty();

    if !tree_node_selected && app.bottom_tab == BottomTab::Snapshots
        && let Some(sid) = app.selected_snapshot_id()
            && (app.store.diffs.contains_key(&sid) || app.last_diff_requested.as_deref() == Some(&sid)) {
                let inner_width = content_area.width;
                let text = render_snapshot_diff_detail(app, &sid, inner_width);
                let scroll = clamped_scroll(app.detail_scroll, text.len(), content_area);
                frame.render_widget(
                    Paragraph::new(text)
                        .wrap(Wrap { trim: false })
                        .scroll((scroll, 0)),
                    content_area,
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
                let inner_width = content_area.width;
                let snapshot_id = app.selected_snapshot_id()
                    .or_else(|| app.get_branch_tip_snapshot_id());
                let mut text = render_array_detail_header(app, node, summary, inner_width);
                text.extend(render_array_detail_storage(app, node.path.as_str(), snapshot_id.as_deref(), summary, inner_width));
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

    let scroll = clamped_scroll(app.detail_scroll, text.len(), content_area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content_area,
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

/// Render a tabbed panel: outer border with title, a tab bar, and return the content area.
/// Used by both the Detail pane and the Bottom (Version Control) pane.
fn render_tabbed_panel(
    title: &str,
    tab_names: &[&str],
    active_index: usize,
    focused: bool,
    theme: &crate::theme::Theme,
    frame: &mut Frame,
    area: Rect,
) -> Option<Rect> {
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(if focused { theme.border_focused } else { theme.border });

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 {
        return None;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),   // content
        ])
        .split(inner);

    let tab_labels: Vec<Line> = tab_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == active_index { "●" } else { "○" };
            Line::from(format!("{marker} {name}"))
        })
        .collect();

    let tabs = Tabs::new(tab_labels)
        .select(active_index)
        .style(theme.text_dim)
        .highlight_style(if focused { theme.selected } else { theme.text });
    frame.render_widget(tabs, chunks[0]);

    Some(chunks[1])
}

/// Build a section-header `Line` with consistent width and dark-gray styling.
/// Total visual width is kept near 40 characters by padding with `─` on the right.
fn section_header(label: &str) -> Line<'static> {
    let prefix = format!("  ─── {label} ");
    let remaining = 36usize.saturating_sub(prefix.chars().count());
    let line = format!("{prefix}{}", "─".repeat(remaining));
    Line::from(Span::styled(line, Style::default().fg(Color::Rgb(120, 120, 120))))
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

    let mut lines = vec![Line::from("")];
    lines.extend(labeled_lines("  Array: ", node.name.clone(), app.theme.text_dim, app.theme.text_bold, max_width));
    lines.extend(labeled_lines("  Path:  ", node.path.clone(), app.theme.text_dim, app.theme.text, max_width));

    // ─── Shape & Layout ──────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Shape & Layout"));

    lines.extend(labeled_lines("  Shape:         ", shape_str.clone(), app.theme.text_dim, app.theme.text, max_width));

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

/// Compute total grid positions = ∏ ceil(shape[i] / chunk_shape[i]).
/// Returns None if shapes are mismatched, empty, or any chunk dimension is zero.
fn compute_grid_chunks(summary: &crate::store::types::ArraySummary, meta: &format::ZarrMetadata) -> Option<u64> {
    if summary.shape.is_empty() || meta.chunk_shape.is_empty() {
        return None;
    }
    if summary.shape.len() != meta.chunk_shape.len() {
        return None;
    }
    summary
        .shape
        .iter()
        .zip(meta.chunk_shape.iter())
        .try_fold(1u64, |acc, (&s, &c)| {
            if c == 0 { return None; }
            acc.checked_mul(s.div_ceil(c))
        })
}

/// Format an initialized-fraction line: "X of Y (Z%)" or "X of Y (100%)" etc.
fn fmt_initialized(written: u64, grid: u64) -> String {
    let pct = if grid > 0 { written * 100 / grid } else { 0 };
    format!("{written} of {grid}  ({pct}%)")
}

/// Render the Storage + Attributes + Raw Metadata sections for an array node (shown after the canvas viz).
fn render_array_detail_storage<'a>(app: &'a App, path: &str, snapshot_id: Option<&str>, summary: &crate::store::types::ArraySummary, max_width: u16) -> Vec<Line<'a>> {
    let meta = if !summary.zarr_metadata.is_empty() {
        format::ZarrMetadata::parse(&summary.zarr_metadata)
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
    let chunk_stats_key = snapshot_id.map(|sid| (sid.to_string(), path.to_string()));
    match chunk_stats_key.as_ref().and_then(|k| app.store.chunk_stats.get(k)) {
        None | Some(LoadState::NotRequested) => {
            // No full stats yet — show snapshot-derived total if available
            if let Some(total) = summary.total_chunks {
                lines.push(Line::from(""));
                lines.push(section_header("Chunk Types"));
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
            lines.push(Line::from(Span::styled(format!("  Error: {e}"), app.theme.error)));
        }
        Some(LoadState::Loaded(stats)) => {
            lines.push(Line::from(""));
            lines.push(section_header("Chunk Types"));

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
                    lines.push(Line::from(Span::styled("    Sources:", app.theme.text_dim)));
                    for (prefix, count) in &stats.virtual_prefixes {
                        lines.push(Line::from(vec![
                            Span::styled(format!("      {prefix}/"), app.theme.text),
                            Span::styled(format!("  ({count} chunks)"), app.theme.text_dim),
                        ]));
                    }
                }
            }
            // Initialized fraction (written / total grid positions)
            if stats.total_chunks > 0 {
                if let Some(grid) = grid_chunks {
                    lines.extend(labeled_lines(
                        "  Initialized:   ",
                        fmt_initialized(stats.total_chunks as u64, grid),
                        app.theme.text_dim,
                        app.theme.text,
                        max_width,
                    ));
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
                let json_lines = json_view::render_json(&attr_json, &app.theme, 10, 50);
                lines.extend(json_lines);
            }
        }

    // ─── Raw Metadata ────────────────────
    if let Some(ref meta) = meta
        && !meta.extra_fields.is_empty() {
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
                let json_lines = json_view::render_json(&extra_json, &app.theme, 10, 50);
                lines.extend(json_lines);
            }
        }

    // Fallback: if metadata was present but couldn't be parsed, show with json_view
    if !summary.zarr_metadata.is_empty() && meta.is_none() {
        lines.push(Line::from(""));
        lines.push(section_header("Raw Metadata"));
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

    let mut lines = Vec::new();

    // ─── Repository ─────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Repository"));

    // Parse Arraylake metadata from label: "al:org/repo|bucket|platform|region"
    if let Some(al_parts) = app.repo_url.strip_prefix("al:") {
        let parts: Vec<&str> = al_parts.splitn(4, '|').collect();
        if let Some(org_repo) = parts.first() {
            if let Some((org, repo_name)) = org_repo.split_once('/') {
                lines.push(Line::from(vec![
                    Span::styled("  Organization:  ", app.theme.text_dim),
                    Span::styled(org.to_string(), app.theme.branch),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Repo name:     ", app.theme.text_dim),
                    Span::styled(repo_name.to_string(), app.theme.branch),
                ]));
            }
        }
        if let Some(bucket) = parts.get(1) {
            lines.push(Line::from(vec![
                Span::styled("  Bucket:        ", app.theme.text_dim),
                Span::styled(bucket.to_string(), app.theme.text),
            ]));
        }
        if let Some(platform) = parts.get(2) {
            lines.push(Line::from(vec![
                Span::styled("  Platform:      ", app.theme.text_dim),
                Span::styled(platform.to_string(), app.theme.text),
            ]));
        }
        if let Some(region) = parts.get(3).filter(|r| *r != &"?") {
            lines.push(Line::from(vec![
                Span::styled("  Region:        ", app.theme.text_dim),
                Span::styled(region.to_string(), app.theme.text),
            ]));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Location:      ", app.theme.text_dim),
            Span::styled(app.repo_url.clone(), app.theme.branch),
        ]));
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
    // Aggregate chunk stats from cached tree nodes + any loaded ChunkStats
    {
        let mut total_arrays: usize = 0;
        let mut total_groups: usize = 0;
        let mut total_written: u64 = 0;
        let _total_grid: u64 = 0;
        // Detailed breakdown from loaded ChunkStats
        let mut known_native: usize = 0;
        let mut known_inline: usize = 0;
        let mut known_virtual: usize = 0;
        let mut native_bytes: u64 = 0;
        let mut inline_bytes: u64 = 0;
        let mut virtual_bytes: u64 = 0;
        let mut stats_loaded: usize = 0;

        for state in app.store.node_children.values() {
            if let crate::store::LoadState::Loaded(nodes) = state {
                for node in nodes {
                    match &node.node_type {
                        TreeNodeType::Group => total_groups += 1,
                        TreeNodeType::Array(summary) => {
                            total_arrays += 1;
                            if let Some(tc) = summary.total_chunks {
                                total_written += tc;
                            }
                        }
                    }
                }
            }
        }

        // Compute grid size from metadata (requires parsing, but it's cached in the tree label)
        // We can approximate from the tree: scan all arrays for grid_chunks
        // Actually, we need to parse ZarrMetadata for each — too expensive per frame.
        // Instead, just use total_chunks from all arrays. Grid requires chunk_shape.
        // Let's sum from ChunkStats which have the detailed breakdown:
        for ((_, _), state) in &app.store.chunk_stats {
            if let crate::store::LoadState::Loaded(stats) = state {
                stats_loaded += 1;
                known_native += stats.native_count;
                known_inline += stats.inline_count;
                known_virtual += stats.virtual_count;
                native_bytes += stats.native_total_bytes;
                inline_bytes += stats.inline_total_bytes;
                virtual_bytes += stats.virtual_total_bytes;
            }
        }

        if total_arrays > 0 || total_groups > 0 {
            lines.push(Line::from(""));
            lines.push(section_header("Storage Summary"));
            lines.push(Line::from(vec![
                Span::styled("  Arrays:      ", app.theme.text_dim),
                Span::styled(total_arrays.to_string(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Groups:      ", app.theme.text_dim),
                Span::styled(total_groups.to_string(), app.theme.text),
            ]));
            if total_written > 0 {
                lines.push(Line::from(vec![
                    Span::styled("  Chunks:      ", app.theme.text_dim),
                    Span::styled(total_written.to_string(), app.theme.text),
                ]));
            }

            if stats_loaded > 0 {
                let total_known = known_native + known_inline + known_virtual;
                let total_bytes = native_bytes + inline_bytes + virtual_bytes;
                let size_str = humansize::format_size(total_bytes, humansize::BINARY);

                let mut parts = Vec::new();
                if known_native > 0 { parts.push(format!("{known_native} native")); }
                if known_inline > 0 { parts.push(format!("{known_inline} inline")); }
                if known_virtual > 0 { parts.push(format!("{known_virtual} virtual")); }

                let suffix = if stats_loaded < total_arrays {
                    format!("  ({stats_loaded}/{total_arrays} arrays scanned)")
                } else {
                    String::new()
                };

                lines.push(Line::from(vec![
                    Span::styled("  Breakdown:   ", app.theme.text_dim),
                    Span::styled(format!("{}{}", parts.join(", "), suffix), app.theme.text),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Total size:  ", app.theme.text_dim),
                    Span::styled(size_str, app.theme.text),
                ]));
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
                let style = if flag.enabled { app.theme.status_ok } else { app.theme.text_dim };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}: ", flag.name), app.theme.text_dim),
                    Span::styled(format!("{status}{explicit}"), style),
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

fn render_snapshot_diff_detail<'a>(app: &'a App, snapshot_id: &str, max_width: u16) -> Vec<Line<'a>> {
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

        // Compute position counter: "N of M"
        let ancestry_len = app
            .store
            .ancestry
            .get(&app.current_branch)
            .and_then(|s| s.as_loaded())
            .map(|entries| entries.len())
            .unwrap_or(0);
        let position_n = app.store.ancestry
            .get(&app.current_branch)
            .and_then(|s| s.as_loaded())
            .and_then(|entries| entries.iter().position(|e| Some(&e.id) == app.current_snapshot.as_ref()))
            .map(|i| i + 1)
            .unwrap_or(1);
        let position_str = format!("    ({position_n} of {ancestry_len})");

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Snapshot:  ", app.theme.text_dim),
            Span::styled(short_id.to_string(), app.theme.snapshot_id),
            Span::styled(position_str, app.theme.text_dim),
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
        lines.extend(labeled_lines(
            "  Message:   ",
            entry.message.clone(),
            app.theme.text_dim,
            app.theme.text,
            max_width,
        ));
    }

    // --- Separator ---
    lines.push(Line::from(""));
    lines.push(section_header("Changes"));

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
                // Initial commit: no parent to diff against — show a simple message.
                lines.push(Line::from(Span::styled(
                    "  Repository initialized",
                    app.theme.text_bold,
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  This is the first snapshot. No diff available \u{2014}",
                    app.theme.text_dim,
                )));
                lines.push(Line::from(Span::styled(
                    "  select a later snapshot to see what changed.",
                    app.theme.text_dim,
                )));
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
                        let chunk_key = (snapshot_id.to_string(), path.clone());
                        let (annotation, extra_source_lines) =
                            match app.store.chunk_stats.get(&chunk_key) {
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
    let active_tab = match app.bottom_tab {
        BottomTab::Snapshots => 0,
        BottomTab::Branches => 1,
        BottomTab::Tags => 2,
    };
    let content_area = match render_tabbed_panel(
        "[3] Version Control", &["Snapshots", "Branches", "Tags"], active_tab, focused, &app.theme, frame, area,
    ) {
        Some(area) => area,
        None => return,
    };

    match app.bottom_tab {
        BottomTab::Snapshots => render_snapshot_list(app, frame, content_area, focused),
        BottomTab::Branches => render_branch_list(app, frame, content_area, focused),
        BottomTab::Tags => render_tag_list(app, frame, content_area, focused),
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
            let visible_start = app.bottom_offset();
            let rows: Vec<Row> = entries
                .iter()
                .enumerate()
                .skip(visible_start)
                .map(|(i, entry)| {
                    let is_selected = i == app.bottom_selected();
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
                    if is_selected {
                        // Cursor row stays highlighted regardless of focus
                        row.style(app.theme.selected)
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

/// Render a scrollable list with selection highlight. Shared by Branches and Tags tabs.
fn render_scrollable_list<T, F>(
    items: &[T],
    label_fn: F,
    default_style: Style,
    app: &App,
    focused: bool,
    frame: &mut Frame,
    area: Rect,
) where F: Fn(&T) -> &str {
    let visible_start = app.bottom_offset();
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .skip(visible_start)
        .map(|(i, item)| {
            let is_selected = i == app.bottom_selected();
            let style = if is_selected && focused {
                app.theme.selected
            } else if is_selected {
                app.theme.selected_inactive
            } else {
                default_style
            };
            ListItem::new(Span::styled(label_fn(item), style))
        })
        .collect();
    frame.render_widget(List::new(list_items), area);
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
            render_scrollable_list(branches, |b| &b.name, app.theme.branch, app, focused, frame, area);
        }
    }
}

fn render_tag_list(app: &App, frame: &mut Frame, area: Rect, _focused: bool) {
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
            render_scrollable_list(tags, |t| &t.name, app.theme.tag, app, _focused, frame, area);
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
