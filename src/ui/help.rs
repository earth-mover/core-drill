use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::theme;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;

    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled("  Navigation", t.text_bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  1-6          ", t.branch),
            Span::styled("Jump to view by number", t.text),
        ]),
        Line::from(vec![
            Span::styled("  j / \u{2193}        ", t.branch),
            Span::styled("Move selection down", t.text),
        ]),
        Line::from(vec![
            Span::styled("  k / \u{2191}        ", t.branch),
            Span::styled("Move selection up", t.text),
        ]),
        Line::from(vec![
            Span::styled("  Enter        ", t.branch),
            Span::styled("Drill into selected item", t.text),
        ]),
        Line::from(vec![
            Span::styled("  Esc / Bksp   ", t.branch),
            Span::styled("Go back", t.text),
        ]),
        Line::from(""),
        Line::from(Span::styled("  General", t.text_bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ?            ", t.branch),
            Span::styled("Toggle this help", t.text),
        ]),
        Line::from(vec![
            Span::styled("  q            ", t.branch),
            Span::styled("Quit", t.text),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Views", t.text_bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  1  Overview      ", t.tag),
            Span::styled("Repository summary", t.text),
        ]),
        Line::from(vec![
            Span::styled("  2  Branches      ", t.tag),
            Span::styled("List all branches", t.text),
        ]),
        Line::from(vec![
            Span::styled("  3  Tags          ", t.tag),
            Span::styled("List all tags", t.text),
        ]),
        Line::from(vec![
            Span::styled("  4  Log           ", t.tag),
            Span::styled("Snapshot history", t.text),
        ]),
        Line::from(vec![
            Span::styled("  5  Tree          ", t.tag),
            Span::styled("Node tree browser", t.text),
        ]),
        Line::from(vec![
            Span::styled("  6  Ops Log       ", t.tag),
            Span::styled("Mutation history", t.text),
        ]),
    ];

    let block = theme::panel("Help", true, &app.theme);
    let help = Paragraph::new(help_text)
        .block(block)
        .style(t.text);
    frame.render_widget(help, area);
}
