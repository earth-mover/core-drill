use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;

pub fn render(_app: &App, frame: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled("  Navigation", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  1-6          Jump to view by number"),
        Line::from("  j / ↓        Move selection down"),
        Line::from("  k / ↑        Move selection up"),
        Line::from("  Enter        Drill into selected item"),
        Line::from("  Esc / Bksp   Go back"),
        Line::from(""),
        Line::from(Span::styled("  General", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  ?            Toggle this help"),
        Line::from("  q            Quit"),
        Line::from(""),
        Line::from(Span::styled("  Views", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  1  Overview      Repository summary"),
        Line::from("  2  Branches      List all branches"),
        Line::from("  3  Tags          List all tags"),
        Line::from("  4  Log           Snapshot history"),
        Line::from("  5  Tree          Node tree browser"),
        Line::from("  6  Ops Log       Mutation history"),
    ];

    let help = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).title(" Help "))
        .style(Style::default().fg(Color::White));
    frame.render_widget(help, area);
}
