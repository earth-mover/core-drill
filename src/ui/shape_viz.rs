//! Dask-style chunk grid visualization using ratatui's Canvas widget.
//!
//! Renders zarr array chunk grids as graphical diagrams:
//! - 0D (scalar): text label only
//! - 1D: horizontal row of chunk rectangles
//! - 2D: grid of chunk rectangles with dimension labels
//! - 3D+: 2D front face with isometric depth offset for 3rd dimension

use ratatui::prelude::*;
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine};
use ratatui::widgets::Block;

// Brand palette colors — hardcoded for reliable canvas rendering.
// Theme color extraction via .fg returns Option<Color> and palette variants
// (Cyan, DarkGray, etc.) may not render consistently in all terminals.
const OUTER_COLOR: Color = Color::Rgb(94, 196, 247); // icechunk blue
const INNER_COLOR: Color = Color::Rgb(60, 60, 80); // dark blue-gray grid lines
const LABEL_COLOR: Color = Color::Rgb(245, 245, 245); // light gray labels

use crate::store::types::ArraySummary;
use crate::theme::Theme;
use crate::ui::format::ZarrMetadata;

/// Information needed to draw the chunk grid.
struct ChunkGridInfo {
    shape: Vec<u64>,
    chunk_shape: Vec<u64>,
    chunks_per_dim: Vec<u64>,
    dim_names: Vec<String>,
}

impl ChunkGridInfo {
    fn from_summary(summary: &ArraySummary) -> Self {
        let zarr = ZarrMetadata::parse(&summary.zarr_metadata);
        let chunk_shape = zarr.map(|z| z.chunk_shape).unwrap_or_default();
        let ndim = summary.shape.len();

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

        Self {
            shape: summary.shape.clone(),
            chunk_shape,
            chunks_per_dim,
            dim_names,
        }
    }
}

/// Build a Canvas widget that renders the chunk grid visualization.
///
/// Returns `None` for scalar arrays (0D) — the caller should show a text label instead.
pub fn chunk_grid_canvas<'a>(
    summary: &ArraySummary,
    _theme: &'a Theme,
) -> Option<Canvas<'a, impl Fn(&mut Context<'_>) + 'a>> {
    let ndim = summary.shape.len();
    if ndim == 0 {
        return None;
    }

    let info = ChunkGridInfo::from_summary(summary);

    // Clone data that the closure will own.
    let shape = info.shape.clone();
    let chunk_shape = info.chunk_shape.clone();
    let chunks_per_dim = info.chunks_per_dim.clone();
    let dim_names = info.dim_names.clone();

    // Canvas coordinate system: x in [0, 120], y in [0, 60].
    // We reserve margins for labels.
    let x_bounds = [0.0, 120.0];
    let y_bounds = [0.0, 60.0];

    let canvas = Canvas::default()
        .block(Block::default())
        .x_bounds(x_bounds)
        .y_bounds(y_bounds)
        .paint(move |ctx| match ndim {
            1 => paint_1d(ctx, &shape, &chunk_shape, &chunks_per_dim, &dim_names),
            2 => paint_2d(ctx, &shape, &chunk_shape, &chunks_per_dim, &dim_names),
            _ => paint_3d_plus(ctx, &shape, &chunk_shape, &chunks_per_dim, &dim_names, ndim),
        });

    Some(canvas)
}

