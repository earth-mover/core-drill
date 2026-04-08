//! ASCII art visualization of zarr array shapes with chunk grid overlays.
//!
//! Renders 1D arrays as horizontal bars, 2D as rectangles with chunk grid lines,
//! 3D+ as stacked 2D slices (isometric cube effect), and 4D+ with a text note
//! for additional dimensions.

use ratatui::prelude::*;

use crate::store::types::ArraySummary;
use crate::theme::Theme;
use crate::ui::format::ZarrMetadata;

/// Generate lines of styled text representing the array shape visualization.
/// `max_width` constrains the horizontal extent (in terminal columns).
pub fn render_shape(
    summary: &ArraySummary,
    theme: &Theme,
    max_width: u16,
) -> Vec<Line<'static>> {
    let ndim = summary.shape.len();
    let chunk_shape = ZarrMetadata::parse(&summary.zarr_metadata)
        .map(|m| m.chunk_shape)
        .unwrap_or_default();

    // Compute number of chunks per dimension
    let chunks_per_dim: Vec<u64> = summary
        .shape
        .iter()
        .zip(chunk_shape.iter().chain(std::iter::repeat(&1)))
        .map(|(&s, &c)| if c == 0 { 1 } else { (s + c - 1) / c })
        .collect();

    let dim_names: Vec<String> = summary
        .dimension_names
        .as_ref()
        .map(|names| {
            names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    if n.is_empty() {
                        format!("dim{i}")
                    } else {
                        n.clone()
                    }
                })
                .collect()
        })
        .unwrap_or_else(|| (0..ndim).map(|i| format!("dim{i}")).collect());

    let mut lines = Vec::new();

    // Header: dimension names with sizes
    let header: String = dim_names
        .iter()
        .zip(summary.shape.iter())
        .map(|(name, &size)| format!("{name} [{size}]"))
        .collect::<Vec<_>>()
        .join(" x ");
    lines.push(Line::from(Span::styled(
        format!("  {header}"),
        theme.text_bold,
    )));

    match ndim {
        0 => {
            lines.push(Line::from(Span::styled("  (scalar)", theme.text_dim)));
        }
        1 => {
            render_1d(&mut lines, theme, max_width, &chunks_per_dim, &chunk_shape, &summary.shape);
        }
        2 => {
            render_2d(&mut lines, theme, max_width, &chunks_per_dim);
        }
        _ => {
            render_3d_plus(&mut lines, theme, max_width, &chunks_per_dim, ndim, &dim_names);
        }
    }

    // Chunk summary line
    if !chunk_shape.is_empty() && chunk_shape.len() == ndim {
        let chunk_str = chunks_per_dim
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join("x");
        let shape_str = chunk_shape
            .iter()
            .map(|c| format!("[{c}]"))
            .collect::<Vec<_>>()
            .join("x");
        lines.push(Line::from(Span::styled(
            format!("   chunks: {chunk_str} of {shape_str}"),
            theme.text_dim,
        )));
    }

    lines
}

/// Render a 1D array as a horizontal bar with chunk divisions.
fn render_1d(
    lines: &mut Vec<Line<'static>>,
    theme: &Theme,
    max_width: u16,
    chunks_per_dim: &[u64],
    chunk_shape: &[u64],
    shape: &[u64],
) {
    let indent = 2usize;
    // Available width for the bar (leave room for indent + borders)
    let bar_width = (max_width as usize).saturating_sub(indent + 2).max(8);

    let n_chunks = chunks_per_dim.first().copied().unwrap_or(1) as usize;

    let pad = " ".repeat(indent);

    // Top border
    lines.push(Line::from(Span::styled(
        format!("{pad}\u{250c}{}\u{2510}", "\u{2500}".repeat(bar_width)),
        theme.text_dim,
    )));

    // Content row: build multi-span line with colored fill and dim separators
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(format!("{pad}\u{2502}"), theme.text_dim),
    ];

    if n_chunks <= 1 {
        spans.push(Span::styled("\u{2588}".repeat(bar_width), theme.branch));
    } else {
        let total = shape.first().copied().unwrap_or(1) as f64;
        let c_size = chunk_shape.first().copied().unwrap_or(1) as f64;
        let mut used = 0usize;
        for i in 0..n_chunks {
            let remaining = total - (i as f64 * c_size);
            let elem_count = c_size.min(remaining);
            let frac = elem_count / total;
            let w = if i == n_chunks - 1 {
                bar_width.saturating_sub(used)
            } else {
                ((frac * bar_width as f64).round() as usize).max(1)
            };
            if i > 0 && used < bar_width {
                spans.push(Span::styled("\u{2502}", theme.text_dim));
                used += 1;
                let fill = w.saturating_sub(1).min(bar_width.saturating_sub(used));
                if fill > 0 {
                    spans.push(Span::styled("\u{2588}".repeat(fill), theme.branch));
                    used += fill;
                }
            } else {
                let fill = w.min(bar_width.saturating_sub(used));
                spans.push(Span::styled("\u{2588}".repeat(fill), theme.branch));
                used += fill;
            }
        }
    }

    spans.push(Span::styled("\u{2502}", theme.text_dim));
    lines.push(Line::from(spans));

    // Bottom border
    lines.push(Line::from(Span::styled(
        format!("{pad}\u{2514}{}\u{2518}", "\u{2500}".repeat(bar_width)),
        theme.text_dim,
    )));
}

