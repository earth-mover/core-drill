use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::{Result, eyre::bail};
use icechunk::Repository;
use icechunk::storage::{S3Credentials, S3Options, new_s3_storage};

/// Open an Icechunk repository from a path or URL.
///
/// Detects the storage backend from the path prefix and creates
/// the appropriate storage configuration.
pub async fn open(path_or_url: &str) -> Result<Repository> {
    let storage = create_storage(path_or_url).await?;
    let repo = Repository::open(None, storage, HashMap::new()).await?;
    Ok(repo)
}

async fn create_storage(
    path_or_url: &str,
) -> Result<Arc<dyn icechunk::Storage + Send + Sync>> {
    if let Some(without_scheme) = path_or_url.strip_prefix("s3://") {
        let (bucket, prefix) = match without_scheme.find('/') {
            Some(pos) => (
                without_scheme[..pos].to_string(),
                Some(without_scheme[pos + 1..].trim_end_matches('/').to_string()),
            ),
            None => (without_scheme.to_string(), None),
        };

        // Use icechunk defaults — don't hardcode region or other S3 config.
        // Anonymous access for public repos; authenticated access will be
        // handled via interactive dialog in the future.
        let config = S3Options {
            anonymous: true,
            region: None,
            endpoint_url: None,
            allow_http: false,
            force_path_style: false,
            network_stream_timeout_seconds: None,
            requester_pays: false,
        };

        let storage = new_s3_storage(
            config,
            bucket,
            prefix,
            Some(S3Credentials::Anonymous),
        )?;
        Ok(storage)
    } else if path_or_url.starts_with("gs://") {
        bail!("GCS storage support coming in Phase 2. Path: {}", path_or_url);
    } else if path_or_url.starts_with("az://") || path_or_url.starts_with("azure://") {
        bail!("Azure storage support coming in Phase 2. Path: {}", path_or_url);
    } else if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        bail!("HTTP storage support coming in Phase 2. URL: {}", path_or_url);
    } else {
        // Local filesystem
        let path = PathBuf::from(path_or_url)
            .canonicalize()
            .map_err(|e| color_eyre::eyre::eyre!("Invalid repo path '{}': {}", path_or_url, e))?;

        let storage = icechunk::new_local_filesystem_storage(&path).await?;
        Ok(storage)
    }
}
