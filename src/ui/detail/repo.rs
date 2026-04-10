use ratatui::prelude::*;

use crate::app::App;
use crate::ui::widgets::{format_vcc_prefix, section_header, storage_stats_lines};

pub(super) fn render_repo_overview<'a>(app: &'a App) -> Vec<Line<'a>> {
    let branch_count = app.store.branches.as_loaded().map(|b| b.len()).unwrap_or(0);
    let tag_count = app.store.tags.as_loaded().map(|t| t.len()).unwrap_or(0);
    let snapshot_count = app
        .store
        .ancestry
        .get(&app.current_branch)
        .and_then(|s| s.as_loaded())
        .map(|a| a.len())
        .unwrap_or(0);

    let mut lines = Vec::new();

    // ─── Repository ─────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Repository"));

    match &app.repo_info {
        crate::app::RepoIdentity::Arraylake {
            org,
            repo,
            bucket,
            platform,
            region,
        } => {
            lines.push(Line::from(vec![
                Span::styled("  Organization:  ", app.theme.text_dim),
                Span::styled(org.clone(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Repo name:     ", app.theme.text_dim),
                Span::styled(repo.clone(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Bucket:        ", app.theme.text_dim),
                Span::styled(bucket.clone(), app.theme.text),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Platform:      ", app.theme.text_dim),
                Span::styled(platform.clone(), app.theme.text),
            ]));
            if region != "?" {
                lines.push(Line::from(vec![
                    Span::styled("  Region:        ", app.theme.text_dim),
                    Span::styled(region.clone(), app.theme.text),
                ]));
            }
        }
        crate::app::RepoIdentity::Local { path } => {
            lines.push(Line::from(vec![
                Span::styled("  Location:      ", app.theme.text_dim),
                Span::styled(path.clone(), app.theme.text),
            ]));
        }
        crate::app::RepoIdentity::S3 { url } => {
            lines.push(Line::from(vec![
                Span::styled("  Location:      ", app.theme.text_dim),
                Span::styled(url.clone(), app.theme.text),
            ]));
        }
    }

    lines.push(Line::from(vec![
        Span::styled("  Branch:        ", app.theme.text_dim),
        Span::styled(app.current_branch.clone(), app.theme.branch),
    ]));

    // ─── Contents ───────────────────────
    lines.push(Line::from(""));
    lines.push(section_header("Contents"));
    lines.push(Line::from(vec![
        Span::styled("  Branches:    ", app.theme.text_dim),
        Span::styled(branch_count.to_string(), app.theme.text),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Tags:        ", app.theme.text_dim),
        Span::styled(tag_count.to_string(), app.theme.text),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Snapshots:   ", app.theme.text_dim),
        Span::styled(snapshot_count.to_string(), app.theme.text),
    ]));

    // ─── Storage Summary ─────────────────
    {
        let ss = app.store.storage_stats();

        if ss.total_arrays > 0 || ss.total_groups > 0 {
            lines.push(Line::from(""));
            lines.push(section_header("Storage Summary"));
            lines.extend(storage_stats_lines(&ss, &app.theme, true));

            // ─── Virtual Sources ─────────────
            if !ss.virtual_prefixes.is_empty() {
                let total_vchunks: usize = ss.virtual_prefixes.iter().map(|(_, c)| *c).sum();

                lines.push(Line::from(""));
                lines.push(section_header("Virtual Sources"));
                lines.push(Line::from(vec![
                    Span::styled("  Total:       ", app.theme.text_dim),
                    Span::styled(
                        format!(
                            "{total_vchunks} chunks, {}",
                            humansize::format_size(ss.virtual_bytes, humansize::BINARY)
                        ),
                        app.theme.text,
                    ),
                ]));
                for (prefix, count) in &ss.virtual_prefixes {
                    let display = format_vcc_prefix(prefix, &app.repo_info);
                    let display = display.trim_start();
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {display}"), app.theme.text),
                        Span::styled(format!("  ({count} chunks)"), app.theme.text_dim),
                    ]));
                }
                if ss.stats_loaded < ss.total_arrays {
                    lines.push(Line::from(Span::styled(
                        format!("  ({}/{} arrays scanned)", ss.stats_loaded, ss.total_arrays),
                        app.theme.text_dim,
                    )));
                }
            }
        }
    }

    // ─── Configuration ──────────────────
    if let crate::store::LoadState::Loaded(config) = &app.store.repo_config {
        lines.push(Line::from(""));
        lines.push(section_header("Configuration"));
        lines.push(Line::from(vec![
            Span::styled("  Spec version:  ", app.theme.text_dim),
            Span::styled(config.spec_version.clone(), app.theme.text),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Status:        ", app.theme.text_dim),
            Span::styled(config.availability.clone(), app.theme.text),
        ]));
        if let Some(threshold) = config.inline_chunk_threshold {
            lines.push(Line::from(vec![
                Span::styled("  Inline \u{2264}       ", app.theme.text_dim),
                Span::styled(format!("{threshold} bytes"), app.theme.text),
            ]));
        }

        // ─── Feature Flags ──────────────
        if !config.feature_flags.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_header("Feature Flags"));
            for flag in &config.feature_flags {
                let status = if flag.enabled { "on" } else { "off" };
                let explicit = if flag.explicit { "" } else { " (default)" };
                let style = if flag.enabled {
                    app.theme.status_ok
                } else {
                    app.theme.text_dim
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}: ", flag.name), app.theme.text_dim),
                    Span::styled(format!("{status}{explicit}"), style),
                ]));
            }
        }

        // ─── Virtual Chunk Containers ───
        if !config.virtual_chunk_containers.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_header("Virtual Sources"));
            for (name, prefix) in &config.virtual_chunk_containers {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {name}: "), app.theme.text_dim),
                    Span::styled(prefix.clone(), app.theme.text),
                ]));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Navigate the tree or select a snapshot to see details.",
        app.theme.text_dim,
    )));

    lines
}