/// Render a 2D array as a rectangle with chunk grid lines.
fn render_2d(
    lines: &mut Vec<Line<'static>>,
    theme: &Theme,
    max_width: u16,
    chunks_per_dim: &[u64],
) {
    let indent = 2usize;
    let avail = (max_width as usize).saturating_sub(indent + 2).max(8);

    let ny = chunks_per_dim.first().copied().unwrap_or(1).max(1) as usize;
    let nx = chunks_per_dim.get(1).copied().unwrap_or(1).max(1) as usize;

    // Scale: each chunk column gets some chars, each chunk row gets some rows
    let col_w = ((avail) / nx).max(2).min(8);
    let row_h = 2usize.min(4).max(1);
    let grid_w = col_w * nx;

    // Top border
    let top = format!("{:indent$}\u{250c}{}\u{2510}", "", "\u{2500}".repeat(grid_w));
    lines.push(Line::from(Span::styled(top, theme.text_dim)));

    for r in 0..ny {
        for _sub in 0..row_h {
            let mut row = String::new();
            for c in 0..nx {
                if c > 0 {
                    row.push('\u{250a}'); // dotted vertical ┊
                }
                let fill = if c > 0 { col_w - 1 } else { col_w };
                row.push_str(&" ".repeat(fill));
            }
            let line = format!("{:indent$}\u{2502}{row}\u{2502}", "");
            lines.push(Line::from(Span::styled(line, theme.text_dim)));
        }
        // Horizontal chunk boundary (dotted) except after last row
        if r < ny - 1 {
            let sep = format!(
                "{:indent$}\u{2502}{}\u{2502}",
                "",
                "\u{00b7}".repeat(grid_w)
            );
            lines.push(Line::from(Span::styled(sep, theme.text_dim)));
        }
    }

    // Bottom border
    let bottom = format!("{:indent$}\u{2514}{}\u{2518}", "", "\u{2500}".repeat(grid_w));
    lines.push(Line::from(Span::styled(bottom, theme.text_dim)));
}

