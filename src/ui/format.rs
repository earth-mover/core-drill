//! Format zarr metadata for human-friendly display

/// Parse zarr metadata JSON and extract key fields for display
pub struct ZarrMetadata {
    pub data_type: String,
    pub chunk_shape: Vec<u64>,
    pub codecs: Vec<String>,
    pub fill_value: String,
    pub dimension_separator: String,
    pub zarr_format: u32,
}

impl ZarrMetadata {
    /// Parse from zarr metadata JSON string
    pub fn parse(json_str: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

        let data_type = v
            .get("data_type")
            .and_then(|d| d.as_str())
            .unwrap_or("unknown")
            .to_string();

        let chunk_shape = v
            .get("chunk_grid")
            .and_then(|cg| cg.get("configuration"))
            .and_then(|c| c.get("chunk_shape"))
            .and_then(|cs| cs.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default();

        let codecs = v
            .get("codecs")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|codec| {
                        codec
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let fill_value = v
            .get("fill_value")
            .map(|fv| fv.to_string())
            .unwrap_or_else(|| "null".to_string());

        let dimension_separator = v
            .get("dimension_separator")
            .and_then(|d| d.as_str())
            .unwrap_or("/")
            .to_string();

        let zarr_format = v
            .get("zarr_format")
            .and_then(|z| z.as_u64())
            .unwrap_or(3) as u32;

        Some(Self {
            data_type,
            chunk_shape,
            codecs,
            fill_value,
            dimension_separator,
            zarr_format,
        })
    }
}
