use ratatui::prelude::*;

use crate::app::App;
use crate::ui::widgets::{labeled_lines, section_header, storage_stats_lines};

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

        let max_width = app.detail_area.width.saturating_sub(2);
        for entry in ancestry.iter().take(10) {
            let ts = entry.timestamp.format("%Y-%m-%d %H:%M");
            let snap = crate::output::truncate(&entry.id, 8);
            let prefix = format!("  {ts}  {snap}  ");
            let msg = if entry.message.is_empty() {
                "(no message)".to_string()
            } else {
                entry.message.clone()
            };
            lines.extend(labeled_lines(
                &prefix,
                msg,
                app.theme.text_dim,
                app.theme.text,
                max_width,
            ));
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
            lines.extend(storage_stats_lines(&ss, &app.theme, false));
        }
    }

    lines
}
