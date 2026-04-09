use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::{Result, eyre::bail};
use icechunk::Repository;
use icechunk::storage::{S3Credentials, S3Options, new_s3_storage};

/// CLI overrides for storage configuration
pub struct StorageOverrides {
    pub region: Option<String>,
    pub endpoint_url: Option<String>,
}

/// Open an Icechunk repository from a path or URL.
///
/// Detects the storage backend from the path prefix and creates
/// the appropriate storage configuration. CLI overrides take
/// precedence over URL query params.
pub async fn open(path_or_url: &str, overrides: &StorageOverrides) -> Result<Repository> {
    let storage = create_storage(path_or_url, overrides).await?;
    let repo = Repository::open(None, storage, HashMap::new()).await?;
    Ok(repo)
}

async fn create_storage(
    path_or_url: &str,
    overrides: &StorageOverrides,
) -> Result<Arc<dyn icechunk::Storage + Send + Sync>> {
    if let Some(without_scheme) = path_or_url.strip_prefix("s3://") {
        // Parse query params (e.g., ?region=us-east-1&endpoint_url=...)
        let (path_part, query_params) = parse_query_params(without_scheme);

        let (bucket, prefix) = match path_part.find('/') {
            Some(pos) => (
                path_part[..pos].to_string(),
                Some(path_part[pos + 1..].trim_end_matches('/').to_string()),
            ),
            None => (path_part.to_string(), None),
        };

        // CLI overrides > URL query params > icechunk defaults
        let config = S3Options {
            region: overrides
                .region
                .clone()
                .or_else(|| query_params.get("region").cloned()),
            endpoint_url: overrides
                .endpoint_url
                .clone()
                .or_else(|| query_params.get("endpoint_url").cloned()),
            anonymous: query_params
                .get("anonymous")
                .map(|v| v == "true")
                .unwrap_or(true),
            allow_http: query_params
                .get("allow_http")
                .map(|v| v == "true")
                .unwrap_or(false),
            force_path_style: query_params
                .get("force_path_style")
                .map(|v| v == "true")
                .unwrap_or(false),
            network_stream_timeout_seconds: None,
            requester_pays: false,
        };

        let credentials = if config.anonymous {
            Some(S3Credentials::Anonymous)
        } else {
            None // Let icechunk/AWS SDK resolve credentials from environment
        };

        let storage = new_s3_storage(config, bucket, prefix, credentials)?;
        Ok(storage)
    } else if path_or_url.starts_with("gs://") {
        bail!(
            "GCS storage support coming in Phase 2. Path: {}",
            path_or_url
        );
    } else if path_or_url.starts_with("az://") || path_or_url.starts_with("azure://") {
        bail!(
            "Azure storage support coming in Phase 2. Path: {}",
            path_or_url
        );
    } else if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        bail!(
            "HTTP storage support coming in Phase 2. URL: {}",
            path_or_url
        );
    } else {
        // Local filesystem
        let path = PathBuf::from(path_or_url)
            .canonicalize()
            .map_err(|e| color_eyre::eyre::eyre!("Invalid repo path '{}': {}", path_or_url, e))?;

        let storage = icechunk::new_local_filesystem_storage(&path).await?;
        Ok(storage)
    }
}

/// Parse query parameters from a URL path portion.
/// e.g., "bucket/prefix?region=us-east-1&foo=bar" → ("bucket/prefix", {region: us-east-1, foo: bar})
fn parse_query_params(path: &str) -> (&str, HashMap<String, String>) {
    match path.find('?') {
        Some(pos) => {
            let path_part = &path[..pos];
            let query = &path[pos + 1..];
            let params = query
                .split('&')
                .filter_map(|pair| {
                    let mut parts = pair.splitn(2, '=');
                    let key = parts.next()?.to_string();
                    let value = parts.next().unwrap_or("").to_string();
                    Some((key, value))
                })
                .collect();
            (path_part, params)
        }
        None => (path, HashMap::new()),
    }
}
