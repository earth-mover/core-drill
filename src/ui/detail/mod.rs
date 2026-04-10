mod array;
mod branch;
mod group;
mod ops_log;
mod repo;

use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::Pane;
use crate::store::types::TreeNodeType;
use crate::ui::widgets::{clamped_scroll, render_tabbed_panel};
use crate::ui::diff::render_snapshot_diff_detail;

pub(super) fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    use crate::component::DetailMode;

    let focused = app.focused_pane == Pane::Detail;
    let active_tab = match app.detail_mode {
        DetailMode::Node => 0,
        DetailMode::Repo => 1,
        DetailMode::Branch => 2,
        DetailMode::Snapshot => 3,
        DetailMode::OpsLog => 4,
    };
    let content_area = match render_tabbed_panel(
        "[2] Detail",
        &["Node", "Repo", "Branch", "Snap", "Ops Log"],
        active_tab,
        focused,
        &app.theme,
        frame,
        area,
    ) {
        Some(area) => area,
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
        render_text(repo::render_repo_overview(app), frame);
        return;
    }

    // Ops Log mode
    if app.detail_mode == DetailMode::OpsLog {
        render_text(ops_log::render_ops_log(app), frame);
        return;
    }

    // Branch mode
    if app.detail_mode == DetailMode::Branch {
        if let Some(branches) = app.store.branches.as_loaded()
            && let Some(branch) = branches.get(app.bottom_selected())
        {
            let branch_name = branch.name.clone();
            let is_current = branch_name == app.current_branch;
            render_text(branch::render_branch_detail(app, &branch_name, is_current), frame);
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
        && let Some(node) = app.store.find_node(path)
        && let TreeNodeType::Array(summary) = &node.node_type
    {
        let inner_width = content_area.width;
        let snapshot_id = app
            .selected_snapshot_id()
            .or_else(|| app.get_branch_tip_snapshot_id());
        let (mut text, zarr_meta) = array::render_array_detail_header(app, node, summary, inner_width);
        text.extend(array::render_array_detail_storage(
            app,
            node.path.as_str(),
            snapshot_id.as_deref(),
            summary,
            zarr_meta,
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
        if let Some(node) = app.store.find_node(path) {
            match &node.node_type {
                TreeNodeType::Array(_) => unreachable!(),
                TreeNodeType::Group => group::render_group_detail(app, node),
            }
        } else {
            repo::render_repo_overview(app)
        }
    } else {
        repo::render_repo_overview(app)
    };

    let scroll = clamped_scroll(app.detail_scroll, text.len(), content_area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content_area,
    );
}
