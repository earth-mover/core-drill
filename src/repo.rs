use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::Result;
use icechunk::Repository;
use icechunk::storage::{
    S3Credentials, S3Options, new_azure_blob_storage, new_gcs_storage, new_http_storage,
    new_s3_storage,
};
use tracing::info;
use url::Url;

/// CLI overrides for storage configuration
pub struct StorageOverrides {
    pub region: Option<String>,
    pub endpoint_url: Option<String>,
    pub anonymous: bool,
}

/// Open an Icechunk repository from a path or URL.
///
/// Detects the storage backend from the URL scheme. For S3 without
/// explicit `?anonymous=true`, tries environment credentials first
/// then falls back to anonymous access.
pub async fn open(path_or_url: &str, overrides: &StorageOverrides) -> Result<Repository> {
    // Try parsing as a URL; fall back to local filesystem for plain paths
    let url = match Url::parse(path_or_url) {
        Ok(u) if u.scheme() == "file" || u.scheme().len() == 1 => {
            return open_local(path_or_url).await;
        }
        Ok(u) => u,
        Err(_) => return open_local(path_or_url).await,
    };

    match url.scheme() {
        "s3" => open_s3(&url, overrides).await,
        "gs" | "gcs" => open_simple(create_gcs_storage(&url)?).await,
        "az" | "azure" => open_simple(create_azure_storage(&url).await?).await,
        "http" | "https" => open_simple(create_http_storage(&url)?).await,
        scheme => color_eyre::eyre::bail!("Unsupported URL scheme: {scheme}://"),
    }
}

async fn open_simple(
    storage: Arc<dyn icechunk::Storage + Send + Sync>,
) -> Result<Repository> {
    Ok(Repository::open(None, storage, HashMap::new()).await?)
}

// ── S3 ──────────────────────────────────────────────────────────────

/// S3: tries env credentials, falls back to anonymous, reports both errors.
async fn open_s3(url: &Url, overrides: &StorageOverrides) -> Result<Repository> {
    let (bucket, prefix) = host_and_prefix(url)?;
    let params = query_params(url);

    let config = S3Options {
        region: Some(
            overrides
                .region
                .clone()
                .or_else(|| params.get("region").cloned())
                .unwrap_or_else(|| "us-east-1".to_string()),
        ),
        endpoint_url: overrides
            .endpoint_url
            .clone()
            .or_else(|| params.get("endpoint_url").cloned()),
        anonymous: false,
        allow_http: parse_bool(&params, "allow_http").unwrap_or(false),
        force_path_style: parse_bool(&params, "force_path_style").unwrap_or(false),
        network_stream_timeout_seconds: None,
        requester_pays: parse_bool(&params, "requester_pays").unwrap_or(false),
    };

    // Explicit anonymous (--anon flag or ?anonymous=true) — skip credential probing
    if overrides.anonymous || parse_bool(&params, "anonymous") == Some(true) {
        let mut anon_config = config;
        anon_config.anonymous = true;
        let storage =
            new_s3_storage(anon_config, bucket, prefix, Some(S3Credentials::Anonymous))?;
        return Ok(Repository::open(None, storage, HashMap::new()).await?);
    }

    // Try environment credentials first
    let env_storage = new_s3_storage(config.clone(), bucket.clone(), prefix.clone(), None)?;
    match Repository::open(None, env_storage, HashMap::new()).await {
        Ok(repo) => Ok(repo),
        Err(env_err) => {
            info!("Environment credentials failed, trying anonymous access");
            let mut anon_config = config;
            anon_config.anonymous = true;
            let anon_storage = new_s3_storage(
                anon_config,
                bucket.clone(),
                prefix.clone(),
                Some(S3Credentials::Anonymous),
            )?;
            match Repository::open(None, anon_storage, HashMap::new()).await {
                Ok(repo) => {
                    info!("Opened with anonymous access (environment credentials failed)");
                    Ok(repo)
                }
                Err(anon_err) => {
                    let path_suffix =
                        prefix.as_deref().map(|p| format!("/{p}")).unwrap_or_default();
                    color_eyre::eyre::bail!(
                        "Could not open s3://{bucket}{path_suffix}\n\
                         \n  With environment credentials: {env_err}\
                         \n  With anonymous access: {anon_err}\
                         \n\nHints:\
                         \n  - Set AWS_REGION or pass --region (required if not in AWS environment)\
                         \n  - For public repos: s3://{bucket}{path_suffix}?anonymous=true\
                         \n  - Check bucket name and prefix are correct",
                    );
                }
            }
        }
    }
}

// ── GCS ─────────────────────────────────────────────────────────────

fn create_gcs_storage(url: &Url) -> Result<Arc<dyn icechunk::Storage + Send + Sync>> {
    let (bucket, prefix) = host_and_prefix(url)?;
    let params = query_params(url);

    let credentials = if parse_bool(&params, "anonymous") == Some(true) {
        Some(icechunk::storage::GcsCredentials::Anonymous)
    } else {
        None
    };

    let config: Option<HashMap<String, String>> = if params.is_empty() {
        None
    } else {
        Some(params)
    };

    Ok(new_gcs_storage(bucket, prefix, credentials, config)?)
}

// ── Azure ───────────────────────────────────────────────────────────

/// az://account/container/prefix
async fn create_azure_storage(
    url: &Url,
) -> Result<Arc<dyn icechunk::Storage + Send + Sync>> {
    let params = query_params(url);

    let account = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "Azure URL missing account: az://<account>/<container>[/<prefix>]"
            )
        })?
        .to_string();

    let path = url.path().trim_start_matches('/').trim_end_matches('/');
    let (container, prefix) = match path.find('/') {
        Some(pos) => {
            let pfx = path[pos + 1..].trim_end_matches('/');
            (
                path[..pos].to_string(),
                if pfx.is_empty() { None } else { Some(pfx.to_string()) },
            )
        }
        None if !path.is_empty() => (path.to_string(), None),
        _ => {
            color_eyre::eyre::bail!(
                "Azure URL missing container: az://<account>/<container>[/<prefix>]"
            );
        }
    };

    let config: Option<HashMap<String, String>> = if params.is_empty() {
        None
    } else {
        Some(params)
    };

    Ok(new_azure_blob_storage(account, container, prefix, None, config).await?)
}

// ── HTTP ────────────────────────────────────────────────────────────

fn create_http_storage(url: &Url) -> Result<Arc<dyn icechunk::Storage + Send + Sync>> {
    Ok(new_http_storage(url.as_str(), None)?)
}

// ── Local ───────────────────────────────────────────────────────────

async fn open_local(path_or_url: &str) -> Result<Repository> {
    let path = PathBuf::from(path_or_url)
        .canonicalize()
        .map_err(|e| color_eyre::eyre::eyre!("Invalid repo path '{}': {}", path_or_url, e))?;
    let storage = icechunk::new_local_filesystem_storage(&path).await?;
    Ok(Repository::open(None, storage, HashMap::new()).await?)
}

// ── Helpers ─────────────────────────────────────────────────────────

fn query_params(url: &Url) -> HashMap<String, String> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn host_and_prefix(url: &Url) -> Result<(String, Option<String>)> {
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| color_eyre::eyre::eyre!("URL missing host/bucket: {url}"))?
        .to_string();

    let path = url.path().trim_start_matches('/').trim_end_matches('/');
    let prefix = if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    };

    Ok((host, prefix))
}

fn parse_bool(params: &HashMap<String, String>, key: &str) -> Option<bool> {
    params.get(key).map(|v| v == "true")
}
