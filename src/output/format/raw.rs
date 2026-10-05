//! Markdown for raw object inspection: decoded flatbuffer trees, chunk-ref
//! lookups, and per-commit chunk changes.

use humansize::{BINARY, format_size};

use crate::fetch::raw::{ChangeKind, ChunkChanges, ChunkLocation, ChunkRefLookup};
use crate::raw::{Decoded, RawObject, RawValue, hex};

/// Decoded JSON longer than this is cut in tree view; select the field with
/// `--at` (or use JSON output) to see all of it.
const INLINE_JSON_CHARS: usize = 200;

pub(crate) fn fmt_raw_object(obj: &RawObject) -> String {
    let mut out = format!("# {}", obj.path);
    if let RawValue::Table { type_name, .. } = &obj.value
        && obj.selector.is_none()
    {
        out.push_str(&format!(" — {type_name}"));
    }
    out.push_str("\n\n");

    match &obj.header {
        Some(h) => {
            out.push_str(&format!(
                "- **Header:** spec v{} · {} · {} compression · written by `{}`\n",
                h.spec_version, h.file_type_name, h.compression_name, h.implementation
            ));
            out.push_str(&format!(
                "- **Size:** {} stored, {} decoded\n",
                format_size(obj.stored_bytes, BINARY),
                format_size(obj.decoded_bytes.unwrap_or(0), BINARY)
            ));
            match &obj.verify_error {
                None => out.push_str("- **Verification:** passed\n"),
                Some(e) => out.push_str(&format!(
                    "- **Verification:** FAILED ({e}); fields below were read with bounds checks only\n"
                )),
            }
        }
        None => out.push_str(&format!(
            "- **Size:** {} (no Icechunk header: raw chunk data)\n",
            format_size(obj.stored_bytes, BINARY)
        )),
    }
    if let Some(sel) = &obj.selector {
        out.push_str(&format!("- **Showing:** `{sel}`\n"));
    }

    out.push_str("\n```text\n");
    let base = &obj.window_base;
    match &obj.value {
        RawValue::Table { fields, .. } => write_fields(&mut out, fields, 0, base),
        RawValue::Vector { .. } => write_vector_items(&mut out, &obj.value, 0, base),
        RawValue::Bytes {
            decoded: Some(Decoded::Json(v)),
            len,
            ..
        } => {
            out.push_str(&format!("<{len} bytes>, decoded:\n"));
            out.push_str(&serde_json::to_string_pretty(v).unwrap_or_default());
            out.push('\n');
        }
        other => {
            out.push_str(&inline(other).unwrap_or_default());
            out.push('\n');
        }
    }
    out.push_str("```\n");
    out.push_str(
        "\n*Follow a `→ dir/ID` link by passing `dir/ID` as the target. Narrow to a field or a vector window with `--at` (MCP: `at`), e.g. `nodes/0` or `arrays/0/refs/100..150`.*",
    );
    out
}

fn child_path(base: &str, step: &str) -> String {
    if base.is_empty() {
        step.to_string()
    } else {
        format!("{base}/{step}")
    }
}

fn write_fields(out: &mut String, fields: &[(String, RawValue)], indent: usize, base: &str) {
    for (name, value) in fields {
        write_entry(out, name, value, indent, &child_path(base, name));
    }
}

fn write_entry(out: &mut String, label: &str, value: &RawValue, indent: usize, path: &str) {
    let pad = " ".repeat(indent);
    if let Some(text) = inline(value) {
        out.push_str(&format!("{pad}{label}: {text}\n"));
        return;
    }
    match value {
        RawValue::Table { type_name, fields } => {
            out.push_str(&format!("{pad}{label}: {type_name}\n"));
            write_fields(out, fields, indent + 2, path);
        }
        RawValue::Vector { len, .. } => {
            let noun = if *len == 1 { "item" } else { "items" };
            out.push_str(&format!("{pad}{label}: {len} {noun}\n"));
            write_vector_items(out, value, indent + 2, path);
        }
        _ => {}
    }
}

