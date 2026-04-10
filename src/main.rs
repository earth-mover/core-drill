mod app;
mod cli;
mod component;
pub mod config;
mod fetch;
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
pub mod util;

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

    // Handle alias subcommand early — no repo needed
    if let Some(cli::Command::Alias { command }) = cli.command {
        return run_alias_command(command);
    }

    // For TUI mode (no --output, --serve, --repl), show loading screen while opening
    let is_tui = !cli.serve && !cli.repl && cli.output.is_none();

    if cli.serve {
        // MCP server mode: repo is optional — agents can `open` on demand
        let (repo, label) = if let Some(ref repo_str) = cli.repo {
            let (repository, repo_id) = open_repo(
                repo_str,
                cli.arraylake_api.as_deref(),
                &repo::StorageOverrides {
                    region: cli.region.clone(),
                    endpoint_url: cli.endpoint_url.clone(),
                    anonymous: cli.anonymous,
                },
            )
            .await?;
            (Some(repository), repo_id.display_short())
        } else {
            (None, String::new())
        };
        mcp::serve(repo, label).await?;
    } else if is_tui {
        let repo_str = cli
            .repo
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("A repo argument is required for TUI mode"))?;
        let is_arraylake = looks_like_arraylake_ref(&repo_str);
        let api_url = cli.arraylake_api.clone();
        let overrides = repo::StorageOverrides {
            region: cli.region.clone(),
            endpoint_url: cli.endpoint_url.clone(),
            anonymous: cli.anonymous,
        };

        let label = if is_arraylake {
            format!(
                "Connecting to {}...",
                repo_str.strip_prefix("al:").unwrap_or(&repo_str)
            )
        } else {
            format!("Opening {}...", repo_str)
        };

        tui::run_with_loading(
            &label,
            async move {
                if is_arraylake {
                    open_via_arraylake(&repo_str, api_url.as_deref()).await
                } else {
                    let repo = repo::open(&repo_str, &overrides).await?;
                    let identity = if repo_str.contains("://") {
                        app::RepoIdentity::S3 { url: repo_str }
                    } else {
                        app::RepoIdentity::Local { path: repo_str }
                    };
                    Ok((repo, identity))
                }
            },
            |(repository, repo_id)| {
                let data_store = store::DataStore::new(repository);
                app::App::new(data_store, repo_id)
            },
        )
        .await?;
    } else {
        // Non-TUI modes (REPL, --output): repo is required
        let repo_str = cli
            .repo
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("A repo argument is required"))?;

        let (repository, repo_id) = open_repo(
            &repo_str,
            cli.arraylake_api.as_deref(),
            &repo::StorageOverrides {
                region: cli.region.clone(),
                endpoint_url: cli.endpoint_url.clone(),
                anonymous: cli.anonymous,
            },
        )
        .await?;

        let display_label = repo_id.display_short();

        if cli.repl {
            let format = cli.output.unwrap_or(cli::OutputFormat::Md);
            output::run_repl(repository, format, &display_label).await?;
        } else if let Some(format) = cli.output {
            output::run(repository, format, cli.command, &display_label).await?;
        }
    }

    Ok(())
}

/// Open a repository from a string reference (local path, URL, alias, or arraylake ref).
/// Shared by main dispatch and MCP server's `open` tool.
///
/// If `repo_str` matches a saved alias, expands it and merges stored overrides
/// (CLI flags take precedence over alias values).
pub async fn open_repo(
    repo_str: &str,
    arraylake_api: Option<&str>,
    overrides: &repo::StorageOverrides,
) -> Result<(icechunk::Repository, app::RepoIdentity)> {
    // Try alias resolution — if the string matches an alias, expand it
    let (resolved, resolved_overrides);
    if let Some(alias) = config::resolve_alias(repo_str)? {
        resolved = alias.repo;
        // CLI flags override alias values
        resolved_overrides = repo::StorageOverrides {
            region: overrides.region.clone().or(alias.region),
            endpoint_url: overrides.endpoint_url.clone().or(alias.endpoint_url),
            anonymous: overrides.anonymous || alias.anonymous,
        };
    } else {
        resolved = repo_str.to_string();
        resolved_overrides = repo::StorageOverrides {
            region: overrides.region.clone(),
            endpoint_url: overrides.endpoint_url.clone(),
            anonymous: overrides.anonymous,
        };
    }

    if looks_like_arraylake_ref(&resolved) {
        open_via_arraylake(&resolved, arraylake_api).await
    } else {
        let repo = repo::open(&resolved, &resolved_overrides).await?;
        let identity = if resolved.contains("://") {
            app::RepoIdentity::S3 { url: resolved }
        } else {
            app::RepoIdentity::Local { path: resolved }
        };
        Ok((repo, identity))
    }
}

