use ratatui::prelude::*;

use crate::app::App;
use crate::ui::widgets::{section_header, split_wrap_tokens};

pub(super) fn render_branch_detail<'a>(app: &'a App, branch_name: &str, is_current: bool) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    // ─── Branch Header ─────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Branch"));

    lines.push(Line::from(vec![
        Span::styled("  Name:        ", app.theme.text_dim),
        Span::styled(branch_name.to_string(), app.theme.branch),
        if is_current {
            Span::styled("  (active)", app.theme.status_ok)
        } else {
            Span::styled("  (press Enter to switch)", app.theme.text_dim)
        },
    ]));

    // Find the BranchInfo for snapshot ID
    if let Some(branch) = app.store.branches.as_loaded()
        .and_then(|bs| bs.iter().find(|b| b.name == branch_name))
    {
        lines.push(Line::from(vec![
            Span::styled("  Tip:         ", app.theme.text_dim),
            Span::styled(
                crate::output::truncate(&branch.snapshot_id, 12).to_string(),
                app.theme.text,
            ),
        ]));
    }

    // ─── Recent Commits ────────────────
    if let Some(crate::store::LoadState::Loaded(ancestry)) = app.store.ancestry.get(branch_name) {
        lines.push(Line::from(""));
        lines.push(section_header(&format!("Recent Commits ({})", ancestry.len())));

        let max_width = app.detail_area.width.saturating_sub(2) as usize;
        for entry in ancestry.iter().take(10) {
            let ts = entry.timestamp.format("%Y-%m-%d %H:%M");
            let snap = crate::output::truncate(&entry.id, 8);
            let prefix = format!("  {ts}  {snap}  ");
            let msg = if entry.message.is_empty() {
                "(no message)".to_string()
            } else {
                entry.message.clone()
            };
            let prefix_len = prefix.len();
            let available = max_width.saturating_sub(prefix_len);

            if msg.len() <= available || available == 0 {
                lines.push(Line::from(vec![
                    Span::styled(prefix, app.theme.text_dim),
                    Span::styled(msg, app.theme.text),
                ]));
            } else {
                // Wrap: first line gets the prefix, continuations get indented
                let indent = " ".repeat(prefix_len);
                let mut first = true;
                let mut current = String::new();
                for word in split_wrap_tokens(&msg) {
                    if current.len() + word.len() <= available || current.is_empty() {
                        current.push_str(&word);
                    } else {
                        if first {
                            lines.push(Line::from(vec![
                                Span::styled(prefix.clone(), app.theme.text_dim),
                                Span::styled(current.trim_end().to_string(), app.theme.text),
                            ]));
                            first = false;
                        } else {
                            lines.push(Line::from(vec![
                                Span::styled(indent.clone(), app.theme.text_dim),
                                Span::styled(current.trim_end().to_string(), app.theme.text),
                            ]));
                        }
                        current = word.to_string();
                    }
                }
                if !current.is_empty() {
                    if first {
                        lines.push(Line::from(vec![
                            Span::styled(prefix, app.theme.text_dim),
                            Span::styled(current.trim_end().to_string(), app.theme.text),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(indent, app.theme.text_dim),
                            Span::styled(current.trim_end().to_string(), app.theme.text),
                        ]));
                    }
                }
            }
        }
        if ancestry.len() > 10 {
            lines.push(Line::from(Span::styled(
                format!("  \u{2026} {} more", ancestry.len() - 10),
                app.theme.text_dim,
            )));
        }
    } else if is_current {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Loading commit history\u{2026}",
            app.theme.loading,
        )));
    }

    // ─── Storage Stats (only for active branch — data already loaded) ───
    if is_current {
        let ss = app.store.storage_stats();

        if ss.total_arrays > 0 || ss.total_groups > 0 {
            lines.push(Line::from(""));
            lines.push(section_header("Storage"));
            lines.push(Line::from(vec![
                Span::styled("  Arrays:      ", app.theme.text_dim),
                Span::styled(ss.total_arrays.to_string(), app.theme.text),
            ]));
            if ss.empty_arrays > 0 || ss.filled_arrays > 0 {
                lines.push(Line::from(vec![
                    Span::styled("    Filled:    ", app.theme.text_dim),
                    Span::styled(ss.filled_arrays.to_string(), app.theme.text),
                    Span::styled(
                        format!("  empty: {}", ss.empty_arrays),
                        app.theme.text_dim,
                    ),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled("  Groups:      ", app.theme.text_dim),
                Span::styled(ss.total_groups.to_string(), app.theme.text),
            ]));
            if ss.total_written > 0 {
                lines.push(Line::from(vec![
                    Span::styled("  Chunks:      ", app.theme.text_dim),
                    Span::styled(ss.total_written.to_string(), app.theme.text),
                ]));
            }

            if ss.stats_loaded > 0 {
                let parts = ss.breakdown_parts();

                let suffix = if ss.stats_loaded < ss.total_arrays {
                    format!("  ({}/{} arrays scanned)", ss.stats_loaded, ss.total_arrays)
                } else {
                    String::new()
                };

                lines.push(Line::from(vec![
                    Span::styled("  Breakdown:   ", app.theme.text_dim),
                    Span::styled(format!("{}{}", parts.join(", "), suffix), app.theme.text),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Data size:   ", app.theme.text_dim),
                    Span::styled(
                        humansize::format_size(ss.total_bytes(), humansize::BINARY),
                        app.theme.text,
                    ),
                ]));
            }
        }
    }

    lines
}
