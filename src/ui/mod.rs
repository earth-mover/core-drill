mod help;
mod overview;

use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::{App, View};

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
        View::Help => help::render(app, frame, chunks[2]),
        _ => render_placeholder(app, frame, chunks[2]),
    }

    render_help_hint(frame, chunks[3]);
}

fn render_status_bar(app: &App, frame: &mut Frame, area: Rect) {
    let status = if app.loading { "Loading..." } else { &app.status_message };
    let block = Block::default()
        .title(" core-drill ")
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = format!("  Status: {}  |  Branches: {}  |  Tags: {}",
        status, app.branches.len(), app.tags.len());
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
        View::Help => 0, // overlay, keep previous highlight
    };
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(" Views "))
        .select(selected)
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    frame.render_widget(tabs, area);
}

fn render_branch_list(app: &App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app.branches.iter()
        .enumerate()
        .map(|(i, name)| {
            let style = if i == app.selected_index {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("  {}", name)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Branches "));
    frame.render_widget(list, area);
}

fn render_tag_list(app: &App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app.tags.iter()
        .enumerate()
        .map(|(i, name)| {
            let style = if i == app.selected_index {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("  {}", name)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Tags "));
    frame.render_widget(list, area);
}

fn render_placeholder(_app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Coming Soon ");
    let text = Paragraph::new("  This view is not yet implemented.")
        .block(block)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(text, area);
}

fn render_help_hint(frame: &mut Frame, area: Rect) {
    let text = Span::styled(
        " q:quit  ?:help  1-6:views  j/k:navigate  Enter:select  Esc:back ",
        Style::default().fg(Color::DarkGray),
    );
    frame.render_widget(Paragraph::new(text), area);
}
