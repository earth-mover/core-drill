use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::{Result, eyre::bail};
use icechunk::Repository;

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
    if path_or_url.starts_with("s3://") {
        bail!("S3 storage support coming in Phase 2. Path: {}", path_or_url);
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