/// Draw a 1D chunk row.
fn paint_1d(
    ctx: &mut Context<'_>,
    shape: &[u64],
    chunk_shape: &[u64],
    chunks_per_dim: &[u64],
    dim_names: &[String],
) {
    let nx = chunks_per_dim.first().copied().unwrap_or(1).min(20) as usize;
    let cs = chunk_shape.first().copied().unwrap_or(shape.first().copied().unwrap_or(1));
    let total = shape.first().copied().unwrap_or(1);

    // Grid area: x in [10, 110], y centred around 30
    let grid_left = 10.0_f64;
    let grid_right = 110.0_f64;
    let grid_bottom = 22.0_f64;
    let grid_top = 38.0_f64;
    let grid_w = grid_right - grid_left;
    let cell_w = grid_w / nx as f64;

    // Label first chunk with chunk size; leave others unlabeled
    let chunk_label = format!("{cs}");
    if nx >= 1 && cell_w > 4.0 {
        let x = grid_left;
        let lx = x + cell_w / 2.0 - chunk_label.len() as f64 / 2.0;
        let ly = (grid_bottom + grid_top) / 2.0;
        ctx.print(lx, ly, chunk_label.fg(LABEL_COLOR));
    }

    // Draw grid lines between chunks (inner dividers)
    for i in 1..nx {
        let x = grid_left + i as f64 * cell_w;
        ctx.draw(&CanvasLine {
            x1: x,
            y1: grid_bottom,
            x2: x,
            y2: grid_top,
            color: INNER_COLOR,
        });
    }

    // Outer border — solid icechunk blue
    draw_rect_border(ctx, grid_left, grid_bottom, grid_right, grid_top, OUTER_COLOR);

    // Dimension name and size below
    let dim_label = dim_names.first().map(|s| s.as_str()).unwrap_or("dim0");
    let bottom_label = format!("{dim_label}: {total} \u{2192}");
    ctx.print(
        grid_left + grid_w / 2.0 - bottom_label.len() as f64,
        grid_bottom - 4.0,
        bottom_label.fg(LABEL_COLOR),
    );

    // Chunk grid summary at bottom of canvas
    let grid_desc = if nx == 1 {
        format!("Chunk grid: {nx} chunk of {cs}")
    } else {
        format!("Chunk grid: {nx} chunks \u{00d7} {cs} each")
    };
    ctx.print(
        grid_left + grid_w / 2.0 - grid_desc.len() as f64,
        1.5,
        grid_desc.fg(LABEL_COLOR),
    );
}

/// Draw a 2D chunk grid.
fn paint_2d(
    ctx: &mut Context<'_>,
    shape: &[u64],
    chunk_shape: &[u64],
    chunks_per_dim: &[u64],
    dim_names: &[String],
) {
    let ny = chunks_per_dim.first().copied().unwrap_or(1).min(12) as usize;
    let nx = chunks_per_dim.get(1).copied().unwrap_or(1).min(16) as usize;

    let cs_y = chunk_shape.first().copied().unwrap_or(1);
    let cs_x = chunk_shape.get(1).copied().unwrap_or(1);
    let total_y = shape.first().copied().unwrap_or(1);
    let total_x = shape.get(1).copied().unwrap_or(1);

    // Grid area with margins for labels
    let grid_left = 8.0_f64;
    let grid_right = 100.0_f64;
    let grid_bottom = 8.0_f64;
    let grid_top = 54.0_f64;
    let grid_w = grid_right - grid_left;
    let grid_h = grid_top - grid_bottom;
    let cell_w = grid_w / nx as f64;
    let cell_h = grid_h / ny as f64;

    // Label first chunk (top-left, row=0 col=0) with chunk shape; leave others unlabeled
    if cell_w > 4.0 && cell_h > 3.0 {
        let first_label = format!("{cs_y}\u{00d7}{cs_x}");
        // row=0 is top-most: y = grid_top - 1 * cell_h
        let y0 = grid_top - cell_h;
        let lx = grid_left + cell_w / 2.0 - first_label.len() as f64 / 2.0;
        let ly = y0 + cell_h / 2.0;
        ctx.print(lx, ly, first_label.fg(LABEL_COLOR));
    }

    // Draw internal grid lines (inner dividers)
    for i in 1..nx {
        let x = grid_left + i as f64 * cell_w;
        ctx.draw(&CanvasLine {
            x1: x,
            y1: grid_bottom,
            x2: x,
            y2: grid_top,
            color: INNER_COLOR,
        });
    }
    for j in 1..ny {
        let y = grid_top - j as f64 * cell_h;
        ctx.draw(&CanvasLine {
            x1: grid_left,
            y1: y,
            x2: grid_right,
            y2: y,
            color: INNER_COLOR,
        });
    }

    // Outer border — solid icechunk blue
    draw_rect_border(ctx, grid_left, grid_bottom, grid_right, grid_top, OUTER_COLOR);

    // Bottom axis: dimension name + total size, and per-chunk sizes
    let dim_x_name = dim_names.get(1).map(|s| s.as_str()).unwrap_or("dim1");
    let bottom_label = format!("{dim_x_name}: {total_x} \u{2192}");
    ctx.print(
        grid_left + grid_w / 2.0 - bottom_label.len() as f64,
        grid_bottom - 5.0,
        bottom_label.fg(LABEL_COLOR),
    );

    // Per-column chunk sizes below grid
    if nx <= 8 {
        for i in 0..nx {
            let actual = if i == nx - 1 {
                let rem = total_x % cs_x;
                if rem == 0 { cs_x } else { rem }
            } else {
                cs_x
            };
            let label = format!("{actual}");
            let x = grid_left + i as f64 * cell_w + cell_w / 2.0 - label.len() as f64;
            ctx.print(x, grid_bottom - 2.0, label.fg(LABEL_COLOR));
        }
    }

    // Right axis: dimension name + per-row sizes
    let dim_y_name = dim_names.first().map(|s| s.as_str()).unwrap_or("dim0");
    let right_label = format!("\u{2191} {dim_y_name}: {total_y}");
    ctx.print(grid_right + 2.0, grid_top - grid_h / 2.0, right_label.fg(LABEL_COLOR));

    // Per-row chunk sizes on the right
    if ny <= 8 {
        for j in 0..ny {
            let actual = if j == ny - 1 {
                let rem = total_y % cs_y;
                if rem == 0 { cs_y } else { rem }
            } else {
                cs_y
            };
            let label = format!("{actual}");
            let y = grid_top - j as f64 * cell_h - cell_h / 2.0;
            ctx.print(grid_right + 2.0, y, label.fg(LABEL_COLOR));
        }
    }

    // Chunk grid summary at bottom of canvas
    let grid_desc = if ny == 1 && nx == 1 {
        format!("Chunk grid: 1\u{00d7}1 (single chunk of {cs_y}\u{00d7}{cs_x})")
    } else {
        format!("Chunk grid: {ny}\u{00d7}{nx}  (each chunk: {cs_y}\u{00d7}{cs_x})")
    };
    ctx.print(
        grid_left + grid_w / 2.0 - grid_desc.len() as f64 / 2.0,
        1.5,
        grid_desc.fg(LABEL_COLOR),
    );
}