/// Render a 3D+ array as stacked 2D slices with isometric depth effect.
fn render_3d_plus(
    lines: &mut Vec<Line<'static>>,
    theme: &Theme,
    max_width: u16,
    chunks_per_dim: &[u64],
    ndim: usize,
    dim_names: &[String],
) {
    let indent = 2usize;
    let depth_offset = 2usize; // chars of horizontal offset per depth layer
    let n_slices = chunks_per_dim.first().copied().unwrap_or(1).max(1).min(4) as usize;
    let depth_padding = depth_offset * (n_slices - 1);

    let avail = (max_width as usize)
        .saturating_sub(indent + 2 + depth_padding)
        .max(8);

    let ny = chunks_per_dim.get(1).copied().unwrap_or(1).max(1) as usize;
    let nx = chunks_per_dim.get(2).copied().unwrap_or(1).max(1) as usize;

    let col_w = (avail / nx).max(2).min(8);
    let row_h = 2usize.max(1);
    let grid_w = col_w * nx;

    // Draw back layers (top borders only, stacked)
    for layer in (1..n_slices).rev() {
        let pad = indent + depth_offset * layer;
        let top = format!(
            "{:pad$}\u{250c}{}\u{2510}",
            "",
            "\u{2500}".repeat(grid_w),
        );
        lines.push(Line::from(Span::styled(top, theme.text_dim)));
    }

    // Front layer: full 2D grid
    let front_indent = indent;

    // Top border of front layer
    let top = format!(
        "{:front_indent$}\u{250c}{}\u{2510}{}",
        "",
        "\u{2500}".repeat(grid_w),
        if n_slices > 1 { "\u{2502}" } else { "" },
    );
    lines.push(Line::from(Span::styled(top, theme.text_dim)));

    let _total_content_rows = ny * row_h + (ny - 1); // content rows + separators
    let mut content_row_idx = 0;

    for r in 0..ny {
        for _sub in 0..row_h {
            let mut row = String::new();
            for c in 0..nx {
                if c > 0 {
                    row.push('\u{250a}');
                }
                let fill = if c > 0 { col_w - 1 } else { col_w };
                row.push_str(&" ".repeat(fill));
            }
            // Right-side depth indicator
            let right_depth = if content_row_idx < n_slices - 1 {
                "\u{2518}"
            } else {
                ""
            };
            let line = format!(
                "{:front_indent$}\u{2502}{row}\u{2502}{right_depth}",
                "",
            );
            lines.push(Line::from(Span::styled(line, theme.text_dim)));
            content_row_idx += 1;
        }
        if r < ny - 1 {
            let right_depth = if content_row_idx < n_slices - 1 {
                "\u{2518}"
            } else {
                ""
            };
            let sep = format!(
                "{:front_indent$}\u{2502}{}\u{2502}{right_depth}",
                "",
                "\u{00b7}".repeat(grid_w),
            );
            lines.push(Line::from(Span::styled(sep, theme.text_dim)));
            content_row_idx += 1;
        }
    }

    // Bottom border
    let bottom = format!(
        "{:front_indent$}\u{2514}{}\u{2518}",
        "",
        "\u{2500}".repeat(grid_w),
    );
    lines.push(Line::from(Span::styled(bottom, theme.text_dim)));

    // Extra dimensions note for 4D+
    if ndim > 3 {
        let extra: Vec<String> = dim_names
            .iter()
            .skip(3)
            .map(|n| n.clone())
            .collect();
        lines.push(Line::from(Span::styled(
            format!("   (+{} dims: {})", ndim - 3, extra.join(", ")),
            theme.text_dim,
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary(shape: Vec<u64>, dim_names: Option<Vec<String>>, zarr_meta: &str) -> ArraySummary {
        ArraySummary {
            shape,
            dimension_names: dim_names,
            manifest_count: 1,
            zarr_metadata: zarr_meta.to_string(),
        }
    }

    fn chunk_meta(chunk_shape: &[u64]) -> String {
        let cs: Vec<String> = chunk_shape.iter().map(|c| c.to_string()).collect();
        format!(
            r#"{{"zarr_format":3,"data_type":"float32","chunk_grid":{{"configuration":{{"chunk_shape":[{}]}}}},"codecs":[],"fill_value":0}}"#,
            cs.join(",")
        )
    }

    #[test]
    fn test_1d_renders() {
        let theme = Theme::default();
        let summary = make_summary(vec![561264], Some(vec!["time".into()]), &chunk_meta(&[80000]));
        let lines = render_shape(&summary, &theme, 60);
        assert!(!lines.is_empty());
        // Should contain the header
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("time [561264]"));
        assert!(text.contains("chunks:"));
    }

    #[test]
    fn test_2d_renders() {
        let theme = Theme::default();
        let summary = make_summary(
            vec![721, 1440],
            Some(vec!["latitude".into(), "longitude".into()]),
            &chunk_meta(&[360, 360]),
        );
        let lines = render_shape(&summary, &theme, 60);
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("latitude [721]"));
        assert!(text.contains("longitude [1440]"));
    }

    #[test]
    fn test_3d_renders() {
        let theme = Theme::default();
        let summary = make_summary(
            vec![561264, 721, 1440],
            Some(vec!["time".into(), "latitude".into(), "longitude".into()]),
            &chunk_meta(&[80000, 360, 360]),
        );
        let lines = render_shape(&summary, &theme, 60);
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("time [561264]"));
        assert!(text.contains("chunks:"));
    }

    #[test]
    fn test_4d_shows_extra_dims() {
        let theme = Theme::default();
        let summary = make_summary(
            vec![10, 100, 721, 1440],
            Some(vec!["batch".into(), "time".into(), "lat".into(), "lon".into()]),
            &chunk_meta(&[10, 50, 360, 360]),
        );
        let lines = render_shape(&summary, &theme, 60);
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("+1 dims: lon"));
    }

    #[test]
    fn test_scalar_renders() {
        let theme = Theme::default();
        let summary = make_summary(vec![], None, "{}");
        let lines = render_shape(&summary, &theme, 40);
        let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("scalar"));
    }

    #[test]
    fn test_no_chunk_metadata() {
        let theme = Theme::default();
        let summary = make_summary(vec![100, 200], Some(vec!["x".into(), "y".into()]), "{}");
        let lines = render_shape(&summary, &theme, 40);
        // Should still render without panicking
        assert!(!lines.is_empty());
    }
}
