use ratatui::prelude::*;

use super::widgets::{format_vcc_prefix, labeled_lines, render_grouped_paths, section_header};
use crate::app::App;
use crate::store::LoadState;

pub(super) fn render_snapshot_diff_detail<'a>(
    app: &'a App,
    snapshot_id: &str,
    max_width: u16,
) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    // --- Snapshot header (from ancestry, always available instantly) ---
    let entry = app
        .store
        .ancestry
        .get(&app.current_branch)
        .and_then(|s| s.as_loaded())
        .and_then(|entries| entries.iter().find(|e| e.id == snapshot_id));

    if let Some(entry) = entry {
        let short_id = crate::output::truncate(&entry.id, 12);
        let parent_short = entry
            .parent_id
            .as_ref()
            .map(|p| crate::output::truncate(p, 12))
            .unwrap_or("none");

        // Compute position counter: "N of M"
        let ancestry_len = app
            .store
            .ancestry
            .get(&app.current_branch)
            .and_then(|s| s.as_loaded())
            .map(|entries| entries.len())
            .unwrap_or(0);
        let position_n = app
            .store
            .ancestry
            .get(&app.current_branch)
            .and_then(|s| s.as_loaded())
            .and_then(|entries| {
                entries
                    .iter()
                    .position(|e| Some(&e.id) == app.current_snapshot.as_ref())
            })
            .map(|i| i + 1)
            .unwrap_or(1);
        let position_str = format!("    ({position_n} of {ancestry_len})");

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Snapshot:  ", app.theme.text_dim),
            Span::styled(short_id.to_string(), app.theme.snapshot_id),
            Span::styled(position_str, app.theme.text_dim),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Parent:    ", app.theme.text_dim),
            Span::styled(parent_short.to_string(), app.theme.snapshot_id),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Time:      ", app.theme.text_dim),
            Span::styled(
                entry.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                app.theme.timestamp,
            ),
        ]));
        lines.extend(labeled_lines(
            "  Message:   ",
            entry.message.clone(),
            app.theme.text_dim,
            app.theme.text,
            max_width,
        ));
    }

    // --- Separator ---
    lines.push(Line::from(""));
    lines.push(section_header("Changes"));

    // --- Diff section (may still be loading) ---
    let state = app.store.diffs.get(snapshot_id);

    match state {
        None | Some(LoadState::NotRequested) => {
            lines.push(Line::from(Span::styled(
                "  Waiting for diff request...",
                app.theme.text_dim,
            )));
        }
        Some(LoadState::Loading) => {
            lines.push(Line::from(Span::styled(
                "  Computing diff...",
                app.theme.loading,
            )));
        }
        Some(LoadState::Error(msg)) => {
            lines.push(Line::from(Span::styled(
                format!("  {msg}"),
                app.theme.error,
            )));
        }
        Some(LoadState::Loaded(diff)) => {
            if diff.is_initial_commit {
                // Initial commit: no parent to diff against — show a simple message.
                lines.push(Line::from(Span::styled(
                    "  Repository initialized",
                    app.theme.text_bold,
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  This is the first snapshot. No diff available \u{2014}",
                    app.theme.text_dim,
                )));
                lines.push(Line::from(Span::styled(
                    "  select a later snapshot to see what changed.",
                    app.theme.text_dim,
                )));
            } else {
                let added_count = diff.added_arrays.len() + diff.added_groups.len();
                let deleted_count = diff.deleted_arrays.len() + diff.deleted_groups.len();
                let modified_count = diff.modified_arrays.len() + diff.modified_groups.len();
                let moved_count = diff.moved_nodes.len();

                let total_chunks_changed: usize = diff.chunk_changes.iter().map(|(_, n)| n).sum();
                let mut summary_spans = vec![
                    Span::styled("  ", app.theme.text_dim),
                    Span::styled(format!("{added_count} added"), app.theme.added),
                    Span::styled(", ", app.theme.text_dim),
                    Span::styled(format!("{deleted_count} removed"), app.theme.removed),
                    Span::styled(", ", app.theme.text_dim),
                    Span::styled(format!("{modified_count} modified"), app.theme.modified),
                ];
                if moved_count > 0 {
                    summary_spans.push(Span::styled(", ", app.theme.text_dim));
                    summary_spans.push(Span::styled(
                        format!("{moved_count} moved"),
                        app.theme.modified,
                    ));
                }
                if total_chunks_changed > 0 {
                    summary_spans.push(Span::styled("  |  ", app.theme.text_dim));
                    summary_spans.push(Span::styled(
                        format!("{total_chunks_changed} chunks changed"),
                        app.theme.text_dim,
                    ));
                }
                lines.push(Line::from(summary_spans));

                // Added section (groups + arrays, grouped by parent)
                if !diff.added_groups.is_empty() || !diff.added_arrays.is_empty() {
                    let mut all_added: Vec<String> = diff
                        .added_groups
                        .iter()
                        .map(|p| format!("{p} (group)"))
                        .collect();
                    all_added.extend(diff.added_arrays.iter().cloned());
                    lines.push(Line::from(""));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Added ({added_count}):"),
                        &all_added,
                        "+",
                        app.theme.added,
                    );
                }

                // Removed section
                if !diff.deleted_groups.is_empty() || !diff.deleted_arrays.is_empty() {
                    let mut all_deleted: Vec<String> = diff
                        .deleted_groups
                        .iter()
                        .map(|p| format!("{p} (group)"))
                        .collect();
                    all_deleted.extend(diff.deleted_arrays.iter().cloned());
                    lines.push(Line::from(""));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Removed ({deleted_count}):"),
                        &all_deleted,
                        "-",
                        app.theme.removed,
                    );
                }

                // Modified section
                if !diff.modified_groups.is_empty() || !diff.modified_arrays.is_empty() {
                    let mut all_modified: Vec<String> = diff
                        .modified_groups
                        .iter()
                        .map(|p| format!("{p} (group)"))
                        .collect();
                    all_modified.extend(diff.modified_arrays.iter().cloned());
                    lines.push(Line::from(""));
                    render_grouped_paths(
                        &mut lines,
                        &format!("  Modified ({modified_count}):"),
                        &all_modified,
                        "~",
                        app.theme.modified,
                    );
                }

                // Chunk changes
                if !diff.chunk_changes.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "  Chunk Changes:",
                        app.theme.text_bold,
                    )));
                    let max_show = 20;
                    let total = diff.chunk_changes.len();
                    for (path, count) in diff.chunk_changes.iter().take(max_show) {
                        let chunk_key = (snapshot_id.to_string(), path.clone());
                        let (annotation, extra_source_lines) =
                            match app.store.chunk_stats.get(&chunk_key) {
                                Some(LoadState::Loaded(stats)) if stats.stats_complete => {
                                    let v = stats.virtual_count;
                                    let s = stats.native_count;
                                    let i = stats.inline_count;
                                    if v > 0 && s == 0 && i == 0 {
                                        if stats.virtual_prefixes.len() == 1 {
                                            // Exactly one source — show inline with resolved name
                                            let resolved = format_vcc_prefix(
                                                &stats.virtual_prefixes[0].0,
                                                &app.repo_info,
                                            );
                                            let resolved = resolved.trim();
                                            (format!("  (virtual \u{2192} {resolved})"), vec![])
                                        } else {
                                            // Multiple sources — list them indented below
                                            let source_lines: Vec<Line> = stats
                                                .virtual_prefixes
                                                .iter()
                                                .map(|(prefix, cnt)| {
                                                    let resolved =
                                                        format_vcc_prefix(prefix, &app.repo_info);
                                                    Line::from(vec![
                                                        Span::styled(resolved, app.theme.text),
                                                        Span::styled(
                                                            format!("  ({cnt} chunks)"),
                                                            app.theme.text_dim,
                                                        ),
                                                    ])
                                                })
                                                .collect();
                                            ("  (all virtual)".to_string(), source_lines)
                                        }
                                    } else if s > 0 && v == 0 && i == 0 {
                                        ("  (all stored)".to_string(), vec![])
                                    } else if i > 0 && v == 0 && s == 0 {
                                        ("  (all inline)".to_string(), vec![])
                                    } else {
                                        (
                                            format!("  (virtual: {v}, stored: {s}, inline: {i})"),
                                            vec![],
                                        )
                                    }
                                }
                                _ => (String::new(), vec![]),
                            };
                        let mut row = vec![
                            Span::styled(format!("    {path}  "), app.theme.text),
                            Span::styled(format!("{count} chunks"), app.theme.text_dim),
                        ];
                        if !annotation.is_empty() {
                            row.push(Span::styled(annotation, app.theme.text_dim));
                        }
                        lines.push(Line::from(row));
                        lines.extend(extra_source_lines);
                    }
                    if total > max_show {
                        lines.push(Line::from(Span::styled(
                            format!("    ... and {} more", total - max_show),
                            app.theme.text_dim,
                        )));
                    }
                }

                // Moved section
                if !diff.moved_nodes.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  Moved ({moved_count}):"),
                        app.theme.modified,
                    )));
                    for (from, to) in &diff.moved_nodes {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(from.clone(), app.theme.removed),
                            Span::raw(" \u{2192} "),
                            Span::styled(to.clone(), app.theme.added),
                        ]));
                    }
                }
            }
        }
    }

    lines
}
