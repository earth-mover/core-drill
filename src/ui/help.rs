use ratatui::Frame;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::theme;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;

    // Replicate the main TUI layout: title | sidebar+detail | search+global | bottom | hint
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // title bar
            Constraint::Min(6),     // main area (sidebar + detail)
            Constraint::Length(17), // search + global row
            Constraint::Length(8),  // bottom panel
            Constraint::Length(1),  // hint bar
        ])
        .split(area);

    let title_area = vertical[0];
    let main_area = vertical[1];
    let middle_area = vertical[2];
    let bottom_area = vertical[3];
    let hint_area = vertical[4];

    // Title bar
    let title = Line::from(vec![
        Span::styled(" core-drill", t.text_bold),
        Span::styled(" — Icechunk Repository Inspector", t.text),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    // Main area: sidebar | detail (same 30/70 split)
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Length(1),
            Constraint::Percentage(70),
        ])
        .split(main_area);

    let sidebar_area = horizontal[0];
    let detail_area = horizontal[2];

    // Middle row: search | global (same 30/70 split)
    let middle_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(1),
            Constraint::Percentage(50),
        ])
        .split(middle_area);

    let search_area = middle_cols[0];
    let global_area = middle_cols[2];

    // ─── Sidebar help ───────────────────
    let sidebar_lines = vec![
        Line::from(""),
        Line::from(Span::styled(" Navigation", t.text_bold)),
        kv(t, "j/k ↑↓", "Move selection"),
        kv(t, "gg / G", "Jump to top / bottom"),
        kv(t, "H / M / L", "Top / mid / bottom"),
        kv(t, "Ctrl+d/u", "Half-page"),
        kv(t, "Ctrl+f/b", "Full page"),
        kv(t, "{ / }", "Jump 10"),
        kv(t, "n / N", "Next / prev match"),
        kv(t, "Enter", "Expand / collapse"),
        kv(t, "l / →", "Focus detail"),
        Line::from(""),
        Line::from(Span::styled(" Fold", t.text_bold)),
        kv(t, "zo / zc", "Open / close"),
        kv(t, "zO / zC", "Recursive"),
        kv(t, "zR / zM", "All"),
    ];
    let sidebar_block = theme::panel("[1] Tree", false, t);
    frame.render_widget(
        Paragraph::new(sidebar_lines)
            .block(sidebar_block)
            .wrap(Wrap { trim: false }),
        sidebar_area,
    );

    // ─── Detail pane help ───────────────
    let detail_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " Tabs (auto-switch when browsing branches/snapshots)",
            t.text_bold,
        )),
        kv(t, "Tab / S-Tab", "Cycle tabs forward / backward"),
        kv(
            t,
            "h/← l/→",
            "Navigate between tabs (moves to sidebar at left edge)",
        ),
        Line::from(""),
        desc(t, "Node", "Array/group metadata for selected tree node"),
        desc(
            t,
            "Repo",
            "Repository overview, storage, config, virtual sources",
        ),
        desc(
            t,
            "Ops Log",
            "Mutation history (commits, branch ops, config changes)",
        ),
        desc(t, "Branch", "Branch detail — commits, storage stats"),
        desc(t, "Snap", "Snapshot diff — what changed between snapshots"),
        Line::from(""),
        Line::from(Span::styled(" Scrolling", t.text_bold)),
        kv(t, "j/k ↑↓", "Scroll content line by line"),
        kv(t, "d / u", "Scroll 3 lines down / up"),
        kv(t, "Ctrl+d/u", "Half-page down / up"),
        kv(t, "Ctrl+f/b", "Full page down / up"),
        kv(t, "gg / G", "Scroll to top / bottom"),
    ];
    let detail_block = theme::panel("[2] Detail", false, t);
    frame.render_widget(
        Paragraph::new(detail_lines)
            .block(detail_block)
            .wrap(Wrap { trim: false }),
        detail_area,
    );

    // ─── Search help ────────────────────
    let search_lines = vec![
        Line::from(""),
        Line::from(Span::styled(" Fuzzy Search (/)", t.text_bold)),
        kv(t, "/", "Start search in focused pane"),
        kv(t, "type", "Filter — best match auto-selected"),
        kv(t, "↑ / ↓", "Navigate through matches"),
        kv(t, "Enter", "Accept and apply selection"),
        kv(t, "Esc", "Cancel search"),
    ];
    let search_block = theme::panel("Search", false, t);
    frame.render_widget(
        Paragraph::new(search_lines)
            .block(search_block)
            .wrap(Wrap { trim: false }),
        search_area,
    );

    // ─── Global help ────────────────────
    let global_lines = vec![
        Line::from(""),
        Line::from(Span::styled(" Global", t.text_bold)),
        kv(t, "q", "Quit"),
        kv(t, "?", "Toggle help"),
        kv(t, "R", "Refresh"),
        kv(t, "t", "Toggle bottom panel"),
        kv(t, "1 / 2 / 3", "Focus pane"),
        kv(t, "Ctrl+hjkl", "Move panes"),
        Line::from(""),
        Line::from(Span::styled(" Yank (clipboard)", t.text_bold)),
        kv(t, "yy", "Yank current selection"),
        kv(t, "yp", "Yank Python connect snippet"),
        kv(t, "yr", "Yank Rust connect snippet"),
    ];
    let global_block = theme::panel("Global", false, t);
    frame.render_widget(
        Paragraph::new(global_lines)
            .block(global_block)
            .wrap(Wrap { trim: false }),
        global_area,
    );

    // ─── Bottom panel help ──────────────
    let bottom_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " Tabs: Snapshots | Branches | Tags",
            t.text_bold,
        )),
        kv(t, "h/l ←→", "Switch between tabs"),
        kv(t, "j/k ↑↓", "Move selection"),
        kv(t, "gg / G", "Jump to top / bottom"),
        kv(t, "H / M / L", "Screen top / middle / bottom"),
        kv(t, "Ctrl+d/u", "Half-page down / up"),
        kv(t, "Ctrl+f/b", "Full page down / up"),
        kv(t, "{ / }", "Jump 10 items up / down"),
        kv(t, "n / N", "Next / prev search match"),
        kv(t, "Enter", "Activate (switch branch, load diff)"),
        kv(t, "/", "Search/filter — all panes update as you type"),
    ];
    let bottom_block = theme::panel("[3] Version Control", false, t);
    frame.render_widget(
        Paragraph::new(bottom_lines)
            .block(bottom_block)
            .wrap(Wrap { trim: false }),
        bottom_area,
    );

    // ─── Hint bar ───────────────────────
    let hint = Line::from(Span::styled(" Press q or ? to close help", t.text_dim));
    frame.render_widget(Paragraph::new(hint), hint_area);
}

fn kv<'a>(t: &'a crate::theme::Theme, key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {key:<13}"), t.branch),
        Span::styled(desc, t.text),
    ])
}

fn desc<'a>(t: &'a crate::theme::Theme, label: &'a str, description: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {label:<13}"), t.timestamp),
        Span::styled(description, t.text_dim),
    ])
}
