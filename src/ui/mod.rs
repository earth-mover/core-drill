mod help;
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
        (vertical[0], vertical[1], Some(vertical[2]), vertical[3])
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
            Constraint::Percentage(70), // detail
        ])
        .split(main_area);

    // Store layout areas on App for mouse hit-testing
    app.sidebar_area = horizontal[0];
    app.detail_area = horizontal[1];
    app.bottom_area = bottom_area;

    render_sidebar(app, frame, horizontal[0]);
    render_detail(app, frame, horizontal[1]);

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
                .highlight_style(app.theme.selected)
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

    // Check what's selected in the tree
    let selected = app.tree_state.selected();
    let selected_path = selected.last();

    let text = if let Some(path) = selected_path {
        if let Some(node) = find_node_by_path(&app.store, path) {
            match &node.node_type {
                TreeNodeType::Array(summary) => {
                    render_array_detail(app, node, summary)
                }
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

    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn render_array_detail<'a>(app: &'a App, node: &crate::store::TreeNode, summary: &crate::store::types::ArraySummary) -> Vec<Line<'a>> {
    let shape_str = summary
        .shape
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(" × ");

    let dim_names = summary
        .dimension_names
        .as_ref()
        .map(|dims| dims.join(", "))
        .unwrap_or_else(|| "—".to_string());

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Array", app.theme.text_bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Name:        ", app.theme.text_dim),
            Span::styled(node.name.clone(), app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Path:        ", app.theme.text_dim),
            Span::styled(node.path.clone(), app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Shape:       ", app.theme.text_dim),
            Span::styled(shape_str, app.theme.branch),
        ]),
        Line::from(vec![
            Span::styled("  Dimensions:  ", app.theme.text_dim),
            Span::styled(dim_names, app.theme.text),
        ]),
        Line::from(vec![
            Span::styled("  Manifests:   ", app.theme.text_dim),
            Span::styled(summary.manifest_count.to_string(), app.theme.text),
        ]),
    ];

    // Pretty-print zarr metadata
    if !summary.zarr_metadata.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Zarr Metadata:", app.theme.text_dim)));
        lines.push(Line::from(""));

        // Try to pretty-print JSON; fall back to raw string
        let formatted = serde_json::from_str::<serde_json::Value>(&summary.zarr_metadata)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| summary.zarr_metadata.clone());

        for json_line in formatted.lines() {
            lines.push(Line::from(Span::styled(
                format!    ("  {json_line}"),
                app.theme.text_dim,
            )));
        }
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

// ─── Bottom panel (snapshots / branches / tags) ──────────────

fn render_bottom(app: &App, frame: &mut Frame, area: Rect) {
    let focused = app.focused_pane == Pane::Bottom;

    // Tab bar + content
    let block_area = area;
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // tab bar
            Constraint::Min(1),   // content
        ])
        .split(block_area);

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
        .block(Block::default().borders(Borders::BOTTOM).border_style(app.theme.border))
        .select(match app.bottom_tab {
            BottomTab::Snapshots => 0,
            BottomTab::Branches => 1,
            BottomTab::Tags => 2,
        })
        .style(app.theme.text_dim)
        .highlight_style(if focused { app.theme.selected } else { app.theme.text });
    frame.render_widget(tabs, inner[0]);

    // Content based on active tab
    match app.bottom_tab {
        BottomTab::Snapshots => render_snapshot_list(app, frame, inner[1], focused),
        BottomTab::Branches => render_branch_list(app, frame, inner[1], focused),
        BottomTab::Tags => render_tag_list(app, frame, inner[1], focused),
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
                    let is_selected = focused && i == app.bottom_selected;
                    let style = if is_selected {
                        app.theme.selected
                    } else {
                        app.theme.text
                    };
                    let short_id = if entry.id.len() > 12 {
                        &entry.id[..12]
                    } else {
                        &entry.id
                    };
                    Row::new(vec![
                        Cell::from(Span::styled(short_id, app.theme.snapshot_id)),
                        Cell::from(Span::styled(
                            entry.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                            app.theme.timestamp,
                        )),
                        Cell::from(Span::styled(&entry.message, style)),
                    ])
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
                    let is_selected = focused && i == app.bottom_selected;
                    let style = if is_selected {
                        app.theme.selected
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
            let items: Vec<ListItem> = tags
                .iter()
                .enumerate()
                .map(|(i, tag)| {
                    let is_selected = focused && i == app.bottom_selected;
                    let style = if is_selected {
                        app.theme.selected
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
