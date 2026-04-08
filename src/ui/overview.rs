use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),  // summary
            Constraint::Min(5),    // branches preview
        ])
        .split(area);

    // Summary section
    let summary_text = vec![
        Line::from(vec![
            Span::styled("  Status:    ", Style::default().fg(Color::Cyan)),
            Span::raw(&app.repo_status),
        ]),
        Line::from(vec![
            Span::styled("  Branches:  ", Style::default().fg(Color::Cyan)),
            Span::raw(app.branches.len().to_string()),
        ]),
        Line::from(vec![
            Span::styled("  Tags:      ", Style::default().fg(Color::Cyan)),
            Span::raw(app.tags.len().to_string()),
        ]),
        Line::from(vec![
            Span::styled("  Snapshots: ", Style::default().fg(Color::Cyan)),
            Span::raw(app.snapshot_count.to_string()),
        ]),
    ];

    let summary = Paragraph::new(summary_text)
        .block(Block::default().borders(Borders::ALL).title(" Repository Overview "));
    frame.render_widget(summary, chunks[0]);

    // Branches preview
    let branch_items: Vec<ListItem> = app.branches.iter()
        .take(10)
        .map(|name| ListItem::new(format!("  {}", name)))
        .collect();

    let branch_list = List::new(branch_items)
        .block(Block::default().borders(Borders::ALL).title(" Branches (press 2 for full list) "));
    frame.render_widget(branch_list, chunks[1]);
}