/// Detect if a repo string is an Arraylake reference.
/// Explicit: `al:org/repo`. Implicit: `org/repo` that doesn't exist on disk.
fn looks_like_arraylake_ref(s: &str) -> bool {
    s.starts_with("al:")
}

/// Open a repo via Arraylake, handling credentials automatically.
/// Reads the OAuth token from ~/.arraylake/token.json.
/// API endpoint priority: explicit arg > ARRAYLAKE_SERVICE__URI env > crate default.
async fn open_via_arraylake(
    al_ref: &str,
    api_url: Option<&str>,
) -> Result<(icechunk::Repository, app::RepoIdentity)> {
    let ref_str = al_ref.strip_prefix("al:").unwrap_or(al_ref);
    let (org, repo_name) = ref_str.split_once('/').ok_or_else(|| {
        color_eyre::eyre::eyre!("Invalid Arraylake ref: expected 'al:org/repo', got '{al_ref}'")
    })?;

    // Resolve API endpoint: CLI flag > env var > crate default (None)
    let env_api = std::env::var("ARRAYLAKE_SERVICE__URI").ok();
    let api_url_owned: Option<String> = api_url.or(env_api.as_deref()).map(|url| {
        if !url.contains("://") {
            format!("https://{url}")
        } else {
            url.to_string()
        }
    });
    let api_url = api_url_owned.as_deref();

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
        arraylake::ALClient::new(api_url.map(|s| s.to_string()), id_token.to_string())
            .map_err(|e| color_eyre::eyre::eyre!("Failed to create Arraylake client: {e}"))?,
    );

    // Fetch repo info first to get bucket details for display
    // Verify auth first with a quick user check
    if let Err(e) = client.get_current_user().await {
        color_eyre::eyre::bail!(
            "Arraylake authentication failed:\n  {e}\n\n\
             Run `arraylake auth login` to refresh your token."
        );
    }

    let repo_info = client.get_repo_info(org, repo_name).await.map_err(|e| {
        color_eyre::eyre::eyre!(
            "Repo '{ref_str}' not found:\n  {e}\n\n\
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

    tracing::info!("Arraylake: {org}/{repo_name}  →  {bucket_name} ({platform}, {region})");

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

/// Handle `core-drill alias <subcommand>` — pure config file operations, no repo needed.
fn run_alias_command(command: cli::AliasCommand) -> Result<()> {
    match command {
        cli::AliasCommand::List => {
            let cfg = config::load()?;
            if cfg.aliases.is_empty() {
                println!("No aliases configured.");
                println!(
                    "\nAdd one with: core-drill alias add <name> <repo> [--anonymous] [--region <r>]"
                );
            } else {
                for (name, alias) in &cfg.aliases {
                    let mut flags = Vec::new();
                    if alias.anonymous {
                        flags.push("anonymous".to_string());
                    }
                    if let Some(ref r) = alias.region {
                        flags.push(format!("region={r}"));
                    }
                    if let Some(ref e) = alias.endpoint_url {
                        flags.push(format!("endpoint={e}"));
                    }
                    if flags.is_empty() {
                        println!("  {name:16} → {}", alias.repo);
                    } else {
                        println!("  {name:16} → {}  ({})", alias.repo, flags.join(", "));
                    }
                }
            }
        }
        cli::AliasCommand::Add {
            name,
            repo,
            region,
            endpoint_url,
            anonymous,
        } => {
            let mut cfg = config::load()?;
            let is_update = cfg.aliases.contains_key(&name);
            cfg.aliases.insert(
                name.clone(),
                config::Alias {
                    repo: repo.clone(),
                    region,
                    endpoint_url,
                    anonymous,
                },
            );
            config::save(&cfg)?;
            if is_update {
                println!("Updated alias '{name}' → {repo}");
            } else {
                println!("Added alias '{name}' → {repo}");
            }
        }
        cli::AliasCommand::Remove { name } => {
            let mut cfg = config::load()?;
            if cfg.aliases.remove(&name).is_some() {
                config::save(&cfg)?;
                println!("Removed alias '{name}'");
            } else {
                color_eyre::eyre::bail!("No alias named '{name}'. Run `core-drill alias list` to see available aliases.");
            }
        }
    }
    Ok(())
}