/// Draw a 3D+ array as a 2D front face with isometric depth offset.
fn paint_3d_plus(
    ctx: &mut Context<'_>,
    shape: &[u64],
    chunk_shape: &[u64],
    chunks_per_dim: &[u64],
    dim_names: &[String],
    ndim: usize,
) {
    // For 3D+: dim0 is depth, dim1 is rows (Y), dim2 is cols (X)
    let n_depth = chunks_per_dim.first().copied().unwrap_or(1).min(6) as usize;
    let ny = chunks_per_dim.get(1).copied().unwrap_or(1).min(8) as usize;
    let nx = chunks_per_dim.get(2).copied().unwrap_or(1).min(10) as usize;

    let total_y = shape.get(1).copied().unwrap_or(1);
    let total_x = shape.get(2).copied().unwrap_or(1);
    let total_depth = shape.first().copied().unwrap_or(1);

    // Isometric offset per depth layer
    let dx_per_layer = 2.5_f64;
    let dy_per_layer = 2.0_f64;
    let n_back = n_depth.min(4); // show at most 4 depth layers

    // Front face grid area
    let grid_left = 8.0_f64;
    let grid_right = 88.0_f64;
    let grid_bottom = 6.0_f64;
    let grid_top = 44.0_f64;
    let grid_w = grid_right - grid_left;
    let grid_h = grid_top - grid_bottom;
    let cell_w = grid_w / nx as f64;
    let cell_h = grid_h / ny as f64;

    // Draw back layers (just borders with offset — inner color for depth layers)
    for layer in (1..n_back).rev() {
        let ox = layer as f64 * dx_per_layer;
        let oy = layer as f64 * dy_per_layer;
        let l = grid_left + ox;
        let b = grid_bottom + oy;
        let r = grid_right + ox;
        let t = grid_top + oy;
        draw_rect_border(ctx, l, b, r, t, INNER_COLOR);
    }

    // Draw connecting lines from front to back (corners)
    if n_back > 1 {
        let ox = (n_back - 1) as f64 * dx_per_layer;
        let oy = (n_back - 1) as f64 * dy_per_layer;
        // Top-right corner
        ctx.draw(&CanvasLine {
            x1: grid_right,
            y1: grid_top,
            x2: grid_right + ox,
            y2: grid_top + oy,
            color: INNER_COLOR,
        });
        // Top-left corner
        ctx.draw(&CanvasLine {
            x1: grid_left,
            y1: grid_top,
            x2: grid_left + ox,
            y2: grid_top + oy,
            color: INNER_COLOR,
        });
        // Bottom-right corner
        ctx.draw(&CanvasLine {
            x1: grid_right,
            y1: grid_bottom,
            x2: grid_right + ox,
            y2: grid_bottom + oy,
            color: INNER_COLOR,
        });
    }

    // Front face — label first chunk (top-left) with chunk shape; leave others unlabeled
    let cs_depth = chunk_shape.first().copied().unwrap_or(1);
    let cs_y = chunk_shape.get(1).copied().unwrap_or(1);
    let cs_x = chunk_shape.get(2).copied().unwrap_or(1);
    if cell_w > 6.0 && cell_h > 4.0 {
        let first_label = format!("{cs_y}\u{00d7}{cs_x}");
        let y0 = grid_top - cell_h; // row=0 top cell
        let lx = grid_left + cell_w / 2.0 - first_label.len() as f64 / 2.0;
        let ly = y0 + cell_h / 2.0;
        ctx.print(lx, ly, first_label.fg(LABEL_COLOR));
    }

    // Front face internal grid lines (inner dividers)
    for i in 1..nx {
        let x = grid_left + i as f64 * cell_w;
        ctx.draw(&CanvasLine {
            x1: x,
            y1: grid_bottom,
            x2: x,
            y2: grid_top,
            color: INNER_COLOR,
        });
    }
    for j in 1..ny {
        let y = grid_top - j as f64 * cell_h;
        ctx.draw(&CanvasLine {
            x1: grid_left,
            y1: y,
            x2: grid_right,
            y2: y,
            color: INNER_COLOR,
        });
    }

    // Front face border — solid icechunk blue
    draw_rect_border(ctx, grid_left, grid_bottom, grid_right, grid_top, OUTER_COLOR);

    // Labels
    let dim_x = dim_names.get(2).map(|s| s.as_str()).unwrap_or("dim2");
    let dim_y = dim_names.get(1).map(|s| s.as_str()).unwrap_or("dim1");
    let dim_z = dim_names.first().map(|s| s.as_str()).unwrap_or("dim0");

    let bottom_label = format!("{dim_x}: {total_x} \u{2192}");
    ctx.print(
        grid_left + grid_w / 2.0 - bottom_label.len() as f64,
        grid_bottom - 3.0,
        bottom_label.fg(LABEL_COLOR),
    );

    let right_label = format!("\u{2191} {dim_y}: {total_y}");
    ctx.print(grid_right + 2.0, grid_top - grid_h / 2.0, right_label.fg(LABEL_COLOR));

    // Depth label along the diagonal
    let depth_label = format!("{dim_z}: {total_depth} ({n_depth} chunks)");
    if n_back > 1 {
        let ox = (n_back - 1) as f64 * dx_per_layer;
        let oy = (n_back - 1) as f64 * dy_per_layer;
        ctx.print(
            grid_left + ox / 2.0,
            grid_top + oy / 2.0 + 3.0,
            depth_label.fg(LABEL_COLOR),
        );
    } else {
        ctx.print(grid_left, grid_top + 3.0, depth_label.fg(LABEL_COLOR));
    }

    // Extra dimensions note for 4D+
    if ndim > 3 {
        let extra: Vec<&str> = dim_names.iter().skip(3).map(|s| s.as_str()).collect();
        let extra_label = format!("+{} dims: {}", ndim - 3, extra.join(", "));
        ctx.print(grid_left, grid_bottom - 6.0, extra_label.fg(LABEL_COLOR));
    }

    // Chunk grid summary at bottom of canvas
    let grid_desc = format!(
        "Chunk grid: {n_depth}\u{00d7}{ny}\u{00d7}{nx}  (each chunk: {cs_depth}\u{00d7}{cs_y}\u{00d7}{cs_x})"
    );
    ctx.print(
        grid_left + grid_w / 2.0 - grid_desc.len() as f64 / 2.0,
        1.5,
        grid_desc.fg(LABEL_COLOR),
    );
}

