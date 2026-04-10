mod bottom;
mod detail;
mod diff;
pub mod format;
mod help;
pub mod json_view;
pub mod shape_viz;
mod widgets;

use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::Pane;
use crate::store::LoadState;
use crate::store::types::TreeNodeType;
use crate::theme;

use bottom::render_bottom;
use detail::render_detail;

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
        constraints.push(Constraint::Length(1)); // spacer before bottom panel
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
    use crate::store::{ErrorKind, classify_error};

    // Determine overall status and optional detail message
    let (status, detail, style) = match (&app.store.branches, &app.store.tags) {
        (LoadState::Error(e), _) | (_, LoadState::Error(e)) => {
            let kind = classify_error(e);
            let (label, hint) = match kind {
                ErrorKind::Auth => ("auth error", "credentials may be expired — R to retry"),
                ErrorKind::Network => ("network error", "connection failed — R to retry"),
                ErrorKind::NotFound => ("not found", "repo or branch missing — R to retry"),
                ErrorKind::Other => ("error", "R to retry"),
            };
            (label, Some(hint), app.theme.error)
        }
        (LoadState::Loading, _) | (_, LoadState::Loading) => {
            ("drilling...", None, app.theme.loading)
        }
        (LoadState::Loaded(_), LoadState::Loaded(_)) => ("ready", None, app.theme.status_ok),
        _ => ("", None, app.theme.text_dim),
    };

    let display_url = app.repo_info.display_short();

    let mut spans = vec![
        Span::styled(" ", app.theme.text),
        Span::styled(display_url, app.theme.branch),
        Span::styled("  ", app.theme.text_dim),
        Span::styled(status, style),
    ];
    if let Some(hint) = detail {
        spans.push(Span::styled("  ", app.theme.text_dim));
        spans.push(Span::styled(hint, app.theme.text_dim));
    }

    let line = Line::from(spans);
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
            Constraint::Min(1),    // tree
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
            // When searching the tree, compute visible paths (matches + ancestors)
            // Use the cached tree candidates if available, otherwise build on demand.
            // This avoids rebuilding the full candidate list every render frame.
            let owned_candidates;
            let candidates: &[String] = if let Some(ref cached) = app.tree_candidate_cache {
                cached
            } else {
                owned_candidates = crate::search::tree_candidates(&app.store);
                &owned_candidates
            };

            let visible_paths: Option<std::collections::HashSet<String>> = app
                .search
                .as_ref()
                .filter(|s| s.target == crate::search::SearchTarget::Tree && !s.query.is_empty())
                .map(|search| {
                    let mut visible = std::collections::HashSet::new();
                    for &idx in &search.matches {
                        if let Some(path) = candidates.get(idx) {
                            // Add this path and all ancestor paths
                            let mut p = path.as_str();
                            visible.insert(p.to_string());
                            while let Some(slash) = p.rfind('/') {
                                if slash == 0 {
                                    break;
                                }
                                p = &p[..slash];
                                visible.insert(p.to_string());
                            }
                        }
                    }
                    visible
                });

            let tree_items: Vec<tui_tree_widget::TreeItem<String>> = nodes
                .iter()
                .filter(|node| {
                    visible_paths
                        .as_ref()
                        .is_none_or(|vp| vp.contains(&node.path))
                })
                .map(|node| build_tree_item(node, &app.store, 0, visible_paths.as_ref()))
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
/// `visible` optionally filters which nodes to include (for search).
fn build_tree_item<'a>(
    node: &crate::store::TreeNode,
    store: &crate::store::DataStore,
    depth: usize,
    visible: Option<&std::collections::HashSet<String>>,
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
            let children: Vec<tui_tree_widget::TreeItem<String>> = if depth >= MAX_TREE_DEPTH {
                vec![]
            } else if let Some(LoadState::Loaded(child_nodes)) = store.node_children.get(&node.path)
            {
                child_nodes
                    .iter()
                    .filter(|child| child.path != node.path)
                    .filter(|child| visible.is_none_or(|vp| vp.contains(&child.path)))
                    .map(|child| build_tree_item(child, store, depth + 1, visible))
                    .collect()
            } else {
                // No children loaded yet — show as expandable but empty
                vec![]
            };
            tui_tree_widget::TreeItem::new(node.path.clone(), label, children)
                .expect("unique child identifiers")
        }
        TreeNodeType::Array(_) => tui_tree_widget::TreeItem::new_leaf(node.path.clone(), label),
    }
}

// ─── Hint bar ────────────────────────────────────────────────

fn render_hint_bar(app: &App, frame: &mut Frame, area: Rect) {
    // Search mode: show search bar instead of hints
    if let Some(ref search) = app.search {
        let match_info = if search.query.is_empty() {
            String::new()
        } else {
            format!("  ({}/{})", search.cursor + 1, search.matches.len())
        };
        let line = Line::from(vec![
            Span::styled(" /", app.theme.text_bold),
            Span::styled(&search.query, app.theme.text),
            Span::styled("▏", app.theme.text), // cursor
            Span::styled(match_info, app.theme.text_dim),
            Span::styled(
                "  Enter:select  Esc:cancel  ↑↓:navigate",
                app.theme.text_dim,
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // Show pending z-command indicator
    if app.pending_z {
        let line = Line::from(vec![
            Span::styled(" z", app.theme.text_bold),
            Span::styled("▏", app.theme.text),
            Span::styled(
                "  o:open  c:close  O:open recursive  C:close recursive  R:open all  M:close all",
                app.theme.text_dim,
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let hints = match app.focused_pane {
        Pane::Sidebar => {
            " q:quit  ?:help  /:search  j/k:navigate  Enter:expand  zo/zc:open/close  zR/zM:all "
        }
        Pane::Detail => " q:quit  ?:help  R:retry  t:toggle log  j/k:scroll  h/l:Node/Repo ",
        Pane::Bottom => {
            " q:quit  ?:help  R:retry  t:toggle log  /:search  j/k:navigate  Tab:next tab  Enter:select "
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hints, app.theme.text_dim)),
        area,
    );
}
