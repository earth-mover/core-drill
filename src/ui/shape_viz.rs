//! Chunk grid text summary line for the array detail pane.

use ratatui::prelude::*;

use crate::store::types::ArraySummary;
use crate::theme::Theme;

/// Generate a text summary of the chunk grid (used as a header line above the canvas).
pub fn chunk_summary_line(summary: &ArraySummary, theme: &Theme) -> Option<Line<'static>> {
    let zarr = summary.parsed_metadata.as_ref()?;
    let ndim = summary.shape.len();
    if ndim == 0 || zarr.chunk_shape.is_empty() || zarr.chunk_shape.len() != ndim {
        return None;
    }

    let chunks_per_dim: Vec<u64> = summary
        .shape
        .iter()
        .zip(zarr.chunk_shape.iter())
        .map(|(&s, &c)| if c == 0 { 1 } else { s.div_ceil(c) })
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
        let parsed_metadata = crate::fetch::ZarrMetadata::parse(zarr_meta);
        ArraySummary {
            shape,
            dimension_names: dim_names,
            manifest_count: 1,
            zarr_metadata: zarr_meta.to_string(),
            total_chunks: None,
            parsed_metadata,
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
        // No chunk metadata → returns None
        assert!(chunk_summary_line(&summary, &theme).is_none());
    }

    #[test]
    fn test_scalar_returns_none() {
        let summary = make_summary(vec![], None, "{}");
        let theme = Theme::default();
        assert!(chunk_summary_line(&summary, &theme).is_none());
    }
}
