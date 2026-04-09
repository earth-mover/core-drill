mod app;
mod cli;
mod component;
mod mcp;
mod multiplexer;
mod output;
mod repo;
pub mod sanitize;
pub mod search;
mod store;
mod theme;
mod tui;
mod ui;

use std::sync::Arc;

use clap::Parser;
use cli::Cli;
use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Clean error display: no backtrace unless RUST_BACKTRACE is set
    color_eyre::config::HookBuilder::default()
        .display_env_section(false)
        .install()?;

    let cli = Cli::parse();

    // Suppress icechunk's internal ERROR logs by default — they fire during
    // normal shutdown when manifest pre-load tasks are cancelled, and look
    // like failures to users. RUST_LOG can still override for debugging.
    let env_filter = tracing_subscriber::EnvFilter::new(
        std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,icechunk=off".to_string()),
    );
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();

    // Open the repository
    let (repository, repo_id) = if looks_like_arraylake_ref(&cli.repo) {
        open_via_arraylake(&cli.repo, &cli.arraylake_api).await?
    } else {
        let overrides = repo::StorageOverrides {
            region: cli.region.clone(),
            endpoint_url: cli.endpoint_url.clone(),
        };
        let repo = repo::open(&cli.repo, &overrides).await?;
        let identity = if cli.repo.contains("://") {
            app::RepoIdentity::S3 {
                url: cli.repo.clone(),
            }
        } else {
            app::RepoIdentity::Local {
                path: cli.repo.clone(),
            }
        };
        (repo, identity)
    };

    let display_label = repo_id.display_short();

    if cli.serve {
        mcp::serve(repository, display_label.clone()).await?;
    } else if cli.repl {
        let format = cli.output.unwrap_or(cli::OutputFormat::Md);
        output::run_repl(repository, format, &display_label).await?;
    } else if let Some(format) = cli.output {
        output::run(repository, format, cli.command, &display_label).await?;
    } else {
        let data_store = store::DataStore::new(repository);
        let mut app = app::App::new(data_store, repo_id);
        app.load_initial_data();
        tui::run(app).await?;
    }

    Ok(())
}

/// Detect if a repo string is an Arraylake reference.
/// Explicit: `al:org/repo`. Implicit: `org/repo` that doesn't exist on disk.
fn looks_like_arraylake_ref(s: &str) -> bool {
    s.starts_with("al:")
}

/// Open a repo via Arraylake, handling credentials automatically.
/// Reads the OAuth token from ~/.arraylake/token.json.
async fn open_via_arraylake(
    al_ref: &str,
    api_url: &str,
) -> Result<(icechunk::Repository, app::RepoIdentity)> {
    let ref_str = al_ref.strip_prefix("al:").unwrap_or(al_ref);
    let (org, repo_name) = ref_str.split_once('/').ok_or_else(|| {
        color_eyre::eyre::eyre!("Invalid Arraylake ref: expected 'al:org/repo', got '{al_ref}'")
    })?;

    // Read token from ~/.arraylake/token.json
    let home = std::env::var("HOME")
        .map_err(|_| color_eyre::eyre::eyre!("Cannot determine home directory"))?;
    let token_path = std::path::PathBuf::from(home).join(".arraylake/token.json");

    let token_json = std::fs::read_to_string(&token_path).map_err(|e| {
        color_eyre::eyre::eyre!(
            "Cannot read Arraylake token at {}: {}. Run `arraylake auth login` first.",
            token_path.display(),
            e
        )
    })?;

    let token_data: serde_json::Value = serde_json::from_str(&token_json)?;
    let id_token = token_data["id_token"]
        .as_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("No id_token in Arraylake token file"))?;

    let client = Arc::new(
        arraylake::ALClient::new(Some(api_url.to_string()), id_token.to_string())
            .map_err(|e| color_eyre::eyre::eyre!("Failed to create Arraylake client: {e}"))?,
    );

    // Fetch repo info first to get bucket details for display
    // Verify auth first with a quick user check
    if let Err(e) = client.get_current_user().await {
        color_eyre::eyre::bail!(
            "Arraylake authentication failed ({api_url}):\n  {e}\n\n\
             Run `arraylake auth login` to refresh your token."
        );
    }

    let repo_info = client.get_repo_info(org, repo_name).await.map_err(|e| {
        color_eyre::eyre::eyre!(
            "Repo '{ref_str}' not found on {api_url}:\n  {e}\n\n\
             Check the org/repo name, or verify access with `arraylake repo list {org}`."
        )
    })?;

    // Extract bucket metadata once — used for display and the identity struct
    let bucket_name = repo_info
        .bucket
        .as_ref()
        .map(|b| b.name.as_str())
        .unwrap_or("?");

    let region = repo_info
        .bucket
        .as_ref()
        .and_then(|b| {
            b.extra_config.get("region_name").map(|v| match v {
                arraylake::ALBucketExtraConfigValue::S(s) => s.clone(),
                arraylake::ALBucketExtraConfigValue::B(_) => "?".to_string(),
            })
        })
        .unwrap_or_else(|| "?".to_string());

    let platform = repo_info
        .bucket
        .as_ref()
        .map(|b| {
            let endpoint = b.extra_config.get("endpoint_url").and_then(|v| match v {
                arraylake::ALBucketExtraConfigValue::S(s) => Some(s.as_str()),
                _ => None,
            });
            let is_r2 = endpoint.is_some_and(|u| u.contains(".r2."));
            let is_tigris =
                endpoint.is_some_and(|u| u.contains("tigris.dev") || u.contains("t3.storage.dev"));
            match b.platform {
                arraylake::ALBucketPlatform::S3 => "S3".to_string(),
                arraylake::ALBucketPlatform::S3Compatible if is_r2 => "Cloudflare R2".to_string(),
                arraylake::ALBucketPlatform::S3Compatible if is_tigris => "Tigris".to_string(),
                arraylake::ALBucketPlatform::S3Compatible => "S3-compatible".to_string(),
                arraylake::ALBucketPlatform::Minio => "MinIO".to_string(),
                arraylake::ALBucketPlatform::GS => "GCS".to_string(),
            }
        })
        .unwrap_or_else(|| "?".to_string());

    eprintln!("Arraylake: {org}/{repo_name}  →  {bucket_name} ({platform}, {region})");

    let storage = client
        .get_storage_for_repo(&repo_info)
        .await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to get storage for '{ref_str}': {e}"))?;

    let repository = icechunk::Repository::open(None, storage, std::collections::HashMap::new())
        .await
        .map_err(|e| {
            color_eyre::eyre::eyre!("Failed to open Icechunk repo for '{ref_str}': {e}")
        })?;

    let identity = app::RepoIdentity::Arraylake {
        org: org.to_string(),
        repo: repo_name.to_string(),
        bucket: bucket_name.to_string(),
        platform,
        region,
    };
    Ok((repository, identity))
}
