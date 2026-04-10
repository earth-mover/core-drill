use ratatui::prelude::*;

use crate::app::App;

pub(super) fn render_ops_log<'a>(app: &'a App) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));

    match &app.store.ops_log {
        crate::store::LoadState::Loaded(entries) if !entries.is_empty() => {
            lines.push(Line::from(Span::styled(
                format!("  {} operations", entries.len()),
                app.theme.text_dim,
            )));
            lines.push(Line::from(""));

            for entry in entries {
                let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
                lines.push(Line::from(vec![
                    Span::styled(format!("  {ts}  "), app.theme.text_dim),
                    Span::styled(entry.description.clone(), app.theme.text),
                ]));
            }
        }
        crate::store::LoadState::Loaded(_) => {
            lines.push(Line::from(Span::styled(
                "  No operations recorded.",
                app.theme.text_dim,
            )));
        }
        crate::store::LoadState::Loading => {
            lines.push(Line::from(Span::styled("  Loading...", app.theme.loading)));
        }
        crate::store::LoadState::Error(e) => {
            let kind = crate::store::classify_error(e);
            let hint = match kind {
                crate::store::ErrorKind::Auth => {
                    "  (credentials may be expired \u{2014} press R to retry)"
                }
                crate::store::ErrorKind::Network => "  (network issue \u{2014} press R to retry)",
                crate::store::ErrorKind::NotFound => "  (not found \u{2014} press R to retry)",
                crate::store::ErrorKind::Other => "  (press R to retry)",
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  Error: {e}"), app.theme.error),
                Span::styled(hint, app.theme.text_dim),
            ]));
        }
        crate::store::LoadState::NotRequested => {
            lines.push(Line::from(Span::styled(
                "  Not loaded yet.",
                app.theme.text_dim,
            )));
        }
    }

    lines
}
