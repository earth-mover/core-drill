use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::App;
use crate::component::BottomTab;

/// Render a tabbed panel: outer border with title, a tab bar, and return the content area.
/// Used by both the Detail pane and the Bottom (Version Control) pane.
/// Render a tabbed panel. Returns `(content_area, tab_bar_area)` if there's enough space.
pub(super) fn render_tabbed_panel(
    title: &str,
    tab_names: &[&str],
    active_index: usize,
    focused: bool,
    theme: &crate::theme::Theme,
    frame: &mut Frame,
    area: Rect,
) -> Option<(Rect, Rect)> {
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(if focused {
            theme.border_focused
        } else {
            theme.border
        });

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 {
        return None;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),    // content
        ])
        .split(inner);

    let tab_labels: Vec<Line> = tab_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == active_index { "●" } else { "○" };
            Line::from(format!("{marker} {name}"))
        })
        .collect();

    let tabs = Tabs::new(tab_labels)
        .select(active_index)
        .style(theme.text_dim)
        .highlight_style(if focused { theme.selected } else { theme.text });
    frame.render_widget(tabs, chunks[0]);

    Some((chunks[1], chunks[0]))
}

/// Compute a clamped scroll offset: cap detail_scroll so the last content line
/// is still visible. `content_height` is the number of lines in `text`
/// (an approximation that ignores wrapping, so it's a conservative cap).
pub(super) fn clamped_scroll(detail_scroll: usize, content_height: usize, area: Rect) -> u16 {
    // area still has the border — inner height is area.height - 2 (top + bottom border)
    let visible_height = (area.height as usize).saturating_sub(2);
    let max_scroll = content_height.saturating_sub(visible_height);
    detail_scroll.min(max_scroll) as u16
}

/// Produce one or more `Line`s for a label/value pair.
///
/// If the label + value fit within `max_width` columns, a single line is returned.
/// Otherwise the value is split at word boundaries (spaces) and continuation lines
/// are indented to align with the start of the value column (i.e. `label.len()` spaces).
pub(super) fn labeled_lines<'a>(
    label: &'a str,
    value: String,
    label_style: Style,
    value_style: Style,
    max_width: u16,
) -> Vec<Line<'a>> {
    let label_len = label.len();
    let available = (max_width as usize).saturating_sub(label_len);

    // Fast path: everything fits on one line.
    if value.len() <= available || available == 0 {
        return vec![Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(value, value_style),
        ])];
    }

    // Split the value into chunks that fit within `available` columns.
    // We split on spaces so we don't break inside tokens.
    let indent = " ".repeat(label_len);
    let mut result: Vec<Line<'a>> = Vec::new();
    let mut current_line = String::new();
    let mut first = true;

    for word in value.split_inclusive(' ') {
        if current_line.len() + word.len() <= available || current_line.is_empty() {
            current_line.push_str(word);
        } else {
            // Flush the current line.
            if first {
                result.push(Line::from(vec![
                    Span::styled(label, label_style),
                    Span::styled(current_line.trim_end().to_string(), value_style),
                ]));
                first = false;
            } else {
                let ind = indent.clone();
                result.push(Line::from(vec![
                    Span::styled(ind, label_style),
                    Span::styled(current_line.trim_end().to_string(), value_style),
                ]));
            }
            current_line = word.to_string();
        }
    }

    // Flush any remaining text.
    if !current_line.is_empty() {
        let trimmed = current_line.trim_end().to_string();
        if first {
            result.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(trimmed, value_style),
            ]));
        } else {
            let ind = indent.clone();
            result.push(Line::from(vec![
                Span::styled(ind, label_style),
                Span::styled(trimmed, value_style),
            ]));
        }
    }

    result
}

/// Build a section-header `Line` with consistent width and dark-gray styling.
/// Total visual width is kept near 40 characters by padding with `─` on the right.
pub(super) fn section_header(label: &str) -> Line<'static> {
    let prefix = format!("  ─── {label} ");
    let remaining = 36usize.saturating_sub(prefix.chars().count());
    let line = format!("{prefix}{}", "─".repeat(remaining));
    Line::from(Span::styled(
        line,
        Style::default().fg(Color::Rgb(120, 120, 120)),
    ))
}

/// Format a VCC prefix URL for display. Resolves `__al_source` container names
/// to the Arraylake repo's bucket name when available.
pub(super) fn format_vcc_prefix(prefix: &str, repo_info: &crate::app::RepoIdentity) -> String {
    if let Some(rest) = prefix.strip_prefix("vcc://") {
        if let Some((container, path)) = rest.split_once('/') {
            // For __al_source in Arraylake repos, show the bucket name
            let display_name = if container == "__al_source" {
                if let crate::app::RepoIdentity::Arraylake {
                    org, repo, bucket, platform, ..
                } = repo_info
                {
                    format!("{org}/{repo} \u{2192} {bucket} ({platform})")
                } else {
                    format!("{container} (managed)")
                }
            } else {
                container.to_string()
            };
            format!("      {display_name}: {path}/")
        } else {
            // Just a container name, no subpath
            if rest == "__al_source" {
                if let crate::app::RepoIdentity::Arraylake {
                    org, repo, bucket, platform, ..
                } = repo_info
                {
                    format!("      {org}/{repo} \u{2192} {bucket} ({platform})")
                } else {
                    format!("      {rest} (managed)")
                }
            } else {
                format!("      {rest}")
            }
        }
    } else {
        // Non-VCC URL (e.g., s3://, file://)
        format!("      {prefix}/")
    }
}