/// Draw four lines forming the border of a rectangle.
fn draw_rect_border(ctx: &mut Context<'_>, left: f64, bottom: f64, right: f64, top: f64, color: Color) {
    // Bottom
    ctx.draw(&CanvasLine {
        x1: left,
        y1: bottom,
        x2: right,
        y2: bottom,
        color,
    });
    // Top
    ctx.draw(&CanvasLine {
        x1: left,
        y1: top,
        x2: right,
        y2: top,
        color,
    });
    // Left
    ctx.draw(&CanvasLine {
        x1: left,
        y1: bottom,
        x2: left,
        y2: top,
        color,
    });
    // Right
    ctx.draw(&CanvasLine {
        x1: right,
        y1: bottom,
        x2: right,
        y2: top,
        color,
    });
}

/// Generate a text summary of the chunk grid (used as a header line above the canvas).
pub fn chunk_summary_line(summary: &ArraySummary, theme: &Theme) -> Option<Line<'static>> {
    let zarr = ZarrMetadata::parse(&summary.zarr_metadata)?;
    let ndim = summary.shape.len();
    if ndim == 0 || zarr.chunk_shape.is_empty() || zarr.chunk_shape.len() != ndim {
        return None;
    }

    let chunks_per_dim: Vec<u64> = summary
        .shape
        .iter()
        .zip(zarr.chunk_shape.iter())
        .map(|(&s, &c)| if c == 0 { 1 } else { (s + c - 1) / c })
        .collect();

    let chunk_grid_str = chunks_per_dim
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\u{00d7}");
    let chunk_shape_str = zarr
        .chunk_shape
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\u{00d7}");

    Some(Line::from(vec![
        Span::styled("  Chunk grid:    ", theme.text_dim),
        Span::styled(
            format!("{chunk_grid_str} chunks of [{chunk_shape_str}]"),
            theme.text,
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary(
        shape: Vec<u64>,
        dim_names: Option<Vec<String>>,
        zarr_meta: &str,
    ) -> ArraySummary {
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
    fn test_scalar_returns_none() {
        let summary = make_summary(vec![], None, "{}");
        let theme = Theme::default();
        assert!(chunk_grid_canvas(&summary, &theme).is_none());
    }

    #[test]
    fn test_1d_returns_some() {
        let summary = make_summary(
            vec![561264],
            Some(vec!["time".into()]),
            &chunk_meta(&[80000]),
        );
        let theme = Theme::default();
        assert!(chunk_grid_canvas(&summary, &theme).is_some());
    }

    #[test]
    fn test_2d_returns_some() {
        let summary = make_summary(
            vec![721, 1440],
            Some(vec!["latitude".into(), "longitude".into()]),
            &chunk_meta(&[360, 360]),
        );
        let theme = Theme::default();
        assert!(chunk_grid_canvas(&summary, &theme).is_some());
    }

    #[test]
    fn test_3d_returns_some() {
        let summary = make_summary(
            vec![561264, 721, 1440],
            Some(vec!["time".into(), "latitude".into(), "longitude".into()]),
            &chunk_meta(&[80000, 360, 360]),
        );
        let theme = Theme::default();
        assert!(chunk_grid_canvas(&summary, &theme).is_some());
    }

    #[test]
    fn test_4d_returns_some() {
        let summary = make_summary(
            vec![10, 100, 721, 1440],
            Some(vec![
                "batch".into(),
                "time".into(),
                "lat".into(),
                "lon".into(),
            ]),
            &chunk_meta(&[10, 50, 360, 360]),
        );
        let theme = Theme::default();
        assert!(chunk_grid_canvas(&summary, &theme).is_some());
    }

    #[test]
    fn test_chunk_summary_line() {
        let summary = make_summary(
            vec![1000, 2000],
            Some(vec!["x".into(), "y".into()]),
            &chunk_meta(&[250, 500]),
        );
        let theme = Theme::default();
        let line = chunk_summary_line(&summary, &theme);
        assert!(line.is_some());
        let text = line.unwrap().to_string();
        assert!(text.contains("4\u{00d7}4"));
        assert!(text.contains("250\u{00d7}500"));
    }

    #[test]
    fn test_no_chunk_metadata() {
        let summary = make_summary(vec![100, 200], Some(vec!["x".into(), "y".into()]), "{}");
        let theme = Theme::default();
        // Should still return Some (canvas with default 1-chunk grid)
        assert!(chunk_grid_canvas(&summary, &theme).is_some());
    }
}
