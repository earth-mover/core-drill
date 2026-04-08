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

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    let focused = app.focused_pane == Pane::Detail;
    let block = theme::panel("[2] Detail", focused, &app.theme);

    // Show snapshot diff when bottom pane is focused (or detail pane with bottom context)
    // but NOT when sidebar is focused — sidebar navigation should show tree node details
    if app.focused_pane != Pane::Sidebar && app.bottom_tab == BottomTab::Snapshots {
        if let Some(sid) = app.selected_snapshot_id() {
            if app.store.diffs.contains_key(&sid) || app.last_diff_requested.as_deref() == Some(&sid) {
                let text = render_snapshot_diff_detail(app, &sid);
                frame.render_widget(Paragraph::new(text).block(block).wrap(Wrap { trim: false }), area);
                return;
            }
        }
    }

    // Check what's selected in the tree
    let selected = app.tree_state.selected();
    let selected_path = selected.last();

    // For array nodes: split into header + shape (top), canvas viz (middle), storage + rest (bottom)
    if let Some(path) = selected_path {
        if let Some(node) = find_node_by_path(&app.store, path) {
            if let TreeNodeType::Array(summary) = &node.node_type {
                // Check if we have a canvas visualization (ndim > 0)
                if summary.shape.is_empty() {
                    // Scalar — no canvas, just text (header + all sections together)
                    let pre_text = render_array_detail_header(app, node, summary);
                    let post_text = render_array_detail_storage(app, summary);
                    let mut text = pre_text;
                    text.extend(post_text);
                    frame.render_widget(
                        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                        area,
                    );
                } else {
                    // Three-section split: header+shape | canvas | storage+attrs
                    let pre_text = render_array_detail_header(app, node, summary);
                    let post_text = render_array_detail_storage(app, summary);

                    let inner = block.inner(area);
                    frame.render_widget(block, area);

                    let pre_len = pre_text.len() as u16;
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(pre_len + 1), // header + shape section
                            Constraint::Length(14),           // canvas viz
                            Constraint::Min(4),               // storage + attrs + rest
                        ])
                        .split(inner);

                    frame.render_widget(
                        Paragraph::new(pre_text).wrap(Wrap { trim: false }),
                        chunks[0],
                    );

                    if let Some(canvas) = shape_viz::chunk_grid_canvas(summary, &app.theme) {
                        frame.render_widget(canvas, chunks[1]);
                    }

                    frame.render_widget(
                        Paragraph::new(post_text).wrap(Wrap { trim: false }),
                        chunks[2],
                    );
                }
                return;
            }
        }
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

    frame.render_widget(Paragraph::new(text).block(block).wrap(Wrap { trim: false }), area);
}

/// Render the header + Shape & Layout section for an array node (shown above the canvas viz).
fn render_array_detail_header<'a>(app: &'a App, node: &crate::store::TreeNode, summary: &crate::store::types::ArraySummary) -> Vec<Line<'a>> {
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

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Array: ", app.theme.text_dim),
            Span::styled(node.name.clone(), app.theme.text_bold),
        ]),
        Line::from(vec![
            Span::styled("  Path:  ", app.theme.text_dim),
            Span::styled(node.path.clone(), app.theme.text),
        ]),
    ];

    // ─── Shape & Layout ──────────────────
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {0}{0}{0} Shape & Layout {0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", separator),
        app.theme.text_dim,
    )));

    lines.push(Line::from(vec![
        Span::styled("  Shape:         ", app.theme.text_dim),
        Span::styled(shape_str.clone(), app.theme.branch),
    ]));

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
        lines.push(Line::from(vec![
            Span::styled("  Chunk shape:   ", app.theme.text_dim),
            Span::styled(chunk_str, app.theme.text),
        ]));

        lines.push(Line::from(vec![
            Span::styled("  Data type:     ", app.theme.text_dim),
            Span::styled(meta.data_type.clone(), app.theme.text),
        ]));

        // Show v2 dtype if different from data_type
        if let Some(ref v2dt) = meta.v2_dtype {
            if v2dt != &meta.data_type {
                lines.push(Line::from(vec![
                    Span::styled("  Dtype (v2):    ", app.theme.text_dim),
                    Span::styled(v2dt.clone(), app.theme.text),
                ]));
            }
        }
    }

    lines.push(Line::from(vec![
        Span::styled("  Dimensions:    ", app.theme.text_dim),
        Span::styled(dim_names, app.theme.text),
    ]));

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
                        ((s + c - 1) / c).to_string()
                    } else {
                        "?".to_string()
                    }
                })
                .collect();
            lines.push(Line::from(vec![
                Span::styled("  Chunks/dim:    ", app.theme.text_dim),
                Span::styled(chunks_per_dim.join(" \u{00d7} "), app.theme.text),
            ]));
        }

        // Memory layout order (v2)
        if let Some(ref order) = meta.order {
            lines.push(Line::from(vec![
                Span::styled("  Order:         ", app.theme.text_dim),
                Span::styled(order.clone(), app.theme.text),
            ]));
        }
    }

    // Chunk grid summary line (textual) — the graphical canvas follows immediately below
    if let Some(summary_line) = crate::ui::shape_viz::chunk_summary_line(summary, &app.theme) {
        lines.push(summary_line);
    }

    lines
}