/// Group a list of paths by their parent directory.
/// Returns `(parent_path, vec_of_leaf_names)` sorted by parent.
pub(super) fn group_by_parent(paths: &[String]) -> Vec<(String, Vec<String>)> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for path in paths {
        // Find the last '/' to split parent from leaf
        match path.rfind('/') {
            Some(0) => {
                // Root-level item: parent is "/", leaf is the rest
                groups
                    .entry("/".to_string())
                    .or_default()
                    .push(path[1..].to_string());
            }
            Some(idx) => {
                let parent = format!("{}/", &path[..idx]);
                let leaf = path[idx + 1..].to_string();
                groups.entry(parent).or_default().push(leaf);
            }
            None => {
                // No slash at all — treat entire string as leaf under "/"
                groups
                    .entry("/".to_string())
                    .or_default()
                    .push(path.clone());
            }
        }
    }

    groups.into_iter().collect()
}

/// Render a section of paths grouped by parent directory with truncation.
/// `prefix` is the symbol to show before each leaf ("+", "-", "~").
pub(super) fn render_grouped_paths<'a>(
    lines: &mut Vec<Line<'a>>,
    header: &str,
    paths: &[String],
    prefix: &str,
    style: Style,
) {
    const MAX_ITEMS: usize = 20;
    const SHOW_ITEMS: usize = 15;

    lines.push(Line::from(Span::styled(header.to_string(), style)));

    let grouped = group_by_parent(paths);
    let total_items: usize = paths.len();
    let mut shown = 0;

    for (parent, leaves) in &grouped {
        if shown >= SHOW_ITEMS && total_items > MAX_ITEMS {
            break;
        }

        // If there's only one group and one leaf, show flat
        if grouped.len() == 1 && leaves.len() == 1 {
            lines.push(Line::from(Span::styled(
                format!("    {prefix} {parent}{}", leaves[0]),
                style,
            )));
            shown += 1;
            continue;
        }

        lines.push(Line::from(Span::styled(format!("    {parent}"), style)));

        for leaf in leaves {
            if shown >= SHOW_ITEMS && total_items > MAX_ITEMS {
                break;
            }
            lines.push(Line::from(Span::styled(
                format!("      {prefix} {leaf}"),
                style,
            )));
            shown += 1;
        }
    }

    if total_items > MAX_ITEMS {
        let remaining = total_items - SHOW_ITEMS;
        lines.push(Line::from(Span::styled(
            format!("    ... and {remaining} more"),
            style,
        )));
    }
}

/// Compute total grid positions = ∏ ceil(shape[i] / chunk_shape[i]).
/// Returns None if shapes are mismatched, empty, or any chunk dimension is zero.
pub(super) fn compute_grid_chunks(
    summary: &crate::store::types::ArraySummary,
    meta: &super::format::ZarrMetadata,
) -> Option<u64> {
    if summary.shape.is_empty() || meta.chunk_shape.is_empty() {
        return None;
    }
    if summary.shape.len() != meta.chunk_shape.len() {
        return None;
    }
    summary
        .shape
        .iter()
        .zip(meta.chunk_shape.iter())
        .try_fold(1u64, |acc, (&s, &c)| {
            if c == 0 {
                return None;
            }
            acc.checked_mul(s.div_ceil(c))
        })
}

/// Format an initialized-fraction line: "X of Y (Z%)" or "X of Y (100%)" etc.
pub(super) fn fmt_initialized(written: u64, grid: u64) -> String {
    let pct = if grid > 0 { written * 100 / grid } else { 0 };
    format!("{written} of {grid}  ({pct}%)")
}

/// Resolve search-filtered indices and cursor for a bottom-panel list.
/// Returns (visible_indices, search_cursor_source_index).
pub(super) fn resolve_search_indices(
    app: &App,
    list_len: usize,
    target: crate::search::SearchTarget,
) -> (Vec<usize>, Option<usize>) {
    let search_active = app
        .search
        .as_ref()
        .is_some_and(|s| s.target == target && !s.query.is_empty());

    let indices = if search_active {
        app.search.as_ref().unwrap().matches.clone()
    } else {
        let start = app.bottom_offset();
        (start..list_len).collect()
    };

    let cursor = if search_active {
        app.search.as_ref().and_then(|s| s.selected_index())
    } else {
        None
    };

    (indices, cursor)
}

/// Render a scrollable list with selection highlight and optional search filtering.
/// Shared by Branches, Tags, and potentially Snapshots.
pub(super) fn render_scrollable_list<T, F>(
    items: &[T],
    label_fn: F,
    default_style: Style,
    app: &App,
    focused: bool,
    frame: &mut Frame,
    area: Rect,
) where
    F: Fn(&T) -> &str,
{
    let target = match app.bottom_tab {
        BottomTab::Branches => crate::search::SearchTarget::Branches,
        BottomTab::Tags => crate::search::SearchTarget::Tags,
        BottomTab::Snapshots => crate::search::SearchTarget::Snapshots,
    };
    let (indices, search_cursor_idx) = resolve_search_indices(app, items.len(), target);

    let list_items: Vec<ListItem> = indices
        .iter()
        .filter_map(|&i| items.get(i).map(|item| (i, item)))
        .map(|(i, item)| {
            let is_selected = if app.search.is_some() {
                search_cursor_idx == Some(i)
            } else {
                i == app.bottom_selected()
            };
            let style = if is_selected && focused {
                app.theme.selected
            } else if is_selected {
                app.theme.selected_inactive
            } else {
                default_style
            };
            ListItem::new(Span::styled(label_fn(item), style))
        })
        .collect();
    frame.render_widget(List::new(list_items), area);
}
