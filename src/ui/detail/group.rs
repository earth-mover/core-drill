use ratatui::prelude::*;

use crate::app::App;
use crate::store::types::TreeNodeType;

pub(super) fn render_group_detail<'a>(
    app: &'a App,
    node: &crate::store::TreeNode,
) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Group", app.theme.text_bold)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Path:      ", app.theme.text_dim),
            Span::styled(node.path.clone(), app.theme.text),
        ]),
    ];

    if let Some(crate::store::LoadState::Loaded(children)) = app.store.node_children.get(&node.path)
    {
        lines.push(Line::from(vec![
            Span::styled("  Children:  ", app.theme.text_dim),
            Span::styled(children.len().to_string(), app.theme.text),
        ]));
        lines.push(Line::from(""));

        for child in children {
            let icon = match &child.node_type {
                TreeNodeType::Group => "\u{1f4c1} ",
                TreeNodeType::Array(_) => "\u{1f4ca} ",
            };
            lines.push(Line::from(Span::styled(
                format!("    {icon}{}", child.name),
                app.theme.text,
            )));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Children:  ", app.theme.text_dim),
            Span::styled("not loaded (press Enter to expand)", app.theme.text_dim),
        ]));
    }

    lines
}
