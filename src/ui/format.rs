//! Format zarr metadata for human-friendly display

use std::collections::BTreeMap;

/// Parse zarr metadata JSON and extract key fields for display
pub struct ZarrMetadata {
    pub data_type: String,
    pub chunk_shape: Vec<u64>,
    pub codecs: Vec<CodecEntry>,
    pub fill_value: String,
    pub dimension_separator: String,
    pub zarr_format: u32,
    /// Zarr v2 compressor field
    pub compressor: Option<String>,
    /// Zarr v2 memory layout order (C or F)
    pub order: Option<String>,
    /// Zarr v2 filters
    pub filters: Vec<String>,
    /// Zarr v2 dtype string
    pub v2_dtype: Option<String>,
    /// User-defined attributes
    pub attributes: BTreeMap<String, String>,
    /// Storage transformers (v3)
    pub storage_transformers: Vec<String>,
    /// Extra top-level keys we didn't specifically parse
    pub extra_fields: BTreeMap<String, String>,
}

/// A single codec with its name and configuration parameters
#[derive(Debug, Clone)]
pub struct CodecEntry {
    pub name: String,
    pub config: BTreeMap<String, String>,
}

impl CodecEntry {
    /// Display as `name(key=val, ...)` or just `name` if no config
    pub fn display(&self) -> String {
        if self.config.is_empty() {
            self.name.clone()
        } else {
            let params: Vec<String> = self
                .config
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            format!("{}({})", self.name, params.join(", "))
        }
    }
}

/// Format a serde_json::Value as a compact display string
fn value_display(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(value_display).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(obj) => {
            let items: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{k}: {}", value_display(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// Parse a v3 codec entry from JSON
fn parse_codec_entry(codec: &serde_json::Value) -> Option<CodecEntry> {
    let name = codec.get("name").and_then(|n| n.as_str())?.to_string();
    let mut config = BTreeMap::new();
    if let Some(conf) = codec.get("configuration").and_then(|c| c.as_object()) {
        for (k, v) in conf {
            config.insert(k.clone(), value_display(v));
        }
    }
    Some(CodecEntry { name, config })
}

/// Parse a v2 compressor object into a CodecEntry
fn parse_v2_compressor(comp: &serde_json::Value) -> Option<CodecEntry> {
    let obj = comp.as_object()?;
    let id = obj.get("id").and_then(|v| v.as_str())?.to_string();
    let mut config = BTreeMap::new();
    for (k, v) in obj {
        if k != "id" {
            config.insert(k.clone(), value_display(v));
        }
    }
    Some(CodecEntry { name: id, config })
}

/// Parse a v2 filter object into a display string
fn parse_v2_filter(filter: &serde_json::Value) -> String {
    if let Some(obj) = filter.as_object() {
        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let params: Vec<String> = obj
            .iter()
            .filter(|(k, _)| k.as_str() != "id")
            .map(|(k, v)| format!("{k}={}", value_display(v)))
            .collect();
        if params.is_empty() {
            id.to_string()
        } else {
            format!("{id}({})", params.join(", "))
        }
    } else {
        value_display(filter)
    }
}

/// Well-known top-level keys that we parse into specific fields
const KNOWN_KEYS: &[&str] = &[
    "zarr_format",
    "data_type",
    "dtype",
    "chunk_grid",
    "chunks",
    "codecs",
    "compressor",
    "fill_value",
    "dimension_separator",
    "order",
    "filters",
    "attributes",
    "storage_transformers",
    "shape",
    "node_type",
    "dimension_names",
];

impl ZarrMetadata {
    /// Parse from zarr metadata JSON string
    pub fn parse(json_str: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

        let zarr_format = v
            .get("zarr_format")
            .and_then(|z| z.as_u64())
            .unwrap_or(3) as u32;

        // Data type: v3 uses "data_type", v2 uses "dtype"
        let data_type = v
            .get("data_type")
            .and_then(|d| d.as_str())
            .or_else(|| v.get("dtype").and_then(|d| d.as_str()))
            .unwrap_or("unknown")
            .to_string();

        let v2_dtype = v
            .get("dtype")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());

        // Chunk shape: v3 uses chunk_grid.configuration.chunk_shape, v2 uses "chunks"
        let chunk_shape = v
            .get("chunk_grid")
            .and_then(|cg| cg.get("configuration"))
            .and_then(|c| c.get("chunk_shape"))
            .and_then(|cs| cs.as_array())
            .or_else(|| v.get("chunks").and_then(|c| c.as_array()))
            .map(|arr| arr.iter().filter_map(|val| val.as_u64()).collect())
            .unwrap_or_default();

        // Codecs: v3 format
        let codecs: Vec<CodecEntry> = v
            .get("codecs")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(parse_codec_entry).collect())
            .unwrap_or_default();

        // Compressor: v2 format
        let compressor = v
            .get("compressor")
            .and_then(|c| {
                if c.is_null() {
                    None
                } else {
                    parse_v2_compressor(c)
                }
            })
            .map(|entry| entry.display());

        let order = v
            .get("order")
            .and_then(|o| o.as_str())
            .map(|s| s.to_string());

        let filters: Vec<String> = v
            .get("filters")
            .and_then(|f| {
                if f.is_null() {
                    None
                } else {
                    f.as_array()
                }
            })
            .map(|arr| arr.iter().map(parse_v2_filter).collect())
            .unwrap_or_default();

        let fill_value = v
            .get("fill_value")
            .map(value_display)
            .unwrap_or_else(|| "null".to_string());

        let dimension_separator = v
            .get("dimension_separator")
            .and_then(|d| d.as_str())
            .unwrap_or("/")
            .to_string();

        // User-defined attributes
        let attributes: BTreeMap<String, String> = v
            .get("attributes")
            .and_then(|a| a.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), value_display(v)))
                    .collect()
            })
            .unwrap_or_default();

        // Storage transformers (v3)
        let storage_transformers: Vec<String> = v
            .get("storage_transformers")
            .and_then(|st| st.as_array())
            .map(|arr| arr.iter().map(value_display).collect())
            .unwrap_or_default();

        // Collect any extra top-level keys we didn't specifically handle
        let extra_fields: BTreeMap<String, String> = v
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter(|(k, _)| !KNOWN_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), value_display(v)))
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            data_type,
            chunk_shape,
            codecs,
            fill_value,
            dimension_separator,
            zarr_format,
            compressor,
            order,
            filters,
            v2_dtype,
            attributes,
            storage_transformers,
            extra_fields,
        })
    }

    /// Display the full codec pipeline as a human-friendly string.
    /// Handles both v3 (codecs array) and v2 (compressor field) formats.
    pub fn codec_chain_display(&self) -> String {
        if !self.codecs.is_empty() {
            // v3 style
            self.codecs
                .iter()
                .map(|c| c.display())
                .collect::<Vec<_>>()
                .join(" \u{2192} ")
        } else if let Some(ref comp) = self.compressor {
            // v2 style: compressor is the main codec
            comp.clone()
        } else {
            String::new()
        }
    }
}