fn write_vector_items(out: &mut String, value: &RawValue, indent: usize, path: &str) {
    let RawValue::Vector { len, start, items } = value else {
        return;
    };
    let pad = " ".repeat(indent);
    if *start > 0 {
        out.push_str(&format!("{pad}… {start} earlier\n"));
    }
    for (i, item) in items.iter().enumerate() {
        let idx = start + i;
        let item_path = child_path(path, &idx.to_string());
        match (item, inline(item)) {
            (RawValue::Table { type_name, fields }, None) => {
                out.push_str(&format!("{pad}[{idx}] {type_name}\n"));
                write_fields(out, fields, indent + 2, &item_path);
            }
            (_, Some(text)) => out.push_str(&format!("{pad}[{idx}]: {text}\n")),
            (other, None) => write_entry(out, &format!("[{idx}]"), other, indent, &item_path),
        }
    }
    let end = start + items.len();
    if end < *len {
        let window = items.len().max(1);
        out.push_str(&format!(
            "{pad}… {} more (--at {}/{}..{})\n",
            len - end,
            path,
            end,
            (end + window).min(*len)
        ));
    }
}

/// Single-line rendering, or `None` for values that need nested lines.
fn inline(value: &RawValue) -> Option<String> {
    Some(match value {
        RawValue::Bool(b) => b.to_string(),
        RawValue::Int(i) => i.to_string(),
        RawValue::UInt(u) => u.to_string(),
        RawValue::Float(f) => f.to_string(),
        RawValue::Str(s) => quote(s),
        RawValue::Enum {
            name: Some(n),
            value,
        } => format!("{n} ({value})"),
        RawValue::Enum { name: None, value } => format!("{value} (unknown variant)"),
        RawValue::Time { raw, iso } => format!("{raw} ({iso})"),
        RawValue::Id { id, links } if links.is_empty() => id.clone(),
        RawValue::Id { links, .. } => format!("→ {}", links.join(", ")),
        RawValue::NodeId { id, path: Some(p) } => format!("{id} ({p})"),
        RawValue::NodeId { id, path: None } => id.clone(),
        RawValue::Bytes {
            len,
            preview,
            decoded,
        } => {
            let more = if preview.len() < *len { "…" } else { "" };
            let raw = if *len == 0 {
                "<0 bytes>".to_string()
            } else {
                format!("<{len} bytes> {}{more}", hex(preview))
            };
            match decoded {
                Some(Decoded::Text(t)) => format!("{} <{len} bytes>", quote(t)),
                Some(Decoded::Json(v)) => {
                    let json = v.to_string();
                    let cut = super::truncate(&json, INLINE_JSON_CHARS);
                    let more = if cut.len() < json.len() { "…" } else { "" };
                    format!("{cut}{more} <{len} bytes>")
                }
                None => raw,
            }
        }
        RawValue::Scalars(items) => format!(
            "[{}]",
            items
                .iter()
                .filter_map(inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        RawValue::Table { type_name, fields } if fields.is_empty() => format!("{type_name} {{}}"),
        RawValue::Table { fields, .. }
            if fields.len() <= 3 && fields.iter().all(|(_, v)| is_number(v)) =>
        {
            let parts: Vec<String> = fields
                .iter()
                .filter_map(|(k, v)| inline(v).map(|t| format!("{k}: {t}")))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        RawValue::Table { .. } | RawValue::Vector { .. } => return None,
    })
}

fn is_number(v: &RawValue) -> bool {
    matches!(
        v,
        RawValue::Int(_) | RawValue::UInt(_) | RawValue::Float(_) | RawValue::Scalars(_)
    )
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn fmt_coords(coords: &[u32]) -> String {
    let parts: Vec<String> = coords.iter().map(u32::to_string).collect();
    format!("({})", parts.join(", "))
}

/// One-line description of a decoded `ChunkRef` table.
fn ref_summary(chunk_ref: &RawValue) -> String {
    if let Some(RawValue::Bytes { len, .. }) = chunk_ref.field("inline") {
        return format!("inline, {len} bytes");
    }
    let offset = chunk_ref
        .field("offset")
        .and_then(inline)
        .unwrap_or_else(|| "0".into());
    let length = chunk_ref
        .field("length")
        .and_then(inline)
        .unwrap_or_else(|| "0".into());
    let span = format!("{length} bytes at offset {offset}");
    if let Some(RawValue::Id { id, .. }) = chunk_ref.field("chunk_id") {
        return format!("native chunk `{id}`, {span}");
    }
    let location = match (
        chunk_ref.field("location"),
        chunk_ref.field("compressed_location"),
    ) {
        (Some(RawValue::Str(s)), _) => s.clone(),
        (
            _,
            Some(RawValue::Bytes {
                decoded: Some(Decoded::Text(t)),
                ..
            }),
        ) => t.clone(),
        _ => "<unknown location>".to_string(),
    };
    let checksum = match (
        chunk_ref.field("checksum_etag"),
        chunk_ref.field("checksum_last_modified"),
    ) {
        (Some(RawValue::Str(e)), _) => format!(", ETag {e}"),
        (_, Some(RawValue::Time { iso, .. })) => format!(", last-modified {iso}"),
        _ => ", no checksum".to_string(),
    };
    format!("virtual `{location}`, {span}{checksum}")
}

fn location_line(loc: &ChunkLocation) -> String {
    format!(
        "{} — in `manifests/{}` at `{}`",
        ref_summary(&loc.chunk_ref),
        loc.manifest,
        loc.at
    )
}

pub(crate) fn fmt_chunk_ref(lookup: &ChunkRefLookup) -> String {
    let mut out = format!(
        "# Chunk {} of {}\n\n- **Snapshot:** {}\n- **Node ID:** {}\n",
        fmt_coords(&lookup.coords),
        lookup.path,
        lookup.snapshot,
        lookup.node_id
    );
    match &lookup.location {
        None => out.push_str(
            "\nNo manifest holds a ref for these coordinates, so the chunk reads as the array's fill value.\n",
        ),
        Some(loc) => {
            out.push_str(&format!(
                "- **Manifest:** `manifests/{}` at `{}`\n- **Ref:** {}\n\n```text\n",
                loc.manifest,
                loc.at,
                ref_summary(&loc.chunk_ref)
            ));
            if let RawValue::Table { fields, .. } = &loc.chunk_ref {
                write_fields(&mut out, fields, 0, &loc.at);
            }
            out.push_str("```\n");
        }
    }
    out
}

pub(crate) fn fmt_chunk_changes(changes: &ChunkChanges) -> String {
    let mut out = format!("# Chunk changes in {}\n\n", changes.snapshot);
    out.push_str(&format!("- **Message:** {}\n", changes.message));
    out.push_str(&format!(
        "- **Parent:** {}\n\n",
        changes
            .parent
            .as_deref()
            .unwrap_or("(none, initial commit)")
    ));
    if changes.arrays.is_empty() {
        out.push_str("The transaction log lists no chunk changes.\n");
        return out;
    }
    for array in &changes.arrays {
        let path = array.path.as_deref().unwrap_or("(unknown path)");
        let noun = if array.listed == 1 {
            "coordinate"
        } else {
            "coordinates"
        };
        out.push_str(&format!("## {path} — {} {noun} listed\n\n", array.listed));
        for change in &array.changes {
            let kind = match change.kind {
                ChangeKind::Added => "added",
                ChangeKind::Overwritten => "overwritten",
                ChangeKind::Rewritten => "rewritten (identical ref)",
                ChangeKind::Deleted => "deleted",
                ChangeKind::Absent => "absent before and after",
            };
            out.push_str(&format!("- {} **{kind}**\n", fmt_coords(&change.coords)));
            if let Some(b) = &change.before {
                out.push_str(&format!("  - before: {}\n", location_line(b)));
            }
            if let Some(a) = &change.after {
                out.push_str(&format!("  - after: {}\n", location_line(a)));
            }
        }
        if array.changes.len() < array.listed {
            out.push_str(&format!(
                "- … {} more not classified (raise --limit)\n",
                array.listed - array.changes.len()
            ));
        }
        out.push('\n');
    }
    if changes.truncated {
        out.push_str("*Stopped at --limit; raise it to classify the rest.*\n");
    }
    out
}
