mod help;
mod overview;

use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::View;
use crate::store::LoadState;
use crate::theme;

/// Main render dispatch — called each frame
pub fn render(app: &App, frame: &mut Frame) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // status bar
            Constraint::Length(3),  // navigation tabs
            Constraint::Min(10),   // main content
            Constraint::Length(1), // help hint
        ])
        .split(frame.area());

    render_status_bar(app, frame, chunks[0]);
    render_nav_tabs(app, frame, chunks[1]);

    match app.current_view {
        View::Overview => overview::render(app, frame, chunks[2]),
        View::Branches => render_branch_list(app, frame, chunks[2]),
        View::Tags => render_tag_list(app, frame, chunks[2]),
        View::Log => render_snapshot_log(app, frame, chunks[2]),
        View::NodeTree => render_node_tree(app, frame, chunks[2]),
        View::Help => help::render(app, frame, chunks[2]),
        _ => render_placeholder(app, frame, chunks[2]),
    }

    render_help_hint(app, frame, chunks[3]);
}

fn render_status_bar(app: &App, frame: &mut Frame, area: Rect) {
    let branch_count = match &app.store.branches {
        LoadState::Loaded(b) => b.len().to_string(),
        LoadState::Loading => "...".to_string(),
        LoadState::Error(_) => "err".to_string(),
        LoadState::NotRequested => "-".to_string(),
    };

    let tag_count = match &app.store.tags {
        LoadState::Loaded(t) => t.len().to_string(),
        LoadState::Loading => "...".to_string(),
        LoadState::Error(_) => "err".to_string(),
        LoadState::NotRequested => "-".to_string(),
    };

    let status = match (&app.store.branches, &app.store.tags) {
        (LoadState::Loading, _) | (_, LoadState::Loading) => "Loading...",
        (LoadState::Error(_), _) | (_, LoadState::Error(_)) => "Error",
        (LoadState::Loaded(_), LoadState::Loaded(_)) => "Ready",
        _ => "Idle",
    };

    let block = theme::panel("core-drill", true, &app.theme);

    let text = Line::from(vec![
        Span::styled("  Status: ", app.theme.text_dim),
        Span::styled(status, if status == "Ready" { app.theme.status_ok } else { app.theme.loading }),
        Span::styled("  |  Branches: ", app.theme.text_dim),
        Span::styled(&branch_count, app.theme.branch),
        Span::styled("  |  Tags: ", app.theme.text_dim),
        Span::styled(&tag_count, app.theme.tag),
    ]);

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

fn render_nav_tabs(app: &App, frame: &mut Frame, area: Rect) {
    let titles = vec![
        "1:Overview", "2:Branches", "3:Tags", "4:Log", "5:Tree", "6:Ops",
    ];
    let selected = match app.current_view {
        View::Overview => 0,
        View::Branches => 1,
        View::Tags => 2,
        View::Log => 3,
        View::NodeTree => 4,
        View::OpsLog => 5,
        View::Help => 0,
    };
    let tabs = Tabs::new(titles)
        .block(theme::panel("Views", false, &app.theme))
        .select(selected)
        .style(app.theme.text)
        .highlight_style(app.theme.selected);
    frame.render_widget(tabs, area);
}

fn render_branch_list(app: &App, frame: &mut Frame, area: Rect) {
    let block = theme::panel("Branches", true, &app.theme);

    match &app.store.branches {
        LoadState::NotRequested | LoadState::Loading => {
            let widget = theme::loading_widget(&app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Error(msg) => {
            let widget = theme::error_widget(msg, &app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Loaded(branches) => {
            let items: Vec<ListItem> = branches
                .iter()
                .enumerate()
                .map(|(i, branch)| {
                    let style = if i == app.selected_index {
                        app.theme.selected
                    } else {
                        app.theme.text
                    };
                    let mut spans = vec![Span::styled(&branch.name, if i == app.selected_index { style } else { app.theme.branch })];
                    if let Some(msg) = &branch.tip_message {
                        spans.push(Span::styled("  ", app.theme.text_dim));
                        spans.push(Span::styled(msg, app.theme.text_dim));
                    }
                    ListItem::new(Line::from(spans)).style(style)
                })
                .collect();

            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
    }
}

fn render_tag_list(app: &App, frame: &mut Frame, area: Rect) {
    let block = theme::panel("Tags", true, &app.theme);

    match &app.store.tags {
        LoadState::NotRequested | LoadState::Loading => {
            let widget = theme::loading_widget(&app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Error(msg) => {
            let widget = theme::error_widget(msg, &app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Loaded(tags) => {
            let items: Vec<ListItem> = tags
                .iter()
                .enumerate()
                .map(|(i, tag)| {
                    let style = if i == app.selected_index {
                        app.theme.selected
                    } else {
                        app.theme.text
                    };
                    let mut spans = vec![Span::styled(&tag.name, if i == app.selected_index { style } else { app.theme.tag })];
                    if let Some(msg) = &tag.tip_message {
                        spans.push(Span::styled("  ", app.theme.text_dim));
                        spans.push(Span::styled(msg, app.theme.text_dim));
                    }
                    ListItem::new(Line::from(spans)).style(style)
                })
                .collect();

            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
    }
}

fn render_snapshot_log(app: &App, frame: &mut Frame, area: Rect) {
    let branch_name = app
        .nav_context
        .current_branch
        .as_deref()
        .unwrap_or("main");
    let title = format!("Log ({})", branch_name);
    let block = theme::panel(&title, true, &app.theme);

    let state = app
        .store
        .ancestry
        .get(branch_name)
        .unwrap_or(&LoadState::NotRequested);

    match state {
        LoadState::NotRequested | LoadState::Loading => {
            let widget = theme::loading_widget(&app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Error(msg) => {
            let widget = theme::error_widget(msg, &app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Loaded(entries) => {
            let items: Vec<ListItem> = entries
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    let style = if i == app.selected_index {
                        app.theme.selected
                    } else {
                        app.theme.text
                    };
                    let short_id = if entry.id.len() > 8 {
                        &entry.id[..8]
                    } else {
                        &entry.id
                    };
                    let line = Line::from(vec![
                        Span::styled(short_id, app.theme.snapshot_id),
                        Span::styled("  ", app.theme.text_dim),
                        Span::styled(
                            entry.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                            app.theme.timestamp,
                        ),
                        Span::styled("  ", app.theme.text_dim),
                        Span::styled(&entry.message, style),
                    ]);
                    ListItem::new(line).style(style)
                })
                .collect();

            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
    }
}

fn render_node_tree(app: &App, frame: &mut Frame, area: Rect) {
    use crate::store::TreeNodeType;

    let current_path = app
        .nav_context
        .current_path
        .as_deref()
        .unwrap_or("/");
    let title = format!("Tree ({})", current_path);
    let block = theme::panel(&title, true, &app.theme);

    let state = app
        .store
        .node_children
        .get(current_path)
        .unwrap_or(&LoadState::NotRequested);

    match state {
        LoadState::NotRequested | LoadState::Loading => {
            let widget = theme::loading_widget(&app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Error(msg) => {
            let widget = theme::error_widget(msg, &app.theme).block(block);
            frame.render_widget(widget, area);
        }
        LoadState::Loaded(nodes) => {
            let items: Vec<ListItem> = nodes
                .iter()
                .enumerate()
                .map(|(i, node)| {
                    let style = if i == app.selected_index {
                        app.theme.selected
                    } else {
                        app.theme.text
                    };
                    let (icon, icon_style, detail) = match &node.node_type {
                        TreeNodeType::Group => (
                            "\u{1F4C1} ",
                            app.theme.group_icon,
                            String::new(),
                        ),
                        TreeNodeType::Array(summary) => {
                            let shape_str = format!(
                                "[{}]",
                                summary
                                    .shape
                                    .iter()
                                    .map(|d| d.to_string())
                                    .collect::<Vec<_>>()
                                    .join(" x ")
                            );
                            (
                                "\u{1F4CA} ",
                                app.theme.array_icon,
                                format!("  {}", shape_str),
                            )
                        }
                    };
                    let line = Line::from(vec![
                        Span::styled(icon, icon_style),
                        Span::styled(&node.name, style),
                        Span::styled(detail, app.theme.text_dim),
                    ]);
                    ListItem::new(line).style(style)
                })
                .collect();

            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
    }
}

fn render_placeholder(_app: &App, frame: &mut Frame, area: Rect) {
    let block = theme::panel("Coming Soon", false, &_app.theme);
    let text = Paragraph::new("  This view is not yet implemented.")
        .block(block)
        .style(_app.theme.text_dim);
    frame.render_widget(text, area);
}

fn render_help_hint(app: &App, frame: &mut Frame, area: Rect) {
    let text = Span::styled(
        " q:quit  ?:help  1-6:views  j/k:navigate  Enter:select  Esc:back ",
        app.theme.text_dim,
    );
    frame.render_widget(Paragraph::new(text), area);
}
