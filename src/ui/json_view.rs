//! Pretty-print JSON with syntax highlighting for ratatui.
//! Designed for rendering untrusted metadata safely.
//!
//! All string values are sanitized before display. Large objects/arrays
//! are truncated after `max_entries`. Deep nesting is collapsed.

use ratatui::prelude::*;

use crate::sanitize::sanitize;
use crate::theme::Theme;

/// Render a JSON string as colored, indented Lines for display in a ratatui Paragraph.
///
/// - Object keys are colored with `theme.branch`
/// - String values are colored with `theme.added` (green), sanitized, and quoted
/// - Number values are colored with `theme.text`
/// - Boolean values are colored with `theme.modified` (yellow)
/// - Null is colored with `theme.text_dim`
/// - Indentation: 2 spaces per level
/// - After `max_entries` items in an object/array, shows "... and N more"
/// - After `max_depth` nesting, shows `{...}` or `[...]`
pub fn render_json(
    json_str: &str,
    theme: &Theme,
    max_depth: usize,
    max_entries: usize,
) -> Vec<Line<'static>> {
    let value: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            return vec![Line::from(Span::styled(
                format!("  Invalid JSON: {e}"),
                theme.error,
            ))];
        }
    };

    let mut lines = Vec::new();
    render_value(&value, theme, 1, max_depth, max_entries, &mut lines);
    lines
}

/// Recursively render a serde_json::Value into Lines.
/// `indent` is the current indentation level (each level = 2 spaces).
fn render_value(
    value: &serde_json::Value,
    theme: &Theme,
    indent: usize,
    max_depth: usize,
    max_entries: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let pad = "  ".repeat(indent);

    match value {
        serde_json::Value::Null => {
            lines.push(Line::from(Span::styled(format!("{pad}null"), theme.text_dim)));
        }
        serde_json::Value::Bool(b) => {
            lines.push(Line::from(Span::styled(
                format!("{pad}{b}"),
                theme.modified,
            )));
        }
        serde_json::Value::Number(n) => {
            lines.push(Line::from(Span::styled(
                format!("{pad}{n}"),
                theme.text,
            )));
        }
        serde_json::Value::String(s) => {
            let clean = sanitize(s);
            lines.push(Line::from(Span::styled(
                format!("{pad}\"{clean}\""),
                theme.added,
            )));
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                lines.push(Line::from(Span::styled(format!("{pad}[]"), theme.text_dim)));
                return;
            }
            if indent >= max_depth {
                lines.push(Line::from(Span::styled(
                    format!("{pad}[... {} items]", arr.len()),
                    theme.text_dim,
                )));
                return;
            }
            lines.push(Line::from(Span::styled(format!("{pad}["), theme.text_dim)));
            let show_count = arr.len().min(max_entries);
            for item in arr.iter().take(show_count) {
                render_value(item, theme, indent + 1, max_depth, max_entries, lines);
            }
            if arr.len() > max_entries {
                let inner_pad = "  ".repeat(indent + 1);
                lines.push(Line::from(Span::styled(
                    format!("{inner_pad}... and {} more", arr.len() - max_entries),
                    theme.text_dim,
                )));
            }
            lines.push(Line::from(Span::styled(format!("{pad}]"), theme.text_dim)));
        }
        serde_json::Value::Object(obj) => {
            if obj.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("{pad}{{}}"),
                    theme.text_dim,
                )));
                return;
            }
            if indent >= max_depth {
                lines.push(Line::from(Span::styled(
                    format!("{pad}{{... {} keys}}", obj.len()),
                    theme.text_dim,
                )));
                return;
            }
            lines.push(Line::from(Span::styled(format!("{pad}{{"), theme.text_dim)));
            let inner_pad = "  ".repeat(indent + 1);
            let show_count = obj.len().min(max_entries);
            for (key, val) in obj.iter().take(show_count) {
                let clean_key = sanitize(key);
                // For simple scalar values, render key: value on a single line
                match val {
                    serde_json::Value::Null => {
                        lines.push(Line::from(vec![
                            Span::styled(format!("{inner_pad}\"{clean_key}\": "), theme.branch),
                            Span::styled("null".to_string(), theme.text_dim),
                        ]));
                    }
                    serde_json::Value::Bool(b) => {
                        lines.push(Line::from(vec![
                            Span::styled(format!("{inner_pad}\"{clean_key}\": "), theme.branch),
                            Span::styled(b.to_string(), theme.modified),
                        ]));
                    }
                    serde_json::Value::Number(n) => {
                        lines.push(Line::from(vec![
                            Span::styled(format!("{inner_pad}\"{clean_key}\": "), theme.branch),
                            Span::styled(n.to_string(), theme.text),
                        ]));
                    }
                    serde_json::Value::String(s) => {
                        let clean_val = sanitize(s);
                        lines.push(Line::from(vec![
                            Span::styled(format!("{inner_pad}\"{clean_key}\": "), theme.branch),
                            Span::styled(format!("\"{clean_val}\""), theme.added),
                        ]));
                    }
                    // For nested objects/arrays, put the key on its own line then recurse
                    _ => {
                        lines.push(Line::from(Span::styled(
                            format!("{inner_pad}\"{clean_key}\":"),
                            theme.branch,
                        )));
                        render_value(val, theme, indent + 2, max_depth, max_entries, lines);
                    }
                }
            }
            if obj.len() > max_entries {
                lines.push(Line::from(Span::styled(
                    format!("{inner_pad}... and {} more", obj.len() - max_entries),
                    theme.text_dim,
                )));
            }
            lines.push(Line::from(Span::styled(format!("{pad}}}"), theme.text_dim)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn renders_simple_object() {
        let json = r#"{"key": "value", "num": 42}"#;
        let lines = render_json(json, &test_theme(), 10, 50);
        assert!(lines.len() >= 3); // { + 2 entries + }
    }

    #[test]
    fn sanitizes_string_values() {
        let json = r#"{"evil": "\u001b[31mred\u001b[0m"}"#;
        let lines = render_json(json, &test_theme(), 10, 50);
        // The rendered output should not contain ESC
        let text: String = lines.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.as_ref()).collect();
        assert!(!text.contains('\x1b'));
    }

    #[test]
    fn handles_invalid_json() {
        let lines = render_json("not json {{{", &test_theme(), 10, 50);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Invalid JSON"));
    }

    #[test]
    fn truncates_large_objects() {
        let mut obj = serde_json::Map::new();
        for i in 0..100 {
            obj.insert(format!("key_{i}"), serde_json::Value::Number(i.into()));
        }
        let json = serde_json::to_string(&serde_json::Value::Object(obj)).unwrap();
        let lines = render_json(&json, &test_theme(), 10, 5);
        let text: String = lines.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.as_ref()).collect();
        assert!(text.contains("... and 95 more"));
    }

    #[test]
    fn respects_max_depth() {
        let json = r#"{"a": {"b": {"c": {"d": "deep"}}}}"#;
        let lines = render_json(json, &test_theme(), 2, 50);
        let text: String = lines.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.as_ref()).collect();
        assert!(text.contains("{..."));
    }
}