/// Render the Storage + Attributes + Raw Metadata sections for an array node (shown after the canvas viz).
fn render_array_detail_storage<'a>(app: &'a App, summary: &crate::store::types::ArraySummary) -> Vec<Line<'a>> {
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
            lines.push(Line::from(vec![
                Span::styled("  Codecs:        ", app.theme.text_dim),
                Span::styled(codec_display, app.theme.text),
            ]));
        }

        // v2 compressor (shown separately if codecs were also present)
        if let Some(ref comp) = meta.compressor {
            if !meta.codecs.is_empty() {
                // Already shown via codec_chain_display, but if both exist show compressor
                // separately for clarity
                lines.push(Line::from(vec![
                    Span::styled("  Compressor:    ", app.theme.text_dim),
                    Span::styled(comp.clone(), app.theme.text),
                ]));
            }
        }

        // v2 filters
        if !meta.filters.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  Filters:       ", app.theme.text_dim),
                Span::styled(meta.filters.join(", "), app.theme.text),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled("  Fill value:    ", app.theme.text_dim),
            Span::styled(meta.fill_value.clone(), app.theme.text),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Zarr format:   ", app.theme.text_dim),
            Span::styled(meta.zarr_format.to_string(), app.theme.text),
        ]));

        if meta.dimension_separator != "/" {
            lines.push(Line::from(vec![
                Span::styled("  Dim separator: ", app.theme.text_dim),
                Span::styled(meta.dimension_separator.clone(), app.theme.text),
            ]));
        }

        // Storage transformers
        if !meta.storage_transformers.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  Transformers:  ", app.theme.text_dim),
                Span::styled(meta.storage_transformers.join(", "), app.theme.text),
            ]));
        }
    }

    lines.push(Line::from(vec![
        Span::styled("  Manifests:     ", app.theme.text_dim),
        Span::styled(summary.manifest_count.to_string(), app.theme.text),
    ]));

    // ─── Attributes ──────────────────────
    if let Some(ref meta) = meta {
        if !meta.attributes.is_empty() {
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
    }

    // ─── Raw Metadata ────────────────────
    if let Some(ref meta) = meta {
        if !meta.extra_fields.is_empty() {
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
            let added_count = diff.added_arrays.len() + diff.added_groups.len();
            let deleted_count = diff.deleted_arrays.len() + diff.deleted_groups.len();
            let modified_count = diff.modified_arrays.len() + diff.modified_groups.len();

            lines.push(Line::from(vec![
                Span::styled("  ", app.theme.text_dim),
                Span::styled(format!("{added_count} added"), app.theme.added),
                Span::styled(", ", app.theme.text_dim),
                Span::styled(format!("{deleted_count} removed"), app.theme.removed),
                Span::styled(", ", app.theme.text_dim),
                Span::styled(format!("{modified_count} modified"), app.theme.modified),
            ]));

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
                    lines.push(Line::from(vec![
                        Span::styled(format!("    {path}  "), app.theme.text),
                        Span::styled(format!("{count} chunks"), app.theme.text_dim),
                    ]));
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
            let rows: Vec<Row> = entries
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    let is_selected = i == app.bottom_selected;
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
                        row.style(app.theme.text_dim)
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
                        app.theme.text_dim
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
                        app.theme.text_dim
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
        Pane::Sidebar => " q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:navigate  Enter:expand ",
        Pane::Detail => " q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:scroll ",
        Pane::Bottom => " q:quit  ?:help  t:toggle log  Ctrl+h/l:panes  j/k:navigate  Tab:next tab  Shift+Tab:prev tab  Enter:select ",
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hints, app.theme.text_dim)),
        area,
    );
}
