use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::store::LoadState;
use crate::theme;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),  // summary
            Constraint::Min(5),    // branches preview
        ])
        .split(area);

    render_summary(app, frame, chunks[0]);
    render_branch_preview(app, frame, chunks[1]);
}

fn render_summary(app: &App, frame: &mut Frame, area: Rect) {
    let branch_count = match &app.store.branches {
        LoadState::Loaded(b) => b.len().to_string(),
        LoadState::Loading => "loading...".to_string(),
        LoadState::Error(e) => format!("error: {}", e),
        LoadState::NotRequested => "-".to_string(),
    };

    let tag_count = match &app.store.tags {
        LoadState::Loaded(t) => t.len().to_string(),
        LoadState::Loading => "loading...".to_string(),
        LoadState::Error(e) => format!("error: {}", e),
        LoadState::NotRequested => "-".to_string(),
    };

    let snapshot_count = app
        .nav_context
        .current_branch
        .as_deref()
        .and_then(|b| app.store.ancestry.get(b))
        .and_then(|s| s.as_loaded())
        .map(|entries| entries.len().to_string())
        .unwrap_or_else(|| "-".to_string());

    let status = match (&app.store.branches, &app.store.tags) {
        (LoadState::Loaded(_), LoadState::Loaded(_)) => ("Ready", app.theme.status_ok),
        (LoadState::Loading, _) | (_, LoadState::Loading) => ("Loading...", app.theme.loading),
        (LoadState::Error(_), _) | (_, LoadState::Error(_)) => ("Error", app.theme.error),
        _ => ("Idle", app.theme.text_dim),
    };

    let summary_text = vec![
        Line::from(vec![
            Span::styled("  Status:    ", app.theme.text_dim),
            Span::styled(status.0, status.1),
        ]),
        Line::from(vec![
            Span::styled("  Branches:  ", app.theme.text_dim),
            Span::styled(&branch_count, app.theme.branch),
        ]),
        Line::from(vec![
            Span::styled("  Tags:      ", app.theme.text_dim),
            Span::styled(&tag_count, app.theme.tag),
        ]),
        Line::from(vec![
            Span::styled("  Snapshots: ", app.theme.text_dim),
            Span::styled(&snapshot_count, app.theme.text),
        ]),
    ];

    let block = theme::panel("Repository Overview", true, &app.theme);
    let summary = Paragraph::new(summary_text).block(block).style(app.theme.text);
    frame.render_widget(summary, area);
}

fn render_branch_preview(app: &App, frame: &mut Frame, area: Rect) {
    let block = theme::panel("Branches (press 2 for full list)", false, &app.theme);

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
                .take(10)
                .map(|branch| {
                    let line = Line::from(vec![
                        Span::styled("  ", app.theme.text),
                        Span::styled(&branch.name, app.theme.branch),
                    ]);
                    ListItem::new(line)
                })
                .collect();

            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
    }
}
