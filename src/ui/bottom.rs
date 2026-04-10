use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::{BottomTab, Pane};
use crate::store::LoadState;
use crate::theme;
use super::widgets::{render_tabbed_panel, render_scrollable_list, resolve_search_indices};


pub(super) fn render_bottom(app: &App, frame: &mut Frame, area: Rect) {
    let focused = app.focused_pane == Pane::Bottom;
    let active_tab = match app.bottom_tab {
        BottomTab::Snapshots => 0,
        BottomTab::Branches => 1,
        BottomTab::Tags => 2,
    };
    let content_area = match render_tabbed_panel(
        "[3] Version Control",
        &["Snapshots", "Branches", "Tags"],
        active_tab,
        focused,
        &app.theme,
        frame,
        area,
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

fn render_snapshot_list(app: &App, frame: &mut Frame, area: Rect, _focused: bool) {
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
            let (indices, search_cursor_idx) =
                resolve_search_indices(app, entries.len(), crate::search::SearchTarget::Snapshots);

            let rows: Vec<Row> = indices
                .iter()
                .filter_map(|&i| entries.get(i).map(|e| (i, e)))
                .map(|(i, entry)| {
                    let is_selected = if app.search.is_some() {
                        search_cursor_idx == Some(i)
                    } else {
                        i == app.bottom_selected()
                    };
                    let short_id = crate::output::truncate(&entry.id, 12);
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
            let table = Table::new(rows, widths).header(
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
            render_scrollable_list(
                branches,
                |b| &b.name,
                app.theme.text,
                app,
                focused,
                frame,
                area,
            );
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
            render_scrollable_list(tags, |t| &t.name, app.theme.text, app, focused, frame, area);
        }
    }
}
